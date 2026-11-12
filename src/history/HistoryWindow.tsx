import { useEffect, useMemo, useRef, useState } from "react";
import { useSettings } from "../hooks/useSettings";
import { HistoryGraph } from "./HistoryGraph";
import { IncidentDetails, IncidentTimeline } from "./IncidentTimeline";
import { queryIncidents, type SavedIncident } from "./incidents";
import { HOUR_MS, MINUTE_MS, queryHistory, type HistoryRequest } from "./query";
import { useHistoryRead } from "./useHistoryRead";
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
  const [selected, setSelected] = useState<SavedIncident | null>(null);
  const [historyAttempt, setHistoryAttempt] = useState(0);
  const [incidentAttempt, setIncidentAttempt] = useState(0);
  const pending = useRef<Promise<unknown> | null>(null);
  const focusPanel = useRef<HTMLDivElement>(null);
  const rangeSelect = useRef<HTMLSelectElement>(null);
  useEffect(() => {
    if (selected) focusPanel.current?.focus();
  }, [selected]);
  const graphRange = useMemo<HistoryRequest>(() => selected ? {
    startMs: Math.max(0, selected.startMs - 5 * MINUTE_MS),
    endMs: Math.min(request.endMs, selected.startMs + 10 * MINUTE_MS),
    resolution: "raw",
  } : request, [request, selected]);
  const history = useHistoryRead(graphRange, queryHistory, pending, historyAttempt, "history");
  const incidents = useHistoryRead(request, queryIncidents, pending, incidentAttempt, "incidents");
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
          <select id="history-range" ref={rangeSelect} value={duration} onChange={event => {
            const next = Number(event.currentTarget.value);
            setSelected(null);
            setDuration(next);
            setRequest(recentRange(next));
          }}>
            {ranges.map(range => <option key={range.duration} value={range.duration}>{range.label}</option>)}
          </select>
          <button type="button" disabled={!history && !incidents} onClick={() => {
            setSelected(null);
            setRequest(recentRange(duration));
          }}>Refresh</button>
        </div>
      </header>
      {selected && (
        <div className="incident-focus" ref={focusPanel} tabIndex={-1} aria-label="Incident details and chart controls">
          <IncidentDetails incident={selected} />
          <button type="button" onClick={() => {
            setSelected(null);
            rangeSelect.current?.focus();
          }}>Back to selected time range</button>
        </div>
      )}
      <p className="history-resolution">{graphRange.resolution === "raw"
        ? selected ? "Individual recorded samples around the incident. The dashed line marks its start."
          : "Individual recorded samples"
        : "One-minute averages. RAM shows average used bytes as a percentage of average capacity."}</p>
      {!history ? <p role="status">Loading history…</p> : "error" in history ? (
        <div className="history-message">
          <p role="alert">{history.error}</p>
          <button type="button" onClick={() => setHistoryAttempt(attempt => attempt + 1)}>Retry</button>
        </div>
      ) : history.data.rows.length === 0 ? (
        <p className="history-message" role="status">No saved samples in this range.</p>
      ) : (
        <div className="history-charts">
          <HistoryGraph result={history.data} range={graphRange} metric="cpu" incidentStart={selected?.startMs} />
          <HistoryGraph result={history.data} range={graphRange} metric="ram" incidentStart={selected?.startMs} />
        </div>
      )}
      <IncidentTimeline result={incidents} selected={selected} onSelect={setSelected}
        onRetry={() => setIncidentAttempt(attempt => attempt + 1)} />
    </main>
  );
}
