use super::{BatteryStatus, MetricsSample, SampleHealth, SampleStatus, collector::Collector};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(super) trait Clock {
    fn monotonic(&self) -> Duration;
    fn timestamp_ms(&self) -> u64;
}

pub(super) struct RealClock {
    start: Instant,
}

impl RealClock {
    pub fn new(start: Instant) -> Self {
        Self { start }
    }
}

impl Clock for RealClock {
    fn monotonic(&self) -> Duration {
        self.start.elapsed()
    }
    fn timestamp_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

pub(super) struct Sampler<C, K> {
    collector: C,
    clock: K,
    interval: Duration,
    next_due: Duration,
    sequence: u64,
}

impl<C: Collector, K: Clock> Sampler<C, K> {
    pub fn new(collector: C, clock: K, interval_seconds: u8) -> Self {
        let next_due = clock.monotonic();
        Self {
            collector,
            clock,
            interval: Duration::from_secs(interval_seconds.into()),
            next_due,
            sequence: 0,
        }
    }

    pub fn set_interval(&mut self, seconds: u8) {
        let interval = Duration::from_secs(seconds.into());
        if interval != self.interval {
            self.interval = interval;
            self.next_due = self.clock.monotonic() + interval;
        }
    }

    pub fn delay(&self) -> Duration {
        self.next_due.saturating_sub(self.clock.monotonic())
    }

    pub fn sample_due(&mut self) -> Option<MetricsSample> {
        if !self.delay().is_zero() {
            return None;
        }
        let now = self.clock.monotonic();
        let result = catch_unwind(AssertUnwindSafe(|| self.collector.collect(now)))
            .unwrap_or_else(|_| Err("System metrics collection failed unexpectedly".into()));
        let completed = self.clock.monotonic();
        self.next_due = completed + self.interval;
        self.sequence += 1;
        let (metrics, health) = match result {
            Ok(collected) => {
                let battery_status = if collected.battery_error.is_some() {
                    BatteryStatus::Error
                } else if collected.metrics.battery.is_some() {
                    BatteryStatus::Available
                } else {
                    BatteryStatus::Absent
                };
                let status = if collected.battery_error.is_some() {
                    SampleStatus::Degraded
                } else if collected.metrics.cpu_usage.is_none() {
                    SampleStatus::WarmingUp
                } else {
                    SampleStatus::Healthy
                };
                (
                    Some(collected.metrics),
                    SampleHealth {
                        status,
                        error: None,
                        battery_status,
                        battery_error: collected.battery_error,
                    },
                )
            }
            Err(error) => (
                None,
                SampleHealth {
                    status: SampleStatus::Error,
                    error: Some(error),
                    battery_status: BatteryStatus::Unavailable,
                    battery_error: None,
                },
            ),
        };
        Some(MetricsSample {
            sequence: self.sequence,
            config_revision: 0,
            timestamp_ms: self.clock.timestamp_ms(),
            monotonic_ms: completed.as_millis() as u64,
            interval_ms: self.interval.as_millis() as u64,
            metrics,
            health,
        })
    }
}
