// Axis arithmetic shared by the custom charts. Nothing here knows about React or the DOM.

/** Smallest step of the form {1, 2, 5} x 10^k that is not below `rough`. */
export function niceStep(rough: number): number {
  if (!Number.isFinite(rough) || rough <= 0) return 1;
  const pow = Math.pow(10, Math.floor(Math.log10(rough)));
  const f = rough / pow;
  const m = f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10;
  return m * pow;
}

/** Multiples of `step` inside `[lo, hi]`, capped so a degenerate step cannot flood the chart. */
export function ticksBetween(lo: number, hi: number, step: number): number[] {
  const out: number[] = [];
  if (!Number.isFinite(step) || step <= 0 || !(hi > lo)) return out;
  const first = Math.ceil(lo / step);
  const last = Math.floor(hi / step);
  for (let k = first; k <= last && out.length < 200; k++) out.push(k * step);
  return out;
}

/** Decimal places needed to tell values `step` apart. */
export function decimalsForStep(step: number): number {
  if (!Number.isFinite(step) || step <= 0) return 0;
  return Math.min(10, Math.max(0, Math.ceil(-Math.log10(step) - 1e-9)));
}

interface TimeUnit {
  ns: number;
  label: string;
}

const NS_UNIT: TimeUnit = { ns: 1, label: "ns" };
const TIME_UNITS: TimeUnit[] = [
  { ns: 1e9, label: "s" },
  { ns: 1e6, label: "ms" },
  { ns: 1e3, label: "\u00b5s" },
  NS_UNIT,
];

/**
 * Labels a nanosecond value for an axis whose ticks are `stepNs` apart and whose visible values
 * reach `scaleNs` in magnitude. The unit follows the scale and the decimals follow the step, so
 * two neighbouring ticks never print the same text however far the view is zoomed in (a fixed
 * number of significant digits cannot do that at 60 s with a 10 µs window).
 */
export function fmtAxisNs(valueNs: number, stepNs: number, scaleNs: number): string {
  const unit = TIME_UNITS.find((u) => Math.abs(scaleNs) >= u.ns) ?? NS_UNIT;
  const decimals =
    stepNs > 0 ? Math.min(9, Math.max(0, Math.ceil(Math.log10(unit.ns / stepNs) - 1e-9))) : 0;
  return `${(valueNs / unit.ns).toFixed(decimals)} ${unit.label}`;
}

/** Index of the last element of ascending `xs` that is `<= x`, or -1. */
export function lastAtOrBefore(xs: ArrayLike<number>, x: number): number {
  let lo = 0;
  let hi = xs.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((xs[mid] as number) <= x) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}
