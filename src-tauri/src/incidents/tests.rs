use super::*;
use crate::monitor::{BatteryStatus, SampleHealth, SystemMetrics};

const MIB: u64 = 1024 * 1024;

fn sample(sequence: u64, seconds: u64, interval: u64, cpu: Option<f32>) -> MetricsSample {
    MetricsSample {
        sequence,
        config_revision: 0,
        timestamp_ms: 1_000_000 - seconds * 1000,
        monotonic_ms: seconds * 1000,
        interval_ms: interval * 1000,
        metrics: Some(SystemMetrics {
            cpu_usage: cpu,
            memory_used: 1024 * MIB,
            memory_total: 4096 * MIB,
            swap_used: 0,
            battery: None,
        }),
        health: SampleHealth {
            status: if cpu.is_some() {
                SampleStatus::Healthy
            } else {
                SampleStatus::WarmingUp
            },
            error: None,
            battery_status: BatteryStatus::Absent,
            battery_error: None,
        },
    }
}

fn started(
    kind: IncidentKind,
    start: &MetricsSample,
    detected: &MetricsSample,
) -> IncidentTransition {
    IncidentTransition::Started {
        kind,
        start: start.into(),
        detected: detected.into(),
    }
}

fn recovered(kind: IncidentKind, start: &MetricsSample, end: &MetricsSample) -> IncidentTransition {
    IncidentTransition::Recovered {
        kind,
        start: start.into(),
        end: end.into(),
    }
}

#[test]
fn cpu_spike_requires_both_thresholds_and_stays_open_until_recovery() {
    let mut detector = IncidentDetector::default();
    for (index, cpu) in [60.0, 60.0, 84.0, 85.0, 20.0, 20.0].into_iter().enumerate() {
        assert!(
            detector
                .observe(&sample(index as u64 + 1, index as u64, 1, Some(cpu)))
                .is_empty()
        );
    }
    let spike = sample(7, 6, 1, Some(85.0));
    assert_eq!(
        detector.observe(&spike),
        [started(IncidentKind::CpuSpike, &spike, &spike)]
    );
    assert!(detector.observe(&spike).is_empty());
    assert!(detector.observe(&sample(8, 7, 1, Some(95.0))).is_empty());
    assert!(detector.observe(&sample(9, 8, 1, Some(76.0))).is_empty());
    let end = sample(10, 9, 1, Some(75.0));
    assert_eq!(
        detector.observe(&end),
        [recovered(IncidentKind::CpuSpike, &spike, &end)]
    );
}

#[test]
fn cpu_baseline_uses_prior_samples_and_expires_after_thirty_seconds() {
    let mut detector = IncidentDetector::default();
    for index in 0..=3 {
        let point = sample(
            index + 1,
            index * 10,
            10,
            Some(if index == 0 { 0.0 } else { 70.0 }),
        );
        assert!(detector.observe(&point).is_empty());
    }
    let too_small = sample(5, 40, 10, Some(85.0));
    assert!(detector.observe(&too_small).is_empty());

    let mut detector = IncidentDetector::default();
    assert!(detector.observe(&sample(1, 0, 10, Some(0.0))).is_empty());
    assert!(detector.observe(&sample(2, 10, 10, Some(70.0))).is_empty());
    assert!(detector.observe(&sample(3, 20, 10, Some(70.0))).is_empty());
    let at_cutoff = sample(4, 30, 10, Some(85.0));
    assert_eq!(
        detector.observe(&at_cutoff),
        [started(IncidentKind::CpuSpike, &at_cutoff, &at_cutoff)]
    );
}

#[test]
fn sustained_cpu_and_ram_use_elapsed_time_at_all_supported_intervals() {
    for interval in [1, 2, 5, 10] {
        let mut detector = IncidentDetector::default();
        let mut sequence = 0;
        let mut next = |seconds, cpu, ram_percent| {
            sequence += 1;
            let mut point = sample(sequence, seconds, interval, Some(cpu));
            let metrics = point.metrics.as_mut().unwrap();
            metrics.memory_total = 100 * MIB;
            metrics.memory_used = ram_percent * MIB;
            point
        };
        let start = next(0, 90.0, 90);
        assert!(detector.observe(&start).is_empty());
        for seconds in (interval..30).step_by(interval as usize) {
            assert!(detector.observe(&next(seconds, 90.0, 90)).is_empty());
        }
        let detected = next(30, 90.0, 90);
        assert_eq!(
            detector.observe(&detected),
            [
                started(IncidentKind::SustainedCpu, &start, &detected),
                started(IncidentKind::SustainedMemory, &start, &detected),
            ]
        );
        assert!(detector.observe(&next(30 + interval, 81.0, 81)).is_empty());
        let recovery_start = 30 + 2 * interval;
        for seconds in (recovery_start..recovery_start + 10).step_by(interval as usize) {
            assert!(detector.observe(&next(seconds, 80.0, 80)).is_empty());
        }
        let end = next(recovery_start + 10, 80.0, 80);
        assert_eq!(
            detector.observe(&end),
            [
                recovered(IncidentKind::SustainedCpu, &start, &end),
                recovered(IncidentKind::SustainedMemory, &start, &end),
            ]
        );
    }
}

fn memory_sample(sequence: u64, seconds: u64, used_mib: u64, total_mib: u64) -> MetricsSample {
    let mut point = sample(sequence, seconds, 10, None);
    let metrics = point.metrics.as_mut().unwrap();
    metrics.memory_used = used_mib * MIB;
    metrics.memory_total = total_mib * MIB;
    point
}

#[test]
fn memory_jump_uses_both_thresholds_and_keeps_the_original_recovery_baseline() {
    for (total, entry_gain, recovery_boundary) in [(4096, 512, 205), (10240, 1024, 256)] {
        let mut detector = IncidentDetector::default();
        assert!(
            detector
                .observe(&memory_sample(1, 0, 1024, total))
                .is_empty()
        );
        assert!(
            detector
                .observe(&memory_sample(2, 10, 1024 + entry_gain - 1, total))
                .is_empty()
        );
        let jump = memory_sample(3, 20, 1024 + entry_gain, total);
        assert_eq!(
            detector.observe(&jump),
            [started(IncidentKind::MemoryJump, &jump, &jump)]
        );
        for index in 4..=8 {
            let used = if index == 8 {
                1024 + recovery_boundary
            } else {
                1024 + entry_gain
            };
            assert!(
                detector
                    .observe(&memory_sample(index, (index - 1) * 10, used, total))
                    .is_empty()
            );
        }
        let end = memory_sample(9, 80, 1024 + recovery_boundary - 1, total);
        assert_eq!(
            detector.observe(&end),
            [recovered(IncidentKind::MemoryJump, &jump, &end)]
        );
    }
}

#[test]
fn missing_cpu_restarts_cpu_evidence_without_breaking_valid_memory_evidence() {
    let mut detector = IncidentDetector::default();
    let high = |sequence, seconds, cpu| {
        let mut point = memory_sample(sequence, seconds, 3800, 4096);
        point.metrics.as_mut().unwrap().cpu_usage = cpu;
        if cpu.is_some() {
            point.health.status = SampleStatus::Healthy;
        }
        point
    };
    let first = high(1, 0, Some(90.0));
    assert!(detector.observe(&first).is_empty());
    assert!(detector.observe(&high(2, 10, None)).is_empty());
    let cpu_start = high(3, 20, Some(90.0));
    assert!(detector.observe(&cpu_start).is_empty());
    let memory_detected = high(4, 30, Some(90.0));
    assert_eq!(
        detector.observe(&memory_detected),
        [started(
            IncidentKind::SustainedMemory,
            &first,
            &memory_detected
        )]
    );
    assert!(detector.observe(&high(5, 40, Some(90.0))).is_empty());
    let cpu_detected = high(6, 50, Some(90.0));
    assert_eq!(
        detector.observe(&cpu_detected),
        [started(
            IncidentKind::SustainedCpu,
            &cpu_start,
            &cpu_detected
        )]
    );
}

#[test]
fn sampling_gaps_and_failures_clear_pending_recovery_without_ending_episode() {
    let mut detector = IncidentDetector::default();
    let start = sample(1, 0, 10, Some(90.0));
    for index in 1..=3 {
        assert!(
            detector
                .observe(&sample(index, (index - 1) * 10, 10, Some(90.0)))
                .is_empty()
        );
    }
    let detected = sample(4, 30, 10, Some(90.0));
    assert_eq!(
        detector.observe(&detected),
        [started(IncidentKind::SustainedCpu, &start, &detected)]
    );
    assert!(detector.observe(&sample(5, 40, 10, Some(80.0))).is_empty());
    assert!(detector.observe(&sample(6, 70, 10, Some(80.0))).is_empty());
    assert!(detector.observe(&sample(5, 80, 10, Some(80.0))).is_empty());

    let mut failed = sample(7, 80, 10, Some(80.0));
    failed.health.status = SampleStatus::Error;
    failed.metrics = None;
    assert!(detector.observe(&failed).is_empty());
    assert!(detector.observe(&sample(8, 90, 10, Some(80.0))).is_empty());
    let end = sample(9, 100, 10, Some(80.0));
    assert_eq!(
        detector.observe(&end),
        [recovered(IncidentKind::SustainedCpu, &start, &end)]
    );
}

#[test]
fn skipped_sequences_and_interval_changes_clear_the_spike_baseline() {
    let mut detector = IncidentDetector::default();
    for point in [
        sample(1, 0, 1, Some(20.0)),
        sample(2, 1, 1, Some(20.0)),
        sample(4, 2, 1, Some(90.0)),
        sample(5, 3, 1, Some(20.0)),
        sample(6, 4, 1, Some(20.0)),
        sample(7, 6, 2, Some(90.0)),
        sample(8, 8, 2, Some(f32::NAN)),
        sample(9, 10, 2, Some(90.0)),
    ] {
        assert!(detector.observe(&point).is_empty());
    }
}
