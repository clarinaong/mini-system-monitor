mod events;
mod query;
pub(crate) mod recorder;

pub(crate) use recorder::HistoryRecorder;
pub use recorder::HistoryState;

use crate::monitor::{BatteryStatus, MetricsSample, SampleStatus};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{fmt, path::Path, time::Duration};

const APPLICATION_ID: i32 = 0x4d53_4d48;
const SCHEMA_VERSION: i32 = 2;
const EVENTS_SCHEMA: &str = include_str!("schema_v2.sql");
const SCHEMA: &str = include_str!("schema.sql");

#[derive(Debug)]
pub enum HistoryError {
    Database(rusqlite::Error),
    Incompatible { application_id: i32, version: i32 },
    InvalidSchema(String),
    InvalidSample(&'static str),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "Local history database error: {error}"),
            Self::Incompatible {
                application_id,
                version,
            } => write!(
                formatter,
                "History database identity/version is unsupported (application {application_id}, version {version}). The file was preserved."
            ),
            Self::InvalidSchema(reason) => write!(
                formatter,
                "History database schema is invalid: {reason}. The file was preserved."
            ),
            Self::InvalidSample(field) => write!(formatter, "History sample has invalid {field}."),
        }
    }
}

impl std::error::Error for HistoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for HistoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

pub struct HistoryDatabase {
    connection: Connection,
}

impl HistoryDatabase {
    pub fn open(path: &Path) -> Result<Self, HistoryError> {
        Self::open_with_flags(path, OpenFlags::SQLITE_OPEN_CREATE)
    }

    fn open_existing(path: &Path) -> Result<Self, HistoryError> {
        Self::open_with_flags(path, OpenFlags::empty())
    }

    fn open_with_flags(path: &Path, flags: OpenFlags) -> Result<Self, HistoryError> {
        let mut connection = Connection::open_with_flags(
            path,
            flags | OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_millis(100))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "secure_delete", true)?;
        migrate(
            &mut connection,
            SCHEMA,
            flags.contains(OpenFlags::SQLITE_OPEN_CREATE),
        )?;
        Ok(Self { connection })
    }

    fn record(
        &mut self,
        session_id: Option<i64>,
        sample: &MetricsSample,
    ) -> Result<i64, HistoryError> {
        let to_sql_int =
            |value, field| i64::try_from(value).map_err(|_| HistoryError::InvalidSample(field));
        let timestamp = to_sql_int(sample.timestamp_ms, "timestamp")?;
        let sequence = to_sql_int(sample.sequence, "sequence")?;
        let monotonic = to_sql_int(sample.monotonic_ms, "monotonic timestamp")?;
        let interval = to_sql_int(sample.interval_ms, "interval")?;
        let metrics = sample
            .metrics
            .as_ref()
            .filter(|_| sample.health.status != SampleStatus::Error);
        let cpu = metrics
            .and_then(|metrics| metrics.cpu_usage)
            .filter(valid_percentage);
        let (memory_used, memory_total) = match metrics {
            Some(metrics)
                if metrics.memory_total > 0 && metrics.memory_used <= metrics.memory_total =>
            {
                (
                    Some(to_sql_int(metrics.memory_used, "memory used")?),
                    Some(to_sql_int(metrics.memory_total, "memory total")?),
                )
            }
            _ => (None, None),
        };
        let battery = metrics
            .and_then(|metrics| metrics.battery.as_ref())
            .filter(|battery| {
                sample.health.battery_status == BatteryStatus::Available
                    && valid_percentage(&battery.percentage)
            });
        let sample_status = match sample.health.status {
            SampleStatus::Healthy => "healthy",
            SampleStatus::WarmingUp => "warmingUp",
            SampleStatus::Degraded => "degraded",
            SampleStatus::Error => "error",
        };
        let battery_status = match sample.health.battery_status {
            BatteryStatus::Available => "available",
            BatteryStatus::Absent => "absent",
            BatteryStatus::Error => "error",
            BatteryStatus::Unavailable => "unavailable",
        };

        let transaction = self.connection.transaction()?;
        let session_id = session_for_write(&transaction, session_id, timestamp)?;
        transaction.execute(
            "INSERT INTO metric_samples (
                session_id, sequence, timestamp_ms, monotonic_ms, interval_ms, cpu_percent,
                memory_used_bytes, memory_total_bytes, battery_percent, battery_state,
                sample_status, battery_status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                session_id,
                sequence,
                timestamp,
                monotonic,
                interval,
                cpu,
                memory_used,
                memory_total,
                battery.map(|battery| battery.percentage),
                battery.map(|battery| battery.status.as_str()),
                sample_status,
                battery_status,
            ],
        )?;
        transaction.commit()?;
        Ok(session_id)
    }

    fn prune(&mut self, cutoff_ms: i64, active_session: Option<i64>) -> Result<(), HistoryError> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM metric_samples WHERE timestamp_ms < ?1",
            [cutoff_ms],
        )?;
        transaction.execute(
            "DELETE FROM performance_events WHERE start_ms < ?1",
            [cutoff_ms],
        )?;
        transaction.execute(
            "DELETE FROM recording_sessions WHERE id IS NOT ?1 AND NOT EXISTS (
                SELECT 1 FROM metric_samples WHERE session_id = recording_sessions.id
            ) AND NOT EXISTS (
                SELECT 1 FROM performance_events WHERE session_id = recording_sessions.id
            )",
            [active_session],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn clear(&mut self) -> Result<(), HistoryError> {
        self.connection
            .execute("DELETE FROM recording_sessions", [])?;
        Ok(())
    }
}

fn session_for_write(
    connection: &Connection,
    session_id: Option<i64>,
    timestamp: i64,
) -> Result<i64, HistoryError> {
    if let Some(id) = session_id {
        return Ok(id);
    }
    connection.execute(
        "INSERT INTO recording_sessions (started_at_ms) VALUES (?1)",
        [timestamp],
    )?;
    Ok(connection.last_insert_rowid())
}

fn valid_percentage(value: &f32) -> bool {
    (0.0..=100.0).contains(value)
}

fn migrate(
    connection: &mut Connection,
    schema: &str,
    initialize: bool,
) -> Result<(), HistoryError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let application_id: i32 =
        transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match (application_id, version) {
        (0, 0) if initialize => {
            let count: i64 = transaction.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            if count != 0 {
                return Err(HistoryError::InvalidSchema(
                    "unidentified nonempty database".into(),
                ));
            }
            transaction.execute_batch(schema)?;
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.execute_batch(EVENTS_SCHEMA)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        (APPLICATION_ID, 1) => {
            validate_manifest(&transaction, 1)?;
            transaction.execute_batch(EVENTS_SCHEMA)?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        (APPLICATION_ID, SCHEMA_VERSION) => {}
        _ => {
            return Err(HistoryError::Incompatible {
                application_id,
                version,
            });
        }
    }
    validate_schema(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn validate_schema(connection: &Connection) -> Result<(), HistoryError> {
    let check: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if check != "ok" {
        return Err(HistoryError::InvalidSchema(check));
    }
    validate_manifest(connection, SCHEMA_VERSION)?;
    if connection.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(HistoryError::InvalidSchema("foreign key violations".into()));
    }
    Ok(())
}

fn validate_manifest(connection: &Connection, version: i32) -> Result<(), HistoryError> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(SCHEMA)?;
    if version == SCHEMA_VERSION {
        expected.execute_batch(EVENTS_SCHEMA)?;
    }
    let manifest =
        |database: &Connection| -> Result<Vec<(String, String, String)>, rusqlite::Error> {
            database.prepare("SELECT type, name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect()
        };
    if manifest(connection)? != manifest(&expected)? {
        return Err(HistoryError::InvalidSchema(format!(
            "tables, constraints or indexes do not match version {version}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
