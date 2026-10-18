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
                memory_used: 950,
                memory_total: 1000,
                battery: Some(BatteryMetrics {
                    percentage: 10.0,
                    status: "Discharging".into(),
                }),
            },
            battery_error: None,
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
                        |sample, alerts| {
                            let cached = shared_state.snapshot().unwrap();
                            assert_eq!(cached.latest.unwrap().sequence, sample.sequence);
                            publish_events(&handle, sample, alerts);
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
