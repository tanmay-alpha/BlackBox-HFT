// Result deltas of the two stored runs (from the stored comparison) and the net P&L identity
// read from each run's stored metrics. Statements are arithmetic over these numbers only.

import { fmtDuration, fmtInt, fmtMoney, fmtNum, fmtPct, fmtPnl, fmtPp } from '../../../components/fmt';
import { PNL_IDENTITY_TOLERANCE, metric, pnlBreakdown, scenarioMetrics, type LatencyStory, type ScenarioMetric } from '../story';
import type { Which } from './data';

const KEYS: { key: string; label: string; hint: string }[] = [
  { key: 'fill_count', label: 'Fills', hint: 'Simulated executions' },
  { key: 'fees', label: 'Fees paid', hint: 'Quote currency' },
  { key: 'net_pnl', label: 'Net P&L', hint: 'After fees, quote currency' },
  { key: 'fill_ratio_orders', label: 'Orders filled', hint: 'Share of submitted orders' },
  { key: 'round_trip_ns', label: 'Round trip (median)', hint: 'Order sent → response back' },
  { key: 'orders_submitted', label: 'Orders submitted', hint: 'Sent to the simulated exchange' },
];

function value(m: ScenarioMetric, v: number | null): string {
  switch (m.unit) {
    case 'count':
      return fmtInt(v);
    case 'ratio':
      return fmtPct(v);
    case 'ns':
      return fmtDuration(v);
    case 'quote':
      return m.key === 'fees' ? fmtMoney(v, 2) : fmtPnl(v, 2);
    default:
      return fmtNum(v, 4);
  }
}

function delta(m: ScenarioMetric): string {
  const d = m.delta;
  if (d === null) return '\u2014';
  switch (m.unit) {
    case 'count':
      return `${d > 0 ? '+' : ''}${fmtInt(d)}`;
    case 'ratio':
      return fmtPp(d);
    case 'ns':
      return `${d > 0 ? '+' : ''}${fmtDuration(d)}`;
    case 'quote':
      return fmtPnl(d, 2);
    default:
      return fmtNum(d, 4);
  }
}

/** What the stored numbers say, in sentences that only restate them. */
function summary(story: LatencyStory, rows: ScenarioMetric[], labels: Record<Which, string>): string[] {
  const out: string[] = [];
  const net = metric(rows, 'net_pnl');
  const fills = metric(rows, 'fill_count');
  const fees = metric(rows, 'fees');
  if (net && net.fast !== null && net.slow !== null) {
    if (net.fast < 0 && net.slow < 0) {
      const less = net.slow > net.fast ? 'slow' : 'fast';
      const more = less === 'slow' ? 'fast' : 'slow';
      out.push(
        `Both scenarios lose money. The ${labels[less]} scenario loses ${fmtMoney(Math.abs(net.slow - net.fast), 2)} less than the ${labels[more]} one: a smaller loss, not a profit.`,
      );
    } else {
      out.push(`Net P&L is ${fmtPnl(net.fast, 2)} at ${labels.fast} and ${fmtPnl(net.slow, 2)} at ${labels.slow}.`);
    }
  }
  if (fills?.fast != null && fills.slow != null && fees?.fast != null && fees.slow != null) {
    out.push(
      `The ${labels.slow} scenario has ${fmtInt(fills.slow)} fills against ${fmtInt(fills.fast)}, and pays ${fmtMoney(fees.slow, 2)} in fees against ${fmtMoney(fees.fast, 2)}.`,
    );
  }
  const f = pnlBreakdown(story.fast.run);
  const s = pnlBreakdown(story.slow.run);
  if (Math.abs(f.residual) <= PNL_IDENTITY_TOLERANCE && Math.abs(s.residual) <= PNL_IDENTITY_TOLERANCE) {
    const feesDominate = [f, s].every((b) => b.fees > Math.abs(b.realized + b.unrealized));
    if (feesDominate) {
      out.push(
        `In both, fees are most of the loss: before fees the trading result is ${fmtPnl(f.realized + f.unrealized, 2)} (${labels.fast}) and ${fmtPnl(s.realized + s.unrealized, 2)} (${labels.slow}).`,
      );
    }
  }
  return out;
}

export function Results({ story, labels }: { story: LatencyStory; labels: Record<Which, string> }) {
  if (!story.comparison) {
    return <p className="pa-muted">No stored comparison of the two runs is published, so result differences cannot be shown.</p>;
  }
  const rows = scenarioMetrics(story);
  const shown = KEYS.map((k) => ({ k, m: metric(rows, k.key) })).filter((x): x is { k: (typeof KEYS)[number]; m: ScenarioMetric } => x.m !== null);
  const parts = { fast: pnlBreakdown(story.fast.run), slow: pnlBreakdown(story.slow.run) };
  return (
    <div className="pa-results">
      <div className="pa-results-scroll">
        <table className="pa-table pa-results-table">
          <caption className="visually-hidden">Whole-run results of both scenarios and the difference</caption>
          <thead>
            <tr>
              <th scope="col">Scenario</th>
              {shown.map(({ k }) => (
                <th key={k.key} scope="col" title={k.hint}>
                  {k.label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            <tr>
              <th scope="row">
                <span className="pa-swatch pa-swatch-fast" aria-hidden="true" /> {labels.fast}
              </th>
              {shown.map(({ m }) => (
                <td key={m.key} className="pa-num">
                  {value(m, m.fast)}
                </td>
              ))}
            </tr>
            <tr>
              <th scope="row">
                <span className="pa-swatch pa-swatch-slow" aria-hidden="true" /> {labels.slow}
              </th>
              {shown.map(({ m }) => (
                <td key={m.key} className="pa-num">
                  {value(m, m.slow)}
                </td>
              ))}
            </tr>
            <tr className="pa-delta-row">
              <th scope="row">Difference ({labels.slow} − {labels.fast})</th>
              {shown.map(({ m }) => (
                <td key={m.key} className="pa-num">
                  {delta(m)}
                </td>
              ))}
            </tr>
          </tbody>
        </table>
      </div>
      <table className="pa-table pa-results-narrow">
        <caption className="visually-hidden">Whole-run results of both scenarios and the difference</caption>
        <thead>
          <tr>
            <th scope="col">Result</th>
            <th scope="col">
              <span className="pa-swatch pa-swatch-fast" aria-hidden="true" /> {labels.fast}
            </th>
            <th scope="col">
              <span className="pa-swatch pa-swatch-slow" aria-hidden="true" /> {labels.slow}
            </th>
            <th scope="col">Difference</th>
          </tr>
        </thead>
        <tbody>
          {shown.map(({ k, m }) => (
            <tr key={k.key}>
              <th scope="row">{k.label}</th>
              <td className="pa-num">{value(m, m.fast)}</td>
              <td className="pa-num">{value(m, m.slow)}</td>
              <td className="pa-num">{delta(m)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <ul className="pa-summary">
        {summary(story, rows, labels).map((s) => (
          <li key={s}>{s}</li>
        ))}
      </ul>
      <details className="pa-details">
        <summary>How net P&L is made up</summary>
        <p>Net P&L = realized + unrealized − fees, read from each run's stored metrics.</p>
        <div className="pa-scroll-x">
          <table className="pa-table">
            <thead>
              <tr>
                <th scope="col">Scenario</th>
                <th scope="col">Realized</th>
                <th scope="col">Unrealized</th>
                <th scope="col">Fees</th>
                <th scope="col">Net</th>
                <th scope="col">Identity holds</th>
              </tr>
            </thead>
            <tbody>
              {(['fast', 'slow'] as const).map((w) => (
                <tr key={w}>
                  <th scope="row">{labels[w]}</th>
                  <td className="pa-num">{fmtPnl(parts[w].realized, 4)}</td>
                  <td className="pa-num">{fmtPnl(parts[w].unrealized, 4)}</td>
                  <td className="pa-num">{fmtMoney(parts[w].fees, 4)}</td>
                  <td className="pa-num">{fmtPnl(parts[w].net, 4)}</td>
                  <td>{Math.abs(parts[w].residual) <= PNL_IDENTITY_TOLERANCE ? 'Yes' : `No (residual ${fmtNum(parts[w].residual, 6)})`}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </details>
    </div>
  );
}
