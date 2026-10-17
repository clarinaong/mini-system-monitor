import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import type { SystemMetrics } from "./types/system";
import CpuMonitor from "./components/CpuMonitor";
import MemoryMonitor from "./components/MemoryMonitor";
import BatteryMonitor from "./components/BatteryMonitor";

function getSystemMetrics(): Promise<SystemMetrics> {
  return invoke<SystemMetrics>("get_system_metrics");
}

function App() {
  const [metrics, setMetrics] = useState<SystemMetrics | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

  const [cpuHistory, setCpuHistory] = useState<number[]>([]);
  const [memoryHistory, setMemoryHistory] = useState<number[]>([]);

  useEffect(() => {
    let timerId: ReturnType<typeof setTimeout>;
    let active = true;
    let previouslyFailed = false;

    async function checkMetrics() {
      try {
        const result = await getSystemMetrics();

        if (active) {
          setLoading(false);
          setMetrics(result);
          setError(null);
          previouslyFailed = false;

          // Update CPU history
          setCpuHistory((previous) => {
            const updatedHistory = [...previous, result.cpuUsage];
            return updatedHistory.slice(-60);
          });

          // Calculate memory usage percentage
          const memoryPercentage =
            result.memoryTotal !== 0
              ? (result.memoryUsed / result.memoryTotal) * 100
              : 0;

          // Update RAM history
          setMemoryHistory((previous) => {
            const updatedHistory = [...previous, memoryPercentage];
            return updatedHistory.slice(-60);
          });
        }
      } catch (err) {
        if (active) {
          setLoading(false);
          setError("Unable to retrieve system metrics.");

          if (!previouslyFailed) {
            console.error("Failed to retrieve metrics:", err);
            previouslyFailed = true;
          }
        }
      } finally {
        if (active) {
          timerId = setTimeout(checkMetrics, 1000);
        }
      }
    }

    void checkMetrics();

    return () => {
      active = false;
      clearTimeout(timerId);
    };
  }, []);

  const status = error
    ? "OFFLINE"
    : loading
      ? "CONNECTING"
      : "LIVE";

  return (
    <main className="container">
      <header className="dashboard-header">
        <h1>System Monitor</h1>

        <span
          className={`status-label status-${status.toLowerCase()}`}
          role="status"
        >
          {status}
        </span>
      </header>

      {error && (
        <div className="error-message" role="alert">
          <p>{error}</p>

          {metrics && (
            <p>Showing last successful readings.</p>
          )}
        </div>
      )}

      <div className="dashboard-content">
        {metrics ? (
          <>
            <CpuMonitor
              usage={metrics.cpuUsage}
              history={cpuHistory}
            />

            <MemoryMonitor
              usedMemory={metrics.memoryUsed}
              totalMemory={metrics.memoryTotal}
              history={memoryHistory}
            />

            <BatteryMonitor battery={metrics.battery} />
          </>
        ) : (
          loading && (
            <p className="loading-message">
              Loading system metrics...
            </p>
          )
        )}
      </div>
    </main>
  );
}

export default App;