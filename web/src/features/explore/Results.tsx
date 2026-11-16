// Whole-run results of the two stored runs from the stored comparison: a compact table of the key
// measures with the difference row, and every stored measure behind "Show all".

import { useState } from 'react';

import { fmtDuration, fmtInt, fmtMoney, fmtNum, fmtPct, fmtPnl, fmtPp } from '../../components/fmt';
import { metric, scenarioMetrics, type LatencyStory, type ScenarioMetric } from './story';
import { METRIC_GROUPS, presentMetric, type Focus, type ScenarioKey } from './data';

const KEYS: { key: string; label: string; hint: string }[] = [
  { key: 'fill_count', label: 'Fills', hint: 'Simulated executions' },
  { key: 'fees', label: 'Fees / rebates', hint: 'Quote currency; raw sign, negative = rebate' },
  { key: 'net_pnl', label: 'Net P&L', hint: 'After fees, quote currency' },
  { key: 'fill_ratio_orders', label: 'Orders with any fill', hint: 'Fully or partially filled orders ÷ submitted orders, as of run end' },
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

/** Headline measures of the comparison table and the unit shown next to each label. */
const STRIP: { key: string; unit: string }[] = [
  { key: 'fill_count', unit: '' },
  { key: 'fees', unit: 'quote' },
  { key: 'net_pnl', unit: 'quote' },
  { key: 'round_trip_ns', unit: 'median' },
];

/**
 * The compact comparison: fills, fees, net P&L and (when stored) the median round trip for both
 * scenarios and their difference (slow minus fast), one aligned column per measure, using the
 * same stored values and formatting as the full table. The difference is not marked good or bad.
 */
export function ResultStrip({ story, names }: { story: LatencyStory; names: Record<ScenarioKey, string> }) {
  const rows = scenarioMetrics(story);
  const cols = STRIP.flatMap(({ key, unit }) => {
    const m = metric(rows, key);
    const k = KEYS.find((x) => x.key === key);
    return m && k ? [{ m, k, unit }] : [];
  });
  return (
    <table className="ex-compare">
      <caption className="visually-hidden">
        Whole-run results, {names.fast} versus {names.slow}. Δ is {names.slow} minus {names.fast}; it is not marked good or bad.
      </caption>
      <thead>
        <tr>
          <th scope="col">Order latency</th>
          {cols.map(({ m, k, unit }) => (
            <th key={m.key} scope="col" data-key={m.key} title={k.hint}>
              {k.label.replace(' (median)', '')}
              {unit ? <span className="ex-unit"> {unit}</span> : null}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {(['fast', 'slow'] as const).map((s) => (
          <tr key={s}>
            <th scope="row">
              <span className={`ex-swatch ex-swatch-${s}`} aria-hidden="true" /> {names[s]}
            </th>
            {cols.map(({ m }) => (
              <td key={m.key} className="ex-num" data-key={m.key}>
                {value(m, m[s])}
              </td>
            ))}
          </tr>
        ))}
        <tr className="ex-compare-delta">
          <th scope="row" title={`Difference: ${names.slow} minus ${names.fast}`}>
            Δ {names.slow} − {names.fast}
          </th>
          {cols.map(({ m }) => (
            <td key={m.key} className="ex-num" data-key={m.key}>
              {delta(m)}
            </td>
          ))}
        </tr>
      </tbody>
      {cols.some(({ m }) => m.key === 'round_trip_ns') ? (
        <tfoot className="ex-compare-hidden">
          <tr>
            <td colSpan={cols.length + 1}>Round trip (median) is hidden at this width; it is in the full results under step 1.</td>
          </tr>
        </tfoot>
      ) : null}
    </table>
  );
}

export function Results({ story, names, focus }: { story: LatencyStory; names: Record<ScenarioKey, string>; focus: Focus }) {
  const [all, setAll] = useState(false);
  const rows = scenarioMetrics(story);
  const shown = KEYS.map((k) => ({ k, m: metric(rows, k.key) })).filter((x): x is { k: (typeof KEYS)[number]; m: ScenarioMetric } => x.m !== null);
  const focusAttr = (k: ScenarioKey) => (focus === k ? '' : undefined);
  return (
    <div className="ex-results">
      <div className="ex-scroll-x ex-results-wide">
        <table className="ex-table">
          <caption className="ex-table-cap">Whole-run results. Difference = {names.slow} minus {names.fast}; it is not marked good or bad.</caption>
          <thead>
            <tr>
              <th scope="col">Scenario</th>
              {shown.map(({ k }) => (
                <th key={k.key} scope="col" className="ex-num" title={k.hint}>
                  {k.label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {(['fast', 'slow'] as const).map((s) => (
              <tr key={s} data-focus={focusAttr(s)}>
                <th scope="row">
                  <span className={`ex-swatch ex-swatch-${s}`} aria-hidden="true" /> {names[s]}
                </th>
                {shown.map(({ m }) => (
                  <td key={m.key} className="ex-num">
                    {value(m, m[s])}
                  </td>
                ))}
              </tr>
            ))}
            <tr className="ex-total">
              <th scope="row">
                Difference ({names.slow} − {names.fast})
              </th>
              {shown.map(({ m }) => (
                <td key={m.key} className="ex-num">
                  {delta(m)}
                </td>
              ))}
            </tr>
          </tbody>
        </table>
      </div>
      <table className="ex-table ex-results-narrow">
        <caption className="ex-table-cap">Whole-run results. Difference = {names.slow} minus {names.fast}; it is not marked good or bad.</caption>
        <thead>
          <tr>
            <th scope="col">Result</th>
            <th scope="col" className="ex-num" data-focus={focusAttr('fast')}>
              {names.fast}
            </th>
            <th scope="col" className="ex-num" data-focus={focusAttr('slow')}>
              {names.slow}
            </th>
            <th scope="col" className="ex-num">
              Difference
            </th>
          </tr>
        </thead>
        <tbody>
          {shown.map(({ k, m }) => (
            <tr key={k.key}>
              <th scope="row">{k.label}</th>
              <td className="ex-num">{value(m, m.fast)}</td>
              <td className="ex-num">{value(m, m.slow)}</td>
              <td className="ex-num">{delta(m)}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <button type="button" className="btn-link ex-more" aria-expanded={all} aria-controls="ex-all-measures" onClick={() => setAll((v) => !v)}>
        {all ? 'Hide the other measures' : `Show all ${rows.length} stored measures`}
      </button>
      {all ? (
        <div id="ex-all-measures" className="ex-scroll-x">
          <table className="ex-table">
            <caption className="ex-table-cap">
              Every measure of the stored comparison. Change = {names.slow} minus {names.fast}; it is not marked good or bad.
            </caption>
            <thead>
              <tr>
                <th scope="col">Measure</th>
                <th scope="col">Unit</th>
                <th scope="col" className="ex-num" data-focus={focusAttr('fast')}>
                  {names.fast}
                </th>
                <th scope="col" className="ex-num" data-focus={focusAttr('slow')}>
                  {names.slow}
                </th>
                <th scope="col" className="ex-num">
                  Change
                </th>
              </tr>
            </thead>
            {METRIC_GROUPS.map((g) => {
              const inGroup = rows.map((m) => ({ m, p: presentMetric(m) })).filter((r) => r.p.group === g);
              if (inGroup.length === 0) return null;
              return (
                <tbody key={g}>
                  <tr className="ex-group-row">
                    <th scope="rowgroup" colSpan={5}>
                      {g}
                    </th>
                  </tr>
                  {inGroup.map(({ m, p }) => (
                    <tr key={m.key}>
                      <th scope="row">{p.label}</th>
                      <td className="ex-muted">{p.unit}</td>
                      <td className="ex-num">{p.value(m.fast)}</td>
                      <td className="ex-num">{p.value(m.slow)}</td>
                      <td className="ex-num">{p.delta(m.delta)}</td>
                    </tr>
                  ))}
                </tbody>
              );
            })}
          </table>
        </div>
      ) : null}
    </div>
  );
}
