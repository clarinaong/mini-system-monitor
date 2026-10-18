export type SystemMetrics = {
  cpuUsage: number | null;
  memoryUsed: number;
  memoryTotal: number;
  battery: BatteryMetrics | null;
};

export type BatteryMetrics = {
  percentage: number;
  status: string;
};

export type MetricsSample = {
  sequence: number;
  configRevision: number;
  timestampMs: number;
  monotonicMs: number;
  intervalMs: number;
  metrics: SystemMetrics | null;
  health: {
    status: "healthy" | "warmingUp" | "degraded" | "error";
    error: string | null;
    batteryStatus: "available" | "absent" | "error" | "unavailable";
    batteryError: string | null;
  };
};

export type MonitorSnapshot = {
  monotonicNowMs: number;
  configRevision: number;
  monitoringIntervalSeconds: number;
  latest: MetricsSample | null;
  lastSuccessful: MetricsSample | null;
  history: MetricsSample[];
};

export type GraphReading = {
  value: number | null;
  monotonicMs: number;
  intervalMs: number;
};

export type MonitorClock = {
  monotonicMs: number;
  receivedAtMs: number;
};
