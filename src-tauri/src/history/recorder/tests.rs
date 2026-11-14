use super::*;
use crate::monitor::{BatteryMetrics, BatteryStatus, SampleHealth, SampleStatus, SystemMetrics};
use rusqlite::Connection;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;

fn sample(sequence: u64, monotonic_ms: u64) -> MetricsSample {
    MetricsSample {
        sequence,
        monotonic_ms,
        timestamp_ms: u64::try_from(retention_cutoff() + RETENTION_MS).unwrap(),
        config_revision: 0,
        interval_ms: 1_000,
        metrics: Some(SystemMetrics {
            cpu_usage: Some(25.0),
            memory_used: 10,
            memory_total: 20,
            swap_used: 0,
            battery: None,
        }),
        health: SampleHealth {
            status: SampleStatus::Healthy,
            error: None,
            battery_status: BatteryStatus::Absent,
            battery_error: None,
        },
    }
}

fn rows(path: &Path) -> Vec<(i64, i64)> {
    Connection::open(path)
        .unwrap()
        .prepare("SELECT session_id, sequence FROM metric_samples ORDER BY session_id, sequence")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn disabled_recording_and_settings_clear_leave_absent_database_untouched() {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![clear_history])
        .build(mock_context(noop_assets()))
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("new").join("history.sqlite3");
    assert!(app.manage(HistoryState::at_path(path.clone(), false).unwrap()));
    let state = app.state::<HistoryState>();
    assert!(state.recorder().generation().is_none());
    for label in ["main", "settings"] {
        let window = tauri::WebviewWindowBuilder::new(&app, label, Default::default())
            .build()
            .unwrap();
        let result = tauri::test::get_ipc_response(
            &window,
            InvokeRequest {
                cmd: "clear_history".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(serde_json::json!({})),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        );
        assert_eq!(result.is_ok(), label == "settings");
    }
    state.stop();
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn cadence_uses_monotonic_time_and_clear_discards_previous_generation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite3");
    let state = HistoryState::at_path(path.clone(), false).unwrap();
    let recorder = state.recorder();
    assert!(recorder.generation().is_none());
    state.set_enabled(true);
    let first = recorder.generation().unwrap();
    recorder.offer(first, &sample(1, 0), 1_000);
    state.wait_until_idle();
    recorder.offer(first, &sample(2, 4_999), 1_000);
    recorder.offer(first, &sample(3, 5_000), 1_000);
    state.wait_until_idle();
    assert_eq!(rows(&path), vec![(1, 1), (1, 3)]);
    state.set_enabled(false);
    state.set_enabled(true);
    let second = recorder.generation().unwrap();
    recorder.offer(first, &sample(4, 10_000), 1_000);
    recorder.offer(second, &sample(5, 10_000), 1_000);
    state.wait_until_idle();
    assert_eq!(rows(&path), vec![(1, 1), (1, 3), (2, 5)]);
    state.clear().unwrap();
    assert!(rows(&path).is_empty());
    let third = recorder.generation().unwrap();
    recorder.offer(second, &sample(6, 15_000), 1_000);
    recorder.offer(third, &sample(7, 15_000), 1_000);
    state.wait_until_idle();
    assert_eq!(rows(&path), vec![(3, 7)]);
    state.set_enabled(false);
    state.clear().unwrap();
    assert!(rows(&path).is_empty());
    assert!(recorder.generation().is_none());
}

#[test]
fn mailbox_holds_at_most_one_waiting_sample_even_when_storage_is_busy() {
    let shared = Arc::new(Shared::default());
    shared.pending.lock().unwrap().enabled = true;
    let recorder = HistoryRecorder(Arc::clone(&shared));
    let generation = recorder.generation().unwrap();
    let next_prune = Instant::now() + PRUNE_INTERVAL;
    recorder.offer(generation, &sample(1, 0), 2_000);
    assert!(matches!(
        next_work(&shared, next_prune),
        Some(Work::Write(
            _,
            RecordingWrite::Sample(MetricsSample {
                sequence: 1,
                interval_ms: 2_000,
                ..
            })
        ))
    ));
    recorder.offer(generation, &sample(2, 5_000), 1_000);
    recorder.offer(generation, &sample(3, 10_000), 1_000);
    let pending = shared.pending.lock().unwrap();
    assert!(pending.writing);
    assert_eq!(pending.sample.as_ref().unwrap().1.sequence, 2);
    assert_eq!(pending.last_sample_ms, Some(5_000));
}

#[test]
fn clear_invalidates_waiting_and_inflight_collection_before_acknowledging() {
    let shared = Arc::new(Shared::default());
    shared.pending.lock().unwrap().enabled = true;
    let recorder = HistoryRecorder(Arc::clone(&shared));
    let state = HistoryState {
        recorder: recorder.clone(),
        worker: Mutex::new(None),
    };
    let generation = recorder.generation().unwrap();
    let next_prune = Instant::now() + PRUNE_INTERVAL;
    recorder.offer(generation, &sample(1, 0), 1_000);
    assert!(matches!(
        next_work(&shared, next_prune),
        Some(Work::Write(_, RecordingWrite::Sample(_)))
    ));
    recorder.offer(generation, &sample(2, 5_000), 1_000);
    let response = state.queue_clear().unwrap();
    let mut pending = shared.pending.lock().unwrap();
    assert!(pending.sample.is_none());
    pending.writing = false;
    drop(pending);
    assert!(recorder.generation().is_none());
    recorder.offer(generation, &sample(3, 10_000), 1_000);
    assert!(shared.pending.lock().unwrap().sample.is_none());
    let Some(Work::Clear(reply)) = next_work(&shared, next_prune) else {
        panic!("clear must take precedence over recording");
    };
    assert_eq!(response.try_recv(), Err(mpsc::TryRecvError::Empty));
    assert!(state.queue_clear().is_err());
    reply.send(Ok(())).unwrap();
    assert_eq!(response.try_recv(), Ok(Ok(())));
}

#[test]
fn existing_history_is_pruned_while_disabled_and_retains_the_cutoff() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite3");
    let mut database = HistoryDatabase::open(&path).unwrap();
    let mut older = sample(1, 0);
    older.timestamp_ms = 99;
    database.record(None, &older).unwrap();
    let mut boundary = sample(2, 5_000);
    boundary.timestamp_ms = 100;
    database.record(None, &boundary).unwrap();
    drop(database);

    let mut storage = Storage {
        path: path.clone(),
        database: None,
        session: None,
        writer_lock: None,
    };
    storage.prune(None, 100).unwrap();
    assert_eq!(rows(&path), vec![(2, 2)]);
    let connection = Connection::open(&path).unwrap();
    let sessions: i64 = connection
        .query_row("SELECT count(*) FROM recording_sessions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(sessions, 1);
    let empty = directory.path().join("empty.sqlite3");
    fs::write(&empty, []).unwrap();
    storage = Storage {
        path: empty.clone(),
        database: None,
        session: None,
        writer_lock: None,
    };
    assert!(storage.prune(None, 100).is_err());
    assert_eq!(fs::metadata(empty).unwrap().len(), 0);
}

#[test]
fn recording_maps_optional_readings_and_preserves_error_gaps() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite3");
    let mut database = HistoryDatabase::open(&path).unwrap();
    let mut warming = sample(1, 0);
    let metrics = warming.metrics.as_mut().unwrap();
    metrics.cpu_usage = None;
    metrics.battery = Some(BatteryMetrics {
        percentage: 0.0,
        status: "Discharging".into(),
    });
    warming.health.status = SampleStatus::WarmingUp;
    warming.health.battery_status = BatteryStatus::Available;
    let session = database.record(None, &warming).unwrap();
    let mut failed = sample(2, 5_000);
    failed.metrics = None;
    failed.health.status = SampleStatus::Error;
    failed.health.error = Some("private details".into());
    failed.health.battery_status = BatteryStatus::Unavailable;
    database.record(Some(session), &failed).unwrap();
    let mut too_large = sample(3, 10_000);
    too_large.metrics.as_mut().unwrap().memory_total = u64::MAX;
    assert!(database.record(None, &too_large).is_err());

    let connection = Connection::open(&path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT cpu_percent, memory_used_bytes, battery_percent, battery_state,
                    sample_status, battery_status FROM metric_samples ORDER BY sequence",
        )
        .unwrap();
    let mut rows = statement.query([]).unwrap();
    let warming = rows.next().unwrap().unwrap();
    assert_eq!(warming.get::<_, Option<f32>>(0).unwrap(), None);
    assert_eq!(warming.get::<_, Option<i64>>(1).unwrap(), Some(10));
    assert_eq!(warming.get::<_, Option<f32>>(2).unwrap(), Some(0.0));
    assert_eq!(warming.get::<_, String>(3).unwrap(), "Discharging");
    assert_eq!(warming.get::<_, String>(4).unwrap(), "warmingUp");
    assert_eq!(warming.get::<_, String>(5).unwrap(), "available");
    let failed = rows.next().unwrap().unwrap();
    assert_eq!(failed.get::<_, Option<f32>>(0).unwrap(), None);
    assert_eq!(failed.get::<_, Option<i64>>(1).unwrap(), None);
    assert_eq!(failed.get::<_, Option<f32>>(2).unwrap(), None);
    assert_eq!(failed.get::<_, Option<String>>(3).unwrap(), None);
    assert_eq!(failed.get::<_, String>(4).unwrap(), "error");
    assert_eq!(failed.get::<_, String>(5).unwrap(), "unavailable");
    assert!(rows.next().unwrap().is_none());
}

fn raw_request(start: i64, end: i64) -> QueryRequest {
    serde_json::from_value(serde_json::json!({
        "startMs": start, "endMs": end, "resolution": "raw",
    }))
    .unwrap()
}

#[test]
fn history_command_is_lazy_and_reads_stored_rows_with_recording_off() {
    use crate::incidents::{IncidentKind, Observation};

    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            query_history,
            query_incidents,
            query_performance_summary,
            clear_history
        ])
        .build(tauri::generate_context!(test = true))
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("new").join("history.sqlite3");
    assert!(app.manage(HistoryState::at_path(path.clone(), false).unwrap()));
    let window = crate::windows::open_history(app.handle()).unwrap();
    let invoke = |window: &tauri::WebviewWindow<_>, cmd: &str, body| {
        tauri::test::get_ipc_response(
            window,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
    };
    let query = |start, end, resolution| {
        invoke(
            &window,
            "query_history",
            serde_json::json!({
                "request": {"startMs": start, "endMs": end, "resolution": resolution}
            }),
        )
        .unwrap()
        .deserialize::<serde_json::Value>()
        .unwrap()
    };

    assert_eq!(
        query(0, 1, "raw"),
        serde_json::json!({"resolution": "raw", "rows": []})
    );
    let incidents = invoke(
        &window,
        "query_incidents",
        serde_json::json!({"request": {"startMs": 0, "endMs": 1}}),
    )
    .unwrap()
    .deserialize::<serde_json::Value>()
    .unwrap();
    assert_eq!(incidents, serde_json::json!([]));
    let summary_request = |boundary| {
        serde_json::json!({
            "request": {
                "previous": {"startMs": boundary - 1, "endMs": boundary},
                "current": {"startMs": boundary, "endMs": boundary + 1}
            }
        })
    };
    let summary = |boundary| {
        invoke(
            &window,
            "query_performance_summary",
            summary_request(boundary),
        )
        .unwrap()
        .deserialize::<serde_json::Value>()
        .unwrap()
    };
    let empty = summary(1_i64);
    assert_eq!(empty["current"]["totalMinutes"], 1);
    assert!(empty["current"]["cpu"]["averagePercent"].is_null());
    assert_eq!(empty["current"]["cpu"]["coveragePercent"], 0.0);
    assert_eq!(empty["cpuComparison"], "periodTooShort");
    assert!(!path.parent().unwrap().exists());

    let state = app.state::<HistoryState>();
    state.set_enabled(true);
    let recorder = state.recorder();
    let sample = sample(1, 0);
    let generation = recorder.generation().unwrap();
    recorder.offer(generation, &sample, 1_000);
    let start = Observation::from(&sample);
    let end = Observation {
        sequence: 2,
        monotonic_ms: 5_000,
        timestamp_ms: start.timestamp_ms + 5_000,
    };
    recorder.offer_events(
        generation,
        [
            IncidentTransition::Started {
                kind: IncidentKind::CpuSpike,
                start,
                detected: start,
            },
            IncidentTransition::Recovered {
                kind: IncidentKind::CpuSpike,
                start,
                end,
            },
        ],
    );
    state.wait_until_idle();
    state.set_enabled(false);

    let timestamp = i64::try_from(sample.timestamp_ms).unwrap();
    let result = query(timestamp, timestamp + 1, "raw");
    assert_eq!(result["resolution"], "raw");
    assert_eq!(result["rows"].as_array().unwrap().len(), 1);
    assert_eq!(result["rows"][0]["sessionId"], 1);
    assert_eq!(result["rows"][0]["sequence"], 1);
    assert_eq!(result["rows"][0]["cpuPercent"], 25.0);
    assert!(result["rows"][0]["batteryPercent"].is_null());

    let averages = query(timestamp, timestamp + 1, "minuteAverages");
    assert_eq!(averages["rows"][0]["cpuPercent"]["average"], 25.0);
    let incident_request = serde_json::json!({
        "request": {"startMs": timestamp, "endMs": timestamp + 1}
    });
    let incidents = invoke(&window, "query_incidents", incident_request.clone())
        .unwrap()
        .deserialize::<serde_json::Value>()
        .unwrap();
    assert_eq!(incidents.as_array().unwrap().len(), 1);
    assert_eq!(incidents[0]["sessionId"], result["rows"][0]["sessionId"]);
    assert_eq!(incidents[0]["startSequence"], result["rows"][0]["sequence"]);
    assert_eq!(incidents[0]["startMs"], result["rows"][0]["timestampMs"]);
    assert_eq!(incidents[0]["status"], "recovered");
    assert_eq!(incidents[0]["durationMs"], 5_000);
    assert_eq!(incidents[0]["endMs"], end.timestamp_ms);

    let saved = summary(timestamp);
    assert_eq!(saved["current"]["period"]["startMs"], timestamp);
    assert_eq!(saved["current"]["cpu"]["peakPercent"], 25.0);
    assert_eq!(saved["current"]["cpu"]["coveragePercent"], 100.0);
    assert_eq!(saved["previous"]["cpu"]["coveragePercent"], 0.0);

    let settings = crate::windows::open_settings(app.handle()).unwrap();
    let denial = invoke(
        &settings,
        "query_performance_summary",
        summary_request(timestamp),
    )
    .unwrap_err();
    assert!(denial.as_str().unwrap().contains("not allowed"));
    let denial = invoke(&settings, "query_incidents", incident_request).unwrap_err();
    assert!(denial.as_str().unwrap().contains("not allowed"));
    let denial = invoke(&window, "clear_history", serde_json::json!({})).unwrap_err();
    assert!(denial.as_str().unwrap().contains("not allowed"));
}

#[test]
fn query_mailbox_rejects_requests_while_waiting_and_running() {
    let shared = Arc::new(Shared::default());
    let state = HistoryState {
        recorder: HistoryRecorder(Arc::clone(&shared)),
        worker: Mutex::new(None),
    };
    let request = || raw_request(0, 1).validate().unwrap();
    let incident_request = || {
        serde_json::from_value::<IncidentRequest>(serde_json::json!({"startMs": 0, "endMs": 1}))
            .unwrap()
            .validate()
            .unwrap()
    };
    let summary_request = || {
        serde_json::from_value::<SummaryRequest>(serde_json::json!({
            "previous": {"startMs": 0, "endMs": 1},
            "current": {"startMs": 1, "endMs": 2}
        }))
        .unwrap()
        .validate()
        .unwrap()
    };
    let mut response = state
        .queue_read(|reply| Read::Metrics(request(), reply))
        .unwrap();
    assert!(matches!(
        state.queue_read(|reply| Read::Incidents(incident_request(), reply)),
        Err(QueryError::Busy)
    ));
    let Some(Work::Read(Read::Metrics(_, reply))) =
        next_work(&shared, Instant::now() + PRUNE_INTERVAL)
    else {
        panic!("expected queued query");
    };
    assert!(matches!(
        state.queue_read(|reply| Read::Summary(summary_request(), reply)),
        Err(QueryError::Busy)
    ));
    drop(reply);
    assert!(async_runtime::block_on(response.recv()).is_none());
}

#[test]
fn event_mailbox_bounds_pending_writes_and_rejects_stale_generations() {
    use crate::incidents::IncidentKind;

    let recorder = HistoryRecorder(Arc::new(Shared {
        pending: Mutex::new(Pending {
            enabled: true,
            generation: 1,
            ..Pending::default()
        }),
        ..Shared::default()
    }));
    let event = |sequence| {
        let mut metrics = sample(sequence, sequence * 1000);
        metrics.timestamp_ms = metrics.monotonic_ms;
        let start = (&metrics).into();
        IncidentTransition::Started {
            kind: IncidentKind::CpuSpike,
            start,
            detected: start,
        }
    };
    recorder.offer_events(0, [event(1)]);
    assert!(recorder.0.pending.lock().unwrap().events.is_empty());

    recorder.offer_events(1, (1..=64).map(event));
    recorder.offer_events(1, [event(65)]);
    let pending = recorder.0.pending.lock().unwrap();
    assert_eq!(pending.events.len(), MAX_PENDING_EVENTS);
    assert_eq!(pending.events.front(), Some(&(1, event(1))));
    assert_eq!(
        pending.events.back(),
        Some(&(1, event(MAX_PENDING_EVENTS as u64)))
    );
}

#[test]
fn restart_interrupts_only_unresolved_events_and_new_sessions_can_recover() {
    use crate::incidents::IncidentKind;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite3");
    let start = (&sample(1, 0)).into();
    let end = (&sample(2, 1000)).into();
    let spike = IncidentTransition::Started {
        kind: IncidentKind::CpuSpike,
        start,
        detected: start,
    };
    let recovery = IncidentTransition::Recovered {
        kind: IncidentKind::CpuSpike,
        start,
        end,
    };
    let memory = IncidentTransition::Started {
        kind: IncidentKind::MemoryJump,
        start,
        detected: start,
    };
    let open = |enabled| HistoryState::at_path(path.clone(), enabled).unwrap();
    let read_events = || {
        let database = Connection::open(&path).unwrap();
        database
            .prepare(
                "SELECT session_id, kind, end_sequence, interrupted
             FROM performance_events ORDER BY session_id, kind",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    let first = open(true);
    first.recorder().offer_events(0, [spike, recovery, memory]);
    first.wait_until_idle();
    let existing = [
        (1, "cpuSpike".into(), Some(2), false),
        (1, "memoryJump".into(), None, false),
    ];
    assert_eq!(read_events(), existing);

    let competing = open(false);
    let mut response = competing
        .queue_read(|reply| Read::Metrics(raw_request(0, 1).validate().unwrap(), reply))
        .unwrap();
    assert!(async_runtime::block_on(response.recv()).unwrap().is_ok());
    assert_eq!(read_events(), existing);
    competing.stop();
    first.stop();

    let restarted = open(false);
    let mut response = restarted
        .queue_read(|reply| Read::Metrics(raw_request(0, 1).validate().unwrap(), reply))
        .unwrap();
    assert!(async_runtime::block_on(response.recv()).unwrap().is_ok());
    let interrupted = [
        (1, "cpuSpike".into(), Some(2), false),
        (1, "memoryJump".into(), None, true),
    ];
    assert_eq!(read_events(), interrupted);
    restarted.set_enabled(true);
    let recorder = restarted.recorder();
    recorder.offer_events(recorder.generation().unwrap(), [spike, recovery]);
    restarted.wait_until_idle();
    let expected = [
        interrupted[0].clone(),
        interrupted[1].clone(),
        (2, "cpuSpike".into(), Some(2), false),
    ];
    assert_eq!(read_events(), expected);
    restarted.stop();

    let restarted_again = open(false);
    let mut response = restarted_again
        .queue_read(|reply| Read::Metrics(raw_request(0, 1).validate().unwrap(), reply))
        .unwrap();
    assert!(async_runtime::block_on(response.recv()).unwrap().is_ok());
    assert_eq!(read_events(), expected);
}
