import { useCallback, useEffect, useLayoutEffect, useRef } from "react";
import type { GraphReading, MonitorClock } from "../types/system";
import { graphGeometry, HISTORY_WINDOW_MS, monitorNow } from "../monitor/history";

type UsageGraphProps = {
  data: GraphReading[];
  clock: MonitorClock;
  active: boolean;
};

const WIDTH = 300;
const HEIGHT = 60;
const FRAME_INTERVAL = 100;

function UsageGraph({ data, clock, active }: UsageGraphProps) {
  const pathRef = useRef<SVGPathElement>(null);
  const readingsRef = useRef(data);
  const clockRef = useRef(clock);
  const geometryRef = useRef<ReturnType<typeof graphGeometry> | null>(null);

  const draw = useCallback(() => {
    const path = pathRef.current;
    if (!path) return;
    const now = monitorNow(clockRef.current, performance.now());
    let geometry = geometryRef.current;
    if (!geometry || now < geometry.anchor || now >= geometry.nextEntry || now > geometry.firstExpiry) {
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
    clockRef.current = clock;
    geometryRef.current = null;
    draw();
  }, [data, clock, draw]);

  useEffect(() => {
    let frameId: number | null = null;
    let timerId: ReturnType<typeof setTimeout> | null = null;
    const motionPreference = window.matchMedia("(prefers-reduced-motion: reduce)");

    function stopAnimation() {
      if (frameId !== null) { cancelAnimationFrame(frameId); frameId = null; }
      if (timerId !== null) { clearTimeout(timerId); timerId = null; }
    }

    function animate() {
      frameId = null;
      draw();
      timerId = setTimeout(() => {
        timerId = null;
        frameId = requestAnimationFrame(animate);
      }, FRAME_INTERVAL);
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
    <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" role="img"
      aria-label="System usage over the last 60 seconds">
      <line x1="0" y1={HEIGHT * 0.25} x2={WIDTH} y2={HEIGHT * 0.25} className="graph-gridline" />
      <line x1="0" y1={HEIGHT * 0.5} x2={WIDTH} y2={HEIGHT * 0.5} className="graph-gridline" />
      <line x1="0" y1={HEIGHT * 0.75} x2={WIDTH} y2={HEIGHT * 0.75} className="graph-gridline" />
      <path ref={pathRef} d="" fill="none" stroke="#839bb5" strokeWidth="2"
        strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

export default UsageGraph;
