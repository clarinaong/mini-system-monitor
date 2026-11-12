use rusqlite::params;

use super::{HistoryDatabase, HistoryError, session_for_write};
use crate::incidents::{IncidentKind, IncidentTransition, Observation};

impl HistoryDatabase {
    pub(super) fn record_event(
        &mut self,
        session: Option<i64>,
        transition: IncidentTransition,
    ) -> Result<Option<i64>, HistoryError> {
        match transition {
            IncidentTransition::Started {
                kind,
                start,
                detected,
            } => {
                let [sequence, timestamp, monotonic] = observation_values(start)?;
                let detected = observation_values(detected)?;
                let transaction = self.connection.transaction()?;
                let id = session_for_write(&transaction, session, timestamp)?;
                transaction.execute(
                    "INSERT INTO performance_events (
                        session_id, kind, start_sequence, start_ms, start_monotonic_ms,
                        detected_sequence, detected_ms, detected_monotonic_ms
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    ON CONFLICT(session_id, kind, start_sequence) DO NOTHING",
                    params![
                        id,
                        kind_name(kind),
                        sequence,
                        timestamp,
                        monotonic,
                        detected[0],
                        detected[1],
                        detected[2]
                    ],
                )?;
                transaction.commit()?;
                Ok(Some(id))
            }
            IncidentTransition::Recovered { kind, start, end } => {
                if let Some(id) = session {
                    let [sequence, timestamp, monotonic] = observation_values(end)?;
                    let [start_sequence, _, _] = observation_values(start)?;
                    self.connection.execute(
                        "UPDATE performance_events
                         SET end_sequence = ?1, end_ms = ?2, end_monotonic_ms = ?3
                         WHERE session_id = ?4 AND kind = ?5 AND start_sequence = ?6
                           AND end_sequence IS NULL",
                        params![
                            sequence,
                            timestamp,
                            monotonic,
                            id,
                            kind_name(kind),
                            start_sequence
                        ],
                    )?;
                }
                Ok(session)
            }
        }
    }
}

fn observation_values(observation: Observation) -> Result<[i64; 3], HistoryError> {
    let convert = |value| {
        i64::try_from(value).map_err(|_| HistoryError::InvalidSample("incident observation"))
    };
    Ok([
        convert(observation.sequence)?,
        convert(observation.timestamp_ms)?,
        convert(observation.monotonic_ms)?,
    ])
}

fn kind_name(kind: IncidentKind) -> &'static str {
    match kind {
        IncidentKind::CpuSpike => "cpuSpike",
        IncidentKind::SustainedCpu => "sustainedCpu",
        IncidentKind::SustainedMemory => "sustainedMemory",
        IncidentKind::MemoryJump => "memoryJump",
    }
}
