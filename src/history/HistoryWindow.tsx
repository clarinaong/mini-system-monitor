import { useState } from "react";
import { useSettings } from "../hooks/useSettings";
import { HistoryGraph } from "./HistoryGraph";
import { HOUR_MS, MINUTE_MS, type HistoryRequest } from "./query";
import { useHistory } from "./useHistory";
import "./history.css";

const ranges = [
  { label: "Last 5 minutes", duration: 5 * MINUTE_MS },
  { label: "Last 15 minutes", duration: 15 * MINUTE_MS },
  { label: "Last hour", duration: HOUR_MS },
  { label: "Last 24 hours", duration: 24 * HOUR_MS },
  { label: "Last 7 days", duration: 7 * 24 * HOUR_MS },
];

function recentRange(duration: number): HistoryRequest {
  const endMs = Date.now() + 1;
  return {
    startMs: Math.max(0, endMs - duration),
    endMs,
    resolution: duration <= HOUR_MS ? "raw" : "minuteAverages",
  };
}

export default function HistoryWindow() {
  const [duration, setDuration] = useState(ranges[1].duration);
  const [request, setRequest] = useState(() => recentRange(ranges[1].duration));
  const history = useHistory(request);
  const { snapshot, loading } = useSettings();
  const recording = snapshot?.settings.history.recordingEnabled;

  return (
    <main className="history-page">
      <header className="history-header">
        <div>
          <h1>Performance history</h1>
          <p role="status">{loading ? "Checking recording status…" : snapshot && !snapshot.loadError
            ? (recording ? "Recording is on" : "Recording is off")
            : "Recording status unavailable"}</p>
        </div>
        <div className="history-controls">
          <label htmlFor="history-range">Time range</label>
          <select id="history-range" value={duration} onChange={event => {
            const next = Number(event.currentTarget.value);
            setDuration(next);
            setRequest(recentRange(next));
          }}>
            {ranges.map(range => <option key={range.duration} value={range.duration}>{range.label}</option>)}
          </select>
          <button type="button" disabled={!history} onClick={() => setRequest(recentRange(duration))}>Refresh</button>
        </div>
      </header>
      <p className="history-resolution">{request.resolution === "raw"
        ? "Individual recorded samples"
        : "One-minute averages. RAM shows average used bytes as a percentage of average capacity."}</p>
      {!history ? <p role="status">Loading history…</p> : "error" in history ? (
        <div className="history-message">
          <p role="alert">{history.error}</p>
          <button type="button" onClick={() => setRequest({ ...request })}>Retry</button>
        </div>
      ) : history.data.rows.length === 0 ? (
        <p className="history-message" role="status">No saved samples in this range.</p>
      ) : (
        <div className="history-charts">
          <HistoryGraph result={history.data} range={request} metric="cpu" />
          <HistoryGraph result={history.data} range={request} metric="ram" />
        </div>
      )}
    </main>
  );
}
