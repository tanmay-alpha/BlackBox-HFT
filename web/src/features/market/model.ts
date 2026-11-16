// Pure data shaping for the Market Data Workbench: URL state, exact price and session-time text,
// quality-check status, book arithmetic and event navigation. Nothing here invents a value: a
// missing input yields `null`, which the views print as a dash.

import type { QualityCheck } from '../../api/types';
import { groupThousands, MINUS, THIN_SPACE } from '../../components/ns';
import type { BookLevel, MarketEvent, MetricPoint } from './api';

// ----- URL state ------------------------------------------------------------------------------

export type MarketTab = 'dataset' | 'validation' | 'book' | 'events' | 'metrics';

export const MARKET_TABS: { id: MarketTab; label: string; short: string }[] = [
  { id: 'dataset', label: 'Dataset', short: 'Dataset' },
  { id: 'validation', label: 'Validation', short: 'Validation' },
  { id: 'book', label: 'Order book', short: 'Book' },
  { id: 'events', label: 'Event timeline', short: 'Events' },
  { id: 'metrics', label: 'Research metrics', short: 'Metrics' },
];

/** Largest `from_event_index` the events endpoint accepts. */
export const MAX_FROM_INDEX = 99_999;

export interface WorkbenchQuery {
  tab: MarketTab;
  /** Selected event index, shared by the book, the timeline and the charts. */
  event: number | null;
  /** Timeline filter: one ITCH message-type letter. */
  type: string | null;
  /** Event index the timeline page starts at. */
  from: number | null;
}

function nonNegativeInt(raw: string | null, max = Number.MAX_SAFE_INTEGER): number | null {
  if (raw === null || !/^\d{1,16}$/.test(raw)) return null;
  const n = Number(raw);
  return Number.isSafeInteger(n) && n <= max ? n : null;
}

/** Reads the workbench state from the query string; anything malformed is ignored, not guessed. */
export function parseWorkbenchQuery(sp: URLSearchParams): WorkbenchQuery {
  const rawTab = sp.get('tab');
  const tab = MARKET_TABS.some((t) => t.id === rawTab) ? (rawTab as MarketTab) : 'dataset';
  const rawType = sp.get('type');
  return {
    tab,
    event: nonNegativeInt(sp.get('event')),
    type: rawType !== null && /^[A-Za-z]$/.test(rawType) ? rawType : null,
    from: nonNegativeInt(sp.get('from'), MAX_FROM_INDEX),
  };
}

// ----- exact text ---------------------------------------------------------------------------

/** `price4` (quote currency x 10,000) as an exact decimal with four places: `1234500` -> `123.4500`. */
export function fmtPrice4(p: number | null | undefined): string {
  if (p === null || p === undefined || !Number.isSafeInteger(p)) return '\u2014';
  const neg = p < 0;
  const a = Math.abs(p);
  const whole = Math.floor(a / 10_000);
  const frac = String(a % 10_000).padStart(4, '0');
  return `${neg ? MINUS : ''}${groupThousands(String(whole))}.${frac}`;
}

/** Whole shares with thousands separators; `null` is a dash. */
export function fmtShares(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return '\u2014';
  return `${n < 0 ? MINUS : ''}${groupThousands(String(Math.abs(Math.trunc(n))))}`;
}

const NS_PER_SECOND = 1_000_000_000n;

function parseDecimalNs(text: string | null | undefined): bigint | null {
  if (text === null || text === undefined || !/^-?\d{1,20}$/.test(text)) return null;
  return BigInt(text);
}

/**
 * Session time `HH:MM:SS.mmm uuu nnn` from nanoseconds since the session's midnight, with thin
 * spaces between the sub-second groups. Hours are not wrapped (ITCH allows no value of a day or
 * more, and the quality report flags any).
 */
export function fmtSessionNs(text: string | null | undefined): string {
  const ns = parseDecimalNs(text);
  if (ns === null) return '\u2014';
  const neg = ns < 0n;
  const a = neg ? -ns : ns;
  const secs = a / NS_PER_SECOND;
  const sub = String(a % NS_PER_SECOND).padStart(9, '0');
  const hh = String(secs / 3600n).padStart(2, '0');
  const mm = String((secs / 60n) % 60n).padStart(2, '0');
  const ss = String(secs % 60n).padStart(2, '0');
  return `${neg ? MINUS : ''}${hh}:${mm}:${ss}.${sub.slice(0, 3)}${THIN_SPACE}${sub.slice(3, 6)}${THIN_SPACE}${sub.slice(6)}`;
}

/** `later - earlier` in ns as a decimal string, or `null` when either side is missing. */
export function nsBetween(earlier: string | null | undefined, later: string | null | undefined): string | null {
  const a = parseDecimalNs(earlier);
  const b = parseDecimalNs(later);
  if (a === null || b === null) return null;
  return String(b - a);
}

// ----- ITCH message types -----------------------------------------------------------------------

/** ITCH 5.0 message-type letters (specification §1.1–§1.10). */
export const ITCH_TYPES: Record<string, string> = {
  S: 'System event',
  R: 'Stock directory',
  H: 'Trading action',
  Y: 'Reg SHO restriction',
  L: 'Market participant position',
  V: 'MWCB decline level',
  W: 'MWCB status',
  K: 'IPO quoting period',
  J: 'LULD auction collar',
  h: 'Operational halt',
  A: 'Add order',
  F: 'Add order with MPID',
  E: 'Order executed',
  C: 'Order executed with price',
  X: 'Order cancel (partial)',
  D: 'Order delete',
  U: 'Order replace',
  P: 'Trade (non-cross)',
  Q: 'Cross trade',
  B: 'Broken trade',
  I: 'NOII',
  N: 'Retail price improvement',
  O: 'Direct listing with capital raise',
};

export function itchTypeName(letter: string): string {
  return ITCH_TYPES[letter] ?? 'Not defined by ITCH 5.0';
}

export interface TypeCountRow {
  type: string;
  name: string;
  count: number;
  /** Fraction of all counted messages, 0..1. */
  share: number;
}

/** Message counts by type, largest first (ties by letter), with each type's share of the total. */
export function typeCountRows(byType: Record<string, number>): TypeCountRow[] {
  const entries = Object.entries(byType).filter(([, n]) => Number.isFinite(n));
  const total = entries.reduce((s, [, n]) => s + n, 0);
  return entries
    .map(([type, count]) => ({ type, name: itchTypeName(type), count, share: total > 0 ? count / total : 0 }))
    .sort((a, b) => b.count - a.count || (a.type < b.type ? -1 : a.type > b.type ? 1 : 0));
}

// ----- quality checks -------------------------------------------------------------------------

export type CheckStatus = 'pass' | 'warn' | 'fail' | 'info';

/** A passed check is `pass`; a failed one takes its severity (error -> fail). */
export function checkStatus(c: Pick<QualityCheck, 'passed' | 'severity'>): CheckStatus {
  if (c.passed) return 'pass';
  if (c.severity === 'error') return 'fail';
  if (c.severity === 'warn') return 'warn';
  return 'info';
}

export function checkCounts(checks: readonly Pick<QualityCheck, 'passed' | 'severity'>[]): Record<CheckStatus, number> {
  const out: Record<CheckStatus, number> = { pass: 0, warn: 0, fail: 0, info: 0 };
  for (const c of checks) out[checkStatus(c)] += 1;
  return out;
}

/** Failures first, then warnings, notes and passes; source order within a status. */
export function sortChecks<T extends Pick<QualityCheck, 'passed' | 'severity'>>(checks: readonly T[]): T[] {
  const rank: Record<CheckStatus, number> = { fail: 0, warn: 1, info: 2, pass: 3 };
  return checks
    .map((c, i) => ({ c, i }))
    .sort((a, b) => rank[checkStatus(a.c)] - rank[checkStatus(b.c)] || a.i - b.i)
    .map((x) => x.c);
}

// ----- book arithmetic ------------------------------------------------------------------------

export type TopState = 'two-sided' | 'locked' | 'crossed' | 'one-sided' | 'empty';

export interface Top {
  bid: number | null;
  ask: number | null;
  /** `ask - bid` in price4 units, when both sides exist. */
  spread4: number | null;
  /** Mid in price4 units (may be a half unit). */
  mid4: number | null;
  state: TopState;
}

export function topOfBook(bid: number | null | undefined, ask: number | null | undefined): Top {
  const b = bid ?? null;
  const a = ask ?? null;
  if (b === null && a === null) return { bid: null, ask: null, spread4: null, mid4: null, state: 'empty' };
  if (b === null || a === null) return { bid: b, ask: a, spread4: null, mid4: null, state: 'one-sided' };
  const spread4 = a - b;
  return { bid: b, ask: a, spread4, mid4: (a + b) / 2, state: spread4 < 0 ? 'crossed' : spread4 === 0 ? 'locked' : 'two-sided' };
}

/** `(bid - ask) / (bid + ask)`, in [-1, 1]; `null` when both are zero. */
export function shareImbalance(bid: number, ask: number): number | null {
  const total = bid + ask;
  return total > 0 ? (bid - ask) / total : null;
}

/** Shares and orders listed in a level versus its aggregate (the server may cap the listed orders). */
export function listedOf(level: Pick<BookLevel, 'orders'>): { orders: number; shares: number } {
  let shares = 0;
  for (const o of level.orders) shares += o.shares;
  return { orders: level.orders.length, shares };
}

// ----- event navigation -----------------------------------------------------------------------

export type Step =
  | { kind: 'select'; index: number }
  /** The neighbour is not on the loaded page; ask the server. */
  | { kind: 'fetch' };

/**
 * The next (`dir` 1) or previous (`dir` -1) event after `selected` among the loaded page `items`
 * (ascending event index). When `selected` is not on the page, the neighbour is the nearest item
 * beyond it in that direction, provided the page proves nothing lies between: for `next`, an item
 * above `selected` is the neighbour only if the page started at or before `selected` (`pageFrom`);
 * for `previous`, an item below it only if more items follow on the page or the page is the last.
 */
export function stepWithin(
  items: readonly Pick<MarketEvent, 'event_index'>[],
  selected: number,
  dir: 1 | -1,
  page: { from: number; complete: boolean },
): Step {
  if (items.length === 0) return { kind: 'fetch' };
  if (dir === 1) {
    const i = items.findIndex((e) => e.event_index > selected);
    if (i < 0) return { kind: 'fetch' };
    if (i > 0 || page.from <= selected + 1) return { kind: 'select', index: items[i].event_index };
    return { kind: 'fetch' };
  }
  let i = -1;
  for (let k = items.length - 1; k >= 0; k -= 1) {
    if (items[k].event_index < selected) {
      i = k;
      break;
    }
  }
  if (i < 0) return { kind: 'fetch' };
  if (i < items.length - 1 || page.complete) return { kind: 'select', index: items[i].event_index };
  return { kind: 'fetch' };
}

// ----- metric series ------------------------------------------------------------------------

/**
 * Index of the last sample at or before `eventIndex` (no look-ahead: a later sample never stands
 * for an earlier event), or -1 when every sample is later.
 */
export function sampleAtOrBefore(series: readonly Pick<MetricPoint, 'event_index'>[], eventIndex: number): number {
  let lo = 0;
  let hi = series.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (series[mid].event_index <= eventIndex) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}

/** `part / whole` as a fraction, or `null` without a denominator. */
export function ratio(part: number, whole: number): number | null {
  return whole > 0 ? part / whole : null;
}

/** A fraction as a percentage with one decimal, `null` as a dash. */
export function fmtShare(x: number | null): string {
  return x === null || !Number.isFinite(x) ? '\u2014' : `${(x * 100).toFixed(1)}%`;
}
