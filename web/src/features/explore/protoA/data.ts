// Page-level loaders and pure derivations for prototype A (chart-first latency experiment).
// Everything is read from the stored demo, runs, comparison and dataset replay through
// `../story`; nothing here simulates or recomputes results.

import { ApiError, api } from '../../../api/client';
import type { OrderDetail } from '../../../api/model';
import type { DatasetManifest, OrderPair, OrderRecord, PairClass } from '../../../api/types';
import { fmtDurationExact, words } from '../../../components/fmt';
import { loadLatencyStory, pairRun, type LatencyStory, type Moment, type Scenario } from '../story';

export type View = 'fast' | 'slow' | 'both';
export type Which = 'fast' | 'slow';

export interface PageData {
  story: LatencyStory;
  manifest: DatasetManifest;
}

/** The experiment plus the manifest of the exact dataset version both runs replayed. */
export async function loadPageData(signal: AbortSignal): Promise<PageData> {
  const story = await loadLatencyStory(signal);
  const ds = story.fast.run.dataset;
  const version = await api.datasetVersion(ds.id, ds.version, { signal });
  return { story, manifest: version.manifest };
}

/** The stored aligned pairs (the server keeps the earliest ones; `truncated` says so). */
export async function loadStoredPairs(story: LatencyStory, signal: AbortSignal): Promise<{ pairs: Map<number, OrderPair>; truncated: boolean }> {
  const pairs = new Map<number, OrderPair>();
  if (!story.comparisonId) return { pairs, truncated: false };
  let cursor: string | null = null;
  // The comparison stores at most `pairs_cap` pairs; this reads them all in a few pages.
  for (let page = 0; page < 10; page += 1) {
    const res = await api.comparisonPairs(story.comparisonId, { cursor, limit: 1000 }, { signal });
    for (const p of res.items) pairs.set(p.order_id, p);
    cursor = res.next_cursor;
    if (!cursor) break;
  }
  return { pairs, truncated: story.comparison?.alignment?.pairs_truncated ?? false };
}

async function orderOrNull(runId: string, orderId: number, signal: AbortSignal): Promise<OrderDetail | null> {
  try {
    return await api.orderDetail(runId, orderId, { signal });
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}

/** One decision slot's full order record in both scenarios; `null` where that run never placed it. */
export async function loadLifecycles(story: LatencyStory, orderId: number, signal: AbortSignal): Promise<Record<Which, OrderDetail | null>> {
  const [fast, slow] = await Promise.all([orderOrNull(story.fast.runId, orderId, signal), orderOrNull(story.slow.runId, orderId, signal)]);
  return { fast, slow };
}

/** "1 ms" / "50 ms": the scenario's one-way order latency, exact. */
export function shortLabel(s: Scenario): string {
  return s.entryNs === s.responseNs ? fmtDurationExact(s.entryNs) : `${fmtDurationExact(s.entryNs)} / ${fmtDurationExact(s.responseNs)}`;
}

export function scenarioOf(story: LatencyStory, which: Which): Scenario {
  return which === 'fast' ? story.fast : story.slow;
}

export function shows(view: View, which: Which): boolean {
  return view === 'both' || view === which;
}

/** Plain-language verdict of an aligned pair, oriented to scenario names. */
export function pairVerdict(story: LatencyStory, pair: OrderPair): string {
  const c: PairClass = pair.class;
  switch (c) {
    case 'identical':
      return 'Identical in both scenarios';
    case 'same_fills_different_history':
      return 'Same fills; the recorded history differs';
    case 'same_fills_different_fees':
      return 'Same fills; the fees differ';
    case 'same_outcome_different_timing':
      return 'Same outcome, different timing';
    case 'same_decision_different_outcome':
      return 'Same decision, different outcome';
    case 'different_decision':
      return 'The strategy decided differently';
    case 'only_a':
      return `Decided only in the ${shortLabel(pairRun(story, 'a'))} scenario`;
    case 'only_b':
      return `Decided only in the ${shortLabel(pairRun(story, 'b'))} scenario`;
    default:
      return words(c);
  }
}

const DIFFERENCE_WORDS: Record<string, string> = {
  exch_arrival_ts: 'exchange arrival time',
  first_response_ts: 'time the response reached the strategy',
  fill_ts: 'fill time',
  terminal_ts: 'completion time',
  lifecycle_events: 'lifecycle events',
  decision_ts: 'decision time',
  outcome: 'outcome',
  filled_qty: 'filled quantity',
  fees: 'fees',
  price_tick: 'price',
  qty: 'quantity',
  side: 'side',
  cancel_sent_ts: 'cancel time',
};

export function differenceWords(field: string): string {
  return DIFFERENCE_WORDS[field] ?? words(field.replace(/_ts$/, ' time'));
}

/** One decision slot seen in the window, with each scenario's order record when it has one. */
export interface WindowOrder {
  orderId: number;
  decisionTs: number;
  fast: OrderRecord | null;
  slow: OrderRecord | null;
}

/** Orders alive in the window in either scenario, in decision order. */
export function windowOrders(m: Moment): WindowOrder[] {
  const byId = new Map<number, WindowOrder>();
  const add = (which: Which, o: OrderRecord) => {
    const cur = byId.get(o.order_id) ?? { orderId: o.order_id, decisionTs: o.decision_ts, fast: null, slow: null };
    cur[which] = o;
    cur.decisionTs = Math.min(cur.decisionTs, o.decision_ts);
    byId.set(o.order_id, cur);
  };
  for (const o of m.fast.orders) add('fast', o);
  for (const o of m.slow.orders) add('slow', o);
  return [...byId.values()].sort((a, b) => a.decisionTs - b.decisionTs || a.orderId - b.orderId);
}

/** Orders resting at the simulated exchange at `t` (arrived, not yet terminal). */
export function restingAt(orders: OrderRecord[], t: number): OrderRecord[] {
  return orders.filter((o) => o.exch_arrival_ts !== null && o.exch_arrival_ts <= t && (o.terminal_ts === null || o.terminal_ts > t));
}

/** Index of the last element of ascending `ts` at or before `t`, or -1. */
export function lastAtOrBefore(ts: readonly number[], t: number): number {
  let lo = 0;
  let hi = ts.length - 1;
  let ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (ts[mid] <= t) {
      ans = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return ans;
}

/** Seconds after the dataset origin with `digits` decimals (`0.1089 s`). */
export function fmtSec(ns: number | null, digits = 4): string {
  if (ns === null) return '\u2014';
  return `${(ns / 1e9).toFixed(digits)} s`;
}

/** Outcome of an order record in plain words. */
export function outcomeWords(o: OrderRecord): string {
  switch (o.outcome) {
    case 'filled':
      return 'Filled';
    case 'partially_filled':
      return 'Partly filled';
    case 'canceled':
      return 'Cancelled';
    case 'expired':
      return 'Expired';
    case 'refused':
      return 'Refused by the exchange model';
    case 'risk_blocked':
      return 'Blocked by a risk limit';
    case 'in_flight_at_end':
      return 'Still in flight at the end';
    case 'working_at_end':
      return 'Still working at the end';
    default:
      return words(o.outcome);
  }
}
