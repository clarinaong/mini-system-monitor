use super::*;
use crate::monitor::{BatteryMetrics, SampleHealth, SystemMetrics};
use std::{cell::Cell, rc::Rc};

struct Rig {
    now: Rc<Cell<Duration>>,
    evaluator: AlertEvaluator<Box<dyn Fn() -> Duration>>,
    settings: Settings,
    sequence: u64,
}

impl Rig {
    fn new() -> Self {
        let mut settings = Settings::default();
        settings.alerts.enabled = true;
        settings.alerts.cpu.sustained_seconds = 2;
        settings.alerts.memory.sustained_seconds = 2;
        settings.alerts.cooldown_seconds = 10;
        let now = Rc::new(Cell::new(Duration::ZERO));
        let clock = Rc::clone(&now);
        let clock: Box<dyn Fn() -> Duration> = Box::new(move || clock.get());
        let evaluator = AlertEvaluator::new(&settings, clock).unwrap();
        Self {
            now,
            evaluator,
            settings,
            sequence: 0,
        }
    }

    fn configure(&mut self, change: impl FnOnce(&mut Settings)) {
        change(&mut self.settings);
        self.evaluator.update_settings(&self.settings).unwrap();
    }

    fn at(&mut self, ms: u64, change: impl FnOnce(&mut MetricsSample)) -> Vec<EligibleAlert> {
        self.now.set(Duration::from_millis(ms));
        self.sequence += 1;
        let mut sample = sample(self.sequence, ms);
        change(&mut sample);
        self.evaluator.evaluate(Some(&sample))
    }

    fn usage(&mut self, ms: u64, cpu: f32, memory_used: u64) -> Vec<EligibleAlert> {
        self.at(ms, |sample| {
            let metrics = sample.metrics.as_mut().unwrap();
            metrics.cpu_usage = Some(cpu);
            metrics.memory_used = memory_used;
        })
    }

    fn battery(&mut self, ms: u64, percentage: f32, status: &str) -> Vec<EligibleAlert> {
        self.at(ms, |sample| {
            sample.metrics.as_mut().unwrap().battery = Some(BatteryMetrics {
                percentage,
                status: status.into(),
            });
        })
    }
}

fn sample(sequence: u64, ms: u64) -> MetricsSample {
    MetricsSample {
        sequence,
        config_revision: 0,
        timestamp_ms: 1_800_000_000_000 + ms,
        monotonic_ms: ms,
        interval_ms: 1000,
        metrics: Some(SystemMetrics {
            cpu_usage: Some(0.0),
            memory_used: 500,
            memory_total: 1000,
            battery: Some(BatteryMetrics {
                percentage: 50.0,
                status: "Discharging".into(),
            }),
        }),
        health: SampleHealth {
            status: SampleStatus::Healthy,
            error: None,
            battery_status: BatteryStatus::Available,
            battery_error: None,
        },
    }
}

fn rules(alerts: Vec<EligibleAlert>) -> Vec<AlertRule> {
    alerts.into_iter().map(|alert| alert.rule).collect()
}

#[test]
fn usage_thresholds_include_equality_and_one_hundred_percent() {
    for (cpu, memory, expected) in [
        (89.999, 899, vec![]),
        (90.0, 900, vec![AlertRule::Cpu, AlertRule::Memory]),
        (100.0, 1000, vec![AlertRule::Cpu, AlertRule::Memory]),
    ] {
        let mut rig = Rig::new();
        rig.configure(|settings| {
            settings.alerts.cpu.sustained_seconds = 1;
            settings.alerts.memory.sustained_seconds = 1;
        });
        assert!(rig.usage(0, cpu, memory).is_empty());
        assert_eq!(rules(rig.usage(1000, cpu, memory)), expected);
    }
    let mut rig = Rig::new();
    rig.configure(|settings| {
        settings.alerts.cpu.threshold_percent = 100;
        settings.alerts.memory.threshold_percent = 100;
    });
    for ms in [0, 1000] {
        assert!(rig.usage(ms, 100.0, 1000).is_empty());
    }
    assert_eq!(
        rules(rig.usage(2000, 100.0, 1000)),
        [AlertRule::Cpu, AlertRule::Memory]
    );
}

#[test]
fn brief_load_recovers_and_full_sustained_duration_is_required() {
    let mut rig = Rig::new();
    for (ms, cpu) in [
        (0, 90.0),
        (999, 89.0),
        (1000, 90.0),
        (1999, 90.0),
        (2000, 90.0),
        (2999, 90.0),
    ] {
        assert!(rig.usage(ms, cpu, 500).is_empty());
    }
    assert_eq!(rules(rig.usage(3000, 90.0, 500)), [AlertRule::Cpu]);
}

#[test]
fn usage_rules_have_independent_sustained_durations() {
    let mut rig = Rig::new();
    rig.configure(|settings| settings.alerts.cpu.sustained_seconds = 1);
    assert!(rig.usage(0, 90.0, 900).is_empty());
    assert_eq!(rules(rig.usage(1000, 90.0, 900)), [AlertRule::Cpu]);
    assert_eq!(rules(rig.usage(2000, 90.0, 900)), [AlertRule::Memory]);
}

#[test]
fn memory_boundaries_compare_bytes_exactly_without_rounding_or_overflow() {
    for (used, total, threshold, expected) in [
        (28, 100, 29, 0),
        (29, 100, 29, 1),
        (30, 100, 29, 1),
        (u64::MAX - 1, u64::MAX, 100, 0),
        (u64::MAX, u64::MAX, 100, 1),
    ] {
        let mut rig = Rig::new();
        rig.configure(|settings| {
            settings.alerts.memory.threshold_percent = threshold;
            settings.alerts.memory.sustained_seconds = 1;
        });
        for ms in [0, 1000] {
            let alerts = rig.at(ms, |sample| {
                let metrics = sample.metrics.as_mut().unwrap();
                metrics.memory_used = used;
                metrics.memory_total = total;
            });
            assert_eq!(alerts.len(), if ms == 1000 { expected } else { 0 });
        }
    }
}

#[test]
fn sustained_duration_uses_sample_times_and_cooldown_uses_injected_clock() {
    let mut rig = Rig::new();
    for (sequence, sampled_ms, now_ms) in [(1, 0, 3000), (2, 1000, 3500)] {
        rig.now.set(Duration::from_millis(now_ms));
        let mut reading = sample(sequence, sampled_ms);
        reading.metrics.as_mut().unwrap().cpu_usage = Some(90.0);
        assert!(rig.evaluator.evaluate(Some(&reading)).is_empty());
    }
    rig.now.set(Duration::from_millis(4000));
    let mut reading = sample(3, 2000);
    reading.timestamp_ms = 0;
    reading.metrics.as_mut().unwrap().cpu_usage = Some(90.0);
    let alerts = rig.evaluator.evaluate(Some(&reading));
    assert_eq!(
        alerts,
        [EligibleAlert {
            rule: AlertRule::Cpu,
            sample_sequence: 3,
            sample_monotonic_ms: 2000,
            evaluated_monotonic_ms: 4000,
            value_percent: 90.0,
            threshold_percent: 90,
        }]
    );
}

#[test]
fn uninterrupted_episode_does_not_repeat_after_cooldown() {
    let mut rig = Rig::new();
    for ms in [0, 1000] {
        assert!(rig.usage(ms, 90.0, 900).is_empty());
    }
    assert_eq!(
        rules(rig.usage(2000, 90.0, 900)),
        [AlertRule::Cpu, AlertRule::Memory]
    );
    for ms in (3000..=20_000).step_by(1000) {
        assert!(rig.usage(ms, 90.0, 900).is_empty());
    }
}

#[test]
fn recovered_episode_waits_until_exact_cooldown_expiry() {
    let mut rig = Rig::new();
    rig.configure(|settings| {
        settings.alerts.cpu.sustained_seconds = 1;
        settings.alerts.cooldown_seconds = 5;
    });
    assert!(rig.usage(0, 90.0, 500).is_empty());
    assert_eq!(rules(rig.usage(1000, 90.0, 500)), [AlertRule::Cpu]);
    assert!(rig.usage(1500, 89.999, 500).is_empty());
    for ms in [2000, 3000, 4000, 5000, 5999] {
        assert!(rig.usage(ms, 90.0, 500).is_empty());
    }
    assert_eq!(rules(rig.usage(6000, 90.0, 500)), [AlertRule::Cpu]);
    assert!(rig.usage(7000, 89.0, 500).is_empty());
    assert!(rig.usage(12_000, 90.0, 500).is_empty());
    assert_eq!(rules(rig.usage(13_000, 90.0, 500)), [AlertRule::Cpu]);
}

#[test]
fn recovery_of_one_rule_does_not_rearm_another() {
    let mut rig = Rig::new();
    rig.configure(|settings| settings.alerts.cooldown_seconds = 1);
    rig.usage(0, 90.0, 900);
    rig.usage(1000, 90.0, 900);
    assert_eq!(rig.usage(2000, 90.0, 900).len(), 2);
    assert!(rig.usage(3000, 0.0, 900).is_empty());
    assert!(rig.usage(4000, 90.0, 900).is_empty());
    assert!(rig.usage(5000, 90.0, 900).is_empty());
    assert_eq!(rules(rig.usage(6000, 90.0, 900)), [AlertRule::Cpu]);
}

#[test]
fn battery_threshold_boundaries_and_power_states_are_explicit() {
    for (percentage, status, expected) in [
        (20.001, "Discharging", vec![]),
        (20.0, "Discharging", vec![AlertRule::BatteryLow]),
        (0.0, "Discharging", vec![AlertRule::BatteryLow]),
        (20.0, "Charging", vec![]),
        (20.0, "Full", vec![]),
        (79.999, "Charging", vec![]),
        (80.0, "Charging", vec![AlertRule::BatteryCharging]),
        (100.0, "Full", vec![AlertRule::BatteryCharging]),
        (80.0, "Discharging", vec![]),
        (10.0, "Unknown", vec![]),
        (100.0, "Empty", vec![]),
    ] {
        assert_eq!(rules(Rig::new().battery(0, percentage, status)), expected);
    }
}

#[test]
fn battery_cooldowns_are_independent_and_transitions_rearm() {
    let mut rig = Rig::new();
    assert_eq!(
        rules(rig.battery(0, 20.0, "Discharging")),
        [AlertRule::BatteryLow]
    );
    assert_eq!(
        rules(rig.battery(1000, 80.0, "Charging")),
        [AlertRule::BatteryCharging]
    );
    assert!(rig.battery(2000, 100.0, "Full").is_empty());
    assert!(rig.battery(3000, 20.0, "Discharging").is_empty());
    assert!(rig.battery(9999, 20.0, "Discharging").is_empty());
    assert_eq!(
        rules(rig.battery(10_000, 20.0, "Discharging")),
        [AlertRule::BatteryLow]
    );
    assert!(rig.battery(10_999, 80.0, "Charging").is_empty());
    assert_eq!(
        rules(rig.battery(11_000, 80.0, "Charging")),
        [AlertRule::BatteryCharging]
    );
}

#[test]
fn charging_full_and_unknown_do_not_create_duplicate_episodes() {
    let mut rig = Rig::new();
    assert_eq!(rig.battery(0, 80.0, "Charging").len(), 1);
    assert!(rig.battery(10_000, 100.0, "Full").is_empty());
    assert!(rig.battery(11_000, 10.0, "Unknown").is_empty());
    assert!(rig.battery(12_000, 90.0, "Charging").is_empty());
    assert!(rig.battery(13_000, 79.999, "Charging").is_empty());
    assert_eq!(
        rules(rig.battery(14_000, 80.0, "Charging")),
        [AlertRule::BatteryCharging]
    );
}

#[test]
fn master_and_each_rule_switch_suppress_evaluation() {
    let mut rig = Rig::new();
    rig.configure(|settings| settings.alerts.enabled = false);
    for ms in 0..=4 {
        assert!(
            rig.at(ms * 1000, |sample| {
                let metrics = sample.metrics.as_mut().unwrap();
                metrics.cpu_usage = Some(100.0);
                metrics.memory_used = 1000;
                metrics.battery.as_mut().unwrap().percentage = 10.0;
            })
            .is_empty()
        );
    }
    for (disabled, disabled_rule) in RULES.into_iter().enumerate() {
        let mut rig = Rig::new();
        rig.configure(|settings| match disabled {
            0 => settings.alerts.cpu.enabled = false,
            1 => settings.alerts.memory.enabled = false,
            2 => settings.alerts.battery_low.enabled = false,
            _ => settings.alerts.battery_charging.enabled = false,
        });
        let mut emitted = Vec::new();
        for ms in [0, 1000, 2000] {
            emitted.extend(rules(rig.at(ms, |sample| {
                let metrics = sample.metrics.as_mut().unwrap();
                metrics.cpu_usage = Some(100.0);
                metrics.memory_used = 1000;
                metrics.battery.as_mut().unwrap().percentage = 10.0;
            })));
        }
        emitted.extend(rules(rig.battery(3000, 90.0, "Charging")));
        assert!(!emitted.contains(&disabled_rule));
        for rule in RULES.into_iter().filter(|rule| *rule != disabled_rule) {
            assert!(emitted.contains(&rule));
        }
    }
}

#[test]
fn disable_and_reenable_reset_evidence_but_preserve_cooldown() {
    for master in [false, true] {
        let mut rig = Rig::new();
        assert_eq!(rig.battery(0, 10.0, "Discharging").len(), 1);
        rig.configure(|settings| {
            if master {
                settings.alerts.enabled = false;
            } else {
                settings.alerts.battery_low.enabled = false;
            }
        });
        assert!(rig.battery(1000, 10.0, "Discharging").is_empty());
        rig.configure(|settings| {
            settings.alerts.enabled = true;
            settings.alerts.battery_low.enabled = true;
        });
        assert!(rig.battery(2000, 10.0, "Discharging").is_empty());
        assert_eq!(rig.battery(10_000, 10.0, "Discharging").len(), 1);
    }
    let mut rig = Rig::new();
    rig.usage(0, 90.0, 500);
    rig.usage(1000, 90.0, 500);
    rig.configure(|settings| settings.alerts.cpu.enabled = false);
    rig.usage(2000, 90.0, 500);
    rig.configure(|settings| settings.alerts.cpu.enabled = true);
    assert!(rig.usage(3000, 90.0, 500).is_empty());
    assert!(rig.usage(4000, 90.0, 500).is_empty());
    assert_eq!(rules(rig.usage(5000, 90.0, 500)), [AlertRule::Cpu]);
}

#[test]
fn threshold_change_restarts_only_affected_rule() {
    let mut rig = Rig::new();
    rig.usage(0, 100.0, 900);
    rig.usage(1000, 100.0, 900);
    rig.configure(|settings| settings.alerts.cpu.threshold_percent = 95);
    assert_eq!(rules(rig.usage(2000, 100.0, 900)), [AlertRule::Memory]);
    assert!(rig.usage(3000, 100.0, 900).is_empty());
    assert_eq!(rules(rig.usage(4000, 100.0, 900)), [AlertRule::Cpu]);
}

#[test]
fn duration_changes_do_not_reuse_previous_evidence() {
    let mut rig = Rig::new();
    rig.usage(0, 90.0, 500);
    rig.usage(1000, 90.0, 500);
    rig.configure(|settings| settings.alerts.cpu.sustained_seconds = 1);
    assert!(rig.usage(2000, 90.0, 500).is_empty());
    assert_eq!(rules(rig.usage(3000, 90.0, 500)), [AlertRule::Cpu]);
}

#[test]
fn repeated_settings_and_presentation_or_sound_changes_preserve_state() {
    let mut rig = Rig::new();
    rig.usage(0, 90.0, 900);
    rig.configure(|_| {});
    rig.usage(1000, 90.0, 900);
    rig.configure(|settings| {
        settings.visible_cards.cpu = false;
        settings.graphs.memory = false;
        settings.alerts.notification_sound = true;
    });
    assert_eq!(
        rules(rig.usage(2000, 90.0, 900)),
        [AlertRule::Cpu, AlertRule::Memory]
    );
    rig.configure(|_| {});
    assert!(rig.usage(20_000, 90.0, 900).is_empty());
}

#[test]
fn cooldown_changes_use_last_eligibility_without_erasing_it() {
    let mut rig = Rig::new();
    assert_eq!(rig.battery(0, 10.0, "Discharging").len(), 1);
    rig.configure(|settings| settings.alerts.cooldown_seconds = 20);
    assert!(rig.battery(10_000, 10.0, "Discharging").is_empty());
    rig.configure(|settings| settings.alerts.cooldown_seconds = 15);
    assert!(rig.battery(14_999, 10.0, "Discharging").is_empty());
    assert_eq!(rig.battery(15_000, 10.0, "Discharging").len(), 1);
}

#[test]
fn interval_changes_break_duration_and_reject_old_interval_samples() {
    let mut rig = Rig::new();
    rig.usage(0, 90.0, 500);
    rig.usage(1000, 90.0, 500);
    rig.configure(|settings| settings.monitoring_interval_seconds = 5);
    assert!(rig.usage(2000, 90.0, 500).is_empty());
    for ms in [3000, 8000] {
        let alerts = rig.at(ms, |sample| {
            sample.interval_ms = 5000;
            sample.metrics.as_mut().unwrap().cpu_usage = Some(90.0);
        });
        assert_eq!(
            rules(alerts),
            if ms == 8000 {
                vec![AlertRule::Cpu]
            } else {
                vec![]
            }
        );
    }
}

#[test]
fn samples_collected_before_configuration_application_cannot_supply_evidence() {
    let mut rig = Rig::new();
    rig.now.set(Duration::from_millis(1500));
    rig.configure(|settings| settings.alerts.cpu.threshold_percent = 95);
    let mut old = sample(1, 1000);
    old.metrics.as_mut().unwrap().cpu_usage = Some(100.0);
    assert!(rig.evaluator.evaluate(Some(&old)).is_empty());
    rig.sequence = 1;
    assert!(rig.usage(2000, 100.0, 500).is_empty());
    assert!(rig.usage(3000, 100.0, 500).is_empty());
    assert_eq!(rules(rig.usage(4000, 100.0, 500)), [AlertRule::Cpu]);
}

#[test]
fn invalid_configuration_is_rejected_atomically_using_existing_validation() {
    let mut rig = Rig::new();
    rig.usage(0, 90.0, 500);
    rig.usage(1000, 90.0, 500);
    let mut invalid = rig.settings.clone();
    invalid.alerts.cpu.threshold_percent = 0;
    assert!(AlertEvaluator::new(&invalid, || Duration::ZERO).is_err());
    assert!(rig.evaluator.update_settings(&invalid).is_err());
    assert_eq!(rules(rig.usage(2000, 90.0, 500)), [AlertRule::Cpu]);
}

#[test]
fn restart_clears_episode_and_cooldown_but_requires_new_usage_duration() {
    let mut first = Rig::new();
    first.usage(0, 90.0, 500);
    first.usage(1000, 90.0, 500);
    assert_eq!(first.usage(2000, 90.0, 500).len(), 1);
    assert_eq!(first.battery(3000, 10.0, "Discharging").len(), 1);
    let mut restarted = Rig::new();
    assert_eq!(restarted.battery(0, 10.0, "Discharging").len(), 1);
    assert!(restarted.usage(1000, 90.0, 500).is_empty());
    assert!(restarted.usage(2000, 90.0, 500).is_empty());
    assert_eq!(rules(restarted.usage(3000, 90.0, 500)), [AlertRule::Cpu]);
}

#[test]
fn cpu_warmup_or_missing_cpu_breaks_only_cpu_evidence() {
    for warming_up in [false, true] {
        let mut rig = Rig::new();
        rig.usage(0, 90.0, 900);
        assert!(
            rig.at(1000, |sample| {
                let metrics = sample.metrics.as_mut().unwrap();
                metrics.memory_used = 900;
                if warming_up {
                    sample.health.status = SampleStatus::WarmingUp;
                    metrics.cpu_usage = Some(100.0);
                } else {
                    metrics.cpu_usage = None;
                }
            })
            .is_empty()
        );
        assert_eq!(rules(rig.usage(2000, 90.0, 900)), [AlertRule::Memory]);
        assert!(rig.usage(3000, 90.0, 900).is_empty());
        assert_eq!(rules(rig.usage(4000, 90.0, 900)), [AlertRule::Cpu]);
    }
}

#[test]
fn full_collection_failures_or_missing_metrics_break_all_duration_evidence() {
    for failure in 0..3 {
        let mut rig = Rig::new();
        rig.usage(0, 90.0, 900);
        assert!(
            rig.at(1000, |sample| match failure {
                0 => sample.metrics = None,
                1 => sample.health.status = SampleStatus::Error,
                _ => sample.health.error = Some("collection failed".into()),
            })
            .is_empty()
        );
        assert!(rig.usage(2000, 90.0, 900).is_empty());
        assert!(rig.usage(3000, 90.0, 900).is_empty());
        assert_eq!(rig.usage(4000, 90.0, 900).len(), 2);
    }
}

#[test]
fn missing_readings_are_not_recovery_and_do_not_clear_cooldowns() {
    let mut rig = Rig::new();
    assert_eq!(rig.battery(0, 10.0, "Discharging").len(), 1);
    assert!(rig.at(1000, |sample| sample.metrics = None).is_empty());
    assert!(rig.battery(20_000, 10.0, "Discharging").is_empty());
    assert!(rig.battery(21_000, 21.0, "Discharging").is_empty());
    assert_eq!(rig.battery(22_000, 10.0, "Discharging").len(), 1);
    assert!(rig.at(23_000, |sample| sample.metrics = None).is_empty());
    assert!(rig.battery(24_000, 21.0, "Discharging").is_empty());
    assert!(rig.battery(25_000, 10.0, "Discharging").is_empty());
    assert_eq!(rig.battery(32_000, 10.0, "Discharging").len(), 1);
}

#[test]
fn invalid_cpu_and_memory_values_are_missing_instead_of_healthy_or_high() {
    for invalid_cpu in [f32::NAN, f32::INFINITY, -1.0, 100.001] {
        let mut rig = Rig::new();
        rig.usage(0, 90.0, 900);
        rig.usage(1000, invalid_cpu, 900);
        assert_eq!(rules(rig.usage(2000, 90.0, 900)), [AlertRule::Memory]);
    }
    for (used, total) in [(0, 0), (1001, 1000)] {
        let mut rig = Rig::new();
        rig.usage(0, 90.0, 900);
        rig.at(1000, |sample| {
            let metrics = sample.metrics.as_mut().unwrap();
            metrics.cpu_usage = Some(90.0);
            metrics.memory_used = used;
            metrics.memory_total = total;
        });
        assert_eq!(rules(rig.usage(2000, 90.0, 900)), [AlertRule::Cpu]);
    }
}

#[test]
fn battery_absence_errors_and_invalid_values_do_not_block_usage_or_rearm_battery() {
    for variant in 0..8 {
        let mut rig = Rig::new();
        assert_eq!(rig.battery(0, 10.0, "Discharging").len(), 1);
        rig.at(1000, |sample| {
            let metrics = sample.metrics.as_mut().unwrap();
            metrics.cpu_usage = Some(90.0);
            metrics.memory_used = 900;
            metrics.battery.as_mut().unwrap().percentage = 10.0;
        });
        assert!(
            rig.at(2000, |sample| {
                let metrics = sample.metrics.as_mut().unwrap();
                metrics.cpu_usage = Some(90.0);
                metrics.memory_used = 900;
                metrics.battery.as_mut().unwrap().percentage = 90.0;
                match variant {
                    0 => {
                        metrics.battery = None;
                        sample.health.battery_status = BatteryStatus::Absent;
                    }
                    1 => sample.health.battery_status = BatteryStatus::Error,
                    2 => sample.health.battery_status = BatteryStatus::Unavailable,
                    3 => sample.health.battery_error = Some("battery failed".into()),
                    4 => metrics.battery.as_mut().unwrap().percentage = f32::NAN,
                    5 => metrics.battery.as_mut().unwrap().percentage = -1.0,
                    6 => metrics.battery.as_mut().unwrap().percentage = 101.0,
                    _ => metrics.battery.as_mut().unwrap().status = "Unknown".into(),
                }
                sample.health.status = SampleStatus::Degraded;
            })
            .is_empty()
        );
        assert_eq!(
            rules(rig.at(3000, |sample| {
                let metrics = sample.metrics.as_mut().unwrap();
                metrics.cpu_usage = Some(90.0);
                metrics.memory_used = 900;
                metrics.battery.as_mut().unwrap().percentage = 10.0;
            })),
            [AlertRule::Cpu, AlertRule::Memory]
        );
        assert!(rig.battery(20_000, 10.0, "Discharging").is_empty());
    }
}

#[test]
fn no_sample_breaks_evidence_and_clock_ticks_alone_never_emit() {
    let mut rig = Rig::new();
    rig.usage(0, 90.0, 500);
    rig.now.set(Duration::from_millis(1000));
    assert!(rig.evaluator.evaluate(None).is_empty());
    assert!(rig.usage(2000, 90.0, 500).is_empty());
    assert!(rig.usage(3000, 90.0, 500).is_empty());
    assert_eq!(rig.usage(4000, 90.0, 500).len(), 1);
}

#[test]
fn stale_boundary_uses_configured_interval_and_cannot_recover_a_rule() {
    for (interval_seconds, age, expected) in
        [(1, 3999, 1), (1, 4000, 0), (5, 11_999, 1), (5, 12_000, 0)]
    {
        let mut rig = Rig::new();
        rig.configure(|settings| settings.monitoring_interval_seconds = interval_seconds);
        rig.now.set(Duration::from_millis(age));
        let mut reading = sample(1, 0);
        reading.interval_ms = u64::from(interval_seconds) * 1000;
        reading
            .metrics
            .as_mut()
            .unwrap()
            .battery
            .as_mut()
            .unwrap()
            .percentage = 10.0;
        assert_eq!(rig.evaluator.evaluate(Some(&reading)).len(), expected);
    }
    let mut rig = Rig::new();
    rig.battery(0, 10.0, "Discharging");
    rig.now.set(Duration::from_millis(5000));
    assert!(rig.evaluator.evaluate(Some(&sample(2, 1000))).is_empty());
    rig.sequence = 2;
    assert!(rig.battery(20_000, 10.0, "Discharging").is_empty());
}

#[test]
fn gap_tolerance_has_exact_boundary_and_skipped_sequences_break_evidence() {
    for (gap, skipped, expected) in [(1750, false, 1), (1751, false, 0), (1000, true, 0)] {
        let mut rig = Rig::new();
        rig.configure(|settings| settings.alerts.cpu.sustained_seconds = 1);
        rig.usage(0, 90.0, 500);
        if skipped {
            rig.sequence += 1;
        }
        assert_eq!(rig.usage(gap, 90.0, 500).len(), expected);
        if expected == 0 {
            assert_eq!(rig.usage(gap + 1000, 90.0, 500).len(), 1);
        }
    }
}

#[test]
fn duplicate_and_out_of_order_samples_cannot_recover_or_retrigger() {
    let mut rig = Rig::new();
    rig.battery(0, 10.0, "Discharging");
    rig.battery(1000, 10.0, "Discharging");
    rig.now.set(Duration::from_millis(20_000));
    for reading in [sample(2, 1000), sample(1, 500), sample(3, 1000)] {
        assert!(rig.evaluator.evaluate(Some(&reading)).is_empty());
    }
    assert!(rig.battery(21_000, 10.0, "Discharging").is_empty());
}

#[test]
fn replaying_a_cached_high_sample_does_not_complete_a_duration() {
    let mut rig = Rig::new();
    let mut reading = sample(1, 0);
    reading.metrics.as_mut().unwrap().cpu_usage = Some(90.0);
    assert!(rig.evaluator.evaluate(Some(&reading)).is_empty());
    rig.now.set(Duration::from_millis(2000));
    assert!(rig.evaluator.evaluate(Some(&reading)).is_empty());
}

#[test]
fn stale_cached_reading_breaks_evidence_before_buffered_samples_arrive() {
    let mut rig = Rig::new();
    let mut reading = sample(1, 0);
    reading.metrics.as_mut().unwrap().cpu_usage = Some(90.0);
    assert!(rig.evaluator.evaluate(Some(&reading)).is_empty());
    rig.now.set(Duration::from_millis(4000));
    assert!(rig.evaluator.evaluate(Some(&reading)).is_empty());
    for (sequence, ms) in [(2, 1000), (3, 2000)] {
        reading.sequence = sequence;
        reading.monotonic_ms = ms;
        assert!(rig.evaluator.evaluate(Some(&reading)).is_empty());
    }
    reading.sequence = 4;
    reading.monotonic_ms = 3000;
    assert_eq!(
        rules(rig.evaluator.evaluate(Some(&reading))),
        [AlertRule::Cpu]
    );
}

#[test]
fn future_samples_and_clock_regression_fail_closed_without_poisoning_order() {
    let mut rig = Rig::new();
    rig.usage(0, 90.0, 500);
    rig.usage(1000, 90.0, 500);
    let mut future = sample(100, 100_000);
    future.metrics.as_mut().unwrap().cpu_usage = Some(90.0);
    assert!(rig.evaluator.evaluate(Some(&future)).is_empty());
    assert!(rig.usage(2000, 90.0, 500).is_empty());
    rig.now.set(Duration::from_millis(1500));
    assert!(rig.evaluator.evaluate(Some(&sample(4, 1500))).is_empty());
    assert!(rig.usage(3000, 90.0, 500).is_empty());
    assert!(rig.usage(4000, 90.0, 500).is_empty());
    assert_eq!(rig.usage(5000, 90.0, 500).len(), 1);
}
