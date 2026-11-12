import { invoke } from "@tauri-apps/api/core";
import { HOUR_MS, type HistoryRequest } from "./query";

type Period = { startMs: number; endMs: number };

export type SummaryRequest = { current: Period; previous: Period };

export type MetricSummary = {
  averagePercent: number | null;
  peakPercent: number | null;
  coveredMinutes: number;
  coveragePercent: number;
};

type PeriodSummary = {
  period: Period;
  totalMinutes: number;
  cpu: MetricSummary;
  ram: MetricSummary;
};

export type Eligibility = "eligible" | "periodTooShort" | "unequalPeriodLengths" | "insufficientCoverage";

export type PerformanceSummary = {
  current: PeriodSummary;
  previous: PeriodSummary;
  cpuComparison: Eligibility;
  ramComparison: Eligibility;
};

export function querySummary(request: SummaryRequest) {
  return invoke<PerformanceSummary>("query_performance_summary", { request });
}

export function localTimeZone() {
  return new Intl.DateTimeFormat().resolvedOptions().timeZone;
}

export function historySelection(range: number | "yesterday", nowMs = Date.now()) {
  let current: Period;
  let previous: Period;
  if (range === "yesterday") {
    const now = new Date(nowMs);
    const dayStart = (offset: number) =>
      new Date(now.getFullYear(), now.getMonth(), now.getDate() + offset).getTime();
    current = { startMs: dayStart(-1), endMs: dayStart(0) };
    previous = { startMs: dayStart(-2), endMs: current.startMs };
  } else {
    const endMs = nowMs + 1;
    const startMs = Math.max(0, endMs - range);
    current = { startMs, endMs };
    previous = { startMs: Math.max(0, startMs - range), endMs: startMs };
  }

  const history: HistoryRequest = {
    ...current,
    resolution: range !== "yesterday" && range <= HOUR_MS ? "raw" : "minuteAverages",
  };
  return { history, summary: { current, previous }, timeZone: localTimeZone() };
}
