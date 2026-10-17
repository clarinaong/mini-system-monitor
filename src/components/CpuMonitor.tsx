import UsageGraph from "./UsageGraph";

type CpuMonitorProps = {
  usage: number;
  history: number[];
};

function CpuMonitor({ usage, history }: CpuMonitorProps) {
  return (
    <section className="metric-section">
      <div className="metric-header">
        <h2 className="metric-label">CPU</h2>
        <span className="metric-value">{usage.toFixed(1)}%</span>
      </div>
      <UsageGraph data={history} />
    </section>
  );
}

export default CpuMonitor;