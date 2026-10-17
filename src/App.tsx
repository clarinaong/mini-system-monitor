import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";
import type { SystemMetrics } from "./types/system";
import { bytesToGiB } from "./utils/format";

function getSystemMetrics(): Promise<SystemMetrics> {
  return invoke<SystemMetrics>("get_system_metrics");
}

function App() {
  const [metrics, setMetrics] = useState<SystemMetrics | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);

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
        }
      } catch (error) {
        if (active) {
          setLoading(false);
          setError("Unable to retrieve system metrics.");
        }
        if (active && !previouslyFailed) {
          console.error("Failed to retrieve metrics", error);
          previouslyFailed = true;
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

  const memoryPercentage =
    metrics && metrics.memoryTotal !== 0
      ? (metrics.memoryUsed / metrics.memoryTotal) * 100
      : 0;

  return (
    <main className="container">
      <p>Mini System Monitor</p>
      {error && (
        <>
          <p>{error}</p>
          {metrics && <p>Showing last successful readings.</p>}
        </>
      )}
      {metrics ? (
        <>
          <p>CPU: {metrics.cpuUsage.toFixed(1)}%</p>
          <p>
            MEMORY: {bytesToGiB(metrics.memoryUsed).toFixed(1)} /{" "}
            {bytesToGiB(metrics.memoryTotal).toFixed(1)} GiB (
            {memoryPercentage.toFixed(1)}%)
          </p>
          {metrics.battery ? (
            <p>
              BATTERY: {metrics.battery.percentage.toFixed(0)}% (
              {metrics.battery.status})
            </p>
          ) : (
            <p>BATTERY: Unavailable</p>
          )}
        </>
      ) : (
        loading && <p>Loading system metrics...</p>
      )}
    </main>
  );
}

export default App;
