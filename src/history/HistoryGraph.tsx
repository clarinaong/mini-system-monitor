import { MINUTE_MS, type HistoryRequest, type HistoryResult, type RawSample } from "./query";

const WIDTH = 600;
const HEIGHT = 104;
const PADDING = 3;

type Metric = "cpu" | "ram";

function memoryPercent(used: number | null, total: number | null) {
  return used !== null && total !== null && total > 0 ? (used / total) * 100 : null;
}

function follows(previous: RawSample, current: RawSample) {
  const maxGap = Math.max(5_000, previous.intervalMs, current.intervalMs) * 1.5 + 250;
  const elapsed = current.monotonicMs - previous.monotonicMs;
  return current.sessionId === previous.sessionId
    && current.sequence > previous.sequence
    && elapsed > 0
    && elapsed <= maxGap
    && current.timestampMs - previous.timestampMs <= maxGap;
}

function graphX(timestamp: number, range: HistoryRequest) {
  return PADDING + (WIDTH - 2 * PADDING) * (timestamp - range.startMs) / (range.endMs - range.startMs);
}

export function graphPaths(result: HistoryResult, range: HistoryRequest, metric: Metric) {
  const line: string[] = [];
  const dots: string[] = [];
  let lastPoint: string | null = null;
  let connected = false;

  function add(timestamp: number, value: number | null, adjacent: boolean) {
    if (value === null || !adjacent) {
      if (lastPoint !== null && !connected) dots.push(`M${lastPoint}h0`);
      lastPoint = null;
      connected = false;
    }
    if (value === null) return;

    const x = graphX(timestamp, range);
    const y = PADDING + (HEIGHT - 2 * PADDING) * (1 - value / 100);
    const point = `${x.toFixed(2)} ${y.toFixed(2)}`;
    connected = lastPoint !== null;
    if (connected) line.push(`L${point}`);
    else line.push(`M${point}`);
    lastPoint = point;
  }

  if (result.resolution === "raw") {
    let previous: RawSample | null = null;
    for (const sample of result.rows) {
      const value = metric === "cpu" ? sample.cpuPercent : memoryPercent(sample.memoryUsedBytes, sample.memoryTotalBytes);
      add(sample.timestampMs, value, previous !== null && follows(previous, sample));
      previous = sample;
    }
  } else {
    let previousStart = -2 * MINUTE_MS;
    for (const bucket of result.rows) {
      const from = Math.max(bucket.startMs, range.startMs);
      const to = Math.min(bucket.startMs + MINUTE_MS, range.endMs);
      const value = metric === "cpu"
        ? bucket.cpuPercent.average
        : memoryPercent(bucket.memoryUsedBytes.average, bucket.memoryTotalBytes.average);
      add((from + to) / 2, value, bucket.startMs - previousStart === MINUTE_MS);
      previousStart = bucket.startMs;
    }
  }

  if (lastPoint !== null && !connected) dots.push(`M${lastPoint}h0`);
  return { line: line.join(" "), dots: dots.join(" ") };
}

const timeLabel = new Intl.DateTimeFormat(undefined, {
  month: "short", day: "numeric", hour: "numeric", minute: "2-digit",
});

export function HistoryGraph({ result, range, metric, incidentStart }: {
  result: HistoryResult;
  range: HistoryRequest;
  metric: Metric;
  incidentStart?: number;
}) {
  const label = metric === "cpu" ? "CPU usage" : "RAM usage";
  const { line, dots } = graphPaths(result, range, metric);
  const incidentX = incidentStart === undefined ? null : graphX(incidentStart, range);

  return (
    <section className="history-chart" aria-label={label}>
      <h2>{label}</h2>
      {line || dots ? (
        <>
          <div className="history-chart-plot">
            <div className="history-chart-scale" aria-hidden="true"><span>100%</span><span>0%</span></div>
            <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" role="img"
              aria-label={`${label} from ${timeLabel.format(range.startMs)} to ${timeLabel.format(range.endMs)}`}>
              <path className="history-grid" d={`M0 3H${WIDTH} M0 52H${WIDTH} M0 101H${WIDTH}`} />
              {incidentX !== null && (
                <line className="history-incident-line" x1={incidentX} x2={incidentX} y1={0} y2={HEIGHT}>
                  <title>Incident start</title>
                </line>
              )}
              <path className="history-line" d={line} />
              <path className="history-dot" d={dots} />
            </svg>
          </div>
          <div className="history-chart-time">
            <time dateTime={new Date(range.startMs).toISOString()}>{timeLabel.format(range.startMs)}</time>
            <time dateTime={new Date(range.endMs).toISOString()}>{timeLabel.format(range.endMs)}</time>
          </div>
        </>
      ) : <p>No {metric === "cpu" ? "CPU" : "RAM"} readings in this range.</p>}
    </section>
  );
}
