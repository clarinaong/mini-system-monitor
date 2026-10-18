import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { useWidgetSize } from "../src/hooks/useWidgetSize";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const OriginalObserver = globalThis.ResizeObserver;
let measure: () => void;
let disconnect: ReturnType<typeof vi.fn>;
let height: number;

beforeEach(() => {
  height = 400;
  disconnect = vi.fn();
  globalThis.ResizeObserver = class {
    constructor(callback: () => void) { measure = callback; }
    observe() {}
    unobserve() {}
    disconnect = disconnect;
  } as unknown as typeof ResizeObserver;
  invoke.mockResolvedValue(undefined);
});

afterEach(() => { globalThis.ResizeObserver = OriginalObserver; });

function elementRef() {
  const element = document.createElement("main");
  element.getBoundingClientRect = () => ({ height }) as DOMRect;
  return { current: element };
}

test("initial loading keeps the original size until preferences and content are ready", async () => {
  const ref = elementRef();
  const hook = renderHook(({ ready }) => useWidgetSize(ref, ready), { initialProps: { ready: false } });
  expect(invoke).not.toHaveBeenCalled();
  hook.rerender({ ready: true });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("resize_widget", { height: 400 }));
});

test("widget resizing serializes requests and finishes at the latest content height", async () => {
  let finish!: () => void;
  invoke.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve; }));
  const hook = renderHook(() => useWidgetSize(elementRef()));
  expect(invoke).toHaveBeenCalledWith("resize_widget", { height: 400 });
  height = 240;
  act(() => { measure(); });
  height = 180;
  act(() => { measure(); });
  expect(invoke).toHaveBeenCalledTimes(1);
  await act(async () => { finish(); });
  await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("resize_widget", { height: 180 }));
  expect(invoke).toHaveBeenCalledTimes(2);
  hook.unmount();
  expect(disconnect).toHaveBeenCalledOnce();
});

test("unmount cancels pending follow-up resizing", async () => {
  let finish!: () => void;
  invoke.mockReturnValueOnce(new Promise<void>(resolve => { finish = resolve; }));
  const hook = renderHook(() => useWidgetSize(elementRef()));
  height = 200;
  act(() => { measure(); });
  hook.unmount();
  await act(async () => { finish(); });
  expect(invoke).toHaveBeenCalledTimes(1);
});

test("resize failures become a visible error and recover after a content change", async () => {
  invoke.mockRejectedValueOnce(new Error("Resize rejected"));
  const ref = elementRef();
  const hook = renderHook(() => useWidgetSize(ref));
  await waitFor(() => expect(hook.result.current).toBe("Unable to resize the widget to its content."));
  height = 300;
  act(() => { measure(); });
  await waitFor(() => expect(hook.result.current).toBeNull());
});
