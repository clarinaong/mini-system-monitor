import { useMemo, useRef } from "react";
import "./App.css";
import CpuMonitor from "./components/CpuMonitor";
import MemoryMonitor from "./components/MemoryMonitor";
import BatteryMonitor from "./components/BatteryMonitor";
import { useSystemMetrics } from "./hooks/useSystemMetrics";
import { graphReadings } from "./monitor/history";
import { useSettings } from "./hooks/useSettings";
import { useWidgetSize } from "./hooks/useWidgetSize";

function App() {
  const widget = useRef<HTMLElement>(null);
  const preferences = useSettings();
  const settings = preferences.snapshot?.settings;
  const { view, error, visible, stale } = useSystemMetrics();
  const snapshot = view?.snapshot;
  const latest = snapshot?.latest;
  const successful = snapshot?.lastSuccessful;
  const metrics = successful?.metrics;
  const allHidden = settings && !Object.values(settings.visibleCards).some(Boolean);
  const resizeError = useWidgetSize(widget, Boolean(settings && (metrics || allHidden)));
  const batteryHealth = latest?.metrics ? latest.health : successful?.health;
  const cpuHistory = useMemo(
    () => graphReadings(snapshot?.history ?? [], "cpu"), [snapshot?.history],
  );
  const memoryHistory = useMemo(
    () => graphReadings(snapshot?.history ?? [], "memory"), [snapshot?.history],
  );
  const message = resizeError ?? error ?? latest?.health.error ?? (
    stale ? "System readings are delayed. Showing the last available readings." : null
  );
  const status = message ? "OFFLINE"
    : !latest ? "CONNECTING"
    : latest.health.batteryError ? "DEGRADED"
    : metrics?.cpuUsage === null ? "WARMING UP"
    : "LIVE";

  return (
    <main ref={widget} className={`container layout-${settings?.layout ?? "expanded"}`} data-tauri-drag-region>
      <header className="dashboard-header">
        <h1>System Monitor</h1>
        <span
          className={`status-label status-${status.toLowerCase().replace(/ /g, "-")}`}
          role="status"
        >
          {status}
        </span>
      </header>
      {(preferences.error || preferences.snapshot?.loadError) && (
        <div className="error-message" role="alert">
          <p>Saved preferences could not be loaded. Open Settings from the menu bar to retry.</p>
        </div>
      )}
      {message && (
        <div className="error-message" role="alert">
          <p>{message}</p>
          {metrics && latest?.health.status === "error" && <p>Showing last successful readings.</p>}
        </div>
      )}
      <div className="dashboard-content">
        {allHidden ? (
          <p className="empty-widget">All metrics are hidden. Open Settings… from the menu bar to show them.</p>
        ) : metrics && view && batteryHealth && settings ? (
          <>
            {settings.visibleCards.cpu && <CpuMonitor
              usage={metrics.cpuUsage}
              history={cpuHistory}
              clock={view.clock}
              active={visible}
              showGraph={settings.graphs.cpu}
            />}
            {settings.visibleCards.memory && <MemoryMonitor
              usedMemory={metrics.memoryUsed}
              totalMemory={metrics.memoryTotal}
              history={memoryHistory}
              clock={view.clock}
              active={visible}
              showGraph={settings.graphs.memory}
            />}
            {settings.visibleCards.battery && <BatteryMonitor battery={metrics.battery} health={batteryHealth} />}
          </>
        ) : !message && <p className="loading-message">{settings ? "Loading system metrics..." : "Loading preferences..."}</p>}
      </div>
    </main>
  );
}

export default App;
