// Number, time and price formatting shared by every workspace (UI_DESIGN_SPEC.md §7). All
// functions are pure and locale-independent: `.` is the decimal point and `,` the thousands
// separator whatever the browser locale, so the same data always renders the same text.
// Display minus is U+2212; nanosecond values go through `ns.ts` and never through floats.

import type { InstrumentMeta } from '../api/types';
import {
  MINUS,
  NsError,
  fmtNsDuration,
  fmtNsDurationExact,
  fmtNsRel,
  fmtNsUtc,
  groupThousands,
  toNs,
  type NsDigits,
  type NsInput,
} from './ns';

/** Shown wherever a value does not exist. Never replaced by a guess. */
export const DASH = '\u2014';

const reported = new Set<string>();

/** A value the formatter cannot show exactly: an em dash on screen, one console error per value. */
function unrepresentable(e: unknown): string {
  if (!(e instanceof NsError)) throw e;
  if (!reported.has(e.message)) {
    reported.add(e.message);
    console.error(`BlackBox: ${e.message}`);
  }
  return DASH;
}

/** `x` with `digits` decimals, `,` thousands separators and U+2212 for negatives. */
function fixed(x: number, digits: number): string {
  const text = Math.abs(x).toFixed(digits);
  const [whole, frac] = text.split('.');
  const body = `${groupThousands(whole)}${frac === undefined ? '' : `.${frac}`}`;
  // A value that rounds to zero at this precision is printed unsigned.
  return x < 0 && Number(text) !== 0 ? `${MINUS}${body}` : body;
}

/** Three significant digits without exponent notation, for `0 <= v < 1000`. */
function sig3(v: number): string {
  if (v >= 99.95) return v.toFixed(0);
  if (v >= 9.995) return v.toFixed(1);
  return v.toFixed(2);
}

/**
 * Adaptive duration: ns, µs, ms or s with three significant digits. Integer nanosecond values
 * (numbers, decimal strings, bigints) are formatted exactly with integer arithmetic; a fractional
 * number (a statistic such as a mean of durations) uses the same rule in floating point.
 */
export function fmtDuration(ns: NsInput | null | undefined): string {
  if (ns === null || ns === undefined) return DASH;
  if (typeof ns === 'number' && Number.isFinite(ns) && !Number.isInteger(ns)) {
    const sign = ns < 0 ? MINUS : '';
    const a = Math.abs(ns);
    if (a < 999.5) return `${sign}${sig3(a)} ns`;
    if (a < 999.5e3) return `${sign}${sig3(a / 1e3)} \u00b5s`;
    if (a < 999.5e6) return `${sign}${sig3(a / 1e6)} ms`;
    const s = a / 1e9;
    return `${sign}${s >= 1000 ? groupThousands(Math.round(s).toFixed(0)) : sig3(s)} s`;
  }
  try {
    return fmtNsDuration(ns);
  } catch (e) {
    return unrepresentable(e);
  }
}

/**
 * Exact duration in the largest unit not above the value, trailing zeros trimmed
 * (`1227000000` -> `1.227 s`). For prose where three significant digits would merge values.
 */
export function fmtDurationExact(ns: NsInput): string {
  try {
    return fmtNsDurationExact(ns);
  } catch (e) {
    return unrepresentable(e);
  }
}

/** Rewrites raw nanosecond counts inside server-written prose (`5000000000 ns` -> `5 s`). */
export function humanizeNsText(text: string): string {
  return text
    .replace(/\[(-?\d+), (-?\d+)\] ns\b/g, (_, lo: string, hi: string) => `[${fmtDurationExact(lo)}, ${fmtDurationExact(hi)}]`)
    .replace(/\b(\d{4,}) ns\b/g, (_, digits: string) => fmtDurationExact(digits));
}

/**
 * Time relative to the dataset origin, `+mm:ss.mmm uuu[ nnn]` (minutes not wrapped). `digits`
 * follows the dataset's timestamp resolution (`digitsForUnit`).
 */
export function fmtClock(relNs: NsInput, digits: NsDigits = 6): string {
  try {
    return fmtNsRel(relNs, digits);
  } catch (e) {
    return unrepresentable(e);
  }
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
  if (tick === null || tick === undefined || !Number.isFinite(tick)) return DASH;
  const base = stepDecimals(inst.tick_size);
  const decimals = Number.isInteger(tick) ? base : Math.min(base + 2, 14);
  const text = (tick * inst.tick_size).toFixed(decimals);
  const trimmed =
    decimals === base ? text : text.replace(new RegExp(`(\\.\\d{${base}}\\d*?)0+$`), '$1').replace(/\.$/, '');
  return trimmed.startsWith('-') ? `${MINUS}${trimmed.slice(1)}` : trimmed;
}

/** Quantity in base units. Decimals come from `lot_size`. */
export function fmtQty(qty: number | null, inst: InstrumentMeta): string {
  if (qty === null || qty === undefined || !Number.isFinite(qty)) return DASH;
  return fixed(qty, stepDecimals(inst.lot_size));
}

/** Fixed decimals (default 2) with thousands separators. */
export function fmtNum(x: number | null | undefined, digits = 2): string {
  if (x === null || x === undefined || !Number.isFinite(x)) return DASH;
  return fixed(x, digits);
}

/** Like `fmtNum` with an explicit `+` on positive values. Zero carries no sign. */
export function fmtSigned(x: number | null | undefined, digits = 2): string {
  if (x === null || x === undefined || !Number.isFinite(x)) return DASH;
  const text = fixed(x, digits);
  return x > 0 && Number(Math.abs(x).toFixed(digits)) !== 0 ? `+${text}` : text;
}

/** Money (notional, cash): 4 decimals in tables, 2 in the KPI strip. */
export function fmtMoney(x: number | null | undefined, digits: 2 | 4 = 4): string {
  return fmtNum(x, digits);
}

/** P&L, fees, drawdown: signed money. */
export function fmtPnl(x: number | null | undefined, digits: 2 | 4 = 4): string {
  return fmtSigned(x, digits);
}

/** A ratio in `[0, 1]` as a percentage with two decimals. */
export function fmtPct(x: number | null | undefined): string {
  if (x === null || x === undefined || !Number.isFinite(x)) return DASH;
  return `${fixed(x * 100, 2)}%`;
}

/** Difference of two ratios, in percentage points. */
export function fmtPp(dx: number | null | undefined): string {
  if (dx === null || dx === undefined || !Number.isFinite(dx)) return DASH;
  return `${fmtSigned(dx * 100, 2)} pp`;
}

/** Basis points with two decimals (`0.50 bp`). */
export function fmtBp(bp: number | null | undefined): string {
  if (bp === null || bp === undefined || !Number.isFinite(bp)) return DASH;
  return `${fixed(bp, 2)} bp`;
}

/** Spread, markout or spread capture in ticks: three decimals, unit in the column header. */
export function fmtTicks(x: number | null | undefined): string {
  return fmtNum(x, 3);
}

/** Dimensionless statistic: two decimals, three when |x| < 0.1. */
export function fmtStat(x: number | null | undefined): string {
  if (x === null || x === undefined || !Number.isFinite(x)) return DASH;
  return fixed(x, Math.abs(x) < 0.1 && x !== 0 ? 3 : 2);
}

/** Whole number with thousands separators. */
export function fmtInt(x: number | null | undefined): string {
  return fmtNum(x, 0);
}

/** Byte count in binary units. */
export function fmtBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes)) return DASH;
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
 * UTC wall-clock time of `relNs` after a dataset origin (absolute epoch ns, a decimal string),
 * computed in `bigint`: `YYYY-MM-DD HH:MM:SS.fff UTC` with `digits` fraction digits.
 */
export function fmtOriginUtc(originNs: NsInput, relNs: NsInput = 0, digits: NsDigits = 3): string {
  try {
    return fmtNsUtc(toNs(originNs) + toNs(relNs), digits);
  } catch (e) {
    return unrepresentable(e);
  }
}

/** Wall-clock creation time of a run (`created_unix_ms`), in UTC to the second. */
export function fmtUnixMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isSafeInteger(ms)) return DASH;
  try {
    return fmtNsUtc(BigInt(ms) * 1_000_000n, 0);
  } catch (e) {
    return unrepresentable(e);
  }
}

/** `snake_case` identifier as words, for table cells (`working_at_end` -> `working at end`). */
export function words(id: string): string {
  return id.replace(/_/g, ' ');
}

/** Raw text for copies and exports: ASCII minus, no thousands separators or thin spaces. */
export function rawText(display: string): string {
  return display.replace(/\u2212/g, '-').replace(/(\d),(?=\d{3})/g, '$1').replace(/\u2009/g, '');
}
