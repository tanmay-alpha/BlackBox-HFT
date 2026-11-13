// Number, time and price formatting shared by every page. All functions are pure and
// locale-independent so the same data always renders the same text.

import type { InstrumentMeta } from '../api/types';

/** Shown wherever a value does not exist. Never replaced by a guess. */
export const DASH = '\u2014';

const NS_PER_US = 1_000;
const NS_PER_MS = 1_000_000;
const NS_PER_S = 1_000_000_000;

/** Three significant digits without exponent notation, for `0 <= v < 1000`. */
function sig3(v: number): string {
  // Boundaries sit where rounding would otherwise add a fourth digit (99.96 -> "100", not "100.0").
  if (v >= 99.95) return v.toFixed(0);
  if (v >= 9.995) return v.toFixed(1);
  return v.toFixed(2);
}

/**
 * Adaptive duration: ns, µs, ms or s with three significant digits. `null` gives an em dash.
 * Seconds above 1000 keep whole seconds rather than switching to another unit.
 */
export function fmtDuration(ns: number | null): string {
  if (ns === null || ns === undefined) return DASH;
  if (!Number.isFinite(ns)) return String(ns);
  const sign = ns < 0 ? '-' : '';
  const a = Math.abs(ns);
  if (a === 0) return '0 ns';
  // Each threshold is where three-significant-digit rounding would reach 1000 of the unit.
  if (a < 999.5) return `${sign}${Number.isInteger(a) ? a.toFixed(0) : sig3(a)} ns`;
  if (a < 999.5 * NS_PER_US) return `${sign}${sig3(a / NS_PER_US)} \u00b5s`;
  if (a < 999.5 * NS_PER_MS) return `${sign}${sig3(a / NS_PER_MS)} ms`;
  const s = a / NS_PER_S;
  return `${sign}${s >= 1000 ? Math.round(s).toLocaleString('en-US') : sig3(s)} s`;
}

/**
 * Time relative to the dataset origin as `mm:ss.mmmuuu` (minutes are not wrapped at 60, so a
 * long dataset reads `125:03.120045`). Callers dim the trailing three microsecond digits.
 */
export function fmtClock(relNs: number): string {
  if (!Number.isFinite(relNs)) return DASH;
  const neg = relNs < 0;
  const us = Math.floor(Math.abs(relNs) / NS_PER_US);
  const micros = us % 1_000_000;
  const totalSec = Math.floor(us / 1_000_000);
  const sec = totalSec % 60;
  const min = Math.floor(totalSec / 60);
  const mm = String(min).padStart(2, '0');
  const ss = String(sec).padStart(2, '0');
  const frac = String(micros).padStart(6, '0');
  return `${neg ? '-' : ''}${mm}:${ss}.${frac}`;
}

/** Number of decimals needed to print multiples of `step` exactly (0.01 -> 2, 0.5 -> 1, 5 -> 0). */
export function stepDecimals(step: number): number {
  if (!Number.isFinite(step) || step <= 0) return 0;
  for (let d = 0; d <= 12; d += 1) {
    const scaled = step * 10 ** d;
    if (Math.abs(scaled - Math.round(scaled)) < 1e-9 * Math.max(1, scaled)) return d;
  }
  return 12;
}

/**
 * Price of a tick value. Decimals come from `tick_size`; fractional ticks (mid prices, average
 * fill prices) get two more so half-tick values are not rounded away.
 */
export function fmtPrice(tick: number | null, inst: InstrumentMeta): string {
  if (tick === null || tick === undefined) return DASH;
  if (!Number.isFinite(tick)) return String(tick);
  const base = stepDecimals(inst.tick_size);
  const decimals = Number.isInteger(tick) ? base : Math.min(base + 2, 14);
  return (tick * inst.tick_size).toFixed(decimals);
}

/** Quantity in base units. Decimals come from `lot_size`. */
export function fmtQty(qty: number | null, inst: InstrumentMeta): string {
  if (qty === null || qty === undefined) return DASH;
  if (!Number.isFinite(qty)) return String(qty);
  return qty.toFixed(stepDecimals(inst.lot_size));
}

/** Fixed decimals (default 2) with thousands separators. */
export function fmtNum(x: number | null, digits = 2): string {
  if (x === null || x === undefined) return DASH;
  if (!Number.isFinite(x)) return String(x);
  return x.toLocaleString('en-US', {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

/** Like `fmtNum` with an explicit `+` on positive values. Zero carries no sign. */
export function fmtSigned(x: number | null, digits = 2): string {
  if (x === null || x === undefined) return DASH;
  if (!Number.isFinite(x)) return String(x);
  const text = fmtNum(Math.abs(x), digits);
  // A value that rounds to zero at this precision is printed unsigned.
  if (Number(Math.abs(x).toFixed(digits)) === 0) return text;
  return `${x < 0 ? '-' : '+'}${text}`;
}

/** A ratio in `[0, 1]` as a percentage with one decimal. */
export function fmtPct(x: number | null): string {
  if (x === null || x === undefined) return DASH;
  if (!Number.isFinite(x)) return String(x);
  return `${(x * 100).toFixed(1)}%`;
}

/** Whole number with thousands separators. */
export function fmtInt(x: number | null): string {
  return fmtNum(x, 0);
}

/** Byte count in binary units. */
export function fmtBytes(bytes: number | null): string {
  if (bytes === null || bytes === undefined) return DASH;
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${sig3(v)} ${units[i]}`;
}

/** Leading characters of a hex digest. The full value belongs in a `title` attribute. */
export function shortHash(hash: string, chars = 12): string {
  return hash.length > chars ? hash.slice(0, chars) : hash;
}

/**
 * UTC wall-clock time of `relNs` after a dataset origin given as a decimal string of epoch
 * nanoseconds. BigInt keeps the full nanosecond value; the result is `YYYY-MM-DD HH:MM:SS.mmm UTC`.
 */
export function fmtOriginUtc(originNs: string, relNs = 0): string {
  let abs: bigint;
  try {
    abs = BigInt(originNs) + BigInt(Math.trunc(relNs));
  } catch {
    return DASH;
  }
  const ms = Number(abs / 1_000_000n);
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return DASH;
  return `${d.toISOString().replace('T', ' ').replace('Z', '')} UTC`;
}

/** Wall-clock creation time of a run (`created_unix_ms`), in UTC to the second. */
export function fmtUnixMs(ms: number | null): string {
  if (ms === null || ms === undefined) return DASH;
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return DASH;
  return `${d.toISOString().slice(0, 19).replace('T', ' ')} UTC`;
}

/** `snake_case` identifier as words, for table cells (`working_at_end` -> `working at end`). */
export function words(id: string): string {
  return id.replace(/_/g, ' ');
}
