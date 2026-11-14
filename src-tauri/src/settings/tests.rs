use super::*;
use model::{Appearance, Layout, SETTINGS_VERSION};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tauri::{
    Listener,
    test::{MockRuntime, mock_builder, mock_context, noop_assets},
};

fn app(fail_serialization: bool) -> tauri::App<MockRuntime> {
    let mut plugin = tauri_plugin_store::Builder::default();
    if fail_serialization {
        plugin = plugin.default_serialize_fn(|_| Err("Injected serialization failure".into()));
    }
    mock_builder()
        .invoke_handler(tauri::generate_handler![get_settings, save_settings])
        .plugin(plugin.build())
        .build(mock_context(noop_assets()))
        .unwrap()
}

fn state(app: &tauri::App<MockRuntime>, path: &Path) -> SettingsState {
    SettingsState::at_path(app.handle(), Ok(path.to_owned()))
}

fn changed_settings() -> Settings {
    let mut settings = Settings {
        monitoring_interval_seconds: 5,
        layout: Layout::Compact,
        appearance: Appearance::Dark,
        ..Settings::default()
    };
    settings.visible_cards.battery = false;
    settings.graphs.cpu = false;
    settings.alerts.enabled = true;
    settings.alerts.cpu.threshold_percent = 75;
    settings.alerts.battery_charging.enabled = false;
    settings.history.recording_enabled = true;
    settings
}

fn event_count(app: &tauri::App<MockRuntime>, name: &str) -> Arc<AtomicUsize> {
    let count = Arc::new(AtomicUsize::new(0));
    let listener_count = Arc::clone(&count);
    app.listen(name, move |_| {
        listener_count.fetch_add(1, Ordering::SeqCst);
    });
    count
}

#[test]
fn defaults_and_camel_case_serialization_contract() {
    let settings = Settings::default();
    settings.validate().unwrap();
    let expected = json!({
        "version": 1,
        "monitoringIntervalSeconds": 1,
        "visibleCards": { "cpu": true, "memory": true, "battery": true },
        "layout": "expanded",
        "appearance": "system",
        "graphs": { "cpu": true, "memory": true },
        "alerts": {
            "enabled": false,
            "cpu": { "enabled": true, "thresholdPercent": 90, "sustainedSeconds": 30 },
            "memory": { "enabled": true, "thresholdPercent": 90, "sustainedSeconds": 30 },
            "batteryLow": { "enabled": true, "thresholdPercent": 20 },
            "batteryCharging": { "enabled": true, "thresholdPercent": 80 },
            "cooldownSeconds": 600,
            "notificationSound": false
        },
        "history": { "recordingEnabled": false }
    });
    assert_eq!(serde_json::to_value(&settings).unwrap(), expected);
    assert_eq!(
        serde_json::from_value::<Settings>(expected).unwrap(),
        settings
    );
    let snapshot = SettingsSnapshot {
        settings,
        load_error: None,
        notification_delivery_available: false,
    };
    let value = serde_json::to_value(snapshot).unwrap();
    assert_eq!(value["loadError"], Value::Null);
    assert_eq!(value["notificationDeliveryAvailable"], false);
    let error = SettingsError::new(SettingsErrorCode::SaveFailed, "retry");
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({"code": "saveFailed", "message": "retry"})
    );
}

#[test]
fn missing_top_level_and_nested_fields_receive_contextual_defaults() {
    assert_eq!(
        serde_json::from_value::<Settings>(json!({})).unwrap(),
        Settings::default()
    );
    let partial: Settings = serde_json::from_value(json!({
        "visibleCards": {"cpu": false},
        "alerts": {"cpu": {"enabled": false}, "batteryCharging": {"enabled": false}, "batteryLow": {}}
    })).unwrap();
    assert!(!partial.visible_cards.cpu);
    assert!(partial.visible_cards.memory);
    assert!(!partial.alerts.cpu.enabled);
    assert_eq!(partial.alerts.cpu.threshold_percent, 90);
    assert_eq!(partial.alerts.battery_low.threshold_percent, 20);
    assert_eq!(partial.alerts.battery_charging.threshold_percent, 80);
    assert_eq!(partial.alerts.cooldown_seconds, 600);
    assert!(!partial.history.recording_enabled);
    partial.validate().unwrap();
}

#[test]
fn invalid_json_types_and_unknown_fields_are_rejected() {
    for invalid in [
        json!({"monitoringIntervalSeconds": 1.5}),
        json!({"monitoringIntervalSeconds": -1}),
        json!({"monitoringIntervalSeconds": "5"}),
        json!({"visibleCards": null}),
        json!({"layout": "wide"}),
        json!({"unknownPreference": true}),
        json!({"alerts": {"batteryCharging": {"thresholdPercent": null}}}),
        json!({"alerts": {"cpu": {"thresholdPercent": 256}}}),
        json!({"history": {"recordingEnabled": "true"}}),
    ] {
        assert!(
            serde_json::from_value::<Settings>(invalid.clone()).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn backend_validation_enforces_ranges_even_when_alerts_are_disabled() {
    let defaults = serde_json::to_value(Settings::default()).unwrap();
    for (pointer, invalid_values) in [
        ("/monitoringIntervalSeconds", vec![0, 3, 255]),
        ("/alerts/cpu/thresholdPercent", vec![0, 101]),
        ("/alerts/memory/thresholdPercent", vec![0, 101]),
        ("/alerts/cpu/sustainedSeconds", vec![0, 3601]),
        ("/alerts/memory/sustainedSeconds", vec![0, 3601]),
        ("/alerts/batteryLow/thresholdPercent", vec![0, 101]),
        ("/alerts/batteryCharging/thresholdPercent", vec![0, 101]),
        ("/alerts/cooldownSeconds", vec![0, 86401]),
    ] {
        for value in invalid_values {
            let mut candidate = defaults.clone();
            *candidate.pointer_mut(pointer).unwrap() = json!(value);
            let settings: Settings = serde_json::from_value(candidate).unwrap();
            assert_eq!(
                settings.validate().unwrap_err().code,
                SettingsErrorCode::Validation,
                "{pointer}={value}"
            );
        }
    }
    for version in [0, SETTINGS_VERSION + 1] {
        let settings = Settings {
            version,
            ..Settings::default()
        };
        assert_eq!(
            settings.validate().unwrap_err().code,
            SettingsErrorCode::UnsupportedVersion
        );
    }
}

#[test]
fn validation_accepts_boundaries_intervals_and_all_cards_hidden() {
    for interval in [1, 2, 5, 10] {
        for (threshold, duration, cooldown) in [(1, 1, 1), (100, 3600, 86400)] {
            let mut settings = Settings {
                monitoring_interval_seconds: interval,
                visible_cards: model::VisibleCards {
                    cpu: false,
                    memory: false,
                    battery: false,
                },
                ..Settings::default()
            };
            settings.alerts.cpu.threshold_percent = threshold;
            settings.alerts.memory.threshold_percent = threshold;
            settings.alerts.battery_low.threshold_percent = threshold;
            settings.alerts.battery_charging.threshold_percent = threshold;
            settings.alerts.cpu.sustained_seconds = duration;
            settings.alerts.memory.sustained_seconds = duration;
            settings.alerts.cooldown_seconds = cooldown;
            settings.validate().unwrap();
        }
    }
}

#[test]
fn missing_file_returns_defaults_without_writing() {
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/settings.json");
    let snapshot = state(&app, &path).get(app.handle()).unwrap();
    assert_eq!(snapshot.settings, Settings::default());
    assert!(snapshot.load_error.is_none());
    assert!(!snapshot.notification_delivery_available);
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn saved_preferences_survive_a_new_app_instance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/settings.json");
    let expected = changed_settings();
    {
        let app = app(false);
        let state = state(&app, &path);
        let store_events = event_count(&app, "store://change");
        let mut published = None;
        let result = state
            .save(app.handle(), expected.clone(), |saved| {
                let disk: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                assert_eq!(disk["settings"], serde_json::to_value(saved).unwrap());
                published = Some(saved.clone());
            })
            .unwrap();
        assert_eq!(result, expected);
        assert_eq!(published, Some(expected.clone()));
        assert_eq!(state.get(app.handle()).unwrap().settings, expected);
        assert_eq!(store_events.load(Ordering::SeqCst), 0);
    }
    let restarted = app(false);
    assert_eq!(
        state(&restarted, &path)
            .get(restarted.handle())
            .unwrap()
            .settings,
        expected
    );
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn restored_recording_preferences_preserve_history_and_start_a_new_session() {
    use crate::{
        history::{HistoryState, recorder::query_history},
        monitor::{BatteryStatus, MetricsSample, SampleHealth, SampleStatus},
    };

    let directory = tempfile::tempdir().unwrap();
    let settings_path = directory.path().join("settings.json");
    let history_path = directory.path().join("history.sqlite3");
    let expected = changed_settings();
    let start_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let record = |app: &tauri::App<_>, timestamp_ms| {
        let history = app.state::<HistoryState>();
        let recorder = history.recorder();
        recorder.offer(
            recorder.generation().unwrap(),
            &MetricsSample {
                sequence: 1,
                config_revision: 0,
                timestamp_ms,
                monotonic_ms: 0,
                interval_ms: 5000,
                metrics: None,
                health: SampleHealth {
                    status: SampleStatus::Error,
                    battery_status: BatteryStatus::Unavailable,
                    error: None,
                    battery_error: None,
                },
            },
            5000,
        );
        history.wait_until_idle();
    };
    let read_rows = |app: &tauri::App<_>| {
        let request = serde_json::from_value(json!({
            "startMs": start_ms, "endMs": start_ms + 10_000, "resolution": "raw"
        }))
        .unwrap();
        let result =
            tauri::async_runtime::block_on(query_history(app.handle().clone(), request)).unwrap();
        serde_json::to_value(result).unwrap()["rows"]
            .as_array()
            .unwrap()
            .clone()
    };

    let first_rows = {
        let app = app(false);
        app.manage(state(&app, &settings_path));
        app.manage(HistoryState::at_path(history_path.clone(), false).unwrap());
        tauri::async_runtime::block_on(save_settings(app.handle().clone(), expected.clone()))
            .unwrap();
        record(&app, start_ms);
        let rows = read_rows(&app);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["timestampMs"], start_ms);
        app.state::<HistoryState>().stop();
        rows
    };

    let restarted = app(false);
    let restored = state(&restarted, &settings_path);
    let preferences = restored.current_settings().unwrap();
    assert_eq!(preferences, expected);
    restarted.manage(restored);
    restarted.manage(
        HistoryState::at_path(history_path, preferences.history.recording_enabled).unwrap(),
    );
    assert_eq!(read_rows(&restarted), first_rows);
    record(&restarted, start_ms + 5000);
    let rows = read_rows(&restarted);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], first_rows[0]);
    assert_eq!(rows[1]["timestampMs"], start_ms + 5000);
    assert_eq!(rows[1]["sequence"], rows[0]["sequence"]);
    assert_ne!(rows[1]["sessionId"], rows[0]["sessionId"]);
    restarted.state::<HistoryState>().stop();
}

#[test]
fn partial_store_defaults_do_not_rewrite_original_bytes() {
    use tauri_plugin_store::StoreExt;
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let original = br#"{"settings":{"alerts":{"batteryCharging":{"enabled":false}}}}"#;
    fs::write(&path, original).unwrap();
    let snapshot = state(&app, &path).get(app.handle()).unwrap();
    assert_eq!(
        snapshot.settings.alerts.battery_charging.threshold_percent,
        80
    );
    assert!(!snapshot.settings.alerts.battery_charging.enabled);
    assert!(snapshot.load_error.is_none());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(
        app.get_store(&path).is_none(),
        "Shutdown must never save a cache at the real preferences path"
    );
}

#[test]
fn corrupt_invalid_and_future_data_are_preserved_and_block_saves() {
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    for (bytes, code) in [
        (b"not JSON".as_slice(), SettingsErrorCode::InvalidData),
        (b"\xff\xfe", SettingsErrorCode::InvalidData),
        (br#"{"settings":null}"#, SettingsErrorCode::InvalidData),
        (
            br#"{"settings":{"monitoringIntervalSeconds":3}}"#,
            SettingsErrorCode::InvalidData,
        ),
        (
            br#"{"settings":{"unexpected":true}}"#,
            SettingsErrorCode::InvalidData,
        ),
        (br#"{"otherData":true}"#, SettingsErrorCode::InvalidData),
        (
            br#"{"settings":{"version":2,"layout":[]}}"#,
            SettingsErrorCode::UnsupportedVersion,
        ),
        (
            br#"{"settings":{"version":4294967296}}"#,
            SettingsErrorCode::UnsupportedVersion,
        ),
    ] {
        fs::write(&path, bytes).unwrap();
        let state = state(&app, &path);
        let snapshot = state.get(app.handle()).unwrap();
        assert_eq!(snapshot.settings, Settings::default());
        assert_eq!(snapshot.load_error.unwrap().code, code);
        let error = state
            .save(app.handle(), changed_settings(), |_| {
                panic!("Must not publish")
            })
            .unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn unreadable_path_is_nonfatal_and_can_be_repaired_without_restart() {
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::create_dir(&path).unwrap();
    let state = state(&app, &path);
    assert!(state.get(app.handle()).unwrap().load_error.is_some());
    assert!(
        state
            .save(app.handle(), changed_settings(), |_| panic!(
                "Must not publish"
            ))
            .is_err()
    );
    assert!(path.is_dir());
    fs::remove_dir(&path).unwrap();
    let expected = changed_settings();
    persistence::write(app.handle(), &path, &expected).unwrap();
    let snapshot = state.get(app.handle()).unwrap();
    assert_eq!(snapshot.settings, expected);
    assert!(snapshot.load_error.is_none());
}

#[test]
fn damaged_file_after_startup_blocks_overwrite_and_retains_last_settings() {
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let state = state(&app, &path);
    let saved = state
        .save(app.handle(), changed_settings(), |_| {})
        .unwrap();
    fs::write(&path, b"damaged after startup").unwrap();
    assert!(
        state
            .save(app.handle(), Settings::default(), |_| panic!(
                "Must not publish"
            ))
            .is_err()
    );
    let snapshot = state.get(app.handle()).unwrap();
    assert_eq!(snapshot.settings, saved);
    assert!(snapshot.load_error.is_some());
    assert_eq!(fs::read(&path).unwrap(), b"damaged after startup");
}

#[test]
fn failed_save_preserves_disk_state_and_publishes_nothing() {
    let good_app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let saved = changed_settings();
    persistence::write(good_app.handle(), &path, &saved).unwrap();
    let original = fs::read(&path).unwrap();
    let failed_app = app(true);
    let state = state(&failed_app, &path);
    let store_events = event_count(&failed_app, "store://change");
    for _ in 0..2 {
        let error = state
            .save(failed_app.handle(), Settings::default(), |_| {
                panic!("Must not publish")
            })
            .unwrap_err();
        assert_eq!(error.code, SettingsErrorCode::SaveFailed);
        assert_eq!(state.get(failed_app.handle()).unwrap().settings, saved);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    assert_eq!(store_events.load(Ordering::SeqCst), 0);
    assert_eq!(state_from_restart(&path), saved);
}

fn state_from_restart(path: &Path) -> Settings {
    let app = app(false);
    state(&app, path).get(app.handle()).unwrap().settings
}

#[test]
fn filesystem_write_and_replace_failures_clean_up_candidates() {
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, b"preserve me").unwrap();
    let error = persistence::write(
        app.handle(),
        &blocker.join("settings.json"),
        &Settings::default(),
    )
    .unwrap_err();
    assert_eq!(error.code, SettingsErrorCode::SaveFailed);
    assert_eq!(fs::read(&blocker).unwrap(), b"preserve me");
    let destination = dir.path().join("settings.json");
    fs::create_dir(&destination).unwrap();
    let sentinel = destination.join("keep");
    fs::write(&sentinel, b"original").unwrap();
    let error = persistence::write(app.handle(), &destination, &Settings::default()).unwrap_err();
    assert_eq!(error.code, SettingsErrorCode::SaveFailed);
    assert_eq!(fs::read(&sentinel).unwrap(), b"original");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn validation_failure_does_not_write_or_publish() {
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let state = state(&app, &path);
    let invalid = Settings {
        monitoring_interval_seconds: 3,
        ..Settings::default()
    };
    assert_eq!(
        state
            .save(app.handle(), invalid, |_| panic!("Must not publish"))
            .unwrap_err()
            .code,
        SettingsErrorCode::Validation
    );
    assert!(!path.exists());
    assert_eq!(
        state.get(app.handle()).unwrap().settings,
        Settings::default()
    );
}

#[test]
fn tauri_commands_return_saved_settings_and_emit_after_persistence() {
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    app.manage(state(&app, &path));
    let events = event_count(&app, SETTINGS_CHANGED_EVENT);
    let handle = app.handle().clone();
    let event_path = path.clone();
    app.listen(SETTINGS_CHANGED_EVENT, move |event| {
        let payload: Value = serde_json::from_str(event.payload()).unwrap();
        let disk: Value = serde_json::from_slice(&fs::read(&event_path).unwrap()).unwrap();
        assert_eq!(payload, disk["settings"]);
    });
    let expected = changed_settings();
    let saved =
        tauri::async_runtime::block_on(save_settings(handle.clone(), expected.clone())).unwrap();
    assert_eq!(saved, expected);
    assert_eq!(events.load(Ordering::SeqCst), 1);
    assert_eq!(
        tauri::async_runtime::block_on(get_settings(handle))
            .unwrap()
            .settings,
        expected
    );
}

#[test]
fn failed_save_command_emits_no_change_event() {
    let app = app(true);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    app.manage(state(&app, &path));
    let events = event_count(&app, SETTINGS_CHANGED_EVENT);
    let error =
        tauri::async_runtime::block_on(save_settings(app.handle().clone(), changed_settings()))
            .unwrap_err();
    assert_eq!(error.code, SettingsErrorCode::SaveFailed);
    assert_eq!(events.load(Ordering::SeqCst), 0);
    assert!(!path.exists());
}

#[test]
fn frontend_ipc_validates_input_and_returns_the_serialized_contract() {
    use tauri::{
        ipc::{CallbackFn, InvokeBody},
        webview::InvokeRequest,
    };
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    app.manage(state(&app, &path));
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let invoke = |command: &str, body: Value| {
        tauri::test::get_ipc_response(
            &webview,
            InvokeRequest {
                cmd: command.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
    };
    let response = invoke("get_settings", json!({}))
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
    assert_eq!(response["settings"]["version"], 1);
    assert_eq!(response["loadError"], Value::Null);
    assert_eq!(response["notificationDeliveryAvailable"], false);
    let error = invoke(
        "save_settings",
        json!({"settings": {"monitoringIntervalSeconds": 3}}),
    )
    .unwrap_err();
    assert_eq!(error["code"], "validation");
    assert!(!path.exists());
    let saved = invoke("save_settings", json!({"settings": {"layout": "compact"}}))
        .unwrap()
        .deserialize::<Value>()
        .unwrap();
    assert_eq!(saved["layout"], "compact");
    assert_eq!(saved["alerts"]["batteryCharging"]["thresholdPercent"], 80);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["settings"],
        saved
    );
}

#[test]
fn concurrent_saves_publish_in_persistence_order() {
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let state = Arc::new(state(&app, &path));
    let published = Arc::new(Mutex::new(Vec::new()));
    let mut threads = Vec::new();
    for interval in [1, 2, 5, 10] {
        let handle = app.handle().clone();
        let state = Arc::clone(&state);
        let path = path.clone();
        let published = Arc::clone(&published);
        threads.push(std::thread::spawn(move || {
            let settings = Settings {
                monitoring_interval_seconds: interval,
                ..Settings::default()
            };
            state
                .save(&handle, settings, |saved| {
                    let disk: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                    assert_eq!(disk["settings"], serde_json::to_value(saved).unwrap());
                    published.lock().unwrap().push(saved.clone());
                })
                .unwrap();
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    let published = published.lock().unwrap();
    assert_eq!(published.len(), 4);
    assert_eq!(
        &state.get(app.handle()).unwrap().settings,
        published.last().unwrap()
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn only_successfully_persisted_preferences_reconfigure_the_existing_monitor() {
    use crate::history::HistoryState;
    use crate::monitor::{MonitorState, tests::assert_applied_preferences};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let history_path = dir.path().join("history.sqlite3");
    let good_app = app(false);
    good_app.manage(MonitorState::new());
    good_app.manage(HistoryState::at_path(history_path.clone(), false).unwrap());
    good_app.manage(state(&good_app, &path));
    let saved = tauri::async_runtime::block_on(save_settings(
        good_app.handle().clone(),
        changed_settings(),
    ))
    .unwrap();
    assert_eq!(saved.monitoring_interval_seconds, 5);
    let monitor = good_app.state::<MonitorState>();
    assert!(
        good_app
            .state::<HistoryState>()
            .recorder()
            .generation()
            .is_some()
    );
    assert_applied_preferences(&monitor, &saved);
    assert_eq!(
        good_app
            .state::<SettingsState>()
            .current_settings()
            .unwrap(),
        saved
    );
    assert_eq!(monitor.snapshot().unwrap().monitoring_interval_seconds, 5);

    let mut invalid_alert = saved.clone();
    invalid_alert.alerts.cpu.threshold_percent = 101;
    assert!(
        tauri::async_runtime::block_on(save_settings(good_app.handle().clone(), invalid_alert,))
            .is_err()
    );
    assert_applied_preferences(&monitor, &saved);
    assert!(monitor.snapshot().unwrap().latest.is_none());
    let invalid = Settings {
        monitoring_interval_seconds: 3,
        ..Settings::default()
    };
    assert!(
        tauri::async_runtime::block_on(save_settings(good_app.handle().clone(), invalid)).is_err()
    );
    assert_eq!(monitor.snapshot().unwrap().monitoring_interval_seconds, 5);

    let failed_app = app(true);
    failed_app.manage(MonitorState::new());
    failed_app.manage(HistoryState::at_path(history_path.clone(), true).unwrap());
    failed_app.manage(state(&failed_app, &path));
    failed_app
        .state::<MonitorState>()
        .apply_settings(&saved)
        .unwrap();
    assert!(
        tauri::async_runtime::block_on(save_settings(
            failed_app.handle().clone(),
            Settings::default()
        ))
        .is_err()
    );
    assert_applied_preferences(&failed_app.state::<MonitorState>(), &saved);
    assert!(
        failed_app
            .state::<HistoryState>()
            .recorder()
            .generation()
            .is_some()
    );
    assert!(!history_path.exists());
    assert_eq!(
        failed_app
            .state::<MonitorState>()
            .snapshot()
            .unwrap()
            .monitoring_interval_seconds,
        5
    );
}

#[test]
fn recovering_a_preserved_file_applies_its_rules_without_starting_a_worker() {
    use crate::history::HistoryState;
    use crate::monitor::{MonitorState, tests::assert_applied_preferences};
    let app = app(false);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, b"corrupt").unwrap();
    app.manage(MonitorState::new());
    let history_path = dir.path().join("history.sqlite3");
    app.manage(HistoryState::at_path(history_path.clone(), false).unwrap());
    app.manage(state(&app, &path));
    persistence::write(app.handle(), &path, &changed_settings()).unwrap();
    let recovered = tauri::async_runtime::block_on(get_settings(app.handle().clone())).unwrap();
    assert!(recovered.load_error.is_none());
    assert!(
        app.state::<HistoryState>()
            .recorder()
            .generation()
            .is_some()
    );
    assert!(!history_path.exists());
    let monitor = app.state::<MonitorState>();
    assert_applied_preferences(&monitor, &changed_settings());
    assert_eq!(monitor.snapshot().unwrap().monitoring_interval_seconds, 5);
    assert!(monitor.snapshot().unwrap().latest.is_none());
}

#[test]
fn appearance_defaults_to_system_and_rejects_unknown_choices() {
    let legacy: Settings = serde_json::from_value(json!({ "version": 1 })).unwrap();
    assert_eq!(legacy.appearance, Appearance::System);
    assert!(serde_json::from_value::<Settings>(json!({ "appearance": "sepia" })).is_err());
}
