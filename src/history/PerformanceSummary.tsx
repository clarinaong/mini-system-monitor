import type { Eligibility, MetricSummary, PerformanceSummary as Summary, SummaryRequest } from "./summary";

const unavailable: Record<Exclude<Eligibility, "eligible">, string> = {
  periodTooShort: "Comparison requires at least 15 minutes in each period.",
  unequalPeriodLengths: "Comparison requires periods of equal elapsed length.",
  insufficientCoverage: "Comparison requires at least 75% coverage in each period.",
};

const number = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 });
const signedNumber = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 1,
  signDisplay: "exceptZero",
});

function percent(value: number | null) {
  return value === null ? "Unavailable" : `${number.format(value)}%`;
}

function change(current: number, previous: number) {
  return `${signedNumber.format(current - previous)} pp`;
}

function MetricComparison({ current, previous, eligibility }: {
  current: MetricSummary;
  previous: MetricSummary;
  eligibility: Eligibility;
}) {
  if (eligibility !== "eligible") return <p>{unavailable[eligibility]}</p>;

  if (current.averagePercent === null || previous.averagePercent === null ||
      current.peakPercent === null || previous.peakPercent === null) return null;

  return <p>Change in percentage points (pp) · Average {change(current.averagePercent, previous.averagePercent)}
    {" · "}Peak {change(current.peakPercent, previous.peakPercent)}</p>;
}

export function PerformanceSummary({ result, request, timeZone, onRetry }: {
  result: { data: Summary } | { error: string } | null;
  request: SummaryRequest;
  timeZone: string;
  onRetry: () => void;
}) {
  const dateTime = new Intl.DateTimeFormat(undefined, {
    timeZone, month: "short", day: "numeric", year: "numeric",
    hour: "numeric", minute: "2-digit", second: "2-digit", timeZoneName: "short",
  });
  const periodLabel = ({ startMs, endMs }: SummaryRequest["current"]) =>
    `${dateTime.format(startMs)} to ${dateTime.format(endMs)}`;

  return (
    <section className="history-summary history-chart" aria-labelledby="history-summary-heading">
      <h2 id="history-summary-heading">Performance summary</h2>
      <dl className="incident-details">
        <dt>Selected range</dt><dd>{periodLabel(request.current)}</dd>
        <dt>Previous range</dt><dd>{periodLabel(request.previous)}</dd>
      </dl>
      {!result ? <p role="status">Loading performance summary…</p> : "error" in result ? (
        <>
          <p role="alert">{result.error}</p>
          <button type="button" onClick={onRetry}>Retry summary</button>
        </>
      ) : (
        <>
          {result.data.current.cpu.coveredMinutes === 0 && result.data.current.ram.coveredMinutes === 0 &&
            <p role="status">No valid CPU or RAM readings in the selected range.</p>}
          <div className="history-summary-metrics">
            {(["cpu", "ram"] as const).map(metric => {
              const { current, previous, cpuComparison, ramComparison } = result.data;
              const label = metric === "cpu" ? "CPU" : "RAM";
              return (
                <div key={metric}>
                  <table aria-label={`${label} summary`}>
                    <caption>{label}</caption>
                    <thead><tr><th scope="col">Range</th><th scope="col">Average</th>
                      <th scope="col">Peak</th><th scope="col">Coverage</th></tr></thead>
                    <tbody>
                      {([["Selected", current], ["Previous", previous]] as const).map(([name, period]) => (
                        <tr key={name}>
                          <th scope="row">{name}</th>
                          <td>{percent(period[metric].averagePercent)}</td>
                          <td>{percent(period[metric].peakPercent)}</td>
                          <td>{percent(period[metric].coveragePercent)} ({period[metric].coveredMinutes}/{period.totalMinutes} min)</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                  <MetricComparison current={current[metric]} previous={previous[metric]}
                    eligibility={metric === "cpu" ? cpuComparison : ramComparison} />
                </div>
              );
            })}
          </div>
        </>
      )}
    </section>
  );
}
