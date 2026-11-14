mod collector;
mod sampler;

use crate::{
    alerts::{ALERT_ELIGIBLE_EVENT, AlertEvaluator, EligibleAlert},
    history::{HistoryRecorder, HistoryState},
    settings::Settings,
};
use collector::HardwareCollector;
use sampler::{RealClock, Sampler};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};
use tauri::{AppHandle, Emitter, Manager, Runtime};

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
    evaluation_revision: u64,
    evaluator: AlertEvaluator<Box<dyn Fn() -> std::time::Duration + Send>>,
    stopped: bool,
    settings: Settings,
}

struct Shared {
    clock: RealClock,
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
        let clock = RealClock::new();
        Self::with_clock(clock, move || clock.elapsed())
    }

    fn with_clock(
        clock: RealClock,
        evaluation_clock: impl Fn() -> std::time::Duration + Send + 'static,
    ) -> Self {
        let evaluation_clock: Box<dyn Fn() -> std::time::Duration + Send> =
            Box::new(evaluation_clock);
        Self {
            shared: Arc::new(Shared {
                clock,
                control: Mutex::new(Control {
                    interval_seconds: 1,
                    revision: 0,
                    evaluation_revision: 0,
                    evaluator: AlertEvaluator::new(&Settings::default(), evaluation_clock)
                        .expect("Default preferences must be valid"),
                    stopped: false,
                    settings: Settings::default(),
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
        settings: &Settings,
    ) -> Result<bool, String> {
        let app = app.clone();
        let history = app
            .try_state::<HistoryState>()
            .map(|history| history.recorder());
        let interval_seconds = settings.monitoring_interval_seconds;
        self.start_with(settings, move |shared| {
            let sampler = Sampler::new(HardwareCollector::new(), shared.clock, interval_seconds);
            run_worker(
                shared,
                sampler,
                history.as_ref(),
                |sample, alerts, revision| {
                    publish_events(&app, sample, alerts, revision);
                },
            );
        })
    }

    fn start_with(
        &self,
        settings: &Settings,
        run: impl FnOnce(Arc<Shared>) + Send + 'static,
    ) -> Result<bool, String> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| "Monitor worker lock is unavailable")?;
        if worker.is_some() {
            return Ok(false);
        }
        self.apply_settings(settings)?;
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

    pub fn apply_settings(&self, settings: &Settings) -> Result<(), String> {
        settings.validate().map_err(|error| error.to_string())?;
        let mut control = self
            .shared
            .control
            .lock()
            .map_err(|_| "Monitor control lock is unavailable")?;
        let changed = control
            .evaluator
            .update_settings(settings)
            .map_err(|error| error.to_string())?;
        control.settings = settings.clone();
        if changed {
            control.evaluation_revision += 1;
            self.shared.wake.notify_one();
        }
        if control.interval_seconds != settings.monitoring_interval_seconds {
            control.interval_seconds = settings.monitoring_interval_seconds;
            control.revision += 1;
            self.shared.wake.notify_one();
        }
        Ok(())
    }

    pub(crate) fn delivery_settings(&self, revision: u64) -> Option<Settings> {
        let control = self.shared.control.lock().ok()?;
        (!control.stopped && control.evaluation_revision == revision)
            .then(|| control.settings.clone())
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
                snapshot.monotonic_now_ms = self.shared.clock.elapsed().as_millis() as u64;
                snapshot.monitoring_interval_seconds = control.interval_seconds;
                snapshot.config_revision = control.revision;
                snapshot
            })
            .map_err(|_| "Metrics cache is unavailable".into())
    }

    #[cfg(any(target_os = "macos", test))]
    pub fn recheck_deadline(&self) {
        if let Ok(_control) = self.shared.control.lock() {
            self.shared.wake.notify_one();
        }
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

fn publish_events<R: Runtime>(
    app: &AppHandle<R>,
    sample: &MetricsSample,
    alerts: &[EligibleAlert],
    revision: u64,
) {
    if let Err(error) = app.emit(METRICS_EVENT, sample) {
        eprintln!("Cannot deliver system metrics: {error}");
    }
    for alert in alerts {
        if let Err(error) = app.emit(ALERT_ELIGIBLE_EVENT, alert) {
            eprintln!("Cannot publish eligible alert: {error}");
        }
    }
    if !alerts.is_empty()
        && let Some(notifications) = app.try_state::<crate::notifications::NotificationState>()
    {
        notifications.dispatch(app.clone(), alerts.to_vec(), revision);
    }
}

fn run_worker<C: collector::Collector, K: sampler::Clock>(
    shared: Arc<Shared>,
    mut sampler: Sampler<C, K>,
    history: Option<&HistoryRecorder>,
    publish: impl Fn(&MetricsSample, &[EligibleAlert], u64),
) {
    let mut detector = crate::incidents::IncidentDetector::default();
    let mut recording_start = None;
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
        let evaluation_revision = control.evaluation_revision;
        drop(control);
        let recording_generation = history.and_then(HistoryRecorder::generation);
        if let Some(mut sample) = sampler.sample_due() {
            let collection_interval_ms = sample.interval_ms;
            let transitions = detector.observe(&sample);
            if recording_start.map(|(generation, _)| generation) != recording_generation {
                recording_start =
                    recording_generation.map(|generation| (generation, sample.sequence));
            }
            let Ok(mut control) = shared.control.lock() else {
                break;
            };
            if control.stopped {
                break;
            }
            sample.config_revision = control.revision;
            sample.interval_ms = u64::from(control.interval_seconds) * 1000;
            sampler.set_interval(control.interval_seconds);
            let alerts = if evaluation_revision == control.evaluation_revision {
                control.evaluator.evaluate(Some(&sample))
            } else {
                control.evaluator.evaluate(None)
            };
            let Ok(mut cache) = shared.cache.lock() else {
                break;
            };
            cache.record(sample.clone());
            drop(cache);
            drop(control);
            if let (Some(history), Some((generation, first_sequence))) = (history, recording_start)
            {
                history.offer(generation, &sample, collection_interval_ms);
                if !transitions.is_empty() {
                    history.offer_events(
                        generation,
                        transitions
                            .into_iter()
                            .filter(|event| event.start_sequence() >= first_sequence),
                    );
                }
            }
            publish(&sample, &alerts, evaluation_revision);
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
pub(crate) mod tests;

#[cfg(test)]
mod alert_tests;
