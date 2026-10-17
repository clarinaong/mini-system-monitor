import type { BatteryMetrics } from "../types/system";

type BatteryMonitorProps = {
  battery: BatteryMetrics | null;
};

function BatteryMonitor({ battery }: BatteryMonitorProps) {
  const percentage = battery
    ? Math.max(0, Math.min(100, battery.percentage))
    : 0;

  return (
    <section className="metric-section">
      <div className="metric-header">
        <h2 className="metric-label">BATTERY</h2>

        <span className="metric-value">
          {battery ? `${percentage.toFixed(0)}%` : "—"}
        </span>
      </div>

      <div
        className="battery-track"
        role="progressbar"
        aria-label="Battery charge"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={battery ? percentage : undefined}
        aria-valuetext={
          battery ? `${percentage.toFixed(0)}%` : "Unavailable"
        }
      >
        <div
          className="battery-fill"
          style={{ width: `${percentage}%` }}
        />
      </div>

      <p className="metric-detail">
        {battery ? battery.status : "Unavailable"}
      </p>
    </section>
  );
}

export default BatteryMonitor;