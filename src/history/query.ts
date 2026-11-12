import { invoke } from "@tauri-apps/api/core";

export const MINUTE_MS = 60_000;
export const HOUR_MS = 60 * MINUTE_MS;

export type HistoryRequest = {
  startMs: number;
  endMs: number;
  resolution: "raw" | "minuteAverages";
};

export type RawSample = {
  sessionId: number;
  sequence: number;
  timestampMs: number;
  monotonicMs: number;
  intervalMs: number;
  cpuPercent: number | null;
  memoryUsedBytes: number | null;
  memoryTotalBytes: number | null;
  batteryPercent: number | null;
  batteryState: string | null;
  sampleStatus: string;
  batteryStatus: string;
};

type Average = { average: number | null; validCount: number };

export type MinuteBucket = {
  startMs: number;
  partial: boolean;
  cpuPercent: Average;
  memoryUsedBytes: Average;
  memoryTotalBytes: Average;
  batteryPercent: Average;
};

export type HistoryResult =
  | { resolution: "raw"; rows: RawSample[] }
  | { resolution: "minuteAverages"; rows: MinuteBucket[] };

export function queryHistory(request: HistoryRequest) {
  return invoke<HistoryResult>("query_history", { request });
}

export function historyError(error: unknown, subject: "history" | "incidents" | "performance summary") {
  const code = typeof error === "object" && error !== null && "code" in error ? error.code : null;
  switch (code) {
    case "rangeTooWide":
    case "tooManySourceRows":
    case "tooManyResults":
      return "This range exceeds the history query limit. Choose a shorter range.";
    case "busy":
      return "A history request is still running. Try again in a moment.";
    default:
      return `Unable to read saved ${subject}. Try again.`;
  }
}
