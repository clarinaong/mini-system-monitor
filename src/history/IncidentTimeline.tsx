import { useState } from "react";
import {
  incidentKey, incidentKinds, incidentStatuses, type IncidentKind, type IncidentStatus, type SavedIncident,
} from "./incidents";

const timestampLabel = new Intl.DateTimeFormat(undefined, {
  month: "short", day: "numeric", hour: "numeric", minute: "2-digit", second: "2-digit",
});

function IncidentTime({ ms }: { ms: number }) {
  return <time dateTime={new Date(ms).toISOString()}>{timestampLabel.format(ms)}</time>;
}

export function IncidentDetails({ incident }: { incident: SavedIncident }) {
  return (
    <section className="history-chart" aria-label="Selected incident">
      <h2>{incidentKinds[incident.kind]} · {incidentStatuses[incident.status]}</h2>
      <dl className="incident-details">
        <dt>Started</dt><dd><IncidentTime ms={incident.startMs} /></dd>
        <dt>Detected</dt><dd><IncidentTime ms={incident.detectedMs} /></dd>
        <dt>Ended</dt><dd>{incident.endMs === null ? "Unknown" : <IncidentTime ms={incident.endMs} />}</dd>
        {incident.durationMs !== null && <>
          <dt>Duration</dt><dd>{Math.round(incident.durationMs / 1000).toLocaleString()} seconds</dd>
        </>}
      </dl>
    </section>
  );
}

type IncidentRead = { data: SavedIncident[] } | { error: string } | null;

export function IncidentTimeline({ result, selected, onSelect, onRetry }: {
  result: IncidentRead;
  selected: SavedIncident | null;
  onSelect: (incident: SavedIncident | null, trigger?: HTMLButtonElement) => void;
  onRetry: () => void;
}) {
  const [kind, setKind] = useState<IncidentKind | "all">("all");
  const [status, setStatus] = useState<IncidentStatus | "all">("all");
  const rows = result && "data" in result ? result.data : [];
  const matches = rows.filter(incident =>
    (kind === "all" || incident.kind === kind) && (status === "all" || incident.status === status));

  return (
    <section className="history-chart incident-timeline" aria-labelledby="incident-title">
      <header className="history-header">
        <h2 id="incident-title">Incident timeline</h2>
        <div className="history-controls">
          <label htmlFor="incident-kind">Type</label>
          <select id="incident-kind" value={kind} onChange={event => {
            const next = event.currentTarget.value as IncidentKind | "all";
            setKind(next);
            if (selected && next !== "all" && next !== selected.kind) onSelect(null);
          }}>
            <option value="all">All types</option>
            {Object.entries(incidentKinds).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
          <label htmlFor="incident-status">Status</label>
          <select id="incident-status" value={status} onChange={event => {
            const next = event.currentTarget.value as IncidentStatus | "all";
            setStatus(next);
            if (selected && next !== "all" && next !== selected.status) onSelect(null);
          }}>
            <option value="all">All statuses</option>
            {Object.entries(incidentStatuses).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
          </select>
        </div>
      </header>
      {!result ? <p role="status">Loading incidents…</p> : "error" in result ? (
        <>
          <p role="alert">{result.error}</p>
          <button type="button" onClick={onRetry}>Retry incidents</button>
        </>
      ) : rows.length === 0 ? <p role="status">No incidents started in this range.</p>
        : matches.length === 0 ? <p role="status">No incidents match these filters.</p> : (
          <ul>
            {matches.map(incident => {
              const key = incidentKey(incident);
              return (
                <li key={key}>
                  <button type="button" aria-pressed={selected !== null && incidentKey(selected) === key}
                    aria-label={`${incidentKinds[incident.kind]}, ${incidentStatuses[incident.status]}, ${timestampLabel.format(incident.startMs)}`}
                    onClick={event => onSelect(incident, event.currentTarget)}>
                    <IncidentTime ms={incident.startMs} />
                    <strong>{incidentKinds[incident.kind]}</strong>
                    <span>{incidentStatuses[incident.status]}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
    </section>
  );
}
