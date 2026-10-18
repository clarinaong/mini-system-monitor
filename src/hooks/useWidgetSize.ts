import { useLayoutEffect, useState, type RefObject } from "react";
import { invoke } from "@tauri-apps/api/core";

export function useWidgetSize(element: RefObject<HTMLElement | null>, enabled = true) {
  const [error, setError] = useState<string | null>(null);

  useLayoutEffect(() => {
    if (!enabled || !element.current) return;
    const target = element.current;
    let active = true;
    let resizing = false;
    let pending = 0;
    let applied = 0;

    async function resize() {
      if (resizing) return;
      resizing = true;
      while (active && pending !== applied) {
        applied = pending;
        try {
          await invoke("resize_widget", { height: applied });
          if (active) setError(null);
        } catch {
          if (active) setError("Unable to resize the widget to its content.");
        }
      }
      resizing = false;
    }

    function measure() {
      const height = Math.ceil(target.getBoundingClientRect().height);
      if (height > 0 && height !== pending) { pending = height; void resize(); }
    }

    const observer = new ResizeObserver(measure);
    observer.observe(target);
    measure();
    return () => { active = false; observer.disconnect(); };
  }, [element, enabled]);

  return error;
}
