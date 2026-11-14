import { useCallback, useEffect, useLayoutEffect, useRef } from "react";
import type { GraphReading, MonitorClock } from "../types/system";
import { graphGeometry, HISTORY_WINDOW_MS, monitorNow } from "../monitor/history";

type UsageGraphProps = {
  data: GraphReading[];
  clock: MonitorClock;
  active: boolean;
  delayMs: number;
};

const WIDTH = 300;
const HEIGHT = 60;

function UsageGraph({ data, clock, active, delayMs }: UsageGraphProps) {
  const pathRef = useRef<SVGPathElement>(null);
  const readingsRef = useRef(data);
  const timingRef = useRef({ clock, delayMs });
  const geometryRef = useRef<ReturnType<typeof graphGeometry> | null>(null);

  const draw = useCallback(() => {
    const path = pathRef.current;
    if (!path) return;
    const { clock, delayMs } = timingRef.current;
    const now = monitorNow(clock, performance.now()) - delayMs;
    let geometry = geometryRef.current;
    if (!geometry || now < geometry.anchor || (geometry.path && now > geometry.expiresAfter)) {
      geometry = graphGeometry(readingsRef.current, now, WIDTH, HEIGHT);
      geometryRef.current = geometry;
      path.setAttribute("d", geometry.path);
      path.removeAttribute("transform");
    } else if (geometry.path) {
      const offset = (geometry.anchor - now) * WIDTH / HISTORY_WINDOW_MS;
      path.setAttribute("transform", `translate(${offset} 0)`);
    }
  }, []);

  useLayoutEffect(() => {
    readingsRef.current = data;
    timingRef.current = { clock, delayMs };
    geometryRef.current = null;
    draw();
  }, [data, clock, delayMs, draw]);

  useEffect(() => {
    let frameId: number | null = null;
    const motionPreference = window.matchMedia("(prefers-reduced-motion: reduce)");

    function stopAnimation() {
      if (frameId !== null) { cancelAnimationFrame(frameId); frameId = null; }
    }

    function animate() {
      draw();
      frameId = requestAnimationFrame(animate);
    }

    function updateAnimation() {
      stopAnimation();
      draw();
      if (active && !document.hidden && !motionPreference.matches) {
        frameId = requestAnimationFrame(animate);
      }
    }

    document.addEventListener("visibilitychange", updateAnimation);
    motionPreference.addEventListener("change", updateAnimation);
    updateAnimation();

    return () => {
      stopAnimation();
      document.removeEventListener("visibilitychange", updateAnimation);
      motionPreference.removeEventListener("change", updateAnimation);
    };
  }, [active, draw]);

  return (
    <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" overflow="hidden" role="img"
      aria-label="System usage over a 60-second window">
      <line x1="0" y1={HEIGHT * 0.25} x2={WIDTH} y2={HEIGHT * 0.25} className="graph-gridline" />
      <line x1="0" y1={HEIGHT * 0.5} x2={WIDTH} y2={HEIGHT * 0.5} className="graph-gridline" />
      <line x1="0" y1={HEIGHT * 0.75} x2={WIDTH} y2={HEIGHT * 0.75} className="graph-gridline" />
      <path ref={pathRef} d="" fill="none" strokeWidth="2"
        strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export default UsageGraph;
