use super::*;

fn database(path: &Path) -> HistoryDatabase {
    let database = HistoryDatabase::open(path).unwrap();
    database
        .connection
        .execute(
            "INSERT INTO recording_sessions (started_at_ms) VALUES (0)",
            [],
        )
        .unwrap();
    database
}

fn insert(
    database: &HistoryDatabase,
    timestamp: i64,
    cpu: Option<f64>,
    memory: Option<(i64, i64)>,
) {
    database
        .connection
        .execute(
            "INSERT INTO metric_samples (session_id, sequence, timestamp_ms, monotonic_ms,
                interval_ms, cpu_percent, memory_used_bytes, memory_total_bytes,
                sample_status, battery_status)
             VALUES (1, (SELECT count(*) + 1 FROM metric_samples), ?1, 0, 1000,
                ?2, ?3, ?4, 'healthy', 'absent')",
            params![
                timestamp,
                cpu,
                memory.map(|(used, _)| used),
                memory.map(|(_, total)| total)
            ],
        )
        .unwrap();
}

fn request(start_ms: i64, boundary_ms: i64, end_ms: i64) -> SummaryRequest {
    SummaryRequest {
        previous: Period {
            start_ms,
            end_ms: boundary_ms,
        },
        current: Period {
            start_ms: boundary_ms,
            end_ms,
        },
    }
}

#[test]
fn summarizes_saved_samples_and_independent_minute_coverage() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite3");
    let database = database(&path);
    insert(&database, MINUTE_MS, Some(100.0), Some((8, 8)));
    insert(&database, MINUTE_MS + 1, Some(0.0), Some((0, 8)));
    insert(&database, MINUTE_MS + 2, Some(30.0), Some((8, 16)));
    insert(&database, 2 * MINUTE_MS, None, Some((8, 8)));
    insert(&database, 2 * MINUTE_MS + 1, None, None);
    insert(&database, 4 * MINUTE_MS + 1, Some(60.0), None);
    insert(&database, 4 * MINUTE_MS + 2, Some(100.0), Some((8, 8)));
    drop(database);
    let before = std::fs::read(&path).unwrap();

    let summary = request(1, MINUTE_MS + 1, 4 * MINUTE_MS + 2)
        .validate()
        .unwrap()
        .read_existing(&path)
        .unwrap();
    assert_eq!(summary.current.total_minutes, 4);
    assert_eq!(summary.current.cpu.covered_minutes, 2);
    assert_eq!(summary.current.ram.covered_minutes, 2);
    assert_eq!(summary.current.cpu.coverage_percent, 50.0);
    assert_eq!(summary.current.ram.coverage_percent, 50.0);
    assert_eq!(summary.current.cpu.average_percent, Some(30.0));
    assert_eq!(summary.current.cpu.peak_percent, Some(60.0));
    assert_eq!(summary.current.ram.average_percent, Some(50.0));
    assert_eq!(summary.current.ram.peak_percent, Some(100.0));
    assert_eq!(summary.previous.total_minutes, 2);
    assert_eq!(summary.previous.cpu.covered_minutes, 1);
    assert_eq!(summary.previous.cpu.peak_percent, Some(100.0));
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn comparison_uses_exact_duration_and_coverage_thresholds_per_metric() {
    let directory = tempfile::tempdir().unwrap();
    let database = database(&directory.path().join("history.sqlite3"));
    let start = 30 * MINUTE_MS + 1;
    let boundary = start + MIN_COMPARISON_MS;
    let end = boundary + MIN_COMPARISON_MS;
    for period_start in [start, boundary] {
        for offset in 0..12 {
            insert(
                &database,
                period_start + offset * MINUTE_MS,
                Some(0.0),
                (period_start == start || offset < 11).then_some((0, 16)),
            );
        }
    }
    let run = |start, boundary, end| {
        request(start, boundary, end)
            .validate()
            .unwrap()
            .run(&database)
            .unwrap()
    };
    let exact = run(start, boundary, end);
    assert_eq!(exact.current.total_minutes, 16);
    assert_eq!(exact.previous.cpu.coverage_percent, 75.0);
    assert_eq!(exact.current.cpu.coverage_percent, 75.0);
    assert_eq!(exact.current.cpu.peak_percent, Some(0.0));
    assert_eq!(exact.cpu_comparison, Eligibility::Eligible);
    assert_eq!(exact.current.ram.covered_minutes, 11);
    assert_eq!(exact.previous.ram.covered_minutes, 12);
    assert_eq!(exact.previous.ram.average_percent, Some(0.0));
    assert_eq!(exact.ram_comparison, Eligibility::InsufficientCoverage);
    let short = run(start + 1, boundary, end - 1);
    assert_eq!(short.cpu_comparison, Eligibility::PeriodTooShort);

    let absent_prior = run(start - MIN_COMPARISON_MS, start, boundary);
    assert_eq!(absent_prior.current.cpu.coverage_percent, 75.0);
    assert_eq!(absent_prior.previous.cpu.average_percent, None);
    assert_eq!(absent_prior.previous.cpu.peak_percent, None);
    assert_eq!(absent_prior.previous.cpu.coverage_percent, 0.0);
    assert_eq!(
        absent_prior.cpu_comparison,
        Eligibility::InsufficientCoverage
    );
}

#[test]
fn unequal_elapsed_local_days_are_ineligible() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("absent.sqlite3");
    let day = 24 * 60 * MINUTE_MS;
    for hours in [23, 25] {
        let summary = request(0, day, day + hours * 60 * MINUTE_MS)
            .validate()
            .unwrap()
            .read_existing(&path)
            .unwrap();
        assert_eq!(summary.current.total_minutes, hours as u32 * 60);
        assert_eq!(summary.previous.total_minutes, 24 * 60);
        assert_eq!(summary.cpu_comparison, Eligibility::UnequalPeriodLengths);
        assert_eq!(summary.ram_comparison, Eligibility::UnequalPeriodLengths);
    }
    assert!(!path.exists());
}

#[test]
fn validates_both_periods_and_enforces_the_source_cap() {
    let max = MAX_AVERAGE_RANGE_MS;
    assert!(
        request(i64::MAX - 2 * max, i64::MAX - max, i64::MAX)
            .validate()
            .is_ok()
    );
    for request in [request(0, max + 1, max + 2), request(0, 1, max + 2)] {
        assert!(matches!(request.validate(), Err(QueryError::RangeTooWide)));
    }
    for request in [
        request(-1, 0, 1),
        request(0, 0, 1),
        SummaryRequest {
            previous: Period {
                start_ms: 0,
                end_ms: 2,
            },
            current: Period {
                start_ms: 3,
                end_ms: 4,
            },
        },
    ] {
        assert!(matches!(request.validate(), Err(QueryError::InvalidRange)));
    }
    let directory = tempfile::tempdir().unwrap();
    let database = database(&directory.path().join("history.sqlite3"));
    let cap = MAX_AVERAGE_SOURCE_ROWS;
    database
        .connection
        .execute(
            "WITH RECURSIVE numbers(n) AS (
            SELECT 1 UNION ALL SELECT n + 1 FROM numbers WHERE n <= ?1
         )
         INSERT INTO metric_samples (session_id, sequence, timestamp_ms, monotonic_ms,
                                     interval_ms, sample_status, battery_status)
         SELECT 1, n, n, 0, 1000, 'healthy', 'absent' FROM numbers",
            [cap],
        )
        .unwrap();
    assert!(
        request(0, 1, cap + 1)
            .validate()
            .unwrap()
            .run(&database)
            .is_ok()
    );
    for request in [request(0, 1, cap + 2), request(1, cap + 2, cap + 3)] {
        assert!(matches!(
            request.validate().unwrap().run(&database),
            Err(QueryError::TooManySourceRows)
        ));
    }
}
