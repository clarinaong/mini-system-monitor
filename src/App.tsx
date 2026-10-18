import { useMemo } from "react";
import "./App.css";
import CpuMonitor from "./components/CpuMonitor";
import MemoryMonitor from "./components/MemoryMonitor";
import BatteryMonitor from "./components/BatteryMonitor";
import { useSystemMetrics } from "./hooks/useSystemMetrics";
import { graphReadings } from "./monitor/history";

function App() {
  const { view, error, visible, stale } = useSystemMetrics();
  const snapshot = view?.snapshot;
  const latest = snapshot?.latest;
  const successful = snapshot?.lastSuccessful;
  const metrics = successful?.metrics;
  const batteryHealth = latest?.metrics ? latest.health : successful?.health;
  const cpuHistory = useMemo(
    () => graphReadings(snapshot?.history ?? [], "cpu"), [snapshot?.history],
  );
  const memoryHistory = useMemo(
    () => graphReadings(snapshot?.history ?? [], "memory"), [snapshot?.history],
  );
  const message = error ?? latest?.health.error ?? (
    stale ? "System readings are delayed. Showing the last available readings." : null
  );
  const status = message ? "OFFLINE"
    : !latest ? "CONNECTING"
    : latest.health.batteryError ? "DEGRADED"
    : metrics?.cpuUsage === null ? "WARMING UP"
    : "LIVE";

  return (
    <main className="container" data-tauri-drag-region>
      <header className="dashboard-header">
        <h1>System Monitor</h1>
        <span
          className={`status-label status-${status.toLowerCase().replace(/ /g, "-")}`}
          role="status"
        >
          {status}
        </span>
      </header>
      {message && (
        <div className="error-message" role="alert">
          <p>{message}</p>
          {metrics && latest?.health.status === "error" && <p>Showing last successful readings.</p>}
        </div>
      )}
      <div className="dashboard-content">
        {metrics && view && batteryHealth ? (
          <>
            <CpuMonitor
              usage={metrics.cpuUsage}
              history={cpuHistory}
              clock={view.clock}
              active={visible}
            />
            <MemoryMonitor
              usedMemory={metrics.memoryUsed}
              totalMemory={metrics.memoryTotal}
              history={memoryHistory}
              clock={view.clock}
              active={visible}
            />
            <BatteryMonitor battery={metrics.battery} health={batteryHealth} />
          </>
        ) : !message && <p className="loading-message">Loading system metrics...</p>}
      </div>
    </main>
  );
}

export default App;
