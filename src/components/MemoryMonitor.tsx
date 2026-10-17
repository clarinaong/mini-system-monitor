import { bytesToGiB } from "../utils/format";

type MemoryMonitorProps = {
  usedMemory: number;
  totalMemory: number;
};

function MemoryMonitor({ usedMemory, totalMemory }: MemoryMonitorProps) {
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

      <p className="metric-detail">
        {usedGiB.toFixed(1)} / {totalGiB.toFixed(1)} GiB
      </p>
    </section>
  );
}

export default MemoryMonitor;