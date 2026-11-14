import { bytesToGiB } from "../utils/format";
import UsageGraph from "./UsageGraph";
import type { GraphReading, MonitorClock } from "../types/system";

type MemoryMonitorProps = {
  usedMemory: number;
  totalMemory: number;
  history: GraphReading[];
  clock: MonitorClock;
  active: boolean;
  graphDelayMs: number;
  showGraph: boolean;
};

function MemoryMonitor({ usedMemory, totalMemory, history, clock, active, graphDelayMs, showGraph }: MemoryMonitorProps) {
  const memoryPercentage =
    totalMemory !== 0 ? (usedMemory / totalMemory) * 100 : 0;

  const usedGiB = bytesToGiB(usedMemory);
  const totalGiB = bytesToGiB(totalMemory);

  return (
    <section className="metric-section">
      <div className="metric-header">
        <h2 className="metric-label">MEMORY</h2>

        <span className="metric-value">
          {memoryPercentage.toFixed(1)}%
        </span>
      </div>

      <p className="metric-detail memory-detail">
        {usedGiB.toFixed(1)} / {totalGiB.toFixed(1)} GiB
      </p>
      {showGraph && <UsageGraph data={history} clock={clock} active={active} delayMs={graphDelayMs} />}
    </section>
  );
}

export default MemoryMonitor;
