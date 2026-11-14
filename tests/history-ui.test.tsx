import { env } from "node:process";
import { StrictMode } from "react";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import HistoryWindow from "../src/history/HistoryWindow";
import { graphPaths, HistoryGraph } from "../src/history/HistoryGraph";
import { HOUR_MS, MINUTE_MS, type HistoryRequest, type HistoryResult, type RawSample } from "../src/history/query";
import { historySelection, type PerformanceSummary, type SummaryRequest } from "../src/history/summary";
import type { SavedIncident } from "../src/history/incidents";
import type { Settings } from "../src/types/settings";
import { settingsSnapshot } from "./fixtures/settings";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));

function sample(timestampMs: number, fields: Partial<RawSample> = {}): RawSample {
  return {
    sessionId: 1, sequence: timestampMs + 1, timestampMs, monotonicMs: timestampMs, intervalMs: 1_000,
    cpuPercent: 50, memoryUsedBytes: 30, memoryTotalBytes: 100,
    batteryPercent: null, batteryState: null, sampleStatus: "healthy", batteryStatus: "absent",
    ...fields,
  };
}

function bucket(startMs: number) {
  return {
    startMs, partial: false, cpuPercent: { average: 40, validCount: 2 },
    memoryUsedBytes: { average: 20, validCount: 2 }, memoryTotalBytes: { average: 100, validCount: 2 },
    batteryPercent: { average: null, validCount: 0 },
  };
}

let read: ReturnType<typeof vi.fn>;
let readIncidents: ReturnType<typeof vi.fn>;
let readSummary: ReturnType<typeof vi.fn>;
const requests = () => mocks.invoke.mock.calls.filter(([command]) => command === "query_history");

beforeEach(() => {
  read = vi.fn(async (request: HistoryRequest) => ({ resolution: request.resolution, rows: [] }));
  readIncidents = vi.fn(async () => []);
  readSummary = vi.fn(async (request: SummaryRequest) => emptySummary(request));
  mocks.invoke.mockImplementation(async (command, args) => {
    if (command === "query_history") return read(args.request);
    if (command === "query_incidents") return readIncidents(args.request);
    if (command === "query_performance_summary") return readSummary(args.request);
    if (command === "get_settings") return settingsSnapshot();
    throw new Error(`Unexpected command ${command}`);
  });
  mocks.listen.mockResolvedValue(() => {});
});

test("shows the empty range and switches from raw to bounded minute averages", async () => {
  render(<HistoryWindow />);
  expect(screen.getByText("Loading history…")).toBeVisible();
  await screen.findByText("No saved samples in this range.");
  expect(await screen.findByText("Recording is off")).toBeVisible();

  const raw = read.mock.calls[0][0] as HistoryRequest;
  expect(raw.resolution).toBe("raw");
  expect(raw.endMs - raw.startMs).toBe(15 * MINUTE_MS);
  expect(screen.getByText("Individual recorded samples")).toBeVisible();

  read.mockImplementationOnce(async (request: HistoryRequest) => ({
    resolution: "minuteAverages",
    rows: [bucket(Math.floor(request.startMs / MINUTE_MS) * MINUTE_MS)],
  }));
  fireEvent.change(screen.getByLabelText("Time range"), { target: { value: 7 * 24 * HOUR_MS } });

  const ram = await screen.findByRole("region", { name: "RAM usage" });
  expect(within(ram).getByRole("img", { name: /^RAM usage from/ })).toBeVisible();
  expect(screen.getByRole("region", { name: "CPU usage" })).toBeVisible();
  expect(screen.getByText(/^One-minute averages/)).toBeVisible();
  const averaged = read.mock.calls[1][0] as HistoryRequest;
  expect(averaged.resolution).toBe("minuteAverages");
  expect(averaged.endMs - averaged.startMs).toBe(7 * 24 * HOUR_MS);
  expect(mocks.invoke.mock.calls.every(([command]) => ["get_settings", "query_history", "query_incidents", "query_performance_summary"].includes(command))).toBe(true);
});

test("offers retry on busy and prompts for a shorter range when capped", async () => {
  read.mockRejectedValueOnce({ code: "busy" }).mockRejectedValueOnce({ code: "tooManySourceRows" });
  render(<HistoryWindow />);
  expect(await screen.findByRole("alert")).toHaveTextContent("still running");
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Choose a shorter range");
  expect(read.mock.calls[1][0]).toEqual(read.mock.calls[0][0]);
  fireEvent.change(screen.getByLabelText("Time range"), { target: { value: 5 * MINUTE_MS } });
  expect(await screen.findByText("No saved samples in this range.")).toBeVisible();
});

test("waits for one outstanding request, skips intermediate ranges, and ignores an old error", async () => {
  let rejectOld!: (error: unknown) => void;
  read.mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectOld = reject; }));
  render(<StrictMode><HistoryWindow /></StrictMode>);
  const select = screen.getByLabelText("Time range");
  fireEvent.change(select, { target: { value: HOUR_MS } });
  fireEvent.change(select, { target: { value: 24 * HOUR_MS } });
  expect(requests()).toHaveLength(1);
  await act(async () => rejectOld({ code: "tooManyResults" }));
  expect(await screen.findByText("No saved samples in this range.")).toBeVisible();
  expect(screen.queryByRole("alert")).toBeNull();
  expect(requests()).toHaveLength(2);
  expect(read.mock.calls[1][0]).toMatchObject({ resolution: "minuteAverages" });
});

test("closing discards pending range changes", async () => {
  let finish!: (result: HistoryResult) => void;
  read.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const view = render(<HistoryWindow />);
  fireEvent.change(screen.getByLabelText("Time range"), { target: { value: HOUR_MS } });
  view.unmount();
  await act(async () => finish({ resolution: "raw", rows: [] }));
  expect(requests()).toHaveLength(1);
});

test("raw graphs use wall time and break across nulls, sessions, and missing readings", () => {
  const request = { startMs: 0, endMs: 60_000, resolution: "raw" } as const;
  const rows = [
    sample(0), sample(5_000), sample(10_000, { cpuPercent: null }),
    sample(15_000), sample(20_000, { sessionId: 2 }),
    sample(25_000, { sessionId: 2 }), sample(50_000, { sessionId: 2 }),
  ];
  expect(graphPaths({ resolution: "raw", rows }, request, "cpu")).toEqual({
    line: "M3.00 52.00 L52.50 52.00 M151.50 52.00 M201.00 52.00 L250.50 52.00 M498.00 52.00",
    dots: "M151.50 52.00h0 M498.00 52.00h0",
  });
  expect(graphPaths({ resolution: "raw", rows }, request, "ram").line).toContain("L102.00 71.60");
});

test("minute averages preserve missing buckets and clip edge points to the request", () => {
  const request = { startMs: 30_000, endMs: 210_000, resolution: "minuteAverages" } as const;
  const result = { resolution: "minuteAverages", rows: [bucket(0), bucket(60_000), bucket(180_000)] } as const;
  const paths = graphPaths({ ...result, rows: [...result.rows] }, request, "ram");
  expect(paths.line).toBe("M52.50 81.40 L201.00 81.40 M547.50 81.40");
  expect(paths.dots).toBe("M547.50 81.40h0");
});

test("reuses chart geometry across parent and marker updates, and redraws for new ranges and results", () => {
  const range = { startMs: 0, endMs: MINUTE_MS, resolution: "raw" } as const;
  const rows = vi.fn(() => [sample(0), sample(5_000)]);
  const result = { resolution: "raw", get rows() { return rows(); } } as const;
  const chart = (incidentStart?: number) => <HistoryGraph result={result} range={range} metric="cpu" incidentStart={incidentStart} />;
  const { container, rerender } = render(<div data-status="loading">{chart()}</div>);
  const line = container.querySelector(".history-line")!;
  const originalPath = line.getAttribute("d");

  rerender(<div data-status="settled">{chart()}</div>);
  expect(rows).toHaveBeenCalledTimes(1);
  rerender(<div>{chart(30_000)}</div>);
  expect(rows).toHaveBeenCalledTimes(1);
  expect(container.querySelector(".history-incident-line")).toHaveAttribute("x1", "300");
  expect(line).toHaveAttribute("d", originalPath);

  const wider = { ...range, endMs: 2 * MINUTE_MS };
  rerender(<div><HistoryGraph result={result} range={wider} metric="cpu" incidentStart={30_000} /></div>);
  expect(rows).toHaveBeenCalledTimes(2);
  expect(container.querySelector(".history-incident-line")).toHaveAttribute("x1", "151.5");
  expect(line.getAttribute("d")).not.toBe(originalPath);

  rerender(<div><HistoryGraph result={{ resolution: "raw", rows: [sample(0, { cpuPercent: 100 })] }}
    range={wider} metric="cpu" /></div>);
  expect(container.querySelector(".history-dot")).toHaveAttribute("d", "M3.00 3.00h0");
});

test("filters incident types and statuses, then aligns both charts with the selected incident", async () => {
  const startMs = Date.now() - 8 * MINUTE_MS;
  const interrupted: SavedIncident = {
    sessionId: 1, kind: "cpuSpike", startSequence: 1, startMs, detectedMs: startMs,
    status: "interrupted", endMs: null, durationMs: null,
  };
  readIncidents.mockResolvedValue([
    { ...interrupted, kind: "sustainedMemory", status: "open", startSequence: 3 },
    { ...interrupted, status: "recovered", startSequence: 2, endMs: startMs + 5000, durationMs: 5000 },
    interrupted,
  ]);
  read.mockImplementation(async (request: HistoryRequest) => ({
    resolution: "raw", rows: [sample(startMs - 5_000), sample(startMs), sample(startMs + 5_000)]
      .filter(row => row.timestampMs >= request.startMs && row.timestampMs < request.endMs),
  }));

  const { container } = render(<HistoryWindow />);
  const timeline = screen.getByRole("region", { name: "Incident timeline" });
  expect(await within(timeline).findByRole("button", { name: /Sustained RAM usage, Open/ })).toBeVisible();
  fireEvent.change(within(timeline).getByLabelText("Type"), { target: { value: "cpuSpike" } });
  expect(within(timeline).getAllByRole("button")).toHaveLength(2);
  fireEvent.change(within(timeline).getByLabelText("Status"), { target: { value: "interrupted" } });
  expect(within(timeline).getAllByRole("button")).toHaveLength(1);
  const selected = within(timeline).getByRole("button", { name: /CPU spike, Interrupted/ });
  fireEvent.click(selected);
  expect(selected).toHaveAttribute("aria-pressed", "true");
  const detail = screen.getByRole("region", { name: "Selected incident" });
  expect(within(detail).getByText("Unknown")).toBeVisible();
  expect(within(detail).queryByText("Duration")).toBeNull();
  await screen.findByRole("img", { name: /^CPU usage from/ });
  expect(container.querySelectorAll(".history-incident-line")).toHaveLength(2);
  expect(read.mock.calls[1][0]).toMatchObject({ startMs: startMs - 5 * MINUTE_MS, resolution: "raw" });
  expect(readIncidents).toHaveBeenCalledTimes(1);

  fireEvent.click(screen.getByRole("button", { name: "Back to selected time range" }));
  expect(screen.queryByRole("region", { name: "Selected incident" })).toBeNull();
  await screen.findByRole("img", { name: /^CPU usage from/ });
  expect(container.querySelectorAll(".history-incident-line")).toHaveLength(0);
});

test("retries a capped incident read independently of the graphs", async () => {
  readIncidents.mockRejectedValueOnce({ code: "tooManyResults" });
  render(<HistoryWindow />);
  const timeline = screen.getByRole("region", { name: "Incident timeline" });
  expect(await within(timeline).findByRole("alert")).toHaveTextContent("Choose a shorter range");
  expect(screen.getByText("No saved samples in this range.")).toBeVisible();

  fireEvent.click(within(timeline).getByRole("button", { name: "Retry incidents" }));
  expect(await within(timeline).findByText("No incidents started in this range.")).toBeVisible();
  expect(read).toHaveBeenCalledTimes(1);
  expect(readIncidents.mock.calls[1][0]).toEqual(readIncidents.mock.calls[0][0]);
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(finish => { resolve = finish; });
  return { promise, resolve };
}

function incident(kind: SavedIncident["kind"], startMs: number): SavedIncident {
  return {
    sessionId: 1, kind, startSequence: startMs, startMs, detectedMs: startMs,
    status: "open", endMs: null, durationMs: null,
  };
}

test("a stale incident success cannot reappear after two rapid range changes", async () => {
  const first = deferred<SavedIncident[]>();
  const last = deferred<SavedIncident[]>();
  const startMs = Date.now() - MINUTE_MS;
  readIncidents.mockReturnValueOnce(first.promise).mockReturnValueOnce(last.promise);

  render(<StrictMode><HistoryWindow /></StrictMode>);
  await waitFor(() => expect(readIncidents).toHaveBeenCalledTimes(1));
  const readsBeforeChanges = read.mock.calls.length;
  const range = screen.getByLabelText("Time range");
  fireEvent.change(range, { target: { value: HOUR_MS } });
  fireEvent.change(range, { target: { value: 24 * HOUR_MS } });
  expect(read).toHaveBeenCalledTimes(readsBeforeChanges);

  await act(async () => first.resolve([incident("cpuSpike", startMs)]));
  expect(read).toHaveBeenCalledTimes(readsBeforeChanges + 1);
  expect(readIncidents).toHaveBeenCalledTimes(2);
  expect(screen.getByText("Loading incidents…")).toBeVisible();
  expect(screen.queryByRole("button", { name: /CPU spike/ })).toBeNull();

  await act(async () => last.resolve([incident("memoryJump", startMs)]));
  expect(await screen.findByRole("button", { name: /RAM usage jump/ })).toBeVisible();
  const { startMs: latestStart, endMs: latestEnd } = readIncidents.mock.calls[1][0];
  expect(latestEnd - latestStart).toBe(24 * HOUR_MS);
  expect(read.mock.lastCall?.[0]).toEqual({
    startMs: latestStart, endMs: latestEnd, resolution: "minuteAverages",
  });
});

test("switching incidents discards an obsolete chart response and renders gaps in the current charts", async () => {
  const now = Date.now();
  const first = incident("cpuSpike", now - 10 * MINUTE_MS);
  const second = incident("memoryJump", now - 7 * MINUTE_MS);
  const oldCharts = deferred<HistoryResult>();
  const currentCharts = deferred<HistoryResult>();
  readIncidents.mockResolvedValue([second, first]);
  read.mockImplementationOnce(async () => ({ resolution: "raw", rows: [] }))
    .mockReturnValueOnce(oldCharts.promise).mockReturnValueOnce(currentCharts.promise);

  const { container } = render(<HistoryWindow />);
  await screen.findByText("No valid CPU or RAM readings in the selected range.");
  fireEvent.click(await screen.findByRole("button", { name: /CPU spike, Open/ }));
  fireEvent.click(screen.getByRole("button", { name: /RAM usage jump, Open/ }));
  expect(read).toHaveBeenCalledTimes(2);
  await act(async () => oldCharts.resolve({ resolution: "raw", rows: [sample(first.startMs)] }));
  expect(screen.queryByRole("img", { name: /^CPU usage from/ })).toBeNull();
  expect(screen.getByText("Loading history…")).toBeVisible();

  const rows = [
    sample(second.startMs - 10_000), sample(second.startMs - 5_000),
    sample(second.startMs, { cpuPercent: null, memoryUsedBytes: null, memoryTotalBytes: null }),
    sample(second.startMs + 5_000, { sessionId: 2 }), sample(second.startMs + 10_000, { sessionId: 2 }),
  ];
  await act(async () => currentCharts.resolve({ resolution: "raw", rows }));
  expect(screen.getByRole("region", { name: "Selected incident" })).toHaveTextContent("RAM usage jump");
  const paths = Array.from(container.querySelectorAll(".history-line"), path => path.getAttribute("d"));
  expect(paths).toHaveLength(2);
  for (const path of paths) expect(path?.match(/M/g)).toHaveLength(2);
  const markers = Array.from(container.querySelectorAll(".history-incident-line"), line => line.getAttribute("x1"));
  expect(markers).toHaveLength(2);
  expect(markers[0]).toEqual(markers[1]);
  expect(read.mock.calls[2][0].startMs).toBe(second.startMs - 5 * MINUTE_MS);

  fireEvent.change(screen.getByLabelText("Type"), { target: { value: "sustainedCpu" } });
  expect(screen.queryByRole("region", { name: "Selected incident" })).toBeNull();
  expect(screen.getByText("No incidents match these filters.")).toBeVisible();
  await screen.findByText("No saved samples in this range.");
});

test("reopening History recovers from a read left busy by the previous window", async () => {
  const closedWindowRead = deferred<HistoryResult>();
  const callbacks = new Set<(event: { payload: Settings }) => void>();
  mocks.listen.mockImplementation(async (_event, callback) => {
    callbacks.add(callback);
    return () => callbacks.delete(callback);
  });
  read.mockReturnValueOnce(closedWindowRead.promise).mockRejectedValueOnce({ code: "busy" });
  readIncidents.mockRejectedValueOnce({ code: "busy" });

  const first = render(<HistoryWindow />);
  await screen.findByText("Recording is off");
  expect(callbacks.size).toBe(1);
  first.unmount();
  expect(callbacks.size).toBe(0);

  const reopened = render(<HistoryWindow />);
  await waitFor(() => expect(screen.getAllByRole("alert")).toHaveLength(2));
  expect(readIncidents).toHaveBeenCalledTimes(1);
  await act(async () => closedWindowRead.resolve({ resolution: "raw", rows: [sample(Date.now())] }));
  expect(screen.getAllByRole("alert")).toHaveLength(2);

  fireEvent.click(screen.getByRole("button", { name: "Retry", exact: true }));
  fireEvent.click(screen.getByRole("button", { name: "Retry incidents" }));
  await screen.findByText("No saved samples in this range.");
  await screen.findByText("No incidents started in this range.");
  expect(await screen.findByText("Recording is off")).toBeVisible();
  act(() => {
    const saved = settingsSnapshot().settings;
    saved.history.recordingEnabled = true;
    for (const callback of callbacks) callback({ payload: saved });
  });
  expect(screen.getByText("Recording is on")).toBeVisible();
  expect(mocks.invoke.mock.calls.some(([command]) => command === "save_settings")).toBe(false);
  reopened.unmount();
  expect(callbacks.size).toBe(0);

  const third = render(<HistoryWindow />);
  await screen.findByText("No incidents started in this range.");
  expect(callbacks.size).toBe(1);
  third.unmount();
  expect(callbacks.size).toBe(0);
});

function emptySummary(request: SummaryRequest): PerformanceSummary {
  const empty = { averagePercent: null, peakPercent: null, coveragePercent: 0, coveredMinutes: 0 };
  const periodSummary = (period: SummaryRequest["current"]) => ({
    period,
    totalMinutes: Math.floor((period.endMs - 1) / MINUTE_MS) - Math.floor(period.startMs / MINUTE_MS) + 1,
    cpu: empty,
    ram: empty,
  });
  return {
    current: periodSummary(request.current),
    previous: periodSummary(request.previous),
    cpuComparison: "insufficientCoverage",
    ramComparison: "insufficientCoverage",
  };
}

test("shows observed stats for sparse data, missing prior readings, and eligible percentage point changes", async () => {
  readSummary.mockImplementation(async (request: SummaryRequest) => {
    const result = emptySummary(request);
    const covered = (period: typeof result.current) => Math.ceil(period.totalMinutes * 0.75);
    return {
      ...result,
      current: {
        ...result.current,
        cpu: { averagePercent: 45, peakPercent: 60, coveragePercent: covered(result.current) / result.current.totalMinutes * 100, coveredMinutes: covered(result.current) },
        ram: { averagePercent: 0, peakPercent: 0, coveragePercent: 100 / result.current.totalMinutes, coveredMinutes: 1 },
      },
      previous: {
        ...result.previous,
        cpu: { averagePercent: 35, peakPercent: 70, coveragePercent: covered(result.previous) / result.previous.totalMinutes * 100, coveredMinutes: covered(result.previous) },
      },
      cpuComparison: "eligible",
    };
  });
  render(<HistoryWindow />);
  const summary = screen.getByRole("region", { name: "Performance summary" });
  expect(within(summary).getByText("Loading performance summary…")).toBeVisible();
  expect(await within(summary).findByText(/Average \+10 pp · Peak -10 pp/)).toBeVisible();
  const cpu = within(summary).getByRole("table", { name: "CPU summary" });
  expect(within(cpu).getByRole("row", { name: /^Selected 45% 60% .*\(12\/(15|16) min\)$/ })).toBeVisible();
  const ram = within(summary).getByRole("table", { name: "RAM summary" });
  expect(within(ram).getByRole("row", { name: /Selected 0% 0% .*\(1\/(15|16) min\)/ })).toBeVisible();
  expect(within(ram).getByRole("row", { name: /^Previous Unavailable Unavailable 0% \(0\/(15|16) min\)$/ })).toBeVisible();
  expect(within(summary).getByText("Comparison requires at least 75% coverage in each period.")).toBeVisible();
});

test("retries the same summary period independently and leaves it selected while focusing an incident", async () => {
  readSummary.mockRejectedValueOnce({ code: "busy" });
  readIncidents.mockResolvedValue([incident("cpuSpike", Date.now() - MINUTE_MS)]);
  render(<HistoryWindow />);
  const summary = screen.getByRole("region", { name: "Performance summary" });
  expect(await within(summary).findByRole("alert")).toHaveTextContent("still running");
  fireEvent.click(within(summary).getByRole("button", { name: "Retry summary" }));
  await within(summary).findByText("No valid CPU or RAM readings in the selected range.");
  expect(readSummary.mock.calls[1][0]).toEqual(readSummary.mock.calls[0][0]);
  const label = within(summary).getByText("Selected range").nextElementSibling?.textContent;
  fireEvent.click(screen.getByRole("button", { name: /CPU spike, Open/ }));
  await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  expect(within(summary).getByText("Selected range").nextElementSibling).toHaveTextContent(label!);
  expect(readSummary).toHaveBeenCalledTimes(2);
  expect(readIncidents).toHaveBeenCalledTimes(1);
});

test("discards obsolete summary replies and stops queued reads when the window closes", async () => {
  const obsolete = deferred<PerformanceSummary>();
  const closing = deferred<PerformanceSummary>();
  readSummary.mockReturnValueOnce(obsolete.promise).mockReturnValueOnce(closing.promise);
  const view = render(<StrictMode><HistoryWindow /></StrictMode>);
  await waitFor(() => expect(readSummary).toHaveBeenCalledTimes(1));
  const historyCount = read.mock.calls.length;
  const incidentCount = readIncidents.mock.calls.length;
  const range = screen.getByLabelText("Time range");
  fireEvent.change(range, { target: { value: HOUR_MS } });
  fireEvent.change(range, { target: { value: 5 * MINUTE_MS } });
  expect(read).toHaveBeenCalledTimes(historyCount);
  const oldRequest = readSummary.mock.calls[0][0];
  await act(async () => obsolete.resolve(emptySummary(oldRequest)));
  expect(read).toHaveBeenCalledTimes(historyCount + 1);
  expect(readIncidents).toHaveBeenCalledTimes(incidentCount + 1);
  expect(readSummary).toHaveBeenCalledTimes(2);
  expect(screen.getByText("Loading performance summary…")).toBeVisible();
  expect(screen.queryByText("No valid CPU or RAM readings in the selected range.")).toBeNull();
  const request = readSummary.mock.calls[1][0];
  expect(request.current.endMs - request.current.startMs).toBe(5 * MINUTE_MS);
  fireEvent.change(range, { target: { value: HOUR_MS } });
  view.unmount();
  await act(async () => closing.resolve(emptySummary(request)));
  expect(read).toHaveBeenCalledTimes(historyCount + 1);
  expect(readIncidents).toHaveBeenCalledTimes(incidentCount + 1);
  expect(readSummary).toHaveBeenCalledTimes(2);
});

test.each([
  ["ordinary", "2026-02-03T17:00:00Z", "2026-02-01T05:00:00Z", "2026-02-02T05:00:00Z", "2026-02-03T05:00:00Z"],
  ["spring-forward", "2026-03-09T17:00:00Z", "2026-03-07T05:00:00Z", "2026-03-08T05:00:00Z", "2026-03-09T04:00:00Z"],
  ["fall-back", "2026-11-02T17:00:00Z", "2026-10-31T04:00:00Z", "2026-11-01T04:00:00Z", "2026-11-02T05:00:00Z"],
])("uses both actual local midnight boundaries for an %s day", (_label, now, previous, start, end) => {
  const original = env.TZ;
  try {
    env.TZ = "America/Toronto";
    expect(historySelection("yesterday", Date.parse(now)).summary).toEqual({
      current: { startMs: Date.parse(start), endMs: Date.parse(end) },
      previous: { startMs: Date.parse(previous), endMs: Date.parse(start) },
    });
  } finally {
    if (original === undefined) delete env.TZ;
    else env.TZ = original;
  }
});

test("reselects the local day on focus after a timezone change and explains unequal elapsed days", async () => {
  const original = env.TZ;
  vi.spyOn(Date, "now").mockReturnValue(Date.parse("2026-03-09T05:00:00Z"));
  try {
    env.TZ = "America/Toronto";
    readSummary.mockImplementation(async (request: SummaryRequest) => ({
      ...emptySummary(request),
      cpuComparison: "unequalPeriodLengths",
      ramComparison: "unequalPeriodLengths",
    }));
    render(<HistoryWindow />);
    await waitFor(() => expect(readSummary).toHaveBeenCalledTimes(1));
    fireEvent.change(screen.getByLabelText("Time range"), { target: { value: "yesterday" } });
    await waitFor(() => expect(readSummary).toHaveBeenCalledTimes(2));
    const summary = screen.getByRole("region", { name: "Performance summary" });
    await waitFor(() => expect(within(summary).getAllByText("Comparison requires periods of equal elapsed length.")).toHaveLength(2));
    const toronto = readSummary.mock.calls[1][0].current;
    expect(toronto.endMs - toronto.startMs).toBe(23 * HOUR_MS);

    env.TZ = "America/Phoenix";
    fireEvent(window, new Event("focus"));
    await waitFor(() => expect(readSummary).toHaveBeenCalledTimes(3));
    const phoenix = readSummary.mock.calls[2][0].current;
    expect(phoenix).toEqual({
      startMs: Date.parse("2026-03-07T07:00:00Z"),
      endMs: Date.parse("2026-03-08T07:00:00Z"),
    });
    expect(read.mock.lastCall?.[0]).toEqual({ ...phoenix, resolution: "minuteAverages" });
    expect(readIncidents.mock.lastCall?.[0]).toMatchObject(phoenix);
  } finally {
    if (original === undefined) delete env.TZ;
    else env.TZ = original;
    vi.restoreAllMocks();
  }
});
