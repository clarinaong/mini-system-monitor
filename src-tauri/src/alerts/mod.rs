use crate::{
    monitor::{BatteryStatus, MetricsSample, SampleStatus},
    settings::{Settings, SettingsError},
};
use serde::Serialize;
use std::time::Duration;

pub const ALERT_ELIGIBLE_EVENT: &str = "alert-eligible";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AlertRule {
    Cpu,
    Memory,
    BatteryLow,
    BatteryCharging,
}

const RULES: [AlertRule; 4] = [
    AlertRule::Cpu,
    AlertRule::Memory,
    AlertRule::BatteryLow,
    AlertRule::BatteryCharging,
];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EligibleAlert {
    pub rule: AlertRule,
    pub sample_sequence: u64,
    pub sample_monotonic_ms: u64,
    pub evaluated_monotonic_ms: u64,
    pub value_percent: f64,
    pub threshold_percent: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RuleConfig {
    enabled: bool,
    threshold_percent: u8,
    sustained_ms: u64,
}

fn rule_configs(settings: &Settings) -> [RuleConfig; 4] {
    let alerts = &settings.alerts;
    [
        RuleConfig {
            enabled: alerts.enabled && alerts.cpu.enabled,
            threshold_percent: alerts.cpu.threshold_percent,
            sustained_ms: u64::from(alerts.cpu.sustained_seconds) * 1000,
        },
        RuleConfig {
            enabled: alerts.enabled && alerts.memory.enabled,
            threshold_percent: alerts.memory.threshold_percent,
            sustained_ms: u64::from(alerts.memory.sustained_seconds) * 1000,
        },
        RuleConfig {
            enabled: alerts.enabled && alerts.battery_low.enabled,
            threshold_percent: alerts.battery_low.threshold_percent,
            sustained_ms: 0,
        },
        RuleConfig {
            enabled: alerts.enabled && alerts.battery_charging.enabled,
            threshold_percent: alerts.battery_charging.threshold_percent,
            sustained_ms: 0,
        },
    ]
}

#[derive(Default)]
struct RuleState {
    pending_since_ms: Option<u64>,
    emitted_for_episode: bool,
    last_eligible_at: Option<Duration>,
    configured_at: Duration,
}

impl RuleState {
    fn reset_configuration(&mut self, now: Duration) {
        self.pending_since_ms = None;
        self.emitted_for_episode = false;
        self.configured_at = now;
    }

    fn observe(
        &mut self,
        condition: Option<bool>,
        sample_ms: u64,
        now: Duration,
        sustained_ms: u64,
        cooldown: Duration,
    ) -> bool {
        match condition {
            None => self.pending_since_ms = None,
            Some(false) => {
                self.pending_since_ms = None;
                self.emitted_for_episode = false;
            }
            Some(true) => {
                let since = *self.pending_since_ms.get_or_insert(sample_ms);
                if !self.emitted_for_episode
                    && sample_ms.saturating_sub(since) >= sustained_ms
                    && self
                        .last_eligible_at
                        .is_none_or(|last| now.saturating_sub(last) >= cooldown)
                {
                    self.emitted_for_episode = true;
                    self.last_eligible_at = Some(now);
                    return true;
                }
            }
        }
        false
    }
}

pub struct AlertEvaluator<C> {
    clock: C,
    configs: [RuleConfig; 4],
    states: [RuleState; 4],
    interval_ms: u64,
    cooldown: Duration,
    last_sample: Option<(u64, u64)>,
    last_now: Duration,
}

impl<C: Fn() -> Duration> AlertEvaluator<C> {
    pub fn new(settings: &Settings, clock: C) -> Result<Self, SettingsError> {
        settings.validate()?;
        let now = clock();
        Ok(Self {
            clock,
            configs: rule_configs(settings),
            states: std::array::from_fn(|_| RuleState {
                configured_at: now,
                ..RuleState::default()
            }),
            interval_ms: u64::from(settings.monitoring_interval_seconds) * 1000,
            cooldown: Duration::from_secs(settings.alerts.cooldown_seconds.into()),
            last_sample: None,
            last_now: now,
        })
    }

    pub fn update_settings(&mut self, settings: &Settings) -> Result<bool, SettingsError> {
        settings.validate()?;
        let now = (self.clock)().max(self.last_now);
        let configs = rule_configs(settings);
        let cooldown = Duration::from_secs(settings.alerts.cooldown_seconds.into());
        let interval_ms = u64::from(settings.monitoring_interval_seconds) * 1000;
        let changed =
            configs != self.configs || cooldown != self.cooldown || interval_ms != self.interval_ms;
        for (index, state) in self.states.iter_mut().enumerate() {
            if configs[index] != self.configs[index] || cooldown != self.cooldown {
                state.reset_configuration(now);
            } else if interval_ms != self.interval_ms {
                state.pending_since_ms = None;
                state.configured_at = now;
            }
        }
        self.configs = configs;
        self.interval_ms = interval_ms;
        self.cooldown = cooldown;
        self.last_now = now;
        Ok(changed)
    }

    fn break_evidence(&mut self) {
        for state in &mut self.states {
            state.pending_since_ms = None;
        }
    }

    pub fn evaluate(&mut self, sample: Option<&MetricsSample>) -> Vec<EligibleAlert> {
        let now = (self.clock)();
        if now < self.last_now {
            self.break_evidence();
            return Vec::new();
        }
        self.last_now = now;
        let Some(sample) = sample else {
            self.break_evidence();
            return Vec::new();
        };
        if self.last_sample.is_some_and(|(sequence, timestamp)| {
            sample.sequence <= sequence || sample.monotonic_ms <= timestamp
        }) {
            if self.last_sample == Some((sample.sequence, sample.monotonic_ms))
                && now.saturating_sub(Duration::from_millis(sample.monotonic_ms))
                    >= Duration::from_millis(self.interval_ms * 2 + 2000)
            {
                self.break_evidence();
            }
            return Vec::new();
        }
        let sampled_at = Duration::from_millis(sample.monotonic_ms);
        if sampled_at > now {
            self.break_evidence();
            return Vec::new();
        }
        if self.last_sample.is_some_and(|(sequence, timestamp)| {
            sample.sequence != sequence.saturating_add(1)
                || sample.monotonic_ms - timestamp > self.interval_ms * 3 / 2 + 250
        }) {
            self.break_evidence();
        }
        self.last_sample = Some((sample.sequence, sample.monotonic_ms));
        if now - sampled_at >= Duration::from_millis(self.interval_ms * 2 + 2000)
            || sample.interval_ms != self.interval_ms
            || sample.health.status == SampleStatus::Error
            || sample.health.error.is_some()
        {
            self.break_evidence();
            return Vec::new();
        }
        let Some(metrics) = &sample.metrics else {
            self.break_evidence();
            return Vec::new();
        };
        let cpu = metrics
            .cpu_usage
            .filter(|_| sample.health.status != SampleStatus::WarmingUp)
            .map(f64::from)
            .filter(|value| valid_percent(*value));
        let memory = (metrics.memory_total > 0 && metrics.memory_used <= metrics.memory_total)
            .then(|| metrics.memory_used as f64 / metrics.memory_total as f64 * 100.0);
        let battery = metrics.battery.as_ref().filter(|battery| {
            sample.health.battery_status == BatteryStatus::Available
                && sample.health.battery_error.is_none()
                && valid_percent(battery.percentage.into())
        });
        let battery_value = battery.map(|battery| f64::from(battery.percentage));
        let battery_state = battery.and_then(|battery| match battery.status.as_str() {
            "Discharging" => Some(false),
            "Charging" | "Full" => Some(true),
            _ => None,
        });
        let values = [cpu, memory, battery_value, battery_value];
        let mut eligible = Vec::new();
        for (index, rule) in RULES.into_iter().enumerate() {
            let config = self.configs[index];
            let state = &mut self.states[index];
            if !config.enabled || sampled_at < state.configured_at {
                state.pending_since_ms = None;
                continue;
            }
            let condition = values[index].and_then(|value| {
                let threshold = f64::from(config.threshold_percent);
                match rule {
                    AlertRule::Cpu => Some(value >= threshold),
                    AlertRule::Memory => Some(
                        u128::from(metrics.memory_used) * 100
                            >= u128::from(metrics.memory_total)
                                * u128::from(config.threshold_percent),
                    ),
                    AlertRule::BatteryLow => {
                        battery_state.map(|charging| !charging && value <= threshold)
                    }
                    AlertRule::BatteryCharging => {
                        battery_state.map(|charging| charging && value >= threshold)
                    }
                }
            });
            if state.observe(
                condition,
                sample.monotonic_ms,
                now,
                config.sustained_ms,
                self.cooldown,
            ) {
                eligible.push(EligibleAlert {
                    rule,
                    sample_sequence: sample.sequence,
                    sample_monotonic_ms: sample.monotonic_ms,
                    evaluated_monotonic_ms: now.as_millis() as u64,
                    value_percent: values[index].unwrap(),
                    threshold_percent: config.threshold_percent,
                });
            }
        }
        eligible
    }
}

fn valid_percent(value: f64) -> bool {
    value.is_finite() && (0.0..=100.0).contains(&value)
}

#[cfg(test)]
mod tests;
