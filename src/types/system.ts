export type SystemMetrics = {
  cpuUsage: number;
  memoryUsed: number;
  memoryTotal: number;
  battery: BatteryMetrics | null;
};

export type BatteryMetrics = {
  percentage: number;
  status: string;
};