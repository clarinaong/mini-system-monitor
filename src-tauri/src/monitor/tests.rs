use super::*;
use collector::{CollectedMetrics, Collector};
use sampler::Clock;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

fn settings(interval: u8) -> Settings {
    Settings {
        monitoring_interval_seconds: interval,
        ..Settings::default()
    }
}

pub(crate) fn assert_applied_preferences(monitor: &MonitorState, expected: &Settings) {
    let mut control = monitor.shared.control.lock().unwrap();
    assert_eq!(
        control.interval_seconds,
        expected.monitoring_interval_seconds
    );
    assert!(!control.evaluator.update_settings(expected).unwrap());
}

#[derive(Clone, Default)]
struct FakeClock {
    millis: Rc<Cell<u64>>,
    wall_offset: Rc<Cell<i64>>,
}

impl FakeClock {
    fn set(&self, millis: u64) {
        self.millis.set(millis);
    }
}

impl Clock for FakeClock {
    fn monotonic(&self) -> Duration {
        Duration::from_millis(self.millis.get())
    }
    fn timestamp_ms(&self) -> u64 {
        (1_800_000_000_000_i64 + self.millis.get() as i64 + self.wall_offset.get()) as u64
    }
}

enum Outcome {
    Ready,
    Warmup,
    BatteryError,
    Error,
    Panic,
}

struct FakeCollector {
    clock: FakeClock,
    calls: Rc<RefCell<Vec<u64>>>,
    duration_ms: u64,
    outcomes: VecDeque<Outcome>,
}

impl Collector for FakeCollector {
    fn collect(&mut self, now: Duration) -> Result<CollectedMetrics, String> {
        self.calls.borrow_mut().push(now.as_millis() as u64);
        self.clock.set(self.clock.millis.get() + self.duration_ms);
        let outcome = self.outcomes.pop_front().unwrap_or(Outcome::Ready);
        match outcome {
            Outcome::Error => return Err("Injected collection failure".into()),
            Outcome::Panic => panic!("Injected collector panic"),
            _ => {}
        }
        Ok(CollectedMetrics {
            metrics: SystemMetrics {
                cpu_usage: if matches!(outcome, Outcome::Warmup) {
                    None
                } else {
                    Some(42.0)
                },
                memory_used: 512,
                memory_total: 1024,
                battery: None,
            },
            battery_error: matches!(outcome, Outcome::BatteryError)
                .then(|| "Battery read failed".into()),
        })
    }
}

fn sampler(
    interval: u8,
    duration_ms: u64,
    outcomes: Vec<Outcome>,
) -> (
    Sampler<FakeCollector, FakeClock>,
    FakeClock,
    Rc<RefCell<Vec<u64>>>,
) {
    let clock = FakeClock::default();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let collector = FakeCollector {
        clock: clock.clone(),
        calls: Rc::clone(&calls),
        duration_ms,
        outcomes: outcomes.into(),
    };
    (
        Sampler::new(collector, clock.clone(), interval),
        clock,
        calls,
    )
}

#[test]
fn sampling_is_due_once_per_interval_and_sequences_increase() {
    let (mut sampler, clock, calls) = sampler(1, 0, vec![]);
    let first = sampler.sample_due().unwrap();
    assert_eq!(first.sequence, 1);
    assert_eq!(first.monotonic_ms, 0);
    assert!(sampler.sample_due().is_none());
    clock.set(999);
    assert!(sampler.sample_due().is_none());
    clock.set(1000);
    let second = sampler.sample_due().unwrap();
    assert_eq!(second.sequence, 2);
    assert_eq!(second.monotonic_ms, 1000);
    assert_eq!(second.timestamp_ms - first.timestamp_ms, 1000);
    assert_eq!(&*calls.borrow(), &[0, 1000]);
}

#[test]
fn slow_collection_waits_a_full_interval_after_completion() {
    let (mut sampler, clock, calls) = sampler(1, 2500, vec![]);
    let sample = sampler.sample_due().unwrap();
    assert_eq!(sample.monotonic_ms, 2500);
    assert_eq!(sampler.delay(), Duration::from_millis(1000));
    assert!(sampler.sample_due().is_none());
    clock.set(3499);
    assert!(sampler.sample_due().is_none());
    clock.set(3500);
    sampler.sample_due().unwrap();
    assert_eq!(&*calls.borrow(), &[0, 3500]);
}

#[test]
fn interval_changes_replace_deadline_and_identical_values_do_not_postpone_it() {
    let (mut sampler, clock, calls) = sampler(1, 0, vec![]);
    sampler.sample_due().unwrap();
    clock.set(500);
    sampler.set_interval(5);
    clock.set(1000);
    assert!(sampler.sample_due().is_none());
    clock.set(4500);
    sampler.set_interval(5);
    assert_eq!(sampler.delay(), Duration::from_secs(1));
    clock.set(5500);
    assert_eq!(sampler.sample_due().unwrap().interval_ms, 5000);
    clock.set(5600);
    sampler.set_interval(1);
    clock.set(6599);
    assert!(sampler.sample_due().is_none());
    clock.set(6600);
    assert_eq!(sampler.sample_due().unwrap().interval_ms, 1000);
    assert_eq!(&*calls.borrow(), &[0, 5500, 6600]);
}

#[test]
fn late_wakeup_collects_once_without_catchup_and_ignores_wall_clock_changes() {
    let (mut sampler, clock, calls) = sampler(1, 0, vec![]);
    sampler.sample_due().unwrap();
    clock.set(600_000);
    clock.wall_offset.set(-3_600_000);
    let resumed = sampler.sample_due().unwrap();
    assert_eq!(resumed.sequence, 2);
    assert_eq!(resumed.monotonic_ms, 600_000);
    assert!(sampler.sample_due().is_none());
    assert_eq!(sampler.delay(), Duration::from_secs(1));
    assert_eq!(calls.borrow().len(), 2);
}

#[test]
fn collection_errors_and_panics_publish_health_then_recover() {
    let (mut sampler, clock, _) = sampler(
        1,
        0,
        vec![
            Outcome::Ready,
            Outcome::Error,
            Outcome::Panic,
            Outcome::Ready,
        ],
    );
    let mut cache = MonitorSnapshot::default();
    for index in 0..4 {
        clock.set(index * 1000);
        let sample = sampler.sample_due().unwrap();
        if index == 1 || index == 2 {
            assert!(sample.metrics.is_none());
            assert_eq!(sample.health.status, SampleStatus::Error);
            assert!(sample.health.error.is_some());
        }
        cache.record(sample);
        if index == 1 || index == 2 {
            assert_eq!(cache.last_successful.as_ref().unwrap().sequence, 1);
        }
    }
    assert_eq!(cache.latest.as_ref().unwrap().sequence, 4);
    assert_eq!(cache.last_successful.as_ref().unwrap().sequence, 4);
    assert_eq!(cache.latest.unwrap().health.status, SampleStatus::Healthy);
}

#[test]
fn cpu_warmup_and_missing_battery_are_distinct_from_battery_failure() {
    let (mut sampler, clock, _) = sampler(
        1,
        0,
        vec![Outcome::Warmup, Outcome::Ready, Outcome::BatteryError],
    );
    let warmup = sampler.sample_due().unwrap();
    assert!(warmup.metrics.unwrap().cpu_usage.is_none());
    assert_eq!(warmup.health.status, SampleStatus::WarmingUp);
    assert_eq!(warmup.health.battery_status, BatteryStatus::Absent);
    clock.set(1000);
    let absent = sampler.sample_due().unwrap();
    assert_eq!(absent.health.status, SampleStatus::Healthy);
    assert_eq!(absent.health.battery_status, BatteryStatus::Absent);
    clock.set(2000);
    let failed = sampler.sample_due().unwrap();
    assert!(failed.metrics.unwrap().cpu_usage.is_some());
    assert_eq!(failed.health.status, SampleStatus::Degraded);
    assert_eq!(failed.health.battery_status, BatteryStatus::Error);
    assert_eq!(
        failed.health.battery_error.as_deref(),
        Some("Battery read failed")
    );
}

#[test]
fn history_is_bounded_by_elapsed_time_and_count_with_last_success_preserved() {
    let (mut sampler, clock, _) = sampler(1, 0, vec![]);
    let mut cache = MonitorSnapshot::default();
    for index in 0..125 {
        clock.set(index * 1000);
        cache.record(sampler.sample_due().unwrap());
    }
    assert_eq!(cache.history.len(), 61);
    assert_eq!(cache.history.front().unwrap().monotonic_ms, 64_000);
    clock.set(300_000);
    cache.record(sampler.sample_due().unwrap());
    assert_eq!(cache.history.len(), 1);
    let mut failed = cache.latest.clone().unwrap();
    failed.sequence += 1;
    failed.monotonic_ms = 500_000;
    failed.metrics = None;
    cache.record(failed);
    assert_eq!(cache.history.len(), 1);
    assert_eq!(cache.last_successful.unwrap().monotonic_ms, 300_000);
}

#[test]
fn serialized_samples_preserve_units_and_health_contract() {
    let (mut sampler, _, _) = sampler(5, 0, vec![Outcome::Warmup]);
    let json = serde_json::to_value(sampler.sample_due().unwrap()).unwrap();
    assert_eq!(json["metrics"]["memoryUsed"], 512);
    assert_eq!(json["metrics"]["cpuUsage"], serde_json::Value::Null);
    assert_eq!(json["metrics"]["battery"], serde_json::Value::Null);
    assert_eq!(json["health"]["status"], "warmingUp");
    assert_eq!(json["health"]["batteryStatus"], "absent");
    assert_eq!(json["intervalMs"], 5000);
    assert_eq!(json["sequence"], 1);
}

#[test]
fn one_worker_survives_multiple_cache_readers_and_window_creation() {
    use tauri::{
        Manager,
        test::{mock_builder, mock_context, noop_assets},
    };
    let app = mock_builder().build(mock_context(noop_assets())).unwrap();
    app.manage(MonitorState::new());
    let state = app.state::<MonitorState>();
    let runs = Arc::new(AtomicUsize::new(0));
    let run_count = Arc::clone(&runs);
    let (started, received) = mpsc::channel();
    assert!(
        state
            .start_with(&settings(5), move |shared| {
                run_count.fetch_add(1, Ordering::SeqCst);
                started.send(()).unwrap();
                let mut control = shared.control.lock().unwrap();
                while !control.stopped {
                    control = shared.wake.wait(control).unwrap();
                }
            })
            .unwrap()
    );
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    for label in ["main", "settings-test"] {
        let _window = tauri::WebviewWindowBuilder::new(&app, label, Default::default())
            .build()
            .unwrap();
        for _ in 0..5 {
            assert!(state.snapshot().unwrap().latest.is_none());
        }
        assert!(
            !state
                .start_with(&settings(1), |_| panic!("A second worker must not start"))
                .unwrap()
        );
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(state.snapshot().unwrap().monitoring_interval_seconds, 5);
    state.stop();
    state.stop();
    assert!(state.start_with(&settings(1), |_| {}).is_err());
}

#[test]
fn hardware_smoke_test_suppresses_first_cpu_value_and_keeps_memory_units() {
    let mut collector = HardwareCollector::new();
    let first = collector.collect(Duration::ZERO).unwrap();
    assert!(first.metrics.cpu_usage.is_none());
    assert!(first.metrics.memory_total > 0);
    assert!(first.metrics.memory_used <= first.metrics.memory_total);
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    let second = collector
        .collect(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
        .unwrap();
    assert!((0.0..=100.0).contains(&second.metrics.cpu_usage.unwrap()));
    if let Some(battery) = second.metrics.battery {
        assert!((0.0..=100.0).contains(&battery.percentage));
    }
}

#[test]
fn worker_caches_before_broadcast_and_recovers_for_multiple_subscribers() {
    use tauri::{
        Listener, Manager,
        test::{mock_builder, mock_context, noop_assets},
    };

    struct ThreadClock(Arc<AtomicUsize>);
    impl Clock for ThreadClock {
        fn monotonic(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::SeqCst) as u64)
        }
        fn timestamp_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst) as u64
        }
    }
    struct ThreadCollector(Arc<AtomicUsize>);
    impl Collector for ThreadCollector {
        fn collect(&mut self, now: Duration) -> Result<CollectedMetrics, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            if now == Duration::from_secs(1) {
                return Err("Temporary failure".into());
            }
            Ok(CollectedMetrics {
                metrics: SystemMetrics {
                    cpu_usage: Some(25.0),
                    memory_used: 10,
                    memory_total: 20,
                    battery: None,
                },
                battery_error: None,
            })
        }
    }

    let app = mock_builder().build(mock_context(noop_assets())).unwrap();
    app.manage(MonitorState::new());
    let (sender, received) = mpsc::channel();
    for subscriber in ["widget", "other-window"] {
        let sender = sender.clone();
        app.listen(METRICS_EVENT, move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            sender.send((subscriber, payload)).unwrap();
        });
    }
    let now = Arc::new(AtomicUsize::new(0));
    let clock = ThreadClock(Arc::clone(&now));
    let collections = Arc::new(AtomicUsize::new(0));
    let collector = ThreadCollector(Arc::clone(&collections));
    let app_handle = app.handle().clone();
    let state = app.state::<MonitorState>();
    state
        .start_with(&settings(1), move |shared| {
            run_worker(shared, Sampler::new(collector, clock, 1), |sample, _, _| {
                let cached = app_handle.state::<MonitorState>().snapshot().unwrap();
                assert_eq!(cached.latest.unwrap().sequence, sample.sequence);
                app_handle.emit(METRICS_EVENT, sample).unwrap();
            });
        })
        .unwrap();
    for step in 0..3 {
        if step > 0 {
            let _control = state.shared.control.lock().unwrap();
            now.store(step * 1000, Ordering::SeqCst);
            state.shared.wake.notify_one();
        }
        let first = received.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_ne!(first.0, second.0);
        assert_eq!(first.1["sequence"], step + 1);
        assert_eq!(first.1, second.1);
        assert_eq!(
            first.1["health"]["status"],
            if step == 1 { "error" } else { "healthy" }
        );
        assert_eq!(collections.load(Ordering::SeqCst), step + 1);
        for _ in 0..10 {
            assert_eq!(
                state.snapshot().unwrap().latest.unwrap().sequence,
                (step + 1) as u64
            );
        }
        assert_eq!(collections.load(Ordering::SeqCst), step + 1);
    }
    state.stop();
}

#[test]
fn delivery_revision_rejects_changed_and_restored_rules_and_shutdown() {
    let state = MonitorState::new();
    let mut settings = Settings::default();
    settings.alerts.enabled = true;
    state.apply_settings(&settings).unwrap();
    let revision = state.shared.control.lock().unwrap().evaluation_revision;
    assert!(state.delivery_settings(revision).is_some());
    settings.alerts.notification_sound = true;
    state.apply_settings(&settings).unwrap();
    assert!(
        state
            .delivery_settings(revision)
            .unwrap()
            .alerts
            .notification_sound
    );
    settings.alerts.cpu.sustained_seconds += 1;
    state.apply_settings(&settings).unwrap();
    settings.alerts.cpu.sustained_seconds -= 1;
    state.apply_settings(&settings).unwrap();
    assert!(state.delivery_settings(revision).is_none());
    let revision = state.shared.control.lock().unwrap().evaluation_revision;
    assert!(state.delivery_settings(revision).is_some());
    state.stop();
    assert!(state.delivery_settings(revision).is_none());
}
