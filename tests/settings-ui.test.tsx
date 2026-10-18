import { StrictMode } from "react";
import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";
import type { Settings, SettingsSnapshot } from "../src/types/settings";
import type { MonitorView } from "../src/monitor/subscription";
import { settingsFixture, settingsSnapshot } from "./fixtures/settings";
import SettingsWindow from "../src/settings/SettingsWindow";
import App from "../src/App";
import { useSettings } from "../src/hooks/useSettings";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), close: vi.fn(), onClose: vi.fn(), metrics: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ close: mocks.close, onCloseRequested: mocks.onClose }) }));
vi.mock("../src/hooks/useSystemMetrics", () => ({ useSystemMetrics: mocks.metrics }));

let callbacks: Set<(event: { payload: Settings }) => void>;
let persisted: SettingsSnapshot;
let closeHandler: (event: { preventDefault: () => void }) => void;

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function loaded() {
  await screen.findByLabelText("Refresh interval");
  await waitFor(() => expect(screen.queryByText("Loading preferences…")).not.toBeInTheDocument());
}

function saves() { return mocks.invoke.mock.calls.filter(call => call[0] === "save_settings"); }

beforeEach(() => {
  callbacks = new Set();
  persisted = settingsSnapshot();
  mocks.invoke.mockImplementation(async (command, args) => {
    if (command === "get_settings") return structuredClone(persisted);
    if (command === "get_notification_status") return { permission: "unavailable", canRequest: false, canSendTest: false };
    if (command === "save_settings") {
      persisted = { settings: structuredClone(args.settings), loadError: null, notificationDeliveryAvailable: false };
      return structuredClone(args.settings);
    }
    return undefined;
  });
  mocks.listen.mockImplementation(async (_name, handler) => {
    callbacks.add(handler);
    return () => callbacks.delete(handler);
  });
  mocks.close.mockResolvedValue(undefined);
  mocks.onClose.mockImplementation(async handler => { closeHandler = handler; return () => {}; });
  const sample = {
    sequence: 1, configRevision: 0, timestampMs: Date.now(), monotonicMs: 1000, intervalMs: 1000,
    metrics: { cpuUsage: 25, memoryUsed: 100, memoryTotal: 200, battery: { percentage: 80, status: "Charging" } },
    health: { status: "healthy" as const, error: null, batteryStatus: "available" as const, batteryError: null },
  };
  const view: MonitorView = {
    clock: { monotonicMs: 1000, receivedAtMs: performance.now() },
    snapshot: { latest: sample, lastSuccessful: sample, history: [sample], configRevision: 0, monitoringIntervalSeconds: 1, monotonicNowMs: 1000 },
  };
  mocks.metrics.mockReturnValue({ view, error: null, visible: true, stale: false });
});

describe("Settings window", () => {
  test("loads preferences and explains packaged notification requirements", async () => {
    render(<SettingsWindow />);
    await loaded();
    expect(screen.getByLabelText("Refresh interval")).toHaveValue("1");
    expect(screen.getByLabelText("Show CPU")).toBeChecked();
    expect(screen.getByLabelText("Show memory graph")).toBeChecked();
    expect(screen.getByLabelText("Layout")).toHaveValue("expanded");
    expect(screen.getByLabelText("Enable alerts")).not.toBeChecked();
    expect(screen.getByLabelText("CPU threshold (%)")).toHaveValue(90);
    expect(screen.getByLabelText("Charging reminder threshold (%)")).toHaveValue(80);
    expect(screen.getByText("Automatic alert delivery requires a packaged macOS release app")).toBeVisible();
    expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
    expect(mocks.invoke).not.toHaveBeenCalledWith("get_system_metrics");
  });

  test("packaged delivery availability still requires saved switches and explicit permission", async () => {
    persisted.notificationDeliveryAvailable = true;
    render(<SettingsWindow />);
    await loaded();
    expect(screen.getByText("Automatic alert delivery is available")).toBeVisible();
    expect(screen.getByLabelText("Enable alerts")).not.toBeChecked();
    expect(screen.getByLabelText("Notification sound")).not.toBeChecked();
    expect(mocks.invoke.mock.calls.map(call => call[0])).not.toContain("request_notification_permission");
    expect(mocks.invoke.mock.calls.map(call => call[0])).not.toContain("send_test_notification");
  });

  test("edits remain a draft until Apply saves and returns authoritative settings", async () => {
    const user = userEvent.setup();
    render(<SettingsWindow />);
    await loaded();
    await user.selectOptions(screen.getByLabelText("Refresh interval"), "5");
    await user.selectOptions(screen.getByLabelText("Layout"), "compact");
    await user.click(screen.getByLabelText("Show CPU"));
    expect(saves()).toHaveLength(0);
    expect(persisted.settings.layout).toBe("expanded");
    await user.click(screen.getByRole("button", { name: "Apply" }));
    await screen.findByText("Preferences saved.");
    expect(saves()).toHaveLength(1);
    expect(saves()[0][1].settings).toMatchObject({ layout: "compact", monitoringIntervalSeconds: 5, visibleCards: { cpu: false } });
    expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
    expect(mocks.close).not.toHaveBeenCalled();
  });

  test("Cancel closes without saving and a new opening starts from persisted preferences", async () => {
    const user = userEvent.setup();
    const first = render(<SettingsWindow />);
    await loaded();
    await user.click(screen.getByLabelText("Show battery"));
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(mocks.close).toHaveBeenCalledOnce();
    expect(saves()).toHaveLength(0);
    first.unmount();
    render(<SettingsWindow />);
    await loaded();
    expect(screen.getByLabelText("Show battery")).toBeChecked();
  });

  test("invalid values are highlighted even when alerts are disabled", async () => {
    render(<SettingsWindow />);
    await loaded();
    fireEvent.change(screen.getByLabelText("CPU threshold (%)"), { target: { value: "101" } });
    fireEvent.change(screen.getByLabelText("Cooldown seconds per rule"), { target: { value: "0" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(screen.getByLabelText("CPU threshold (%)")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText("Cooldown seconds per rule")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("Correct the highlighted values before applying.")).toBeVisible();
    expect(saves()).toHaveLength(0);
  });

  test("failed saves stay visible, preserve the draft, and can be retried", async () => {
    const user = userEvent.setup();
    let fail = true;
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "get_settings") return settingsSnapshot();
      if (command === "save_settings" && fail) throw { code: "saveFailed", message: "Preferences were not saved: disk is full." };
      return args.settings;
    });
    render(<SettingsWindow />);
    await loaded();
    await user.click(screen.getByLabelText("Show battery"));
    await user.click(screen.getByRole("button", { name: "Apply" }));
    await screen.findByText("Preferences were not saved: disk is full.");
    expect(screen.getByLabelText("Show battery")).not.toBeChecked();
    expect(screen.getByRole("button", { name: "Apply" })).toBeEnabled();
    fail = false;
    await user.click(screen.getByRole("button", { name: "Apply" }));
    await screen.findByText("Preferences saved.");
    expect(saves()).toHaveLength(2);
  });

  test("Apply and window closing are blocked while persistence is pending", async () => {
    const pending = deferred<Settings>();
    mocks.invoke.mockImplementation(async (command) => command === "get_settings" ? settingsSnapshot() : pending.promise);
    render(<SettingsWindow />);
    await loaded();
    fireEvent.click(screen.getByLabelText("Show CPU"));
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(screen.getByRole("button", { name: "Saving…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(screen.getByLabelText("Show CPU")).toBeDisabled();
    const preventDefault = vi.fn();
    closeHandler({ preventDefault });
    expect(preventDefault).toHaveBeenCalledOnce();
    await act(async () => { pending.resolve(settingsFixture()); });
    await screen.findByText("Preferences saved.");
  });

  test("unreadable saved data blocks Apply until retry succeeds without losing the draft", async () => {
    const user = userEvent.setup();
    persisted.loadError = { code: "unsupportedVersion", message: "Settings version 2 is unsupported. The file was preserved." };
    render(<SettingsWindow />);
    await loaded();
    await user.click(screen.getByLabelText("Show CPU"));
    expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
    expect(screen.getByText(/Settings version 2 is unsupported/)).toBeVisible();
    persisted.loadError = null;
    await user.click(screen.getByRole("button", { name: "Retry loading" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Apply" })).toBeEnabled());
    expect(screen.getByLabelText("Show CPU")).not.toBeChecked();
    expect(saves()).toHaveLength(0);
  });

  test("external settings changes keep a dirty draft and update pristine controls", async () => {
    render(<SettingsWindow />);
    await loaded();
    const external = settingsFixture();
    external.layout = "compact";
    act(() => { callbacks.forEach(callback => callback({ payload: external })); });
    await waitFor(() => expect(screen.getByLabelText("Layout")).toHaveValue("compact"));
    fireEvent.click(screen.getByLabelText("Show CPU"));
    const nextExternal = structuredClone(external);
    nextExternal.visibleCards.battery = false;
    act(() => { callbacks.forEach(callback => callback({ payload: nextExternal })); });
    expect(screen.getByLabelText("Show CPU")).not.toBeChecked();
    expect(screen.getByText(/Your unsaved edits have been kept/)).toBeVisible();
  });
});

describe("Widget preferences", () => {
  const combinations = Array.from({ length: 8 }, (_, mask) => ["expanded", "compact"].flatMap(layout =>
    Array.from({ length: 4 }, (_, graphs) => ({ mask, layout: layout as Settings["layout"], graphs })),
  )).flat();

  test.each(combinations)("cards $mask, layout $layout, graphs $graphs", async ({ mask, layout, graphs }) => {
    persisted.settings.layout = layout;
    persisted.settings.visibleCards = { cpu: Boolean(mask & 1), memory: Boolean(mask & 2), battery: Boolean(mask & 4) };
    persisted.settings.graphs = { cpu: Boolean(graphs & 1), memory: Boolean(graphs & 2) };
    const { container } = render(<App />);
    await waitFor(() => expect(screen.queryByText("Loading preferences...")).not.toBeInTheDocument());
    expect(Boolean(screen.queryByRole("heading", { name: "CPU" }))).toBe(Boolean(mask & 1));
    expect(Boolean(screen.queryByRole("heading", { name: "MEMORY" }))).toBe(Boolean(mask & 2));
    expect(Boolean(screen.queryByRole("heading", { name: "BATTERY" }))).toBe(Boolean(mask & 4));
    const expectedGraphs = Number(Boolean(mask & 1) && Boolean(graphs & 1)) + Number(Boolean(mask & 2) && Boolean(graphs & 2));
    expect(screen.queryAllByRole("img")).toHaveLength(expectedGraphs);
    expect(container.querySelector("main")).toHaveClass(`layout-${layout}`);
    if (mask === 0) expect(screen.getByText(/All metrics are hidden/)).toBeVisible();
  });

  test("a saved event immediately updates the widget without polling settings", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "CPU" });
    const updated = settingsFixture();
    updated.visibleCards.cpu = false;
    updated.graphs.memory = false;
    updated.layout = "compact";
    act(() => { callbacks.forEach(callback => callback({ payload: updated })); });
    expect(screen.queryByRole("heading", { name: "CPU" })).not.toBeInTheDocument();
    expect(screen.queryAllByRole("img")).toHaveLength(0);
    expect(screen.getByRole("main")).toHaveClass("layout-compact");
    expect(mocks.invoke.mock.calls.filter(call => call[0] === "get_settings")).toHaveLength(1);
  });
});

describe("Settings subscription lifecycle", () => {
  test("an initial snapshot cannot overwrite a newer saved event", async () => {
    const pending = deferred<SettingsSnapshot>();
    mocks.invoke.mockReturnValue(pending.promise);
    const hook = renderHook(() => useSettings());
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("get_settings"));
    const saved = settingsFixture();
    saved.layout = "compact";
    act(() => { callbacks.forEach(callback => callback({ payload: saved })); });
    await act(async () => { pending.resolve(settingsSnapshot()); });
    expect(hook.result.current.snapshot?.settings.layout).toBe("compact");
  });

  test("pending listener registrations are cleaned up across StrictMode remounts", async () => {
    const first = deferred<() => void>();
    const firstCleanup = vi.fn();
    const secondCleanup = vi.fn();
    mocks.listen.mockReturnValueOnce(first.promise).mockResolvedValueOnce(secondCleanup);
    const hook = renderHook(() => useSettings(), { wrapper: StrictMode });
    await waitFor(() => expect(hook.result.current.snapshot).not.toBeNull());
    await act(async () => { first.resolve(firstCleanup); });
    expect(firstCleanup).toHaveBeenCalledOnce();
    hook.unmount();
    expect(secondCleanup).toHaveBeenCalledOnce();
  });

  test("load failures are displayed and can be retried", async () => {
    mocks.invoke.mockRejectedValueOnce(new Error("Connection unavailable"));
    render(<SettingsWindow />);
    await screen.findByText("Connection unavailable");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await loaded();
    expect(screen.queryByText("Connection unavailable")).not.toBeInTheDocument();
  });
});
