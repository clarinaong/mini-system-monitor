import { invoke } from "@tauri-apps/api/core";

export const incidentKinds = {
  cpuSpike: "CPU spike",
  sustainedCpu: "Sustained CPU usage",
  sustainedMemory: "Sustained RAM usage",
  memoryJump: "RAM usage jump",
};

export const incidentStatuses = {
  open: "Open",
  recovered: "Recovered",
  interrupted: "Interrupted",
};

export type IncidentKind = keyof typeof incidentKinds;
export type IncidentStatus = keyof typeof incidentStatuses;
export type SavedIncident = {
  sessionId: number;
  kind: IncidentKind;
  startSequence: number;
  startMs: number;
  detectedMs: number;
  endMs: number | null;
  durationMs: number | null;
  status: IncidentStatus;
};

export function queryIncidents({ startMs, endMs }: { startMs: number; endMs: number }) {
  return invoke<SavedIncident[]>("query_incidents", { request: { startMs, endMs } });
}

export function incidentKey(incident: SavedIncident) {
  return `${incident.sessionId}:${incident.kind}:${incident.startSequence}`;
}
