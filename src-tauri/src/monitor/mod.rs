mod collector;
mod sampler;

use collector::HardwareCollector;
use sampler::{RealClock, Sampler};
use serde::Serialize;
use std::time::Instant;
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};
use tauri::{AppHandle, Emitter, Runtime};

pub const METRICS_EVENT: &str = "system-metrics";
pub const HISTORY_WINDOW_MS: u64 = 60_000;
const MAX_SAMPLES: usize = 61;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetrics {
    pub cpu_usage: Option<f32>,
    pub memory_used: u64,
    pub memory_total: u64,
    pub battery: Option<BatteryMetrics>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryMetrics {
    pub percentage: f32,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SampleStatus {
    Healthy,
    WarmingUp,
    Degraded,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BatteryStatus {
    Available,
    Absent,
    Error,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleHealth {
    pub status: SampleStatus,
    pub error: Option<String>,
    pub battery_status: BatteryStatus,
    pub battery_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSample {
    pub sequence: u64,
    pub config_revision: u64,
    pub timestamp_ms: u64,
    pub monotonic_ms: u64,
    pub interval_ms: u64,
    pub metrics: Option<SystemMetrics>,
    pub health: SampleHealth,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    pub monotonic_now_ms: u64,
    pub config_revision: u64,
    pub monitoring_interval_seconds: u8,
    pub latest: Option<MetricsSample>,
    pub last_successful: Option<MetricsSample>,
    pub history: VecDeque<MetricsSample>,
}

impl MonitorSnapshot {
    fn record(&mut self, sample: MetricsSample) {
        let cutoff = sample.monotonic_ms.saturating_sub(HISTORY_WINDOW_MS);
        while self
            .history
            .front()
            .is_some_and(|old| old.monotonic_ms < cutoff)
        {
            self.history.pop_front();
        }
        if sample.metrics.is_some() {
            self.last_successful = Some(sample.clone());
        }
        self.latest = Some(sample.clone());
        self.history.push_back(sample);
        while self.history.len() > MAX_SAMPLES {
            self.history.pop_front();
        }
    }
}

struct Control {
    interval_seconds: u8,
    revision: u64,
    stopped: bool,
}

struct Shared {
    started: Instant,
    control: Mutex<Control>,
    wake: Condvar,
    cache: Mutex<MonitorSnapshot>,
}

pub struct MonitorState {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl MonitorState {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                started: Instant::now(),
                control: Mutex::new(Control {
                    interval_seconds: 1,
                    revision: 0,
                    stopped: false,
                }),
                wake: Condvar::new(),
                cache: Mutex::new(MonitorSnapshot::default()),
            }),
            worker: Mutex::new(None),
        }
    }

    pub fn start<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        interval_seconds: u8,
    ) -> Result<bool, String> {
        let app = app.clone();
        self.start_with(interval_seconds, move |shared| {
            let sampler = Sampler::new(
                HardwareCollector::new(),
                RealClock::new(shared.started),
                interval_seconds,
            );
            run_worker(shared, sampler, |sample| {
                if let Err(error) = app.emit(METRICS_EVENT, sample) {
                    eprintln!("Cannot deliver system metrics: {error}");
                }
            });
        })
    }

    fn start_with(
        &self,
        interval_seconds: u8,
        run: impl FnOnce(Arc<Shared>) + Send + 'static,
    ) -> Result<bool, String> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| "Monitor worker lock is unavailable")?;
        if worker.is_some() {
            return Ok(false);
        }
        self.set_interval(interval_seconds)?;
        let shared = Arc::clone(&self.shared);
        if shared
            .control
            .lock()
            .map_err(|_| "Monitor control lock is unavailable")?
            .stopped
        {
            return Err("Monitoring has stopped".into());
        }
        *worker = Some(
            thread::Builder::new()
                .name("system-monitor".into())
                .spawn(move || run(shared))
                .map_err(|error| error.to_string())?,
        );
        Ok(true)
    }

    pub fn set_interval(&self, seconds: u8) -> Result<(), String> {
        if ![1, 2, 5, 10].contains(&seconds) {
            return Err("Invalid monitoring interval".into());
        }
        let mut control = self
            .shared
            .control
            .lock()
            .map_err(|_| "Monitor control lock is unavailable")?;
        if control.interval_seconds != seconds {
            control.interval_seconds = seconds;
            control.revision += 1;
            self.shared.wake.notify_one();
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Result<MonitorSnapshot, String> {
        let control = self
            .shared
            .control
            .lock()
            .map_err(|_| "Monitor control lock is unavailable")?;
        self.shared
            .cache
            .lock()
            .map(|cache| {
                let mut snapshot = cache.clone();
                snapshot.monotonic_now_ms = self.shared.started.elapsed().as_millis() as u64;
                snapshot.monitoring_interval_seconds = control.interval_seconds;
                snapshot.config_revision = control.revision;
                snapshot
            })
            .map_err(|_| "Metrics cache is unavailable".into())
    }

    pub fn stop(&self) {
        if let Ok(mut control) = self.shared.control.lock() {
            control.stopped = true;
            self.shared.wake.notify_one();
        }
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

fn run_worker<C: collector::Collector, K: sampler::Clock>(
    shared: Arc<Shared>,
    mut sampler: Sampler<C, K>,
    publish: impl Fn(&MetricsSample),
) {
    loop {
        let Ok(control) = shared.control.lock() else {
            break;
        };
        if control.stopped {
            break;
        }
        sampler.set_interval(control.interval_seconds);
        let delay = sampler.delay();
        if !delay.is_zero() {
            if shared.wake.wait_timeout(control, delay).is_err() {
                break;
            }
            continue;
        }
        drop(control);
        if let Some(mut sample) = sampler.sample_due() {
            let Ok(control) = shared.control.lock() else {
                break;
            };
            if control.stopped {
                break;
            }
            sample.config_revision = control.revision;
            sample.interval_ms = u64::from(control.interval_seconds) * 1000;
            let Ok(mut cache) = shared.cache.lock() else {
                break;
            };
            cache.record(sample.clone());
            drop(cache);
            drop(control);
            publish(&sample);
        }
    }
}

impl Drop for MonitorState {
    fn drop(&mut self) {
        self.stop();
    }
}

#[tauri::command]
pub fn get_system_metrics(
    state: tauri::State<'_, MonitorState>,
) -> Result<MonitorSnapshot, String> {
    state.snapshot()
}

#[cfg(test)]
mod tests;
