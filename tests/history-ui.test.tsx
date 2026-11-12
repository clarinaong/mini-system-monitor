import { StrictMode } from "react";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import HistoryWindow from "../src/history/HistoryWindow";
import { graphPaths } from "../src/history/HistoryGraph";
import { HOUR_MS, MINUTE_MS, type HistoryRequest, type HistoryResult, type RawSample } from "../src/history/query";
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
const requests = () => mocks.invoke.mock.calls.filter(([command]) => command === "query_history");

beforeEach(() => {
  read = vi.fn(async (request: HistoryRequest) => ({ resolution: request.resolution, rows: [] }));
  mocks.invoke.mockImplementation(async (command, args) => {
    if (command === "query_history") return read(args.request);
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
  expect(mocks.invoke.mock.calls.every(([command]) => ["get_settings", "query_history"].includes(command))).toBe(true);
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
