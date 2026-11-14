import test from "node:test";
import assert from "node:assert/strict";
import { setImmediate } from "node:timers/promises";
import { emptySnapshot, graphGeometry, graphReadings, mergeSnapshot, monitorNow, sampleSnapshot } from "../src/monitor/history.ts";
import { subscribeToMetrics } from "../src/monitor/subscription.ts";

function sample(sequence, monotonicMs = sequence * 1000, intervalMs = 1000) {
  return {
    sequence, configRevision: 0, timestampMs: 1_800_000_000_000 + monotonicMs, monotonicMs, intervalMs,
    metrics: { cpuUsage: sequence, memoryUsed: 512, memoryTotal: 1024, battery: null },
    health: { status: "healthy", error: null, batteryStatus: "absent", batteryError: null },
  };
}

function snapshot(samples, now = samples.at(-1)?.monotonicMs ?? 0, interval = 1, revision = 0) {
  return {
    monotonicNowMs: now, monitoringIntervalSeconds: interval, configRevision: revision,
    latest: samples.at(-1) ?? null,
    lastSuccessful: samples.findLast(value => value.metrics !== null) ?? null,
    history: samples,
  };
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function harness(snapshotResult = Promise.resolve(snapshot([sample(1)]))) {
  let receive;
  let reads = 0;
  let cleanups = 0;
  const views = [];
  const errors = [];
  const transport = {
    listen(handler) { receive = handler; return Promise.resolve(() => { cleanups += 1; }); },
    snapshot() { reads += 1; return snapshotResult; },
  };
  const subscription = subscribeToMetrics(transport, view => views.push(view), error => errors.push(error), () => 0);
  return { subscription, views, errors, emit: value => receive(value), reads: () => reads, cleanups: () => cleanups };
}

test("subscription registers before reading the initial snapshot", async () => {
  const order = [];
  const subscription = subscribeToMetrics({
    async listen() { order.push("listen"); return () => {}; },
    async snapshot() { order.push("snapshot"); return snapshot([]); },
  }, () => {}, assert.fail);
  await setImmediate();
  assert.deepEqual(order, ["listen", "snapshot"]);
  subscription.dispose();
});

test("an older initial snapshot cannot overwrite a newer event and still backfills history", async () => {
  const pending = deferred();
  const h = harness(pending.promise);
  await setImmediate();
  h.emit(sample(5));
  pending.resolve(snapshot([sample(1), sample(2), sample(3)]));
  await setImmediate();
  const result = h.views.at(-1).snapshot;
  assert.equal(result.latest.sequence, 5);
  assert.equal(result.lastSuccessful.sequence, 5);
  assert.deepEqual(result.history.map(value => value.sequence), [1, 2, 3, 5]);
  h.subscription.dispose();
});

test("duplicate and out-of-order events cannot regress readings or duplicate history", async () => {
  const h = harness(Promise.resolve(snapshot([sample(8), sample(9)])));
  await setImmediate();
  h.emit(sample(9));
  h.emit(sample(8));
  h.emit(sample(10));
  assert.equal(h.views.at(-1).snapshot.latest.sequence, 10);
  assert.deepEqual(h.views.at(-1).snapshot.history.map(value => value.sequence), [8, 9, 10]);
  h.subscription.dispose();
});

test("cleanup during pending listener registration unlistens exactly once", async () => {
  const registration = deferred();
  let cleanups = 0;
  let reads = 0;
  const subscription = subscribeToMetrics({
    listen: () => registration.promise,
    async snapshot() { reads += 1; return snapshot([]); },
  }, assert.fail, assert.fail);
  subscription.dispose();
  registration.resolve(() => { cleanups += 1; });
  await setImmediate();
  subscription.dispose();
  assert.equal(cleanups, 1);
  assert.equal(reads, 0);
});

test("StrictMode-style cleanup and remount leave only the current subscription active", async () => {
  const first = harness();
  first.subscription.dispose();
  const second = harness();
  await setImmediate();
  first.emit(sample(20));
  second.emit(sample(2));
  assert.equal(first.views.length, 0);
  assert.equal(first.cleanups(), 1);
  assert.equal(second.views.at(-1).snapshot.latest.sequence, 2);
  second.subscription.dispose();
  assert.equal(second.cleanups(), 1);
});

test("cleanup ignores in-flight snapshot responses and queued events", async () => {
  const pending = deferred();
  const h = harness(pending.promise);
  await setImmediate();
  h.subscription.dispose();
  pending.resolve(snapshot([sample(1)]));
  h.emit(sample(2));
  await setImmediate();
  assert.equal(h.views.length, 0);
  assert.equal(h.cleanups(), 1);
});

test("hidden widgets publish no updates and restore timestamped history on show", async () => {
  let receive;
  let reads = 0;
  const views = [];
  const subscription = subscribeToMetrics({
    async listen(handler) { receive = handler; return () => {}; },
    async snapshot() { reads += 1; return reads === 1 ? snapshot([sample(1)]) : snapshot([sample(2), sample(3), sample(4)]); },
  }, view => views.push(view), assert.fail, () => 0);
  await setImmediate();
  subscription.setVisible(false);
  receive(sample(2));
  receive(sample(3));
  assert.equal(views.length, 1);
  assert.equal(reads, 1);
  subscription.setVisible(true);
  await setImmediate();
  assert.equal(reads, 2);
  assert.equal(views.at(-1).snapshot.latest.sequence, 4);
  assert.deepEqual(views.at(-1).snapshot.history.map(value => value.sequence), [1, 2, 3, 4]);
  subscription.dispose();
});

test("starting hidden defers the snapshot request until visible", async () => {
  let reads = 0;
  const subscription = subscribeToMetrics({
    async listen() { return () => {}; },
    async snapshot() { reads += 1; return snapshot([]); },
  }, () => {}, assert.fail, () => 0, false);
  await setImmediate();
  assert.equal(reads, 0);
  subscription.setVisible(true);
  await setImmediate();
  assert.equal(reads, 1);
  subscription.dispose();
});

test("snapshot failures are reported while later events recover the stream", async () => {
  const pending = deferred();
  const h = harness(pending.promise);
  await setImmediate();
  pending.reject(new Error("Injected snapshot failure"));
  await setImmediate();
  assert.equal(h.errors.length, 1);
  h.emit(sample(2));
  assert.equal(h.views.at(-1).snapshot.latest.sequence, 2);
  h.subscription.dispose();
});

test("an obsolete snapshot failure does not overwrite a newer successful event", async () => {
  const pending = deferred();
  const h = harness(pending.promise);
  await setImmediate();
  h.emit(sample(2));
  pending.reject(new Error("Old request failed"));
  await setImmediate();
  assert.equal(h.errors.length, 0);
  h.subscription.dispose();
});

test("multiple independent subscribers only retrieve cached snapshots", async () => {
  const first = harness();
  const second = harness();
  await setImmediate();
  first.emit(sample(2));
  second.emit(sample(2));
  assert.equal(first.reads(), 1);
  assert.equal(second.reads(), 1);
  assert.equal(first.views.at(-1).snapshot.latest.sequence, second.views.at(-1).snapshot.latest.sequence);
  first.subscription.dispose();
  second.subscription.dispose();
});

test("history retention uses elapsed time with a fixed memory bound", () => {
  const fast = Array.from({ length: 150 }, (_, index) => sample(index + 1));
  const merged = mergeSnapshot(emptySnapshot(), snapshot(fast));
  assert.equal(merged.history.length, 61);
  assert.equal(merged.history[0].monotonicMs, 90_000);
  const slow = Array.from({ length: 12 }, (_, index) => sample(index + 1, index * 10_000, 10_000));
  assert.equal(mergeSnapshot(emptySnapshot(), snapshot(slow, 110_000, 10)).history.length, 7);
  assert.equal(mergeSnapshot(merged, snapshot([], 500_000)).history.length, 0);
});

test("failure samples preserve last good card values and create graph gaps", () => {
  const good = sample(1);
  const failed = { ...sample(2), metrics: null, health: { ...sample(2).health, status: "error", error: "Unavailable" } };
  const merged = mergeSnapshot(sampleSnapshot(good), sampleSnapshot(failed));
  assert.equal(merged.latest.sequence, 2);
  assert.equal(merged.lastSuccessful.sequence, 1);
  assert.deepEqual(graphReadings(merged.history, "cpu").map(reading => reading.value), [1, null]);
});

test("a stale snapshot cannot regress a newer interval configuration", () => {
  const current = snapshot([sample(1)], 2000, 10, 1);
  const old = snapshot([sample(1)], 1000, 1);
  assert.equal(mergeSnapshot(current, old).monitoringIntervalSeconds, 10);
});

test("a sample collected under an older configuration cannot undo a saved interval", () => {
  const configured = snapshot([sample(1)], 2000, 10, 2);
  const inFlight = sample(2, 2001, 1000);
  const merged = mergeSnapshot(configured, sampleSnapshot(inFlight));
  assert.equal(merged.latest.sequence, 2);
  assert.equal(merged.configRevision, 2);
  assert.equal(merged.monitoringIntervalSeconds, 10);
  const newer = snapshot([inFlight], 2001, 5, 3);
  assert.equal(mergeSnapshot(merged, newer).monitoringIntervalSeconds, 5);
});

test("graph x positions reflect actual 1, 5, and 10 second spacing", () => {
  const readings = [sample(1, 45_000), sample(2, 50_000, 5000), sample(3, 60_000, 10_000)];
  const path = graphGeometry(graphReadings(readings, "memory"), 60_000, 300, 60).path;
  assert.equal(path, "M225.00,30.00 L250.00,30.00 L300.00,30.00");
});

test("graphs omit old data and break across missing readings or excessive gaps", () => {
  const readings = [
    { value: 50, monotonicMs: 0, intervalMs: 1000 },
    { value: 50, monotonicMs: 10_000, intervalMs: 1000 },
    { value: null, monotonicMs: 11_000, intervalMs: 1000 },
    { value: 50, monotonicMs: 12_000, intervalMs: 1000 },
    { value: 50, monotonicMs: 50_000, intervalMs: 1000 },
    { value: 50, monotonicMs: 51_000, intervalMs: 1000 },
  ];
  assert.equal(graphGeometry(readings, 65_000, 300, 60).path, "M25.00,30.00 M35.00,30.00 M225.00,30.00 L230.00,30.00");
  assert.equal(graphGeometry(readings, 120_000, 300, 60).path, "");
});

test("CPU warmup yields no graph value and zero memory totals avoid division errors", () => {
  const warmup = { ...sample(1), metrics: { ...sample(1).metrics, cpuUsage: null, memoryTotal: 0 } };
  assert.equal(graphReadings([warmup], "cpu")[0].value, null);
  assert.equal(graphReadings([warmup], "memory")[0].value, null);
});

test("graph movement uses a monotonic anchor rather than wall clock time", () => {
  assert.equal(monitorNow({ monotonicMs: 10_000, receivedAtMs: 500 }, 1500), 11_000);
  assert.equal(monitorNow({ monotonicMs: 10_000, receivedAtMs: 500 }, 400), 10_000);
});
