import type { GraphReading, MetricsSample, MonitorClock, MonitorSnapshot } from "../types/system.ts";

export const HISTORY_WINDOW_MS = 60_000;
const GRAPH_BUFFER_MS = 30_000;
export const MAX_SAMPLES = 91;

export function emptySnapshot(): MonitorSnapshot {
  return { monotonicNowMs: 0, configRevision: 0, monitoringIntervalSeconds: 1, latest: null, lastSuccessful: null, history: [] };
}

export function mergeSnapshot(current: MonitorSnapshot, incoming: MonitorSnapshot): MonitorSnapshot {
  const latest = (incoming.latest?.sequence ?? 0) > (current.latest?.sequence ?? 0)
    ? incoming.latest : current.latest;
  const lastSuccessful = (incoming.lastSuccessful?.sequence ?? 0) > (current.lastSuccessful?.sequence ?? 0)
    ? incoming.lastSuccessful : current.lastSuccessful;
  const monotonicNowMs = Math.max(current.monotonicNowMs, incoming.monotonicNowMs, latest?.monotonicMs ?? 0);
  const cutoff = monotonicNowMs - HISTORY_WINDOW_MS - GRAPH_BUFFER_MS;
  const bySequence = new Map<number, MetricsSample>();
  for (const sample of [...incoming.history, ...current.history]) {
    if (sample.monotonicMs >= cutoff) bySequence.set(sample.sequence, sample);
  }
  return {
    monotonicNowMs, latest, lastSuccessful,
    configRevision: Math.max(current.configRevision, incoming.configRevision),
    monitoringIntervalSeconds: incoming.configRevision >= current.configRevision
      ? incoming.monitoringIntervalSeconds : current.monitoringIntervalSeconds,
    history: [...bySequence.values()].sort((a, b) => a.sequence - b.sequence).slice(-MAX_SAMPLES),
  };
}

export function sampleSnapshot(sample: MetricsSample): MonitorSnapshot {
  return {
    monotonicNowMs: sample.monotonicMs, latest: sample,
    configRevision: sample.configRevision,
    monitoringIntervalSeconds: sample.intervalMs / 1000,
    lastSuccessful: sample.metrics ? sample : null, history: [sample],
  };
}

export function monitorNow(clock: MonitorClock, localNow: number): number {
  return clock.monotonicMs + Math.max(0, localNow - clock.receivedAtMs);
}

export function graphReadings(history: MetricsSample[], metric: "cpu" | "memory"): GraphReading[] {
  return history.map(sample => ({
    monotonicMs: sample.monotonicMs,
    intervalMs: sample.intervalMs,
    value: metric === "cpu" ? sample.metrics?.cpuUsage ?? null
      : sample.metrics && sample.metrics.memoryTotal > 0
        ? sample.metrics.memoryUsed / sample.metrics.memoryTotal * 100 : null,
  }));
}

export function graphGeometry(readings: GraphReading[], now: number, width: number, height: number) {
  let path = "";
  let previous: { reading: GraphReading; point: string } | null = null;
  let expiresAfter = -Infinity;
  for (const reading of readings) {
    if (reading.value === null || !Number.isFinite(reading.value)) {
      previous = null;
      continue;
    }
    const x = width * (1 - (now - reading.monotonicMs) / HISTORY_WINDOW_MS);
    const y = height * (1 - Math.max(0, Math.min(100, reading.value)) / 100);
    const point = `${x.toFixed(2)},${y.toFixed(2)}`;
    const cutoff = now - HISTORY_WINDOW_MS;

    if (reading.monotonicMs >= cutoff) {
      if (previous && reading.monotonicMs - previous.reading.monotonicMs
        <= Math.max(previous.reading.intervalMs, reading.intervalMs) * 1.5 + 250) {
        if (previous.reading.monotonicMs < cutoff) path += `M${previous.point} `;
        path += `L${point} `;
      } else {
        path += `M${point} `;
      }
      expiresAfter = reading.monotonicMs + HISTORY_WINDOW_MS;
    }
    previous = { reading, point };
  }
  return { path: path.trim(), anchor: now, expiresAfter };
}
