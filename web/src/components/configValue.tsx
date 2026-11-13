// Comparison cells shared by every page that lists configuration differences or comparison
// metrics (Compare, Latency Lab), so they always read the same.

import type { JsonValue, MetricRow } from '../api/types';
import { fractionToBps } from './describe';
import { fmtDuration, fmtDurationExact, fmtNum } from './fmt';

/** Server labels markout horizons in raw milliseconds (`Markout 30000 ms`); show them as durations. */
export function metricLabel(row: MetricRow): string {
  const m = /^markout_(\d+)ms$/.exec(row.key);
  return m ? `Markout ${fmtDuration(Number(m[1]) * 1_000_000)}` : row.label;
}

/**
 * Nanosecond fields also read as a duration and fee fractions as basis points; the stored value
 * is always shown.
 */
export function ConfigValue({ path, value }: { path: string; value: JsonValue }) {
  const raw = JSON.stringify(value);
  if (path.endsWith('_ns') && typeof value === 'number' && Number.isFinite(value)) {
    return (
      <td className="mono wrap-anywhere" title={`${raw} ns as stored`}>
        {fmtDurationExact(value)} <span className="dim">({fmtNum(value, 0)} ns)</span>
      </td>
    );
  }
  if (path.startsWith('execution.fees.') && typeof value === 'number' && Number.isFinite(value)) {
    return (
      <td className="mono wrap-anywhere">
        {raw} <span className="dim">({fractionToBps(value)} bp)</span>
      </td>
    );
  }
  return <td className="mono wrap-anywhere">{raw}</td>;
}
