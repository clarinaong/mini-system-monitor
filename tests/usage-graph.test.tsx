import { act, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import UsageGraph from "../src/components/UsageGraph";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

test("translates live geometry between readings and rebuilds at both time boundaries", () => {
  vi.useFakeTimers();
  const localNow = vi.spyOn(performance, "now").mockReturnValue(0);
  vi.spyOn(window, "matchMedia").mockReturnValue({
    matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn(),
  } as unknown as MediaQueryList);
  const clock = { monotonicMs: 60_000, receivedAtMs: 0 };
  const data = [
    { monotonicMs: 50, intervalMs: 1_000, value: 0 },
    { monotonicMs: 1_000, intervalMs: 1_000, value: 50 },
    { monotonicMs: 2_000, intervalMs: 1_000, value: 100 },
    { monotonicMs: 60_300, intervalMs: 1_000, value: 50 },
  ];
  const { rerender } = render(<UsageGraph data={data} clock={clock} active />);
  const path = screen.getByRole("img").querySelector("path")!;
  const write = vi.spyOn(path, "setAttribute");
  const tick = (ms: number) => {
    localNow.mockReturnValue(ms);
    act(() => vi.advanceTimersByTime(128));
  };

  expect(path.getAttribute("d")).toBe("M0.25,60.00 L5.00,30.00 L10.00,0.00");
  tick(50);
  expect(path.getAttribute("transform")).toBe("translate(-0.25 0)");
  expect(write).toHaveBeenCalled();
  expect(write.mock.calls.every(([name]) => name === "transform")).toBe(true);

  tick(100);
  expect(path.getAttribute("d")).toBe("M4.50,30.00 L9.50,0.00");
  tick(299);
  expect(path.getAttribute("d")).toBe("M4.50,30.00 L9.50,0.00");
  tick(300);
  expect(path.getAttribute("d")).toBe("M3.50,30.00 L8.50,0.00 M300.00,30.00");

  rerender(<UsageGraph data={[...data, { monotonicMs: 60_300, intervalMs: 1_000, value: 100 }]} clock={clock} active />);
  expect(path.getAttribute("d")).toBe("M3.50,30.00 L8.50,0.00 M300.00,30.00 L300.00,0.00");
  rerender(<UsageGraph data={data} clock={{ monotonicMs: 60_000, receivedAtMs: 300 }} active />);
  expect(path.getAttribute("d")).toBe("M0.25,60.00 L5.00,30.00 L10.00,0.00");
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
  const { rerender } = render(<UsageGraph data={data} clock={clock} active />);
  const path = screen.getByRole("img").querySelector("path")!;
  const write = vi.spyOn(path, "setAttribute");
  const advanceWhilePaused = (ms: number) => {
    write.mockClear();
    localNow.mockReturnValue(ms);
    act(() => vi.advanceTimersByTime(512));
    expect(write).not.toHaveBeenCalled();
  };

  rerender(<UsageGraph data={data} clock={clock} active={false} />);
  advanceWhilePaused(500);
  rerender(<UsageGraph data={data} clock={clock} active />);
  expect(path.getAttribute("transform")).toBe("translate(-2.5 0)");

  hidden.mockReturnValue(true);
  act(() => document.dispatchEvent(new Event("visibilitychange")));
  advanceWhilePaused(1_500);
  hidden.mockReturnValue(false);
  act(() => document.dispatchEvent(new Event("visibilitychange")));
  expect(path.getAttribute("d")).toBe("M2.50,18.00");

  motion.matches = true;
  act(() => motion.dispatchEvent(new Event("change")));
  advanceWhilePaused(2_500);
  motion.matches = false;
  act(() => motion.dispatchEvent(new Event("change")));
  expect(path.getAttribute("d")).toBe("");
});
