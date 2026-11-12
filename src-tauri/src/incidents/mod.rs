use std::collections::VecDeque;

use crate::monitor::{MetricsSample, SampleStatus};

#[cfg(test)]
mod tests;

const BASELINE_WINDOW_MS: u64 = 30_000;
const SUSTAINED_START_MS: u64 = 30_000;
const SUSTAINED_RECOVERY_MS: u64 = 10_000;
const MEMORY_JUMP_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncidentKind {
    CpuSpike,
    SustainedCpu,
    SustainedMemory,
    MemoryJump,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Observation {
    pub sequence: u64,
    pub monotonic_ms: u64,
    pub timestamp_ms: u64,
}

impl From<&MetricsSample> for Observation {
    fn from(sample: &MetricsSample) -> Self {
        Self {
            sequence: sample.sequence,
            monotonic_ms: sample.monotonic_ms,
            timestamp_ms: sample.timestamp_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncidentTransition {
    Started {
        kind: IncidentKind,
        start: Observation,
        detected: Observation,
    },
    Recovered {
        kind: IncidentKind,
        start: Observation,
        end: Observation,
    },
}

impl IncidentTransition {
    pub(crate) fn start_sequence(&self) -> u64 {
        match self {
            Self::Started { start, .. } | Self::Recovered { start, .. } => start.sequence,
        }
    }
}

#[derive(Default)]
struct Episode {
    start: Option<Observation>,
    pending: Option<Observation>,
}

impl Episode {
    fn observe(
        &mut self,
        kind: IncidentKind,
        point: Observation,
        entry: bool,
        recovery: bool,
        entry_ms: u64,
        recovery_ms: u64,
    ) -> Option<IncidentTransition> {
        let (condition, duration) = if self.start.is_some() {
            (recovery, recovery_ms)
        } else {
            (entry, entry_ms)
        };
        if !condition {
            self.pending = None;
            return None;
        }
        let first = *self.pending.get_or_insert(point);
        if point.monotonic_ms - first.monotonic_ms < duration {
            return None;
        }
        self.pending = None;
        Some(match self.start.take() {
            Some(start) => IncidentTransition::Recovered {
                kind,
                start,
                end: point,
            },
            None => {
                self.start = Some(first);
                IncidentTransition::Started {
                    kind,
                    start: first,
                    detected: point,
                }
            }
        })
    }
}

#[derive(Default)]
pub(crate) struct IncidentDetector {
    last_sample: Option<(Observation, u64)>,
    cpu_baseline: VecDeque<(u64, f64)>,
    memory_baseline: VecDeque<(u64, u64)>,
    memory_total: Option<u64>,
    jump_baseline: Option<u64>,
    cpu_spike: Episode,
    sustained_cpu: Episode,
    sustained_memory: Episode,
    memory_jump: Episode,
}

impl IncidentDetector {
    pub(crate) fn observe(&mut self, sample: &MetricsSample) -> Vec<IncidentTransition> {
        let point = Observation::from(sample);
        if let Some((last, interval_ms)) = self.last_sample {
            if point.sequence <= last.sequence || point.monotonic_ms <= last.monotonic_ms {
                return Vec::new();
            }
            if point.sequence != last.sequence + 1
                || sample.interval_ms != interval_ms
                || point.monotonic_ms - last.monotonic_ms > interval_ms * 3 / 2 + 250
            {
                self.break_cpu_evidence();
                self.break_memory_evidence();
            }
        }
        self.last_sample = Some((point, sample.interval_ms));

        let mut transitions = Vec::new();
        let Some(metrics) = sample.metrics.as_ref().filter(|_| {
            sample.health.status != SampleStatus::Error && sample.health.error.is_none()
        }) else {
            self.break_cpu_evidence();
            self.break_memory_evidence();
            return transitions;
        };

        let cpu = metrics
            .cpu_usage
            .filter(|value| {
                sample.health.status != SampleStatus::WarmingUp && (0.0..=100.0).contains(value)
            })
            .map(f64::from);
        if let Some(cpu) = cpu {
            self.observe_cpu(point, cpu, &mut transitions);
        } else {
            self.break_cpu_evidence();
        }

        if metrics.memory_total > 0 && metrics.memory_used <= metrics.memory_total {
            self.observe_memory(
                point,
                metrics.memory_used,
                metrics.memory_total,
                &mut transitions,
            );
        } else {
            self.break_memory_evidence();
        }
        transitions
    }

    fn break_cpu_evidence(&mut self) {
        self.cpu_baseline.clear();
        self.cpu_spike.pending = None;
        self.sustained_cpu.pending = None;
    }

    fn break_memory_evidence(&mut self) {
        self.memory_baseline.clear();
        self.memory_jump.pending = None;
        self.sustained_memory.pending = None;
    }

    fn observe_cpu(
        &mut self,
        point: Observation,
        cpu: f64,
        transitions: &mut Vec<IncidentTransition>,
    ) {
        expire_baseline(&mut self.cpu_baseline, point.monotonic_ms);
        let is_spike = cpu >= 85.0
            && self.cpu_baseline.len() >= 2
            && cpu
                - self
                    .cpu_baseline
                    .iter()
                    .map(|(_, value)| value)
                    .sum::<f64>()
                    / self.cpu_baseline.len() as f64
                >= 25.0;
        self.cpu_baseline.push_back((point.monotonic_ms, cpu));

        transitions.extend(self.cpu_spike.observe(
            IncidentKind::CpuSpike,
            point,
            is_spike,
            cpu <= 75.0,
            0,
            0,
        ));
        transitions.extend(self.sustained_cpu.observe(
            IncidentKind::SustainedCpu,
            point,
            cpu >= 90.0,
            cpu <= 80.0,
            SUSTAINED_START_MS,
            SUSTAINED_RECOVERY_MS,
        ));
    }

    fn observe_memory(
        &mut self,
        point: Observation,
        used: u64,
        total: u64,
        transitions: &mut Vec<IncidentTransition>,
    ) {
        if self.memory_total != Some(total) {
            self.break_memory_evidence();
            self.memory_total = Some(total);
        }
        expire_baseline(&mut self.memory_baseline, point.monotonic_ms);
        let baseline = self.memory_baseline.iter().map(|&(_, used)| used).min();
        self.memory_baseline.push_back((point.monotonic_ms, used));

        let is_jump = baseline.is_some_and(|baseline| {
            let gain = used.saturating_sub(baseline);
            gain >= MEMORY_JUMP_BYTES && at_least_percent(gain, total, 10)
        });
        let recovered = self.jump_baseline.is_some_and(|baseline| {
            let gain = used.saturating_sub(baseline);
            gain < MEMORY_JUMP_BYTES / 2 && !at_least_percent(gain, total, 5)
        });
        if let Some(transition) =
            self.memory_jump
                .observe(IncidentKind::MemoryJump, point, is_jump, recovered, 0, 0)
        {
            self.jump_baseline = match transition {
                IncidentTransition::Started { .. } => baseline,
                IncidentTransition::Recovered { .. } => None,
            };
            transitions.push(transition);
        }
        transitions.extend(self.sustained_memory.observe(
            IncidentKind::SustainedMemory,
            point,
            at_least_percent(used, total, 90),
            u128::from(used) * 100 <= u128::from(total) * 80,
            SUSTAINED_START_MS,
            SUSTAINED_RECOVERY_MS,
        ));
    }
}

fn expire_baseline<T>(baseline: &mut VecDeque<(u64, T)>, now_ms: u64) {
    let cutoff = now_ms.saturating_sub(BASELINE_WINDOW_MS);
    while baseline.front().is_some_and(|(time, _)| *time < cutoff) {
        baseline.pop_front();
    }
}

fn at_least_percent(value: u64, total: u64, percent: u8) -> bool {
    u128::from(value) * 100 >= u128::from(total) * u128::from(percent)
}
