import { useEffect, useLayoutEffect, useRef } from "react";
import type { GraphReading, MonitorClock } from "../types/system";
import { graphPath, monitorNow } from "../monitor/history";

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

  function draw(now: number) {
    pathRef.current?.setAttribute("d", graphPath(readingsRef.current, monitorNow(clockRef.current, now), WIDTH, HEIGHT));
  }

  useLayoutEffect(() => {
    readingsRef.current = data;
    clockRef.current = clock;
    draw(performance.now());
  }, [data, clock]);

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
      draw(performance.now());
      timerId = setTimeout(() => {
        timerId = null;
        frameId = requestAnimationFrame(animate);
      }, FRAME_INTERVAL);
    }

    function updateAnimation() {
      stopAnimation();
      draw(performance.now());
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
  }, [active]);

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
