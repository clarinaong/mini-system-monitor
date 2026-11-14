use super::*;
use std::sync::atomic::AtomicUsize;

struct Recorder {
    permission: Permission,
    status_calls: AtomicUsize,
    requests: AtomicUsize,
    messages: Mutex<Vec<Message>>,
    fail_status: bool,
    fail_submission: AtomicBool,
}

impl Recorder {
    fn new(permission: Permission) -> Self {
        Self {
            permission,
            status_calls: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            messages: Mutex::new(Vec::new()),
            fail_status: false,
            fail_submission: AtomicBool::new(false),
        }
    }
}

impl Backend for Recorder {
    fn permission(&self) -> Result<Permission, NotificationError> {
        self.status_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_status {
            return Err(NotificationError::new("statusUnavailable", "injected"));
        }
        Ok(self.permission)
    }
    fn request_permission(&self) -> Result<(), NotificationError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn submit(&self, message: &Message) -> Result<(), NotificationError> {
        self.messages.lock().unwrap().push(message.clone());
        if self.fail_submission.swap(false, Ordering::SeqCst) {
            return Err(NotificationError::new("submissionFailed", "injected"));
        }
        Ok(())
    }
}

fn settings() -> Settings {
    let mut settings = Settings::default();
    settings.alerts.enabled = true;
    settings
}

fn alert(rule: AlertRule) -> EligibleAlert {
    EligibleAlert {
        rule,
        sample_sequence: 1,
        sample_monotonic_ms: 1000,
        evaluated_monotonic_ms: 1000,
        value_percent: 95.5,
        threshold_percent: 90,
    }
}

#[test]
fn all_rule_messages_use_observed_values_and_saved_sound_but_test_stays_silent() {
    for sound in [false, true] {
        let backend = Recorder::new(Permission::Authorized);
        let mut settings = settings();
        settings.alerts.notification_sound = sound;
        let alerts = [
            AlertRule::Cpu,
            AlertRule::Memory,
            AlertRule::BatteryLow,
            AlertRule::BatteryCharging,
        ]
        .map(alert);
        deliver_batch(&backend, &alerts, |_| Some(settings.clone()));
        let messages = backend.messages.lock().unwrap();
        assert_eq!(messages.len(), 4);
        for (message, title) in messages.iter().zip([
            "Mini System Monitor — High CPU usage",
            "Mini System Monitor — High memory usage",
            "Mini System Monitor — Low battery",
            "Charge target reached.",
        ]) {
            assert_eq!(message.title, title);
            assert!(message.body.contains("95.5%"));
            assert!(message.body.contains("90%"));
            assert_eq!(message.sound, sound);
        }
        assert!(!Message::test().sound);
        assert_eq!(backend.requests.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn unauthorized_delivery_never_prompts_or_replays_after_permission_changes() {
    for permission in [
        Permission::Denied,
        Permission::NotDetermined,
        Permission::Unavailable,
        Permission::Unknown,
        Permission::Authorized,
        Permission::Provisional,
    ] {
        let backend = Recorder::new(permission);
        deliver_batch(&backend, &[alert(AlertRule::Memory)], |_| Some(settings()));
        assert_eq!(
            backend.messages.lock().unwrap().len(),
            usize::from(permission.allows_submission())
        );
        assert_eq!(backend.requests.load(Ordering::SeqCst), 0);
        deliver_batch(&backend, &[], |_| Some(settings()));
        assert_eq!(backend.status_calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn master_and_each_rule_disable_prevent_even_permission_queries() {
    for rule in [
        AlertRule::Cpu,
        AlertRule::Memory,
        AlertRule::BatteryLow,
        AlertRule::BatteryCharging,
    ] {
        for master in [false, true] {
            let backend = Recorder::new(Permission::Authorized);
            let mut settings = settings();
            settings.alerts.enabled = master;
            match rule {
                AlertRule::Cpu => settings.alerts.cpu.enabled = false,
                AlertRule::Memory => settings.alerts.memory.enabled = false,
                AlertRule::BatteryLow => settings.alerts.battery_low.enabled = false,
                AlertRule::BatteryCharging => settings.alerts.battery_charging.enabled = false,
            }
            deliver_batch(&backend, &[alert(rule)], |_| Some(settings.clone()));
            assert_eq!(backend.status_calls.load(Ordering::SeqCst), 0);
            assert!(backend.messages.lock().unwrap().is_empty());
        }
    }
}

#[test]
fn configuration_or_freshness_changes_during_permission_check_cancel_submission() {
    for disable in [false, true] {
        let backend = Recorder::new(Permission::Authorized);
        let mut calls = 0;
        deliver_batch(&backend, &[alert(AlertRule::Cpu)], |_| {
            calls += 1;
            if calls == 1 {
                return Some(settings());
            }
            if !disable {
                return None;
            }
            let mut settings = settings();
            settings.alerts.enabled = false;
            Some(settings)
        });
        assert_eq!(backend.status_calls.load(Ordering::SeqCst), 1);
        assert!(backend.messages.lock().unwrap().is_empty());
    }
}

#[test]
fn sound_change_during_permission_check_uses_latest_saved_choice() {
    let backend = Recorder::new(Permission::Authorized);
    let mut calls = 0;
    deliver_batch(&backend, &[alert(AlertRule::Cpu)], |_| {
        calls += 1;
        let mut settings = settings();
        settings.alerts.notification_sound = calls > 1;
        Some(settings)
    });
    assert!(backend.messages.lock().unwrap()[0].sound);
}

#[test]
fn submission_and_status_failures_are_nonfatal_and_not_retried() {
    let backend = Recorder::new(Permission::Authorized);
    backend.fail_submission.store(true, Ordering::SeqCst);
    deliver_batch(
        &backend,
        &[alert(AlertRule::Cpu), alert(AlertRule::Memory)],
        |_| Some(settings()),
    );
    assert_eq!(backend.messages.lock().unwrap().len(), 2);
    let backend = Recorder {
        fail_status: true,
        ..Recorder::new(Permission::Authorized)
    };
    deliver_batch(&backend, &[alert(AlertRule::Cpu)], |_| Some(settings()));
    assert!(backend.messages.lock().unwrap().is_empty());
    assert_eq!(backend.requests.load(Ordering::SeqCst), 0);
}

#[test]
fn future_and_exactly_stale_alerts_are_rejected_for_all_intervals() {
    let alert = alert(AlertRule::Cpu);
    for interval in [1, 2, 5, 10] {
        let mut settings = settings();
        settings.monitoring_interval_seconds = interval;
        let stale = 1000 + u64::from(interval) * 2000 + 2000;
        assert!(!fresh(&alert, &settings, 999));
        assert!(fresh(&alert, &settings, 1000));
        assert!(fresh(&alert, &settings, stale - 1));
        assert!(!fresh(&alert, &settings, stale));
    }
}

#[test]
fn single_batch_reservation_is_released_after_early_exit_or_unwind() {
    let busy = Arc::new(AtomicBool::new(false));
    let reservation = Reservation::acquire(&busy).unwrap();
    assert!(Reservation::acquire(&busy).is_none());
    drop(reservation);
    let cloned = Arc::clone(&busy);
    let _ = std::panic::catch_unwind(move || {
        let _reservation = Reservation::acquire(&cloned).unwrap();
        panic!("injected");
    });
    assert!(Reservation::acquire(&busy).is_some());
}

#[test]
fn dispatch_delivers_without_windows_and_never_queues_behind_a_busy_batch() {
    use std::sync::mpsc;
    use tauri::test::{mock_builder, mock_context, noop_assets};
    struct Blocking {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        sent: mpsc::Sender<Message>,
    }
    impl Backend for Blocking {
        fn permission(&self) -> Result<Permission, NotificationError> {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            Ok(Permission::Authorized)
        }
        fn request_permission(&self) -> Result<(), NotificationError> {
            panic!("Automatic alerts must never prompt")
        }
        fn submit(&self, message: &Message) -> Result<(), NotificationError> {
            self.sent.send(message.clone()).unwrap();
            Ok(())
        }
    }
    let app = mock_builder().build(mock_context(noop_assets())).unwrap();
    let monitor = crate::monitor::MonitorState::new();
    let mut initial = settings();
    initial.monitoring_interval_seconds = 10;
    monitor.apply_settings(&initial).unwrap();
    app.manage(monitor);
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (sent, messages) = mpsc::channel();
    let state = NotificationState {
        backend: Arc::new(Blocking {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            sent,
        }),
        operation: Arc::new(Mutex::new(())),
        delivery_busy: Arc::new(AtomicBool::new(false)),
    };
    let mut alert = alert(AlertRule::Memory);
    alert.sample_monotonic_ms = 0;
    let first = state
        .dispatch(app.handle().clone(), vec![alert.clone()], 1)
        .unwrap();
    entered
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(
        state
            .dispatch(app.handle().clone(), vec![alert.clone()], 1)
            .is_none()
    );
    assert!(
        app.state::<crate::monitor::MonitorState>()
            .snapshot()
            .is_ok()
    );
    release.send(()).unwrap();
    tauri::async_runtime::block_on(first).unwrap();
    assert!(
        messages
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .title
            .contains("memory")
    );
    assert!(messages.try_recv().is_err());
    assert!(!state.delivery_busy.load(Ordering::SeqCst));
    let second = state
        .dispatch(app.handle().clone(), vec![alert], 1)
        .unwrap();
    entered
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let mut disabled = settings();
    disabled.alerts.enabled = false;
    app.state::<crate::monitor::MonitorState>()
        .apply_settings(&disabled)
        .unwrap();
    release.send(()).unwrap();
    tauri::async_runtime::block_on(second).unwrap();
    assert!(messages.try_recv().is_err());
}
