// Editable "seconds after origin" fields of the Order Book Lab toolbar, with exact integer
// nanosecond conversion (no float formatting of nanoseconds).

import { nsFromDecimal, nsToSafeNumber } from '../../components/ns';

const NS_PER_S = 1_000_000_000n;

/** Exact decimal seconds of integer nanoseconds (`1500000000` -> `"1.5"`). */
export function secondsText(ns: number): string {
  const v = BigInt(Math.round(ns));
  const neg = v < 0n;
  const a = neg ? -v : v;
  const frac = (a % NS_PER_S).toString().padStart(9, '0').replace(/0+$/, '');
  return `${neg ? '-' : ''}${a / NS_PER_S}${frac ? `.${frac}` : ''}`;
}

/** Typed decimal seconds as nanoseconds, or `null` when it is not a valid decimal. */
export function parseSeconds(text: string): number | null {
  try {
    return nsToSafeNumber(nsFromDecimal(text, 's'));
  } catch {
    return null;
  }
}
