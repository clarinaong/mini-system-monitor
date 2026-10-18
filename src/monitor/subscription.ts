import type { MetricsSample, MonitorClock, MonitorSnapshot } from "../types/system.ts";
import { emptySnapshot, mergeSnapshot, monitorNow, sampleSnapshot } from "./history.ts";

export type MetricsTransport = {
  listen: (receive: (sample: MetricsSample) => void) => Promise<() => void>;
  snapshot: () => Promise<MonitorSnapshot>;
};

export type MonitorView = { snapshot: MonitorSnapshot; clock: MonitorClock };

export function subscribeToMetrics(
  transport: MetricsTransport,
  publish: (view: MonitorView) => void,
  reportError: (message: string) => void,
  now: () => number = () => performance.now(),
  initiallyVisible = true,
) {
  let disposed = false;
  let visible = initiallyVisible;
  let unlisten: (() => void) | null = null;
  let connecting = false;
  let retry: ReturnType<typeof setTimeout> | null = null;
  let snapshot = emptySnapshot();
  let clock: MonitorClock = { monotonicMs: 0, receivedAtMs: now() };

  function accept(incoming: MonitorSnapshot) {
    if (disposed) return;
    snapshot = mergeSnapshot(snapshot, incoming);
    const receivedAtMs = now();
    clock = { monotonicMs: Math.max(monitorNow(clock, receivedAtMs), incoming.monotonicNowMs), receivedAtMs };
    if (visible) publish({ snapshot, clock });
  }

  function scheduleRetry(action: () => Promise<void>) {
    if (disposed || !visible || retry !== null) return;
    retry = setTimeout(() => { retry = null; void action(); }, 2000);
  }

  async function refresh() {
    if (disposed || !visible) return;
    const sequence = snapshot.latest?.sequence ?? 0;
    try { accept(await transport.snapshot()); }
    catch {
      if (!disposed && visible && sequence === (snapshot.latest?.sequence ?? 0)) {
        reportError("Unable to retrieve cached system metrics.");
      }
      scheduleRetry(refresh);
    }
  }

  async function connect() {
    if (disposed || connecting || unlisten) return;
    connecting = true;
    try {
      const cleanup = await transport.listen(sample => accept(sampleSnapshot(sample)));
      if (disposed) { cleanup(); return; }
      unlisten = cleanup;
      await refresh();
    } catch {
      if (!disposed && visible) reportError("Unable to subscribe to system metrics.");
      scheduleRetry(connect);
    } finally { connecting = false; }
  }

  void connect();
  return {
    refresh,
    setVisible(next: boolean) {
      if (disposed || visible === next) return;
      visible = next;
      if (!visible && retry !== null) { clearTimeout(retry); retry = null; }
      if (visible) { if (unlisten) void refresh(); else void connect(); }
    },
    dispose() {
      disposed = true;
      unlisten?.();
      unlisten = null;
      if (retry !== null) clearTimeout(retry);
    },
  };
}
