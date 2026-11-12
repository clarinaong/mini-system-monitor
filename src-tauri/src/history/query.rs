use super::{APPLICATION_ID, HistoryDatabase, HistoryError, SCHEMA_VERSION, validate_manifest};
use rusqlite::{Connection, OpenFlags, Row, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

const MINUTE_MS: i64 = 60_000;
const MAX_RAW_RANGE_MS: i64 = 60 * MINUTE_MS;
pub(super) const MAX_AVERAGE_RANGE_MS: i64 = 7 * 24 * 60 * MINUTE_MS;
const MAX_RAW_ROWS: i64 = 1_024;
const MAX_AVERAGE_SOURCE_ROWS: i64 = 150_000;
const MAX_MINUTE_BUCKETS: usize = 10_081;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    Raw,
    MinuteAverages,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRequest {
    start_ms: i64,
    end_ms: i64,
    resolution: Resolution,
}

pub struct Query(QueryRequest);

impl QueryRequest {
    pub fn validate(self) -> Result<Query, QueryError> {
        let max_range = match self.resolution {
            Resolution::Raw => MAX_RAW_RANGE_MS,
            Resolution::MinuteAverages => MAX_AVERAGE_RANGE_MS,
        };
        validate_range(self.start_ms, self.end_ms, max_range)?;
        Ok(Query(self))
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "code", content = "message")]
pub enum QueryError {
    InvalidRange,
    RangeTooWide,
    TooManySourceRows,
    TooManyResults,
    Busy,
    Unavailable,
    Storage(String),
}

impl From<rusqlite::Error> for QueryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::from(HistoryError::from(error))
    }
}

impl From<HistoryError> for QueryError {
    fn from(error: HistoryError) -> Self {
        Self::Storage(error.to_string())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", tag = "resolution", content = "rows")]
pub enum QueryResult {
    Raw(Vec<RawSample>),
    MinuteAverages(Vec<MinuteBucket>),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSample {
    session_id: i64,
    sequence: i64,
    timestamp_ms: i64,
    monotonic_ms: i64,
    interval_ms: i64,
    cpu_percent: Option<f64>,
    memory_used_bytes: Option<i64>,
    memory_total_bytes: Option<i64>,
    battery_percent: Option<f64>,
    battery_state: Option<String>,
    sample_status: String,
    battery_status: String,
}

impl RawSample {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            session_id: row.get(0)?,
            sequence: row.get(1)?,
            timestamp_ms: row.get(2)?,
            monotonic_ms: row.get(3)?,
            interval_ms: row.get(4)?,
            cpu_percent: row.get(5)?,
            memory_used_bytes: row.get(6)?,
            memory_total_bytes: row.get(7)?,
            battery_percent: row.get(8)?,
            battery_state: row.get(9)?,
            sample_status: row.get(10)?,
            battery_status: row.get(11)?,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Average {
    average: Option<f64>,
    valid_count: u32,
}

#[derive(Default)]
struct Mean {
    sum: f64,
    count: u32,
}

impl Mean {
    fn add(&mut self, reading: Option<f64>) {
        if let Some(reading) = reading {
            self.sum += reading;
            self.count += 1;
        }
    }

    fn finish(self) -> Average {
        Average {
            average: (self.count != 0).then(|| self.sum / f64::from(self.count)),
            valid_count: self.count,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MinuteBucket {
    start_ms: i64,
    partial: bool,
    cpu_percent: Average,
    memory_used_bytes: Average,
    memory_total_bytes: Average,
    battery_percent: Average,
}

#[derive(Default)]
struct Bucket {
    start_ms: i64,
    cpu: Mean,
    memory_used: Mean,
    memory_total: Mean,
    battery: Mean,
}

impl Bucket {
    fn add(&mut self, row: &Row<'_>) -> rusqlite::Result<()> {
        self.cpu.add(row.get(1)?);
        self.memory_used.add(row.get(2)?);
        self.memory_total.add(row.get(3)?);
        self.battery.add(row.get(4)?);
        Ok(())
    }

    fn finish(self, range: &QueryRequest) -> MinuteBucket {
        MinuteBucket {
            start_ms: self.start_ms,
            partial: self.start_ms < range.start_ms || range.end_ms - self.start_ms < MINUTE_MS,
            cpu_percent: self.cpu.finish(),
            memory_used_bytes: self.memory_used.finish(),
            memory_total_bytes: self.memory_total.finish(),
            battery_percent: self.battery.finish(),
        }
    }
}

impl Query {
    pub fn empty(&self) -> QueryResult {
        match self.0.resolution {
            Resolution::Raw => QueryResult::Raw(Vec::new()),
            Resolution::MinuteAverages => QueryResult::MinuteAverages(Vec::new()),
        }
    }

    pub(super) fn run(&self, database: &HistoryDatabase) -> Result<QueryResult, QueryError> {
        self.read(&database.connection)
    }

    pub(super) fn read_existing(&self, path: &Path) -> Result<QueryResult, QueryError> {
        read_existing(path, self.empty(), |connection, _| self.read(connection))
    }

    fn read(&self, connection: &Connection) -> Result<QueryResult, QueryError> {
        let (columns, source_cap) = match self.0.resolution {
            Resolution::Raw => (
                "session_id, sequence, timestamp_ms, monotonic_ms, interval_ms, cpu_percent,
                 memory_used_bytes, memory_total_bytes, battery_percent, battery_state,
                 sample_status, battery_status",
                MAX_RAW_ROWS,
            ),
            Resolution::MinuteAverages => (
                "timestamp_ms, cpu_percent, memory_used_bytes, memory_total_bytes, battery_percent",
                MAX_AVERAGE_SOURCE_ROWS,
            ),
        };
        let mut statement = connection.prepare(&format!(
            "SELECT {columns} FROM metric_samples INDEXED BY metric_samples_timestamp
             WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2
             ORDER BY timestamp_ms, session_id, sequence LIMIT ?3"
        ))?;
        let mut rows = statement.query(params![self.0.start_ms, self.0.end_ms, source_cap + 1])?;
        let mut source_count = 0;
        let mut result = self.empty();
        let mut current: Option<Bucket> = None;
        while let Some(row) = rows.next()? {
            source_count += 1;
            if source_count > source_cap {
                return Err(QueryError::TooManySourceRows);
            }
            match &mut result {
                QueryResult::Raw(samples) => samples.push(RawSample::read(row)?),
                QueryResult::MinuteAverages(buckets) => {
                    let timestamp: i64 = row.get(0)?;
                    let start_ms = timestamp.div_euclid(MINUTE_MS) * MINUTE_MS;
                    if current
                        .as_ref()
                        .is_none_or(|bucket| bucket.start_ms != start_ms)
                    {
                        if let Some(bucket) = current.take() {
                            buckets.push(bucket.finish(&self.0));
                        }
                        if buckets.len() == MAX_MINUTE_BUCKETS {
                            return Err(QueryError::TooManyResults);
                        }
                    }
                    current
                        .get_or_insert_with(|| Bucket {
                            start_ms,
                            ..Bucket::default()
                        })
                        .add(row)?;
                }
            }
        }
        if let (Some(bucket), QueryResult::MinuteAverages(buckets)) = (current, &mut result) {
            buckets.push(bucket.finish(&self.0));
        }
        Ok(result)
    }
}

pub(super) fn read_existing<T>(
    path: &Path,
    empty: T,
    read: impl FnOnce(&Connection, i32) -> Result<T, QueryError>,
) -> Result<T, QueryError> {
    if !path
        .try_exists()
        .map_err(|error| QueryError::Storage(error.to_string()))?
    {
        return Ok(empty);
    }
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_millis(100))?;
    let transaction = connection.transaction()?;
    let application_id =
        transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if application_id != APPLICATION_ID || !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(HistoryError::Incompatible {
            application_id,
            version,
        }
        .into());
    }
    validate_manifest(&transaction, version)?;
    let result = read(&transaction, version)?;
    transaction.commit()?;
    Ok(result)
}

pub(super) fn validate_range(start_ms: i64, end_ms: i64, max_ms: i64) -> Result<(), QueryError> {
    if start_ms < 0 || end_ms <= start_ms {
        return Err(QueryError::InvalidRange);
    }
    if end_ms - start_ms > max_ms {
        return Err(QueryError::RangeTooWide);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
