use super::*;

fn database() -> (tempfile::TempDir, HistoryDatabase) {
    let directory = tempfile::tempdir().unwrap();
    let database = HistoryDatabase::open(&directory.path().join("history.sqlite3")).unwrap();
    database
        .connection
        .execute(
            "INSERT INTO recording_sessions (started_at_ms) VALUES (0), (0)",
            [],
        )
        .unwrap();
    (directory, database)
}

fn query(start_ms: i64, end_ms: i64, resolution: Resolution) -> Result<Query, QueryError> {
    QueryRequest {
        start_ms,
        end_ms,
        resolution,
    }
    .validate()
}

fn execute(
    database: &HistoryDatabase,
    start_ms: i64,
    end_ms: i64,
    resolution: Resolution,
) -> QueryResult {
    query(start_ms, end_ms, resolution)
        .unwrap()
        .run(database)
        .unwrap()
}

#[test]
fn validates_range_limits_without_overflow() {
    for (resolution, max) in [
        (Resolution::Raw, MAX_RAW_RANGE_MS),
        (Resolution::MinuteAverages, MAX_AVERAGE_RANGE_MS),
    ] {
        assert!(query(i64::MAX - max, i64::MAX, resolution).is_ok());
        assert!(matches!(
            query(0, max + 1, resolution),
            Err(QueryError::RangeTooWide)
        ));
        for (start, end) in [(i64::MIN, i64::MAX), (0, 0), (10, 9)] {
            assert!(matches!(
                query(start, end, resolution),
                Err(QueryError::InvalidRange)
            ));
        }
    }
}

#[test]
fn raw_rows_use_half_open_ranges_and_stable_identity_order() {
    let (_directory, database) = database();
    database.connection.execute_batch(
        "INSERT INTO metric_samples (session_id, sequence, timestamp_ms, monotonic_ms, interval_ms,
                                     sample_status, battery_status)
         VALUES (1, 1, 100, 0, 1000, 'healthy', 'absent'),
                (1, 2, 99, 1000, 1000, 'healthy', 'absent'),
                (1, 3, 100, 2000, 1000, 'healthy', 'absent'),
                (2, 1, 100, 0, 1000, 'healthy', 'absent'),
                (2, 2, 101, 1000, 1000, 'healthy', 'absent');",
    ).unwrap();
    let identities = |start, end| {
        let QueryResult::Raw(rows) = execute(&database, start, end, Resolution::Raw) else {
            panic!("expected raw rows");
        };
        rows.into_iter()
            .map(|row| (row.timestamp_ms, row.session_id, row.sequence))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        identities(99, 101),
        [(99, 1, 2), (100, 1, 1), (100, 1, 3), (100, 2, 1)]
    );
    assert_eq!(
        identities(100, 101),
        [(100, 1, 1), (100, 1, 3), (100, 2, 1)]
    );
}

#[test]
fn averages_use_only_valid_in_range_values_and_mark_clipped_minutes() {
    let (_directory, database) = database();
    database.connection.execute_batch(
        "INSERT INTO metric_samples (session_id, sequence, timestamp_ms, monotonic_ms, interval_ms,
                   cpu_percent, memory_used_bytes, memory_total_bytes, battery_percent,
                   battery_state, sample_status, battery_status)
         VALUES (1, 1, 99, 0, 1000, 100, NULL, NULL, NULL, NULL, 'healthy', 'absent'),
                (1, 2, 100, 0, 1000, 0, 0, 16, 0, 'Discharging', 'healthy', 'available'),
                (1, 3, 59999, 0, 1000, 50, 8, 32, NULL, NULL, 'healthy', 'absent'),
                (1, 4, 59999, 0, 1000, NULL, NULL, NULL, NULL, NULL, 'error', 'unavailable'),
                (1, 5, 60000, 0, 1000, NULL, 4, 16, NULL, NULL, 'warmingUp', 'absent'),
                (1, 6, 180001, 0, 1000, 12, NULL, NULL, NULL, NULL, 'healthy', 'absent'),
                (1, 7, 180002, 0, 1000, 99, NULL, NULL, NULL, NULL, 'healthy', 'absent');",
    ).unwrap();
    let QueryResult::MinuteAverages(buckets) =
        execute(&database, 100, 180002, Resolution::MinuteAverages)
    else {
        panic!("expected minute buckets");
    };
    assert_eq!(
        buckets
            .iter()
            .map(|bucket| (bucket.start_ms, bucket.partial))
            .collect::<Vec<_>>(),
        [(0, true), (60000, false), (180000, true)]
    );
    let metric = |value: &Average| (value.average, value.valid_count);
    assert_eq!(metric(&buckets[0].cpu_percent), (Some(25.0), 2));
    assert_eq!(metric(&buckets[0].memory_used_bytes), (Some(4.0), 2));
    assert_eq!(metric(&buckets[0].memory_total_bytes), (Some(24.0), 2));
    assert_eq!(metric(&buckets[0].battery_percent), (Some(0.0), 1));
    assert_eq!(metric(&buckets[1].cpu_percent), (None, 0));
    assert_eq!(metric(&buckets[1].battery_percent), (None, 0));
    assert_eq!(metric(&buckets[2].cpu_percent), (Some(12.0), 1));

    let QueryResult::MinuteAverages(single) =
        execute(&database, 100, 101, Resolution::MinuteAverages)
    else {
        panic!("expected minute buckets");
    };
    assert_eq!(single.len(), 1);
    assert!(single[0].partial);
    assert_eq!(metric(&single[0].cpu_percent), (Some(0.0), 1));
}

fn insert_series(database: &HistoryDatabase, count: i64, spacing: i64) {
    database
        .connection
        .execute(
            "WITH RECURSIVE numbers(n) AS (
            SELECT 1 UNION ALL SELECT n + 1 FROM numbers WHERE n < ?1
         )
         INSERT INTO metric_samples (session_id, sequence, timestamp_ms, monotonic_ms, interval_ms,
                                     sample_status, battery_status)
         SELECT 1, n, 1 + (n - 1) * ?2, 0, 1000, 'healthy', 'absent' FROM numbers",
            params![count, spacing],
        )
        .unwrap();
}

#[test]
fn dense_ranges_report_a_cap_error_instead_of_returning_truncated_history() {
    let (_directory, database) = database();
    insert_series(&database, MAX_AVERAGE_SOURCE_ROWS + 1, 1);
    for (resolution, cap) in [
        (Resolution::Raw, MAX_RAW_ROWS),
        (Resolution::MinuteAverages, MAX_AVERAGE_SOURCE_ROWS),
    ] {
        let end = cap + 1;
        let result = execute(&database, 1, end, resolution);
        match result {
            QueryResult::Raw(rows) => assert_eq!(rows.len() as i64, cap),
            QueryResult::MinuteAverages(rows) => assert_eq!(rows.len(), 3),
        }
        assert!(matches!(
            query(1, end + 1, resolution).unwrap().run(&database),
            Err(QueryError::TooManySourceRows)
        ));
    }
}

#[test]
fn unaligned_seven_day_range_can_return_every_intersecting_minute() {
    let (_directory, database) = database();
    insert_series(&database, MAX_MINUTE_BUCKETS as i64, MINUTE_MS);
    let QueryResult::MinuteAverages(buckets) = execute(
        &database,
        2,
        2 + MAX_AVERAGE_RANGE_MS,
        Resolution::MinuteAverages,
    ) else {
        panic!("expected minute buckets");
    };
    assert_eq!(buckets.len(), MAX_MINUTE_BUCKETS - 1);
    database
        .connection
        .execute(
            "UPDATE metric_samples SET timestamp_ms = 2 WHERE sequence = 1",
            [],
        )
        .unwrap();
    let QueryResult::MinuteAverages(buckets) = execute(
        &database,
        2,
        2 + MAX_AVERAGE_RANGE_MS,
        Resolution::MinuteAverages,
    ) else {
        panic!("expected minute buckets");
    };
    assert_eq!(buckets.len(), MAX_MINUTE_BUCKETS);
    assert!(buckets.first().unwrap().partial);
    assert!(buckets.last().unwrap().partial);
    assert!(
        buckets[1..buckets.len() - 1]
            .iter()
            .all(|bucket| !bucket.partial)
    );
}

#[test]
fn cold_read_reports_database_lock_and_recovers_without_modifying_file() {
    let (directory, database) = database();
    let path = directory.path().join("history.sqlite3");
    insert_series(&database, 1, 1);
    let before = std::fs::read(&path).unwrap();
    database
        .connection
        .execute_batch("BEGIN EXCLUSIVE")
        .unwrap();
    let query = query(0, 10, Resolution::Raw).unwrap();
    assert!(matches!(
        query.read_existing(&path),
        Err(QueryError::Storage(_))
    ));
    database.connection.execute_batch("ROLLBACK").unwrap();
    assert!(
        matches!(query.read_existing(&path).unwrap(), QueryResult::Raw(rows) if rows.len() == 1)
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
