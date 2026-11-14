// Nanosecond values (UI_DESIGN_SPEC.md §7.3). Every nanosecond value the UI receives (absolute
// epoch ns, ns relative to a dataset origin, durations) is parsed into a `bigint` here and
// formatted with integer arithmetic. Nothing in this module passes a nanosecond value through
// floating point: `Number(...)` is applied only to whole seconds (for `Date`'s calendar fields)
// and to values already proven to be safe integers.
//
// Inputs are `string | number | bigint`:
// - decimal strings (the v1 API sends every ns value as a string-encoded i64);
// - JS numbers (the legacy `/api` sends relative ns as JSON numbers). A number is accepted only
//   when `Number.isSafeInteger` holds: a JSON number above 2^53 - 1 has already been rounded by
//   `JSON.parse`, so it is rejected with an `NsError` instead of being shown as if exact.

import type { TimeUnit } from '../api/types';

/** A nanosecond value that cannot be represented exactly. Always explicit, never rounded. */
export class NsError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'NsError';
  }
}

export type NsInput = string | number | bigint;

/** Fraction digits shown for a timestamp: seconds, milliseconds, microseconds or nanoseconds. */
export type NsDigits = 0 | 3 | 6 | 9;

const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;
const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);

export const NS_PER_US = 1_000n;
export const NS_PER_MS = 1_000_000n;
export const NS_PER_S = 1_000_000_000n;

/** U+2212, the display minus (same advance as `+` in the mono face). Copies use ASCII `-`. */
export const MINUS = '\u2212';
/** U+2009, between groups of three fraction digits. */
export const THIN_SPACE = '\u2009';

const NS_STRING = /^-?\d{1,19}$/;

function describe(v: unknown): string {
  if (typeof v === 'string') return JSON.stringify(v.length > 40 ? `${v.slice(0, 40)}...` : v);
  return String(v);
}

/** Strict parse of a decimal string: optional '-', 1-19 digits, within the i64 range. */
export function parseNs(s: string): bigint {
  if (typeof s !== 'string' || !NS_STRING.test(s)) {
    throw new NsError(`${describe(s)} is not a nanosecond integer (expected an optional '-' and 1-19 digits)`);
  }
  const n = BigInt(s);
  if (n < I64_MIN || n > I64_MAX) {
    throw new NsError(`${s} ns is outside the signed 64-bit range`);
  }
  return n;
}

/**
 * Any accepted nanosecond input as a `bigint`. Numbers must be safe integers: a value above
 * 2^53 - 1 (or below -(2^53 - 1)) is rejected because its low digits were already lost.
 */
export function toNs(v: NsInput): bigint {
  if (typeof v === 'bigint') {
    if (v < I64_MIN || v > I64_MAX) throw new NsError(`${v} ns is outside the signed 64-bit range`);
    return v;
  }
  if (typeof v === 'string') return parseNs(v);
  if (typeof v === 'number') {
    if (!Number.isSafeInteger(v)) {
      throw new NsError(
        Number.isInteger(v)
          ? `${v} ns exceeds the safe JS integer range (2^53 - 1); it must be sent as a decimal string`
          : `${describe(v)} is not an integer number of nanoseconds`,
      );
    }
    return BigInt(v);
  }
  throw new NsError(`${describe(v)} is not a nanosecond value`);
}

/**
 * A nanosecond value as a JS number for code that does arithmetic on the legacy API's relative
 * times (chart coordinates, window bounds). Exact by construction: anything outside the safe
 * integer range throws `NsError`.
 */
export function nsToSafeNumber(v: NsInput): number {
  const n = toNs(v);
  if (n > MAX_SAFE || n < -MAX_SAFE) {
    throw new NsError(`${n} ns exceeds the safe JS integer range (2^53 - 1) and cannot be used as a number`);
  }
  return Number(n);
}

/** Ordering without conversion. */
export function cmpNs(a: NsInput, b: NsInput): -1 | 0 | 1 {
  const x = toNs(a);
  const y = toNs(b);
  return x < y ? -1 : x > y ? 1 : 0;
}

/** Fraction digits a dataset's declared source resolution supports. */
export function digitsForUnit(unit: TimeUnit): NsDigits {
  switch (unit) {
    case 'seconds':
      return 0;
    case 'millis':
      return 3;
    case 'micros':
      return 6;
    default:
      return 9;
  }
}

/** Floor division (toward negative infinity) and the matching non-negative remainder. */
function floorDivMod(n: bigint, d: bigint): [bigint, bigint] {
  let q = n / d;
  let r = n - q * d;
  if (r < 0n) {
    q -= 1n;
    r += d;
  }
  return [q, r];
}

const pad2 = (n: number) => String(n).padStart(2, '0');

/** Maximum |seconds| `Date` can represent (8.64e15 ms). */
const MAX_DATE_SECONDS = 8_640_000_000_000n;

/**
 * Absolute UTC time: `YYYY-MM-DD HH:MM:SS[.fff…] UTC`. The fraction is truncated (never rounded
 * up into a moment that had not happened) to `digits`, and undecorated so it can be copied.
 */
export function fmtNsUtc(epochNs: NsInput, digits: NsDigits): string {
  const n = toNs(epochNs);
  const [sec, frac] = floorDivMod(n, NS_PER_S);
  if (sec > MAX_DATE_SECONDS || sec < -MAX_DATE_SECONDS) {
    throw new NsError(`${n} ns is outside the representable calendar range`);
  }
  // Whole seconds are exact as a number (|sec| <= 8.64e12 < 2^53).
  const d = new Date(Number(sec) * 1000);
  const year = d.getUTCFullYear();
  const yyyy = year < 0 ? `-${String(-year).padStart(6, '0')}` : String(year).padStart(4, '0');
  const date = `${yyyy}-${pad2(d.getUTCMonth() + 1)}-${pad2(d.getUTCDate())}`;
  const time = `${pad2(d.getUTCHours())}:${pad2(d.getUTCMinutes())}:${pad2(d.getUTCSeconds())}`;
  const fraction = digits === 0 ? '' : `.${frac.toString().padStart(9, '0').slice(0, digits)}`;
  return `${date} ${time}${fraction} UTC`;
}

/** Parts of a relative time, for components that dim the sub-millisecond groups. */
export interface RelParts {
  /** `+` or U+2212. Zero is `+`. */
  sign: string;
  /** `mm:ss` (minutes are not wrapped at 60). */
  clock: string;
  /** Fraction digit groups of three (`['100', '000']`); empty for `digits = 0`. */
  groups: string[];
}

/** The parts of `fmtNsRel`. The magnitude is truncated toward zero to `digits`. */
export function relParts(relNs: NsInput, digits: NsDigits): RelParts {
  const n = toNs(relNs);
  const neg = n < 0n;
  const a = neg ? -n : n;
  const totalSec = a / NS_PER_S;
  const frac = a % NS_PER_S;
  const min = totalSec / 60n;
  const sec = totalSec % 60n;
  const fracText = frac.toString().padStart(9, '0').slice(0, digits);
  const groups: string[] = [];
  for (let i = 0; i < fracText.length; i += 3) groups.push(fracText.slice(i, i + 3));
  return {
    sign: neg && a !== 0n ? MINUS : '+',
    clock: `${min.toString().padStart(2, '0')}:${sec.toString().padStart(2, '0')}`,
    groups,
  };
}

/**
 * Time relative to a dataset origin: `+mm:ss.mmm uuu nnn` with thin spaces between the groups of
 * three, minutes not wrapped, sign always shown.
 */
export function fmtNsRel(relNs: NsInput, digits: NsDigits): string {
  const p = relParts(relNs, digits);
  return p.groups.length === 0 ? `${p.sign}${p.clock}` : `${p.sign}${p.clock}.${p.groups.join(THIN_SPACE)}`;
}

/** Whole number with `,` thousands separators, from a non-negative bigint. */
export function groupThousands(digits: string): string {
  return digits.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/** `value / 10^decimals` as text with exactly `decimals` fraction digits (value >= 0). */
function fixedPoint(value: bigint, decimals: number): string {
  if (decimals === 0) return value.toString();
  const s = value.toString().padStart(decimals + 1, '0');
  return `${s.slice(0, -decimals)}.${s.slice(-decimals)}`;
}

const DURATION_UNITS: [bigint, string][] = [
  [NS_PER_US, '\u00b5s'],
  [NS_PER_MS, 'ms'],
  [NS_PER_S, 's'],
];

/**
 * Adaptive duration with three significant digits in ns, µs, ms or s (the `fmtDuration` rule),
 * rounded half up with integer arithmetic. Seconds from 1000 up keep whole seconds with `,`
 * separators. Negative durations carry U+2212.
 */
export function fmtNsDuration(ns: NsInput): string {
  const n = toNs(ns);
  const sign = n < 0n ? MINUS : '';
  const a = n < 0n ? -n : n;
  if (a === 0n) return '0 ns';
  // Below 999.5 ns: whole nanoseconds.
  if (2n * a < 1999n) return `${sign}${a} ns`;
  for (const [unit, label] of DURATION_UNITS) {
    // Use this unit while three-significant-digit rounding stays below 1000 of it.
    if (label !== 's' && 2n * a >= 1999n * unit) continue;
    if (label === 's' && 2n * a >= 1999n * unit) {
      return `${sign}${groupThousands(((a + unit / 2n) / unit).toString())} s`;
    }
    // value = a / unit; decimals: >= 99.95 -> 0, >= 9.995 -> 1, else 2.
    const decimals = 100n * a >= 9995n * unit ? 0 : 1000n * a >= 9995n * unit ? 1 : 2;
    const scale = 10n ** BigInt(decimals);
    const rounded = (a * scale * 2n + unit) / (unit * 2n);
    return `${sign}${fixedPoint(rounded, decimals)} ${label}`;
  }
  throw new NsError(`unreachable duration ${n}`);
}

/**
 * Exact duration in the largest unit not above the value, trailing zeros trimmed
 * (`1227000000` -> `1.227 s`, `1790156000` -> `1.790156 s`).
 */
export function fmtNsDurationExact(ns: NsInput): string {
  const n = toNs(ns);
  const sign = n < 0n ? MINUS : '';
  const a = n < 0n ? -n : n;
  const [unit, label, places]: [bigint, string, number] =
    a >= NS_PER_S
      ? [NS_PER_S, 's', 9]
      : a >= NS_PER_MS
        ? [NS_PER_MS, 'ms', 6]
        : a >= NS_PER_US
          ? [NS_PER_US, '\u00b5s', 3]
          : [1n, 'ns', 0];
  let text = places === 0 ? a.toString() : fixedPoint(a, places);
  if (places > 0) text = text.replace(/0+$/, '').replace(/\.$/, '');
  const [whole, frac] = text.split('.');
  return `${sign}${groupThousands(whole)}${frac ? `.${frac}` : ''} ${label}`;
}

export type DurationUnit = 'ns' | 'us' | 'ms' | 's';

const UNIT_NS: Record<DurationUnit, bigint> = { ns: 1n, us: NS_PER_US, ms: NS_PER_MS, s: NS_PER_S };

/**
 * A typed decimal in `unit` as exact nanoseconds (`"0.5"` ms -> `500000n`). Digits below one
 * nanosecond are refused rather than rounded.
 */
export function nsFromDecimal(text: string, unit: DurationUnit): bigint {
  const m = /^\s*(-?)(\d+)(?:\.(\d+))?\s*$/.exec(text);
  if (!m) throw new NsError(`${describe(text)} is not a decimal number`);
  const [, minus, whole, frac = ''] = m;
  const per = UNIT_NS[unit];
  const places = per.toString().length - 1;
  if (frac.replace(/0+$/, '').length > places) {
    throw new NsError(`${text} ${unit} has digits below one nanosecond`);
  }
  const n = BigInt(whole) * per + (frac === '' ? 0n : BigInt(frac.padEnd(places, '0').slice(0, places)));
  return toNs(minus ? -n : n);
}
