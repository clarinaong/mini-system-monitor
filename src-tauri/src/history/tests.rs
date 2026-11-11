use super::*;
use rusqlite::params;
use tempfile::tempdir;

fn version(connection: &Connection) -> i32 {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

fn session(connection: &Connection, timestamp: i64) -> i64 {
    connection
        .execute(
            "INSERT INTO recording_sessions(started_at_ms) VALUES (?1)",
            [timestamp],
        )
        .unwrap();
    connection.last_insert_rowid()
}

fn sample(
    connection: &Connection,
    session: i64,
    sequence: i64,
    timestamp: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO metric_samples(session_id, sequence, timestamp_ms, monotonic_ms, interval_ms,
            cpu_percent, memory_used_bytes, memory_total_bytes, battery_percent, battery_state,
            sample_status, battery_status)
         VALUES (?1, ?2, ?3, ?4, 1000, 25.5, 100, 200, NULL, NULL, 'healthy', 'absent')",
        params![session, sequence, timestamp, sequence * 1000],
    )
}

#[test]
fn new_database_has_identity_schema_index_and_foreign_keys() {
    let directory = tempdir().unwrap();
    let database = HistoryDatabase::open(&directory.path().join("history.sqlite3")).unwrap();
    let connection = &database.connection;
    assert_eq!(version(connection), SCHEMA_VERSION);
    assert_eq!(
        connection
            .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
            .unwrap(),
        APPLICATION_ID
    );
    assert!(
        connection
            .pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))
            .unwrap()
    );
    let index: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name = 'metric_samples_timestamp'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(index.contains("timestamp_ms, session_id, sequence"));
    assert!(sample(connection, 999, 1, 1000).is_err());
    let id = session(connection, 1000);
    sample(connection, id, 1, 1000).unwrap();
    connection
        .execute("DELETE FROM recording_sessions WHERE id = ?1", [id])
        .unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM metric_samples", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn reopening_preserves_data_and_does_not_repeat_migration() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("history.sqlite3");
    let id;
    {
        let database = HistoryDatabase::open(&path).unwrap();
        id = session(&database.connection, 1000);
        sample(&database.connection, id, 1, 1000).unwrap();
    }
    for _ in 0..2 {
        let database = HistoryDatabase::open(&path).unwrap();
        assert_eq!(version(&database.connection), 1);
        assert_eq!(database.connection.query_row("SELECT memory_used_bytes FROM metric_samples WHERE session_id = ?1 AND sequence = 1", [id], |row| row.get::<_, i64>(0)).unwrap(), 100);
    }
}

#[test]
fn session_sequence_identity_survives_restarts_and_wall_clock_regression() {
    let directory = tempdir().unwrap();
    let database = HistoryDatabase::open(&directory.path().join("history.sqlite3")).unwrap();
    let connection = &database.connection;
    let first = session(connection, 2000);
    sample(connection, first, 1, 2000).unwrap();
    sample(connection, first, 2, 1000).unwrap();
    assert!(sample(connection, first, 1, 3000).is_err());
    let second = session(connection, 1000);
    assert_ne!(first, second);
    sample(connection, second, 1, 1000).unwrap();
    let rows: Vec<(i64, i64)> = connection.prepare(
        "SELECT session_id, sequence FROM metric_samples ORDER BY timestamp_ms, session_id, sequence",
    ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(rows, vec![(first, 2), (second, 1), (first, 1)]);
}

#[test]
fn missing_warmup_failure_and_battery_states_remain_distinct() {
    let directory = tempdir().unwrap();
    let database = HistoryDatabase::open(&directory.path().join("history.sqlite3")).unwrap();
    let connection = &database.connection;
    let id = session(connection, 0);
    for (sequence, status, battery_status, cpu, memory, percent, state) in [
        (1, "warmingUp", "absent", None, Some(100), None, None),
        (2, "degraded", "error", Some(0.0), Some(100), None, None),
        (3, "error", "unavailable", None, None, None, None),
        (
            4,
            "healthy",
            "available",
            Some(100.0),
            Some(100),
            Some(0.0),
            Some("Empty"),
        ),
        (
            5,
            "healthy",
            "available",
            Some(50.0),
            Some(100),
            Some(100.0),
            Some("Full"),
        ),
    ] {
        connection.execute(
            "INSERT INTO metric_samples VALUES (?1, ?2, 0, ?2, 1000, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, sequence, cpu, memory, memory.map(|_| 200), percent, state, status, battery_status],
        ).unwrap();
    }
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM metric_samples WHERE cpu_percent IS NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM metric_samples WHERE cpu_percent = 0",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM metric_samples WHERE battery_percent IS NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        3
    );
}

#[test]
fn schema_rejects_invalid_readings_types_and_incomplete_pairs() {
    let directory = tempdir().unwrap();
    let database = HistoryDatabase::open(&directory.path().join("history.sqlite3")).unwrap();
    let connection = &database.connection;
    let id = session(connection, 0);
    sample(connection, id, 1, 1000).unwrap();
    for update in [
        "cpu_percent = -1",
        "cpu_percent = 101",
        "cpu_percent = 'bad'",
        "memory_used_bytes = -1",
        "memory_total_bytes = 0",
        "memory_used_bytes = 201",
        "memory_used_bytes = NULL",
        "memory_total_bytes = NULL",
        "memory_used_bytes = 1.5",
        "sequence = 0",
        "timestamp_ms = -1",
        "monotonic_ms = -1",
        "interval_ms = 3000",
        "sample_status = 'unknown'",
        "battery_status = 'unknown'",
        "battery_percent = 50",
        "battery_state = 'Charging'",
        "battery_percent = -1, battery_state = 'Charging', battery_status = 'available'",
        "battery_percent = 101, battery_state = 'Charging', battery_status = 'available'",
        "battery_percent = 50, battery_state = 'Charging'",
        "sample_status = 'error'",
    ] {
        assert!(
            connection
                .execute(&format!("UPDATE metric_samples SET {update}"), [])
                .is_err(),
            "accepted {update}"
        );
    }
    assert_eq!(
        connection
            .query_row("SELECT cpu_percent FROM metric_samples", [], |row| row
                .get::<_, f64>(0))
            .unwrap(),
        25.5
    );
}

#[test]
fn failed_migration_rolls_back_tables_and_version_then_can_retry() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("history.sqlite3");
    let mut connection = Connection::open(&path).unwrap();
    assert!(
        migrate(
            &mut connection,
            "CREATE TABLE partial (value INTEGER); INVALID SQL;"
        )
        .is_err()
    );
    assert_eq!(version(&connection), 0);
    assert_eq!(
        connection
            .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'partial'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let extra_schema = format!("{SCHEMA} CREATE TABLE unexpected(value INTEGER);");
    assert!(matches!(
        migrate(&mut connection, &extra_schema),
        Err(HistoryError::InvalidSchema(_))
    ));
    assert_eq!(version(&connection), 0);
    assert_eq!(
        connection
            .pragma_query_value(None, "application_id", |row| row.get::<_, i32>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(connection);
    assert!(HistoryDatabase::open(&path).is_ok());
}

#[test]
fn future_foreign_and_unidentified_databases_are_preserved_byte_for_byte() {
    for (application, version) in [(APPLICATION_ID, 2), (123, 1), (0, 0), (APPLICATION_ID, 0)] {
        let directory = tempdir().unwrap();
        let path = directory.path().join("history.sqlite3");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE precious(value TEXT); INSERT INTO precious VALUES ('keep');",
                )
                .unwrap();
            connection
                .pragma_update(None, "application_id", application)
                .unwrap();
            connection
                .pragma_update(None, "user_version", version)
                .unwrap();
        }
        let original = std::fs::read(&path).unwrap();
        assert!(HistoryDatabase::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

#[test]
fn corrupt_and_invalid_version_one_schemas_are_not_replaced_or_repaired() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("history.sqlite3");
    std::fs::write(&path, b"not a SQLite database").unwrap();
    assert!(HistoryDatabase::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"not a SQLite database");
    std::fs::remove_file(&path).unwrap();
    {
        let database = HistoryDatabase::open(&path).unwrap();
        database
            .connection
            .execute_batch("DROP INDEX metric_samples_timestamp")
            .unwrap();
    }
    let original = std::fs::read(&path).unwrap();
    assert!(matches!(
        HistoryDatabase::open(&path),
        Err(HistoryError::InvalidSchema(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn lock_and_missing_parent_failures_return_errors_without_altering_data() {
    let directory = tempdir().unwrap();
    assert!(HistoryDatabase::open(&directory.path().join("missing/history.sqlite3")).is_err());
    assert!(!directory.path().join("missing").exists());
    let path = directory.path().join("history.sqlite3");
    let database = HistoryDatabase::open(&path).unwrap();
    database
        .connection
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    assert!(
        matches!(HistoryDatabase::open(&path), Err(HistoryError::Database(rusqlite::Error::SqliteFailure(error, _))) if error.code == rusqlite::ErrorCode::DatabaseBusy)
    );
    database.connection.execute_batch("ROLLBACK").unwrap();
    assert!(HistoryDatabase::open(&path).is_ok());
}

#[test]
fn foreign_key_damage_is_detected_without_deleting_orphaned_samples() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("history.sqlite3");
    {
        let database = HistoryDatabase::open(&path).unwrap();
        database
            .connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        sample(&database.connection, 999, 1, 0).unwrap();
    }
    let original = std::fs::read(&path).unwrap();
    assert!(matches!(
        HistoryDatabase::open(&path),
        Err(HistoryError::InvalidSchema(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), original);
}
