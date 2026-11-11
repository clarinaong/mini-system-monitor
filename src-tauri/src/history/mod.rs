use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::{fmt, path::Path, time::Duration};

const APPLICATION_ID: i32 = 0x4d53_4d48;
const SCHEMA_VERSION: i32 = 1;
const SCHEMA: &str = include_str!("schema.sql");

#[derive(Debug)]
pub enum HistoryError {
    Database(rusqlite::Error),
    Incompatible { application_id: i32, version: i32 },
    InvalidSchema(String),
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
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_millis(100))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut connection, SCHEMA)?;
        Ok(Self { connection })
    }
}

fn migrate(connection: &mut Connection, schema: &str) -> Result<(), HistoryError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let application_id: i32 =
        transaction.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version: i32 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match (application_id, version) {
        (0, 0) => {
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
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(SCHEMA)?;
    let manifest =
        |database: &Connection| -> Result<Vec<(String, String, String)>, rusqlite::Error> {
            database.prepare("SELECT type, name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect()
        };
    if manifest(connection)? != manifest(&expected)? {
        return Err(HistoryError::InvalidSchema(
            "tables, constraints or indexes do not match version 1".into(),
        ));
    }
    if connection.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(HistoryError::InvalidSchema("foreign key violations".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
