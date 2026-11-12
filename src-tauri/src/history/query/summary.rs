use super::{
    MAX_AVERAGE_RANGE_MS, MAX_AVERAGE_SOURCE_ROWS, MINUTE_MS, Mean, QueryError, read_existing,
    validate_range,
};
use crate::history::HistoryDatabase;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

const MIN_COMPARISON_MS: i64 = 15 * MINUTE_MS;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Period {
    start_ms: i64,
    end_ms: i64,
}

impl Period {
    fn duration_ms(self) -> i64 {
        self.end_ms - self.start_ms
    }

    fn minutes(self) -> u32 {
        ((self.end_ms - 1) / MINUTE_MS - self.start_ms / MINUTE_MS + 1) as u32
    }
}

#[derive(Deserialize)]
pub struct SummaryRequest {
    current: Period,
    previous: Period,
}

pub struct SummaryQuery(SummaryRequest);

impl SummaryRequest {
    pub fn validate(self) -> Result<SummaryQuery, QueryError> {
        for period in [self.current, self.previous] {
            validate_range(period.start_ms, period.end_ms, MAX_AVERAGE_RANGE_MS)?;
        }
        if self.previous.end_ms != self.current.start_ms {
            return Err(QueryError::InvalidRange);
        }
        Ok(SummaryQuery(self))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricSummary {
    average_percent: Option<f64>,
    peak_percent: Option<f64>,
    covered_minutes: u32,
    coverage_percent: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodSummary {
    period: Period,
    total_minutes: u32,
    cpu: MetricSummary,
    ram: MetricSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Eligibility {
    Eligible,
    PeriodTooShort,
    UnequalPeriodLengths,
    InsufficientCoverage,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformanceSummary {
    current: PeriodSummary,
    previous: PeriodSummary,
    cpu_comparison: Eligibility,
    ram_comparison: Eligibility,
}

#[derive(Default)]
struct MetricAggregate {
    mean: Mean,
    peak: Option<f64>,
    last_minute: Option<i64>,
    covered_minutes: u32,
}

impl MetricAggregate {
    fn add(&mut self, minute: i64, reading: Option<f64>) {
        if let Some(reading) = reading {
            self.mean.add(Some(reading));
            self.peak = Some(self.peak.map_or(reading, |peak| peak.max(reading)));
            if self.last_minute != Some(minute) {
                self.covered_minutes += 1;
                self.last_minute = Some(minute);
            }
        }
    }

    fn finish(self, total_minutes: u32) -> MetricSummary {
        MetricSummary {
            average_percent: self.mean.finish().average,
            peak_percent: self.peak,
            covered_minutes: self.covered_minutes,
            coverage_percent: 100.0 * f64::from(self.covered_minutes) / f64::from(total_minutes),
        }
    }
}

#[derive(Default)]
struct PeriodAggregate {
    cpu: MetricAggregate,
    ram: MetricAggregate,
}

impl PeriodAggregate {
    fn finish(self, period: Period) -> PeriodSummary {
        let total_minutes = period.minutes();
        PeriodSummary {
            period,
            total_minutes,
            cpu: self.cpu.finish(total_minutes),
            ram: self.ram.finish(total_minutes),
        }
    }
}

impl SummaryQuery {
    pub(in crate::history) fn run(
        &self,
        database: &HistoryDatabase,
    ) -> Result<PerformanceSummary, QueryError> {
        self.read(&database.connection)
    }

    pub(in crate::history) fn read_existing(
        &self,
        path: &Path,
    ) -> Result<PerformanceSummary, QueryError> {
        read_existing(
            path,
            self.finish(PeriodAggregate::default(), PeriodAggregate::default()),
            |connection, _| self.read(connection),
        )
    }

    fn read(&self, connection: &Connection) -> Result<PerformanceSummary, QueryError> {
        let mut statement = connection.prepare(
            "SELECT timestamp_ms, cpu_percent, memory_used_bytes, memory_total_bytes
             FROM metric_samples INDEXED BY metric_samples_timestamp
             WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2
             ORDER BY timestamp_ms LIMIT ?3",
        )?;
        let mut aggregate_period = |period: Period| -> Result<PeriodAggregate, QueryError> {
            let mut aggregate = PeriodAggregate::default();
            let mut rows = statement.query(params![
                period.start_ms,
                period.end_ms,
                MAX_AVERAGE_SOURCE_ROWS + 1
            ])?;
            let mut source_count = 0;
            while let Some(row) = rows.next()? {
                source_count += 1;
                if source_count > MAX_AVERAGE_SOURCE_ROWS {
                    return Err(QueryError::TooManySourceRows);
                }
                let minute = row.get::<_, i64>(0)? / MINUTE_MS;
                aggregate.cpu.add(minute, row.get(1)?);
                let used: Option<i64> = row.get(2)?;
                let total: Option<i64> = row.get(3)?;
                let percent = used.zip(total).and_then(|(used, total)| {
                    (total > 0 && (0..=total).contains(&used))
                        .then(|| 100.0 * used as f64 / total as f64)
                });
                aggregate.ram.add(minute, percent);
            }
            Ok(aggregate)
        };
        let current = aggregate_period(self.0.current)?;
        let previous = aggregate_period(self.0.previous)?;
        Ok(self.finish(current, previous))
    }

    fn finish(&self, current: PeriodAggregate, previous: PeriodAggregate) -> PerformanceSummary {
        let current = current.finish(self.0.current);
        let previous = previous.finish(self.0.previous);
        let current_duration = current.period.duration_ms();
        let previous_duration = previous.period.duration_ms();
        let eligibility = |current_metric: &MetricSummary, previous_metric: &MetricSummary| {
            if current_duration.min(previous_duration) < MIN_COMPARISON_MS {
                Eligibility::PeriodTooShort
            } else if current_duration != previous_duration {
                Eligibility::UnequalPeriodLengths
            } else if current_metric.covered_minutes * 4 < current.total_minutes * 3
                || previous_metric.covered_minutes * 4 < previous.total_minutes * 3
            {
                Eligibility::InsufficientCoverage
            } else {
                Eligibility::Eligible
            }
        };
        PerformanceSummary {
            cpu_comparison: eligibility(&current.cpu, &previous.cpu),
            ram_comparison: eligibility(&current.ram, &previous.ram),
            current,
            previous,
        }
    }
}

#[cfg(test)]
mod tests;
