import { useAppearance } from "../hooks/useAppearance";
import { useEffect, useMemo, useRef, useState } from "react";
import { useSettings } from "../hooks/useSettings";
import { HistoryGraph } from "./HistoryGraph";
import { IncidentDetails, IncidentTimeline } from "./IncidentTimeline";
import { queryIncidents, type SavedIncident } from "./incidents";
import { PerformanceSummary } from "./PerformanceSummary";
import { historySelection, localTimeZone, querySummary } from "./summary";
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

export default function HistoryWindow() {
  const [range, setRange] = useState<number | "yesterday">(ranges[1].duration);
  const [selection, setSelection] = useState(() => historySelection(ranges[1].duration));
  const request = selection.history;
  const [selected, setSelected] = useState<SavedIncident | null>(null);
  const [historyAttempt, setHistoryAttempt] = useState(0);
  const [incidentAttempt, setIncidentAttempt] = useState(0);
  const [summaryAttempt, setSummaryAttempt] = useState(0);
  const pending = useRef<Promise<unknown> | null>(null);
  const focusPanel = useRef<HTMLDivElement>(null);
  const rangeSelect = useRef<HTMLSelectElement>(null);
  useEffect(() => {
    if (selected) focusPanel.current?.focus();
  }, [selected]);
  useEffect(() => {
    if (range !== "yesterday") return;
    const refreshTimeZone = () => {
      if (selection.timeZone !== localTimeZone()) {
        setSelected(null);
        setSelection(historySelection(range));
      }
    };
    window.addEventListener("focus", refreshTimeZone);
    return () => window.removeEventListener("focus", refreshTimeZone);
  }, [range, selection.timeZone]);

  const graphRange = useMemo<HistoryRequest>(() => selected ? {
    startMs: Math.max(0, selected.startMs - 5 * MINUTE_MS),
    endMs: Math.min(request.endMs, selected.startMs + 10 * MINUTE_MS),
    resolution: "raw",
  } : request, [request, selected]);
  const history = useHistoryRead(graphRange, queryHistory, pending, historyAttempt, "history");
  const incidents = useHistoryRead(request, queryIncidents, pending, incidentAttempt, "incidents");
  const summary = useHistoryRead(selection.summary, querySummary, pending, summaryAttempt, "performance summary");
  const { snapshot, loading } = useSettings();
  useAppearance(snapshot?.settings.appearance);
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
          <select id="history-range" ref={rangeSelect} value={range} onChange={event => {
            const value = event.currentTarget.value;
            const next = value === "yesterday" ? value : Number(value);
            setSelected(null);
            setRange(next);
            setSelection(historySelection(next));
          }}>
            {ranges.map(range => <option key={range.duration} value={range.duration}>{range.label}</option>)}
            <option value="yesterday">Yesterday</option>
          </select>
          <button type="button" disabled={!history && !incidents && !summary} onClick={() => {
            setSelected(null);
            setSelection(historySelection(range));
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
      <PerformanceSummary result={summary} request={selection.summary} timeZone={selection.timeZone}
        onRetry={() => setSummaryAttempt(attempt => attempt + 1)} />
      <IncidentTimeline result={incidents} selected={selected} onSelect={setSelected}
        onRetry={() => setIncidentAttempt(attempt => attempt + 1)} />
    </main>
  );
}
