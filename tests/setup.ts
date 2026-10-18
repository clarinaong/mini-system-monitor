import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

afterEach(() => { cleanup(); });

vi.stubGlobal("ResizeObserver", class {
  observe() {}
  disconnect() {}
  unobserve() {}
});

Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: vi.fn(() => ({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() })),
});
