import UsageGraph from "./UsageGraph";
import type { GraphReading, MonitorClock } from "../types/system";

type CpuMonitorProps = {
  usage: number | null;
  history: GraphReading[];
  clock: MonitorClock;
  active: boolean;
};

function CpuMonitor({ usage, history, clock, active }: CpuMonitorProps) {
  return (
    <section className="metric-section">
      <div className="metric-header">
        <h2 className="metric-label">CPU</h2>
        <span className="metric-value">{usage === null ? "—" : `${usage.toFixed(1)}%`}</span>
      </div>
      {usage === null && <p className="metric-detail">Warming up CPU readings...</p>}
      <UsageGraph data={history} clock={clock} active={active} />
    </section>
  );
}

export default CpuMonitor;
