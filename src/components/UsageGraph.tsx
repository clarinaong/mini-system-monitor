import { useEffect, useLayoutEffect, useRef } from "react";

type UsageGraphProps = {
  data: number[];
};

type Reading = {
  value: number;
  time: number;
};

const WIDTH = 300;
const HEIGHT = 100;
const MAX_READINGS = 60;
const SAMPLE_INTERVAL = 1000;
const HISTORY_DURATION = MAX_READINGS * SAMPLE_INTERVAL;
const FRAME_INTERVAL = 100; // 10 FPS

function UsageGraph({ data }: UsageGraphProps) {
  const lineRef = useRef<SVGPolylineElement>(null);
  const readingsRef = useRef<Reading[]>([]);
  const lastDataRef = useRef<number[] | null>(null);

  // Convert a percentage into an SVG Y coordinate.
  function getY(value: number): number {
    const percentage = Math.max(0, Math.min(100, value));

    return HEIGHT - (percentage / 100) * HEIGHT;
  }

  // Draw the graph based on elapsed time.
  function draw(now: number) {
    const line = lineRef.current;
    if (!line) return;

    const points = readingsRef.current
      .map((reading) => {
        const age = now - reading.time;

        // Older readings move toward the left.
        const x = WIDTH - (age / HISTORY_DURATION) * WIDTH;

        const y = getY(reading.value);

        return `${x},${y}`;
      })
      .join(" ");

    line.setAttribute("points", points);
  }

  // Store each new reading with its arrival time.
  useLayoutEffect(() => {
    if (lastDataRef.current === data) return;

    lastDataRef.current = data;

    if (data.length === 0) {
      readingsRef.current = [];
      draw(performance.now());
      return;
    }

    const now = performance.now();

    if (readingsRef.current.length === 0) {
      // Initialize existing history if the component mounts
      // after readings have already been collected.
      readingsRef.current = data.map((value, index) => ({
        value,
        time: now - (data.length - 1 - index) * SAMPLE_INTERVAL,
      }));
    } else {
      readingsRef.current.push({
        value: data[data.length - 1],
        time: now,
      });
    }

    // Never store more than 60 readings.
    readingsRef.current = readingsRef.current.slice(-MAX_READINGS);

    draw(now);
  }, [data]);

  // Continuously move the graph between readings.
  useEffect(() => {
    let frameId: number | null = null;
    let timerId: ReturnType<typeof setTimeout> | null = null;

    const motionPreference = window.matchMedia(
      "(prefers-reduced-motion: reduce)",
    );

    function stopAnimation() {
      if (frameId !== null) {
        cancelAnimationFrame(frameId);
        frameId = null;
      }

      if (timerId !== null) {
        clearTimeout(timerId);
        timerId = null;
      }
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

      if (document.hidden || motionPreference.matches) {
        draw(performance.now());
        return;
      }

      frameId = requestAnimationFrame(animate);
    }

    document.addEventListener("visibilitychange", updateAnimation);
    motionPreference.addEventListener("change", updateAnimation);

    updateAnimation();

    return () => {
      stopAnimation();

      document.removeEventListener("visibilitychange", updateAnimation);

      motionPreference.removeEventListener("change", updateAnimation);
    };
  }, []);

  return (
    <svg
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      preserveAspectRatio="none"
      role="img"
      aria-label="Recent system usage history"
    >
      <line x1="0" y1="25" x2={WIDTH} y2="25" className="graph-gridline" />

      <line x1="0" y1="50" x2={WIDTH} y2="50" className="graph-gridline" />

      <line x1="0" y1="75" x2={WIDTH} y2="75" className="graph-gridline" />

      <polyline
        ref={lineRef}
        points=""
        fill="none"
        stroke="#839bb5"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export default UsageGraph;
