use super::*;
use crate::alerts::AlertRule;
use collector::{CollectedMetrics, Collector};
use sampler::Clock;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};
use tauri::{
    Listener,
    test::{MockRuntime, mock_builder, mock_context, noop_assets},
};

struct TestClock(Arc<AtomicU64>);

impl Clock for TestClock {
    fn monotonic(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::SeqCst))
    }

    fn timestamp_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Copy)]
enum Reading {
    High,
    Low,
    Warmup,
    Failed,
    Battery {
        percentage: f32,
        status: &'static str,
    },
    BatteryError,
    Absent,
}

struct TestCollector {
    reading: Arc<Mutex<Reading>>,
    calls: Arc<AtomicUsize>,
    pause: Arc<AtomicBool>,
    entered: Sender<()>,
    release: Receiver<()>,
}

impl Collector for TestCollector {
    fn collect(&mut self, _: Duration) -> Result<CollectedMetrics, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.pause.swap(false, Ordering::SeqCst) {
            self.entered.send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let reading = *self.reading.lock().unwrap();
        if matches!(reading, Reading::Failed) {
            return Err("Injected collection failure".into());
        }
        Ok(CollectedMetrics {
            metrics: SystemMetrics {
                cpu_usage: match reading {
                    Reading::Warmup => None,
                    Reading::Low => Some(10.0),
                    _ => Some(95.0),
                },
                memory_used: if matches!(reading, Reading::Low) {
                    100
                } else {
                    950
                },
                memory_total: 1000,
                battery: match reading {
                    Reading::BatteryError | Reading::Absent => None,
                    Reading::Battery { percentage, status } => Some(BatteryMetrics {
                        percentage,
                        status: status.into(),
                    }),
                    _ => Some(BatteryMetrics {
                        percentage: 10.0,
                        status: "Discharging".into(),
                    }),
                },
            },
            battery_error: matches!(reading, Reading::BatteryError)
                .then(|| "Injected battery failure".into()),
        })
    }
}

struct WorkerRig {
    app: tauri::App<MockRuntime>,
    state: Arc<MonitorState>,
    now: Arc<AtomicU64>,
    reading: Arc<Mutex<Reading>>,
    calls: Arc<AtomicUsize>,
    pause: Arc<AtomicBool>,
    entered: Receiver<()>,
    release: Sender<()>,
    published: Receiver<(MetricsSample, Vec<EligibleAlert>)>,
}

fn alert_settings() -> Settings {
    let mut settings = Settings::default();
    settings.alerts.enabled = true;
    settings.alerts.cpu.sustained_seconds = 2;
    settings.alerts.cpu.threshold_percent = 90;
    settings.alerts.memory.enabled = false;
    settings.alerts.battery_low.enabled = false;
    settings.alerts.battery_charging.enabled = false;
    settings.alerts.cooldown_seconds = 5;
    settings
}

impl WorkerRig {
    fn new(settings: &Settings) -> Self {
        let app = mock_builder().build(mock_context(noop_assets())).unwrap();
        let now = Arc::new(AtomicU64::new(0));
        let clock_now = Arc::clone(&now);
        let state = Arc::new(MonitorState::with_clock(Instant::now(), move || {
            Duration::from_millis(clock_now.load(Ordering::SeqCst))
        }));
        let reading = Arc::new(Mutex::new(Reading::High));
        let calls = Arc::new(AtomicUsize::new(0));
        let pause = Arc::new(AtomicBool::new(false));
        let (entered_sender, entered) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        let collector = TestCollector {
            reading: Arc::clone(&reading),
            calls: Arc::clone(&calls),
            pause: Arc::clone(&pause),
            entered: entered_sender,
            release: release_receiver,
        };
        let clock = TestClock(Arc::clone(&now));
        let (sender, published) = mpsc::channel();
        let interval = settings.monitoring_interval_seconds;
        let shared_state = Arc::clone(&state);
        let handle = app.handle().clone();
        assert!(
            state
                .start_with(settings, move |shared| {
                    run_worker(
                        shared,
                        Sampler::new(collector, clock, interval),
                        None,
                        |sample, alerts, revision| {
                            let cached = shared_state.snapshot().unwrap();
                            assert_eq!(cached.latest.unwrap().sequence, sample.sequence);
                            publish_events(&handle, sample, alerts, revision);
                            sender.send((sample.clone(), alerts.to_vec())).unwrap();
                        },
                    );
                })
                .unwrap()
        );
        Self {
            app,
            state,
            now,
            reading,
            calls,
            pause,
            entered,
            release,
            published,
        }
    }

    fn receive(&self) -> (MetricsSample, Vec<EligibleAlert>) {
        self.published.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    fn advance(&self, ms: u64) {
        let _control = self.state.shared.control.lock().unwrap();
        self.now.store(ms, Ordering::SeqCst);
        self.state.shared.wake.notify_one();
    }

    fn step(&self, ms: u64) -> (MetricsSample, Vec<EligibleAlert>) {
        self.advance(ms);
        self.receive()
    }

    fn apply_at(&self, ms: u64, settings: &Settings) {
        self.now.store(ms, Ordering::SeqCst);
        self.state.apply_settings(settings).unwrap();
    }
}

impl Drop for WorkerRig {
    fn drop(&mut self) {
        let _ = self.release.send(());
        self.state.stop();
    }
}

#[test]
fn worker_evaluates_without_windows_and_publishes_typed_eligibility_once() {
    let rig = WorkerRig::new(&alert_settings());
    let (sender, events) = mpsc::channel();
    rig.app.listen(ALERT_ELIGIBLE_EVENT, move |event| {
        sender
            .send(serde_json::from_str::<serde_json::Value>(event.payload()).unwrap())
            .unwrap();
    });
    assert!(rig.receive().1.is_empty());
    assert!(rig.step(1000).1.is_empty());
    let (sample, alerts) = rig.step(2000);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].rule, AlertRule::Cpu);
    let event = events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        event,
        serde_json::json!({
            "rule": "cpu", "sampleSequence": sample.sequence,
            "sampleMonotonicMs": 2000, "evaluatedMonotonicMs": 2000,
            "valuePercent": 95.0, "thresholdPercent": 90,
        })
    );
    for ms in [3000, 4000, 5000, 6000, 7000, 8000] {
        assert!(rig.step(ms).1.is_empty());
    }
    assert!(events.try_recv().is_err());
    assert_eq!(rig.calls.load(Ordering::SeqCst), 9);
}

#[test]
fn alert_only_changes_do_not_postpone_sampling_and_toggles_between_samples_take_effect() {
    let mut settings = alert_settings();
    let rig = WorkerRig::new(&settings);
    rig.receive();
    rig.step(1000);
    settings.alerts.enabled = false;
    rig.apply_at(1250, &settings);
    settings.alerts.enabled = true;
    rig.apply_at(1500, &settings);
    assert!(rig.step(2000).1.is_empty());
    assert!(rig.step(3000).1.is_empty());
    assert_eq!(rig.step(4000).1.len(), 1);
    assert_eq!(rig.calls.load(Ordering::SeqCst), 5);
    assert_eq!(rig.state.snapshot().unwrap().config_revision, 0);
}

#[test]
fn presentation_and_sound_changes_preserve_pending_evidence() {
    let mut settings = alert_settings();
    let rig = WorkerRig::new(&settings);
    rig.receive();
    rig.step(1000);
    rig.pause.store(true, Ordering::SeqCst);
    rig.advance(2000);
    rig.entered.recv_timeout(Duration::from_secs(5)).unwrap();
    settings.visible_cards.cpu = false;
    settings.alerts.notification_sound = true;
    rig.state.apply_settings(&settings).unwrap();
    rig.release.send(()).unwrap();
    assert_eq!(rig.receive().1.len(), 1);
    assert!(
        !rig.state
            .start_with(&Settings::default(), |_| panic!("Duplicate worker"))
            .unwrap()
    );
    assert!(rig.step(3000).1.is_empty());
}

#[test]
fn collection_failures_and_warmup_reach_the_live_evaluator() {
    let rig = WorkerRig::new(&alert_settings());
    rig.receive();
    *rig.reading.lock().unwrap() = Reading::Warmup;
    let (sample, alerts) = rig.step(1000);
    assert_eq!(sample.health.status, SampleStatus::WarmingUp);
    assert!(alerts.is_empty());
    *rig.reading.lock().unwrap() = Reading::High;
    assert!(rig.step(2000).1.is_empty());
    *rig.reading.lock().unwrap() = Reading::Failed;
    assert!(rig.step(3000).1.is_empty());
    *rig.reading.lock().unwrap() = Reading::High;
    assert!(rig.step(4000).1.is_empty());
    assert!(rig.step(5000).1.is_empty());
    assert_eq!(rig.step(6000).1.len(), 1);
    *rig.reading.lock().unwrap() = Reading::Low;
    assert!(rig.step(7000).1.is_empty());
    *rig.reading.lock().unwrap() = Reading::High;
    for ms in [8000, 9000, 10_000] {
        assert!(rig.step(ms).1.is_empty());
    }
    assert_eq!(rig.step(11_000).1.len(), 1);
}

#[test]
fn settings_changed_during_collection_reject_old_evidence_even_when_changed_back() {
    let mut settings = alert_settings();
    let rig = WorkerRig::new(&settings);
    rig.receive();
    rig.step(1000);
    rig.pause.store(true, Ordering::SeqCst);
    rig.advance(2000);
    rig.entered.recv_timeout(Duration::from_secs(5)).unwrap();
    settings.alerts.cpu.threshold_percent = 99;
    rig.state.apply_settings(&settings).unwrap();
    settings.alerts.cpu.threshold_percent = 90;
    rig.state.apply_settings(&settings).unwrap();
    rig.release.send(()).unwrap();
    let (sample, alerts) = rig.receive();
    assert_eq!(sample.sequence, 3);
    assert!(alerts.is_empty());
    assert!(rig.step(3000).1.is_empty());
    assert!(rig.step(4000).1.is_empty());
    assert_eq!(rig.step(5000).1.len(), 1);
}

#[test]
fn interval_changes_during_collection_preserve_widget_metadata_and_restart_evidence() {
    let mut settings = alert_settings();
    let rig = WorkerRig::new(&settings);
    rig.receive();
    rig.step(1000);
    rig.pause.store(true, Ordering::SeqCst);
    rig.advance(2000);
    rig.entered.recv_timeout(Duration::from_secs(5)).unwrap();
    settings.monitoring_interval_seconds = 5;
    rig.state.apply_settings(&settings).unwrap();
    rig.release.send(()).unwrap();
    let (sample, alerts) = rig.receive();
    assert_eq!(sample.config_revision, 1);
    assert_eq!(sample.interval_ms, 5000);
    assert!(alerts.is_empty());
    assert!(rig.step(7000).1.is_empty());
    assert_eq!(rig.step(12_000).1.len(), 1);
}

#[test]
fn startup_battery_rules_and_disabled_defaults_use_loaded_settings() {
    let defaults = WorkerRig::new(&Settings::default());
    assert!(defaults.receive().1.is_empty());
    assert!(defaults.step(1000).1.is_empty());
    let mut settings = alert_settings();
    settings.alerts.battery_low.enabled = true;
    let enabled = WorkerRig::new(&settings);
    let alerts = enabled.receive().1;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].rule, AlertRule::BatteryLow);
    assert!(enabled.step(1000).1.is_empty());
}

fn alert_rules(alerts: &[EligibleAlert]) -> Vec<AlertRule> {
    alerts.iter().map(|alert| alert.rule).collect()
}

#[test]
fn usage_duration_and_recovery_without_preference_edits_work_at_every_interval() {
    for seconds in [1, 2, 5, 10] {
        let mut settings = alert_settings();
        settings.monitoring_interval_seconds = seconds;
        settings.alerts.cpu.sustained_seconds = 3;
        settings.alerts.memory.enabled = true;
        settings.alerts.memory.sustained_seconds = 3;
        settings.alerts.cooldown_seconds = 1;
        let rig = WorkerRig::new(&settings);
        assert!(rig.receive().1.is_empty());
        let interval = u64::from(seconds) * 1000;
        let duration = 3000_u64.div_ceil(interval) * interval;
        for ms in (interval..=duration).step_by(interval as usize) {
            let (sample, alerts) = rig.step(ms);
            assert_eq!(sample.interval_ms, interval);
            assert_eq!(
                alert_rules(&alerts),
                if ms == duration {
                    vec![AlertRule::Cpu, AlertRule::Memory]
                } else {
                    vec![]
                }
            );
        }
        assert!(rig.step(duration + interval).1.is_empty());
        *rig.reading.lock().unwrap() = Reading::Low;
        assert!(rig.step(duration + 2 * interval).1.is_empty());
        *rig.reading.lock().unwrap() = Reading::High;
        let restarted_at = duration + 3 * interval;
        assert!(rig.step(restarted_at).1.is_empty());
        for elapsed in (interval..=duration).step_by(interval as usize) {
            let alerts = rig.step(restarted_at + elapsed).1;
            assert_eq!(
                alert_rules(&alerts),
                if elapsed == duration {
                    vec![AlertRule::Cpu, AlertRule::Memory]
                } else {
                    vec![]
                }
            );
        }
        assert_eq!(
            rig.state.shared.control.lock().unwrap().evaluation_revision,
            1
        );
    }
}

#[test]
fn long_sampling_gaps_restart_evidence_without_catchup_or_false_recovery() {
    let rig = WorkerRig::new(&alert_settings());
    rig.receive();
    rig.step(1000);
    let (sample, alerts) = rig.step(600_000);
    assert_eq!(sample.sequence, 3);
    assert!(alerts.is_empty());
    assert_eq!(rig.calls.load(Ordering::SeqCst), 3);
    assert_eq!(rig.state.snapshot().unwrap().history.len(), 1);
    assert!(rig.step(601_000).1.is_empty());
    assert_eq!(alert_rules(&rig.step(602_000).1), [AlertRule::Cpu]);
    assert!(rig.step(1_200_000).1.is_empty());
    assert!(rig.step(1_201_000).1.is_empty());
    assert!(rig.step(1_202_000).1.is_empty());
    *rig.reading.lock().unwrap() = Reading::Low;
    assert!(rig.step(1_203_000).1.is_empty());
    *rig.reading.lock().unwrap() = Reading::High;
    assert!(rig.step(1_204_000).1.is_empty());
    assert!(rig.step(1_205_000).1.is_empty());
    assert_eq!(alert_rules(&rig.step(1_206_000).1), [AlertRule::Cpu]);
}

#[test]
fn battery_transitions_failures_and_usage_rules_keep_independent_live_state() {
    let mut settings = alert_settings();
    settings.alerts.memory.enabled = true;
    settings.alerts.memory.sustained_seconds = 2;
    settings.alerts.battery_low.enabled = true;
    settings.alerts.battery_charging.enabled = true;
    let rig = WorkerRig::new(&settings);
    assert_eq!(alert_rules(&rig.receive().1), [AlertRule::BatteryLow]);
    let cases = [
        (
            1000,
            Reading::Battery {
                percentage: 80.0,
                status: "Charging",
            },
            vec![AlertRule::BatteryCharging],
        ),
        (
            2000,
            Reading::Battery {
                percentage: 100.0,
                status: "Full",
            },
            vec![AlertRule::Cpu, AlertRule::Memory],
        ),
        (3000, Reading::Absent, vec![]),
        (4000, Reading::BatteryError, vec![]),
        (
            5000,
            Reading::Battery {
                percentage: 90.0,
                status: "Unknown",
            },
            vec![],
        ),
        (
            6000,
            Reading::Battery {
                percentage: 90.0,
                status: "Charging",
            },
            vec![],
        ),
        (
            7000,
            Reading::Battery {
                percentage: 20.0,
                status: "Discharging",
            },
            vec![AlertRule::BatteryLow],
        ),
        (
            8000,
            Reading::Battery {
                percentage: 80.0,
                status: "Charging",
            },
            vec![AlertRule::BatteryCharging],
        ),
    ];
    for (ms, reading, expected) in cases {
        *rig.reading.lock().unwrap() = reading;
        let (sample, alerts) = rig.step(ms);
        assert_eq!(alert_rules(&alerts), expected);
        if ms == 3000 {
            assert_eq!(sample.health.battery_status, BatteryStatus::Absent);
        }
        if ms == 4000 {
            assert_eq!(sample.health.status, SampleStatus::Degraded);
            assert_eq!(sample.health.battery_status, BatteryStatus::Error);
        }
        for alert in alerts {
            let json = serde_json::to_value(&alert).unwrap();
            let expected_name = match alert.rule {
                AlertRule::Cpu => "cpu",
                AlertRule::Memory => "memory",
                AlertRule::BatteryLow => "batteryLow",
                AlertRule::BatteryCharging => "batteryCharging",
            };
            assert_eq!(json["rule"], expected_name);
        }
    }
}

#[test]
fn new_worker_session_clears_cooldown_and_requires_new_usage_evidence() {
    let mut settings = alert_settings();
    settings.alerts.battery_low.enabled = true;
    {
        let first = WorkerRig::new(&settings);
        assert_eq!(alert_rules(&first.receive().1), [AlertRule::BatteryLow]);
        assert!(first.step(1000).1.is_empty());
        assert_eq!(alert_rules(&first.step(2000).1), [AlertRule::Cpu]);
    }
    let restarted = WorkerRig::new(&settings);
    let (sample, alerts) = restarted.receive();
    assert_eq!(sample.sequence, 1);
    assert_eq!(sample.monotonic_ms, 0);
    assert_eq!(alert_rules(&alerts), [AlertRule::BatteryLow]);
    assert!(restarted.step(1000).1.is_empty());
    assert_eq!(alert_rules(&restarted.step(2000).1), [AlertRule::Cpu]);
    assert!(restarted.step(3000).1.is_empty());
}

#[test]
fn disabling_and_reenabling_a_live_rule_preserves_its_cooldown() {
    let mut settings = alert_settings();
    settings.alerts.cpu.sustained_seconds = 1;
    let rig = WorkerRig::new(&settings);
    rig.receive();
    assert_eq!(alert_rules(&rig.step(1000).1), [AlertRule::Cpu]);
    settings.alerts.cpu.enabled = false;
    rig.apply_at(1250, &settings);
    assert!(rig.step(2000).1.is_empty());
    settings.alerts.cpu.enabled = true;
    rig.apply_at(2500, &settings);
    for ms in [3000, 4000, 5000] {
        assert!(rig.step(ms).1.is_empty());
    }
    assert_eq!(alert_rules(&rig.step(6000).1), [AlertRule::Cpu]);
}

#[test]
fn stop_during_collection_discards_the_inflight_sample_and_eligibility() {
    let rig = WorkerRig::new(&alert_settings());
    rig.receive();
    rig.step(1000);
    rig.pause.store(true, Ordering::SeqCst);
    rig.advance(2000);
    rig.entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let state = Arc::clone(&rig.state);
    let stop = std::thread::spawn(move || state.stop());
    let control = rig.state.shared.control.lock().unwrap();
    let (control, timeout) = rig
        .state
        .shared
        .wake
        .wait_timeout_while(control, Duration::from_secs(5), |control| !control.stopped)
        .unwrap();
    assert!(control.stopped);
    assert!(!timeout.timed_out());
    drop(control);
    rig.release.send(()).unwrap();
    stop.join().unwrap();
    assert!(rig.published.try_recv().is_err());
    assert_eq!(rig.state.snapshot().unwrap().latest.unwrap().sequence, 2);
    assert_eq!(rig.calls.load(Ordering::SeqCst), 3);
}
