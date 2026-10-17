import type { BatteryMetrics } from "../types/system";

type BatteryMonitorProps = {
  battery: BatteryMetrics | null;
};

function BatteryMonitor({ battery }: BatteryMonitorProps) {
  return (
    <section className="metric-section">
      <div className="metric-header">
        <h2 className="metric-label">BATTERY</h2>

        <span className="metric-value">
          {battery ? `${battery.percentage.toFixed(0)}%` : "—"}
        </span>
      </div>

      <p className="metric-detail">
        {battery ? battery.status : "Unavailable"}
      </p>
    </section>
  );
}

export default BatteryMonitor;