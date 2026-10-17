type CpuMonitorProps = {
  usage: number;
};

function CpuMonitor({ usage }: CpuMonitorProps) {
  return (
    <section className="metric-section">
      <div className="metric-header">
        <h2 className="metric-label">CPU</h2>
        <span className="metric-value">{usage.toFixed(1)}%</span>
      </div>
    </section>
  );
}

export default CpuMonitor;