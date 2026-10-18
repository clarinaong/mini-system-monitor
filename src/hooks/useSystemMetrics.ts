import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { MetricsSample, MonitorSnapshot } from "../types/system";
import { monitorNow } from "../monitor/history";
import { subscribeToMetrics, type MonitorView } from "../monitor/subscription";

export function useSystemMetrics() {
  const [view, setView] = useState<MonitorView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState(!document.hidden);
  const [stale, setStale] = useState(false);

  useEffect(() => {
    let active = true;
    let nativeVisible = true;
    let revision = 0;
    let unlistenVisibility: (() => void) | null = null;
    let unlistenSettings: (() => void) | null = null;
    const subscription = subscribeToMetrics({
      listen: receive => listen<MetricsSample>("system-metrics", event => receive(event.payload)),
      snapshot: () => invoke<MonitorSnapshot>("get_system_metrics"),
    }, next => { setView(next); setError(null); }, setError, () => performance.now(), !document.hidden);

    function updateVisibility() {
      const next = nativeVisible && !document.hidden;
      setVisible(next);
      subscription.setVisible(next);
    }

    document.addEventListener("visibilitychange", updateVisibility);
    void listen("settings-changed", () => {
      if (active) void subscription.refresh();
    }).then(cleanup => {
      if (!active) cleanup(); else unlistenSettings = cleanup;
    }).catch(() => { if (active) setError("Unable to subscribe to monitoring preferences."); });
    void listen<boolean>("widget-visibility", event => {
      if (!active) return;
      revision += 1;
      nativeVisible = event.payload;
      updateVisibility();
    }).then(async cleanup => {
      if (!active) { cleanup(); return; }
      unlistenVisibility = cleanup;
      const requestedAtRevision = revision;
      const initial = await getCurrentWindow().isVisible();
      if (active && revision === requestedAtRevision) { nativeVisible = initial; updateVisibility(); }
    }).catch(() => { if (active) setError("Unable to determine widget visibility."); });

    return () => {
      active = false;
      subscription.dispose();
      unlistenVisibility?.();
      unlistenSettings?.();
      document.removeEventListener("visibilitychange", updateVisibility);
    };
  }, []);

  useEffect(() => {
    if (!visible || !view?.snapshot.latest) { setStale(false); return; }
    const sample = view.snapshot.latest;
    const age = monitorNow(view.clock, performance.now()) - sample.monotonicMs;
    const remaining = Math.max(0, view.snapshot.monitoringIntervalSeconds * 2000 + 2000 - age);
    setStale(remaining === 0);
    if (remaining === 0) return;
    const timer = setTimeout(() => setStale(true), remaining);
    return () => clearTimeout(timer);
  }, [view, visible]);

  return { view, error, visible, stale };
}
