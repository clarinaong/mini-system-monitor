import { useState, useEffect, useCallback } from "react";
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

  const refreshMetrics = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // FOR TESTING
      // await new Promise<void>((resolve) => {
      //   setTimeout(resolve, 2000);
      // });
      const result = await getSystemMetrics();
      setMetrics(result);
    } catch (error) {
      console.error("Backend error:", error);
      setError("Unable to retrieve system metrics.");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshMetrics();
  }, [refreshMetrics]);

  const memoryPercentage =
    metrics && metrics.memoryTotal !== 0
      ? (metrics.memoryUsed / metrics.memoryTotal) * 100
      : 0;

  function handleRefresh() {
    void refreshMetrics();
  }

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
        </>
      ) : (
        loading && <p>Loading system metrics...</p>
      )}
      {loading && metrics !== null && <p>Refreshing system metrics...</p>}
      <button onClick={handleRefresh} disabled={loading}>
        {loading ? "Refreshing..." : "Refresh"}
      </button>
    </main>
  );
}

export default App;
