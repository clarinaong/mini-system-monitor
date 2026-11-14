import { act, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import UsageGraph from "../src/components/UsageGraph";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

test("scrolls a delayed path through both fixed viewport edges without rewriting it each frame", () => {
  vi.useFakeTimers();
  const localNow = vi.spyOn(performance, "now").mockReturnValue(0);
  vi.spyOn(window, "matchMedia").mockReturnValue({
    matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn(),
  } as unknown as MediaQueryList);
  const clock = { monotonicMs: 61_250, receivedAtMs: 0 };
  const data = [
    { monotonicMs: 0, intervalMs: 1_000, value: 0 },
    { monotonicMs: 1_000, intervalMs: 1_000, value: 50 },
    { monotonicMs: 59_000, intervalMs: 1_000, value: 70 },
    { monotonicMs: 60_000, intervalMs: 1_000, value: 50 },
    { monotonicMs: 61_000, intervalMs: 1_000, value: 100 },
  ];
  const { rerender } = render(<UsageGraph data={data} clock={clock} active delayMs={1_250} />);
  const svg = screen.getByRole("img");
  const path = svg.querySelector("path")!;
  const write = vi.spyOn(path, "setAttribute");
  const original = "M0.00,60.00 L5.00,30.00 M295.00,18.00 L300.00,30.00 L305.00,0.00";

  expect(svg).toHaveAttribute("overflow", "hidden");
  expect(path).toHaveAttribute("d", original);
  localNow.mockReturnValue(500);
  act(() => vi.advanceTimersToNextFrame());
  expect(path).toHaveAttribute("transform", "translate(-2.5 0)");
  expect(path).toHaveAttribute("d", original);
  expect(write.mock.calls.map(([name]) => name)).toEqual(["transform"]);

  localNow.mockReturnValue(750);
  rerender(<UsageGraph data={[...data, { monotonicMs: 62_000, intervalMs: 1_000, value: 60 }]}
    clock={{ monotonicMs: 62_000, receivedAtMs: 750 }} active delayMs={1_250} />);
  expect(path).toHaveAttribute("d", "M-3.75,60.00 L1.25,30.00 M291.25,18.00 L296.25,30.00 L301.25,0.00 L306.25,24.00");
});

test("pauses for native hiding, document hiding, and reduced motion, then catches up on resume", () => {
  vi.useFakeTimers();
  const localNow = vi.spyOn(performance, "now").mockReturnValue(0);
  const hidden = vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  const motion = Object.assign(new EventTarget(), { matches: false });
  vi.spyOn(window, "matchMedia").mockReturnValue(motion as unknown as MediaQueryList);
  const clock = { monotonicMs: 60_000, receivedAtMs: 0 };
  const data = [
    { monotonicMs: 1_000, intervalMs: 1_000, value: 50 },
    { monotonicMs: 2_000, intervalMs: 1_000, value: 70 },
  ];
  const { rerender } = render(<UsageGraph data={data} clock={clock} active delayMs={0} />);
  const path = screen.getByRole("img").querySelector("path")!;
  const write = vi.spyOn(path, "setAttribute");
  const advanceWhilePaused = (ms: number) => {
    write.mockClear();
    localNow.mockReturnValue(ms);
    act(() => vi.advanceTimersByTime(512));
    expect(write).not.toHaveBeenCalled();
  };

  rerender(<UsageGraph data={data} clock={clock} active={false} delayMs={0} />);
  advanceWhilePaused(500);
  rerender(<UsageGraph data={data} clock={clock} active delayMs={0} />);
  expect(path.getAttribute("transform")).toBe("translate(-2.5 0)");

  hidden.mockReturnValue(true);
  act(() => document.dispatchEvent(new Event("visibilitychange")));
  advanceWhilePaused(1_500);
  hidden.mockReturnValue(false);
  act(() => document.dispatchEvent(new Event("visibilitychange")));
  expect(path.getAttribute("transform")).toBe("translate(-7.5 0)");

  motion.matches = true;
  act(() => motion.dispatchEvent(new Event("change")));
  advanceWhilePaused(2_500);
  motion.matches = false;
  act(() => motion.dispatchEvent(new Event("change")));
  expect(path.getAttribute("d")).toBe("");
});
