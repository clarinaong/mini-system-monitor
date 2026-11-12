use super::{
    HistoryDatabase, SCHEMA_VERSION,
    query::{MAX_AVERAGE_RANGE_MS, QueryError, read_existing, validate_range},
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

const MAX_INCIDENTS: u32 = 512;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncidentRequest {
    start_ms: i64,
    end_ms: i64,
}

pub struct IncidentQuery(IncidentRequest);

impl IncidentRequest {
    pub fn validate(self) -> Result<IncidentQuery, QueryError> {
        validate_range(self.start_ms, self.end_ms, MAX_AVERAGE_RANGE_MS)?;
        Ok(IncidentQuery(self))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IncidentStatus {
    Open,
    Recovered,
    Interrupted,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedIncident {
    session_id: i64,
    kind: String,
    start_sequence: i64,
    start_ms: i64,
    detected_ms: i64,
    end_ms: Option<i64>,
    duration_ms: Option<i64>,
    status: IncidentStatus,
}

impl IncidentQuery {
    pub(super) fn run(&self, database: &HistoryDatabase) -> Result<Vec<SavedIncident>, QueryError> {
        self.read(&database.connection, SCHEMA_VERSION)
    }

    pub(super) fn read_existing(&self, path: &Path) -> Result<Vec<SavedIncident>, QueryError> {
        read_existing(path, Vec::new(), |connection, version| {
            self.read(connection, version)
        })
    }

    fn read(
        &self,
        connection: &Connection,
        version: i32,
    ) -> Result<Vec<SavedIncident>, QueryError> {
        if version == 1 {
            return Ok(Vec::new());
        }

        let interrupted = if version >= 3 { "interrupted" } else { "0" };
        let mut statement = connection.prepare(&format!(
            "SELECT session_id, kind, start_sequence, start_ms, detected_ms, end_ms,
                    end_monotonic_ms - start_monotonic_ms, {interrupted}
             FROM performance_events INDEXED BY performance_events_start
             WHERE start_ms >= ?1 AND start_ms < ?2
             ORDER BY start_ms DESC, rowid DESC LIMIT ?3"
        ))?;
        let mut rows =
            statement.query(params![self.0.start_ms, self.0.end_ms, MAX_INCIDENTS + 1])?;
        let mut incidents = Vec::new();
        while let Some(row) = rows.next()? {
            if incidents.len() == MAX_INCIDENTS as usize {
                return Err(QueryError::TooManyResults);
            }
            let end_ms = row.get::<_, Option<i64>>(5)?;
            let status = if row.get::<_, bool>(7)? {
                IncidentStatus::Interrupted
            } else if end_ms.is_some() {
                IncidentStatus::Recovered
            } else {
                IncidentStatus::Open
            };
            incidents.push(SavedIncident {
                session_id: row.get(0)?,
                kind: row.get(1)?,
                start_sequence: row.get(2)?,
                start_ms: row.get(3)?,
                detected_ms: row.get(4)?,
                end_ms,
                duration_ms: row.get(6)?,
                status,
            });
        }
        Ok(incidents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{APPLICATION_ID, EVENTS_SCHEMA, SCHEMA};

    fn query(start_ms: i64, end_ms: i64) -> IncidentQuery {
        IncidentRequest { start_ms, end_ms }.validate().unwrap()
    }

    #[test]
    fn half_open_range_preserves_statuses_and_uses_monotonic_duration() {
        let directory = tempfile::tempdir().unwrap();
        let database = HistoryDatabase::open(&directory.path().join("history.sqlite3")).unwrap();
        database
            .connection
            .execute_batch(
                "INSERT INTO recording_sessions(id, started_at_ms) VALUES (1, 0);
             INSERT INTO performance_events (
                 session_id, kind, start_sequence, start_ms, start_monotonic_ms,
                 detected_sequence, detected_ms, detected_monotonic_ms,
                 end_sequence, end_ms, end_monotonic_ms, interrupted
             ) VALUES
                 (1, 'cpuSpike', 1, 100, 200, 1, 100, 200, 2, 90, 400, 0),
                 (1, 'memoryJump', 1, 100, 200, 1, 100, 200, NULL, NULL, NULL, 0),
                 (1, 'sustainedCpu', 2, 300, 400, 3, 500, 600, NULL, NULL, NULL, 1),
                 (1, 'sustainedMemory', 3, 600, 700, 4, 800, 900, NULL, NULL, NULL, 0);",
            )
            .unwrap();

        let incidents = query(100, 600).run(&database).unwrap();
        let json = serde_json::to_value(&incidents).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 3);
        assert_eq!(json[0]["status"], "interrupted");
        assert_eq!(json[0]["detectedMs"], 500);
        assert!(json[0]["endMs"].is_null());
        assert!(json[0]["durationMs"].is_null());
        assert_eq!(json[1]["status"], "open");
        assert_eq!(json[2]["status"], "recovered");
        assert_eq!(json[2]["endMs"], 90);
        assert_eq!(json[2]["durationMs"], 200);
        assert_eq!(json[2]["sessionId"], 1);
        assert_eq!(json[2]["startSequence"], 1);
    }

    #[test]
    fn cold_reads_leave_missing_and_legacy_databases_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite3");
        let range = query(0, 100);
        assert!(range.read_existing(&path).unwrap().is_empty());
        assert!(!path.exists());

        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection
            .pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        assert!(range.read_existing(&path).unwrap().is_empty());

        connection.execute_batch(EVENTS_SCHEMA).unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
        connection
            .execute_batch(
                "INSERT INTO recording_sessions(id, started_at_ms) VALUES (1, 0);
             INSERT INTO performance_events (
                 session_id, kind, start_sequence, start_ms, start_monotonic_ms,
                 detected_sequence, detected_ms, detected_monotonic_ms
             ) VALUES (1, 'cpuSpike', 1, 50, 50, 1, 50, 50);",
            )
            .unwrap();
        let rows = range.read_existing(&path).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].status, IncidentStatus::Open));
        assert_eq!(
            connection
                .pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn oversized_ranges_and_dense_results_are_rejected() {
        assert!(matches!(
            IncidentRequest {
                start_ms: 0,
                end_ms: MAX_AVERAGE_RANGE_MS + 1
            }
            .validate(),
            Err(QueryError::RangeTooWide)
        ));
        let directory = tempfile::tempdir().unwrap();
        let database = HistoryDatabase::open(&directory.path().join("history.sqlite3")).unwrap();
        database
            .connection
            .execute(
                "INSERT INTO recording_sessions(id, started_at_ms) VALUES (1, 0)",
                [],
            )
            .unwrap();
        database.connection.execute(
            "WITH RECURSIVE counts(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM counts WHERE n < ?1)
             INSERT INTO performance_events (
                 session_id, kind, start_sequence, start_ms, start_monotonic_ms,
                 detected_sequence, detected_ms, detected_monotonic_ms
             ) SELECT 1, 'cpuSpike', n, n, n, n, n, n FROM counts",
            [MAX_INCIDENTS + 1],
        ).unwrap();
        assert_eq!(
            query(1, i64::from(MAX_INCIDENTS) + 1)
                .run(&database)
                .unwrap()
                .len(),
            MAX_INCIDENTS as usize
        );
        assert!(matches!(
            query(1, 1000).run(&database),
            Err(QueryError::TooManyResults)
        ));
    }
}
