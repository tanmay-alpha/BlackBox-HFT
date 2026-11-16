// Comparison cells shared by every page that lists configuration differences or comparison
// metrics (Compare, Latency Lab), so they always read the same.

import type { ConfigDiff, JsonValue, MetricRow } from '../api/types';
import { fractionToBps } from './describe';
import { fmtDuration, fmtDurationExact, fmtNum } from './fmt';
import { valueLabel } from './labels';

/** Server labels markout horizons in raw milliseconds (`Markout 30000 ms`); show them as durations. */
export function metricLabel(row: MetricRow): string {
  const m = /^markout_(\d+)ms$/.exec(row.key);
  return m ? `Markout ${fmtDuration(Number(m[1]) * 1_000_000)}` : row.label;
}

/** Keys of a configuration object listed first, in this order (a latency leg reads `min …, jitter …`). */
const KEY_ORDER = ['min_ns', 'median_ns', 'mean_ns', 'jitter', 'sigma', 'clip', 'unit'];

function keyRank(k: string): number {
  const i = KEY_ORDER.indexOf(k);
  return i === -1 ? KEY_ORDER.length : i;
}

/**
 * One stored configuration value as text: `none` for null, durations for `_ns`, basis points
 * beside fee fractions, enum words for strings and `key value` lists for objects
 * (`min 768.92 µs, jitter 0.5, clip 0.1, unit 10`).
 */
export function configCellText(path: string, value: JsonValue): string {
  if (value === null) return 'none';
  if (typeof value === 'boolean') return value ? 'yes' : 'no';
  if (path.endsWith('_ns') && (typeof value === 'number' || (typeof value === 'string' && /^-?\d+$/.test(value)))) {
    return typeof value === 'number' && !Number.isFinite(value) ? String(value) : fmtDurationExact(value);
  }
  if (typeof value === 'number') {
    return path.startsWith('execution.fees.') && Number.isFinite(value) ? `${value} (${fractionToBps(value)} bp)` : String(value);
  }
  if (typeof value === 'string') return valueLabel(value);
  if (Array.isArray(value)) {
    return value.length === 0 ? 'none' : value.map((v) => configCellText(path, v)).join(', ');
  }
  const entries = Object.entries(value).sort(([x], [y]) => keyRank(x) - keyRank(y));
  if (entries.length === 0) return 'none';
  return entries
    .map(([k, v]) => {
      const name = k.replace(/_ns$/, '').replace(/_/g, ' ');
      const text = configCellText(`${path}.${k}`, v);
      return `${name} ${v !== null && typeof v === 'object' && !Array.isArray(v) ? `(${text})` : text}`;
    })
    .join(', ');
}

/** One side of a configuration difference: the stored path that holds the value, and the value. */
export interface ConfigSide {
  path: string;
  value: JsonValue;
}

/** A row of a configuration difference table. `path` names the row; each side keeps its own path. */
export interface ConfigDiffRow {
  path: string;
  a: ConfigSide;
  b: ConfigSide;
}

/**
 * Differences as table rows. A setting stored under two alternative keys depending on the model
 * (`entry_ns` for a constant latency, `entry` for a heavy-tail leg) is one setting: when both
 * keys differ and each run holds exactly one of them, they share one row named after the `_ns` key.
 * A difference that is `null` on both sides (key absent in one config, `null` in the other) is dropped.
 */
export function configDiffRows(diffs: ConfigDiff[]): ConfigDiffRow[] {
  const byPath = new Map(diffs.map((d) => [d.path, d]));
  const merged = new Set<string>();
  const rows: ConfigDiffRow[] = [];
  for (const d of diffs) {
    // A key absent on one side and `null` on the other is listed as a difference; both mean unset.
    if (merged.has(d.path) || (d.a === null && d.b === null)) continue;
    const twinPath = d.path.endsWith('_ns') ? d.path.slice(0, -3) : `${d.path}_ns`;
    const twin = byPath.get(twinPath);
    const pick = (x: JsonValue, y: JsonValue) => (x === null) !== (y === null);
    if (twin && pick(d.a, twin.a) && pick(d.b, twin.b)) {
      merged.add(twinPath);
      const side = (k: 'a' | 'b'): ConfigSide => (d[k] !== null ? { path: d.path, value: d[k] } : { path: twinPath, value: twin[k] });
      rows.push({ path: d.path.endsWith('_ns') ? d.path : twinPath, a: side('a'), b: side('b') });
    } else {
      rows.push({ path: d.path, a: { path: d.path, value: d.a }, b: { path: d.path, value: d.b } });
    }
  }
  return rows;
}

/** Varied paths with alternative keys of one setting (`entry`, `entry_ns`) listed once, as the `_ns` key. */
export function variedPaths(paths: readonly string[]): string[] {
  const set = new Set(paths);
  return paths.filter((p) => !(set.has(`${p}_ns`)));
}

/** A table cell for one stored configuration value, the stored JSON as its title. */
export function ConfigValue({ path, value }: { path: string; value: JsonValue }) {
  const raw = JSON.stringify(value);
  const text = configCellText(path, value);
  if (path.endsWith('_ns') && typeof value === 'number' && Number.isFinite(value)) {
    return (
      <td className="mono wrap-anywhere" title={`${raw} ns as stored`}>
        {text} <span className="dim">({fmtNum(value, 0)} ns)</span>
      </td>
    );
  }
  return (
    <td className={typeof value === 'number' ? 'mono wrap-anywhere' : 'wrap-anywhere'} title={`${raw} as stored`}>
      {text}
    </td>
  );
}
