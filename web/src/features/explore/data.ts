// Loaders and plain-language presentation of the public latency experiment. Every number shown
// comes from `story.ts` loaders or the API; this file only chooses wording, units and
// formatting, and states the arithmetic it does (the P&L identity split).

import { api, ApiError } from '../../api/client';
import type { EquityPage, OrderDetail, RunTimeline } from '../../api/model';
import type { DatasetManifest, OrderPair, OrderRecord, PairClass } from '../../api/types';
import { fmtDuration, fmtDurationExact, fmtInt, fmtNum, fmtPct, fmtPp, fmtSigned, words } from '../../components/fmt';
import { fmtSignedDuration } from '../forensics/labels';
import {
  loadLatencyStory,
  pnlBreakdown,
  PNL_IDENTITY_TOLERANCE,
  type LatencyStory,
  type Moment,
  type PnlBreakdown,
  type Scenario,
  type ScenarioMetric,
} from './story';

export type ScenarioKey = 'fast' | 'slow';
/** Scenario filter: one scenario, or both side by side. */
export type Focus = ScenarioKey | 'both';

export const SCENARIO_KEYS: readonly ScenarioKey[] = ['fast', 'slow'];

export function shows(focus: Focus, k: ScenarioKey): boolean {
  return focus === 'both' || focus === k;
}

export interface Experiment {
  story: LatencyStory;
  /** Manifest of the exact dataset version both runs replayed. */
  dataset: DatasetManifest;
}

export async function loadExperiment(signal: AbortSignal): Promise<Experiment> {
  const story = await loadLatencyStory(signal);
  const ds = story.fast.run.dataset;
  const version = await api.datasetVersion(ds.id, ds.version, { signal });
  if (!version.manifest) throw new Error(`Dataset ${ds.id} version ${ds.version} has no manifest.`);
  return { story, dataset: version.manifest };
}

/**
 * Points (or server buckets) per equity curve. 200 buckets per run (about 34 KB) keep the page
 * within its per-visit data budget; each bucket's low-to-high band shows what a bucket hides.
 */
const EQUITY_POINTS = 200;

/** Both runs' equity over the whole run, at most `EQUITY_POINTS` points (or buckets) each. */
export async function loadEquity(story: LatencyStory, signal: AbortSignal): Promise<Record<ScenarioKey, EquityPage>> {
  const [fast, slow] = await Promise.all([
    api.equity(story.fast.runId, { max_points: EQUITY_POINTS }, { signal }),
    api.equity(story.slow.runId, { max_points: EQUITY_POINTS }, { signal }),
  ]);
  return { fast, slow };
}

/**
 * One order id in both scenarios. The same order id is the same decision slot; a 404 means the
 * strategy never made that decision in that scenario, which is a result, not an error.
 */
export async function loadOrderById(
  story: LatencyStory,
  orderId: number,
  signal: AbortSignal,
): Promise<Record<ScenarioKey, OrderDetail | null>> {
  const get = (s: Scenario) =>
    api.orderDetail(s.runId, orderId, { signal }).catch((e: unknown) => {
      if (e instanceof ApiError && e.status === 404) return null;
      throw e;
    });
  const [fast, slow] = await Promise.all([get(story.fast), get(story.slow)]);
  return { fast, slow };
}

/** Short scenario name from its modeled latency ("1 ms"), the demo label carries the long form. */
export function scenarioName(s: Scenario): string {
  return s.entryNs === s.responseNs ? fmtDurationExact(s.entryNs) : `${fmtDurationExact(s.entryNs)} / ${fmtDurationExact(s.responseNs)}`;
}

/** Seconds after the dataset start with `digits` decimals (`0.1089 s`); plain, never a clock. */
export function fmtSec(ns: number | null, digits = 4): string {
  if (ns === null) return '\u2014';
  return `${(ns / 1e9).toFixed(digits)} s`;
}

// ----- answer summary ----------------------------------------------------------------------

function result(net: number): string {
  if (net < 0) return `lost ${fmtNum(-net, 2)}`;
  if (net > 0) return `made ${fmtNum(net, 2)}`;
  return 'broke even';
}

export interface AnswerSummary {
  /** Prose form of the net results. */
  headline: string;
  /** One sentence on the size of the gap and its dominant part; null when the identity does not hold. */
  gap: string | null;
  /** Where the gap in net result comes from, by the P&L identity; null when it does not hold. */
  split: string | null;
  fast: PnlBreakdown;
  slow: PnlBreakdown;
  identityHolds: boolean;
}

/** The honest answer, built only from the stored run metrics. */
export function answerSummary(story: LatencyStory): AnswerSummary {
  const f = pnlBreakdown(story.fast.run);
  const s = pnlBreakdown(story.slow.run);
  const nf = scenarioName(story.fast);
  const ns = scenarioName(story.slow);
  const bothLose = f.net < 0 && s.net < 0;
  const bothMake = f.net > 0 && s.net > 0;
  const lead = bothLose ? 'Both scenarios lose money. ' : bothMake ? 'Both scenarios make money. ' : '';
  const headline = `${lead}At ${nf} the strategy ${result(f.net)}; at ${ns} it ${result(s.net)} (net after fees, quote currency).`;

  const identityHolds = Math.abs(f.residual) <= PNL_IDENTITY_TOLERANCE && Math.abs(s.residual) <= PNL_IDENTITY_TOLERANCE;
  let gapLine: string | null = null;
  let split: string | null = null;
  const gap = s.net - f.net;
  if (identityHolds && gap !== 0) {
    // net = realized + unrealized − fees, so the gap is the sum of these three contributions.
    const parts = [
      { what: 'the difference in fees', c: -(s.fees - f.fees), detail: `${ns} paid ${fmtNum(s.fees, 2)} in fees, ${nf} paid ${fmtNum(f.fees, 2)}` },
      { what: 'the difference in realized trading P&L', c: s.realized - f.realized, detail: `realized trading P&L was ${fmtSigned(s.realized, 2)} at ${ns} vs ${fmtSigned(f.realized, 2)} at ${nf}` },
      { what: 'the difference in unrealized P&L', c: s.unrealized - f.unrealized, detail: `unrealized P&L at the end was ${fmtSigned(s.unrealized, 2)} vs ${fmtSigned(f.unrealized, 2)}` },
    ];
    const top = parts.reduce((a, b) => (Math.abs(b.c) > Math.abs(a.c) ? b : a));
    const share = top.c / gap;
    const better = gap > 0 ? ns : nf;
    const amount = fmtNum(Math.abs(gap), 2);
    const verb = bothLose ? `loses ${amount} less` : bothMake ? `makes ${amount} more` : `ends ${amount} higher`;
    const rest = parts.filter((p) => p !== top).map((p) => p.detail);
    if (share >= 0.5) {
      gapLine = `${better} ${verb}; ${fmtPct(share)} of that is ${top.what}.`;
      split = `${better} ${verb}. ${fmtPct(share)} of that is ${top.what}: ${top.detail}. Otherwise, ${rest.join('; ')}.`;
    } else {
      gapLine = `${better} ${verb}, with no single dominant part.`;
      split = `${better} ${verb}, with no single dominant part: ${parts.map((p) => p.detail).join('; ')}.`;
    }
  }
  return { headline, gap: gapLine, split, fast: f, slow: s, identityHolds };
}

// ----- metrics table -----------------------------------------------------------------------

export interface MetricPresentation {
  label: string;
  /** Unit shown in the table, or '' when the value carries it (durations, percentages). */
  unit: string;
  group: string;
  value: (v: number | null) => string;
  delta: (d: number | null) => string;
}

const money = (v: number | null) => fmtNum(v, 2);
const signedMoney = (v: number | null) => fmtSigned(v, 2);

const PLAIN: Record<string, Partial<MetricPresentation>> = {
  net_pnl: { label: 'Net result after fees', group: 'Money', value: signedMoney },
  realized_pnl: { label: 'Realized trading P&L', group: 'Money', value: signedMoney },
  unrealized_pnl: { label: 'Unrealized P&L on the open position at the end', group: 'Money', value: signedMoney },
  fees: { label: 'Fees paid', group: 'Money', value: money },
  max_drawdown: { label: 'Largest fall from a previous high', group: 'Money', value: money },
  turnover: { label: 'Traded value', group: 'Activity', value: money },
  orders_submitted: { label: 'Orders sent', group: 'Activity' },
  fill_count: { label: 'Fills', group: 'Activity' },
  fill_ratio_orders: { label: 'Share of orders that filled', group: 'Activity' },
  fill_ratio_qty: { label: 'Share of order quantity that filled', group: 'Activity' },
  volume: { label: 'Quantity traded', group: 'Activity' },
  cancels_sent: { label: 'Cancel requests sent', group: 'Activity' },
  cancels_rejected: { label: 'Cancel requests the exchange rejected', group: 'Activity' },
  risk_blocks: { label: 'Orders blocked by risk limits', group: 'Activity' },
  entry_latency_ns: { label: 'Order travel time to the exchange (median)', group: 'Timing' },
  round_trip_ns: { label: 'Round trip to the exchange and back (median)', group: 'Timing' },
  time_to_first_fill_ns: { label: 'Time from reaching the exchange to first fill (median)', group: 'Timing' },
  order_age_ns: { label: 'Order lifetime (median)', group: 'Timing' },
  final_position: { label: 'Position at the end', group: 'Inventory' },
  max_long: { label: 'Largest long position', group: 'Inventory' },
  max_short: { label: 'Largest short position', group: 'Inventory' },
  mean_abs_position: { label: 'Average absolute position', group: 'Inventory' },
  spread_capture: { label: 'Spread captured per fill', group: 'Execution quality' },
  spread_capture_ticks: { label: 'Spread captured per fill', group: 'Execution quality' },
  markout_100ms: { label: 'Price move after a fill, 100 ms later', group: 'Execution quality' },
  markout_1000ms: { label: 'Price move after a fill, 1 s later', group: 'Execution quality' },
  markout_5000ms: { label: 'Price move after a fill, 5 s later', group: 'Execution quality' },
  markout_30000ms: { label: 'Price move after a fill, 30 s later', group: 'Execution quality' },
};

export const METRIC_GROUPS = ['Money', 'Activity', 'Timing', 'Inventory', 'Execution quality', 'Other'];

const UNIT_TEXT: Record<string, string> = { quote: 'quote ccy', qty: 'base units', ticks: 'ticks', count: '', ratio: '', ns: '' };

export function presentMetric(m: ScenarioMetric): MetricPresentation {
  const p = PLAIN[m.key] ?? {};
  const byUnit: Pick<MetricPresentation, 'value' | 'delta'> =
    m.unit === 'count'
      ? { value: fmtInt, delta: (d) => fmtSigned(d, 0) }
      : m.unit === 'ratio'
        ? { value: fmtPct, delta: fmtPp }
        : m.unit === 'ns'
          ? { value: fmtDuration, delta: fmtSignedDuration }
          : m.unit === 'quote'
            ? { value: money, delta: signedMoney }
            : { value: (v) => fmtNum(v, 3), delta: (d) => fmtSigned(d, 3) };
  return {
    label: p.label ?? m.label,
    unit: UNIT_TEXT[m.unit] ?? m.unit,
    group: p.group ?? 'Other',
    value: p.value ?? byUnit.value,
    delta: byUnit.delta,
  };
}

// ----- order pairs -------------------------------------------------------------------------

/** Plain wording of an alignment class; `only_*` name the scenario that made the decision. */
export function pairClassText(story: LatencyStory, cls: PairClass): string {
  const sideName = (side: 'a' | 'b') => scenarioName((side === 'a') === story.oriented ? story.fast : story.slow);
  switch (cls) {
    case 'identical':
      return 'Identical in both scenarios';
    case 'same_fills_different_history':
      return 'Same fills, different recorded history';
    case 'same_fills_different_fees':
      return 'Same fills, different fees';
    case 'same_outcome_different_timing':
      return 'Same outcome, different timing';
    case 'same_decision_different_outcome':
      return 'Same decision, different outcome';
    case 'different_decision':
      return 'Different decision';
    case 'only_a':
      return `Decided only in the ${sideName('a')} scenario`;
    case 'only_b':
      return `Decided only in the ${sideName('b')} scenario`;
  }
}

/** The fast/slow view of a pair's two sides. */
export function pairSides(story: LatencyStory, pair: OrderPair) {
  return story.oriented ? { fast: pair.a, slow: pair.b } : { fast: pair.b, slow: pair.a };
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

/** A compared field of an order pair in words (`exch_arrival_ts` → "exchange arrival time"). */
export function differenceWords(field: string): string {
  return DIFFERENCE_WORDS[field] ?? words(field.replace(/_ts$/, ' time'));
}

export const TIF_TEXT: Record<string, string> = {
  gtc: 'good till cancel',
  gtx: 'post-only',
  ioc: 'immediate or cancel',
  fok: 'fill or kill',
};

/**
 * The stored comparison's order pairs, read page by page only as far as a view needs them.
 * The comparison stores the non-identical pairs in decision order (`Alignment.pairs`), so once the
 * pages read reach past a decision time, an order decided before it with no stored pair is
 * identical in both scenarios. One instance per comparison is shared by every view of the page;
 * a page in flight is never requested twice.
 */
export class PairIndex {
  private readonly byOrder = new Map<number, OrderPair>();
  private cursor: string | null = null;
  private started = false;
  private inflight: Promise<void> | null = null;
  /** Every stored pair has been read. */
  done = false;
  /** Decision time of the last pair read; orders decided strictly before it are covered. */
  horizon = -1;

  constructor(
    private readonly comparisonId: string,
    /** The comparison kept only the earliest pairs (`pairs_truncated`). */
    readonly truncated: boolean,
  ) {}

  private static readonly PAGE = 100;

  private next(): Promise<void> {
    // Not tied to a caller's abort signal: a page that is read is kept for every later view.
    this.inflight ??= api
      .comparisonPairs(this.comparisonId, { cursor: this.cursor, limit: PairIndex.PAGE })
      .then((page) => {
        for (const p of page.items) {
          this.byOrder.set(p.order_id, p);
          this.horizon = Math.max(this.horizon, p.decision_ts);
        }
        this.started = true;
        this.cursor = page.next_cursor;
        if (!page.next_cursor) this.done = true;
      })
      .finally(() => {
        this.inflight = null;
      });
    return this.inflight;
  }

  /** Reads pages until every order decided at or before `ts` is covered (or the list ends). */
  async coverUpTo(ts: number, signal: AbortSignal): Promise<void> {
    while (!this.done && (!this.started || this.horizon <= ts)) {
      await this.next();
      if (signal.aborted) throw new DOMException('aborted', 'AbortError');
    }
  }

  /**
   * The comparison class of an order: its stored pair, `identical` when the pages read cover its
   * decision time without a pair, or `null` when that is not known (not read yet, or beyond the
   * stored pairs of a truncated comparison).
   */
  lookup(orderId: number, decisionTs: number): OrderPair | 'identical' | null {
    const pair = this.byOrder.get(orderId);
    if (pair) return pair;
    if (this.done) return this.truncated && decisionTs >= this.horizon ? null : 'identical';
    return decisionTs < this.horizon ? 'identical' : null;
  }
}

// ----- the replay window -------------------------------------------------------------------

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
  const add = (k: ScenarioKey, o: OrderRecord) => {
    const cur = byId.get(o.order_id) ?? { orderId: o.order_id, decisionTs: o.decision_ts, fast: null, slow: null };
    cur[k] = o;
    cur.decisionTs = Math.min(cur.decisionTs, o.decision_ts);
    byId.set(o.order_id, cur);
  };
  for (const o of m.fast.orders) add('fast', o);
  for (const o of m.slow.orders) add('slow', o);
  return [...byId.values()].sort((a, b) => a.decisionTs - b.decisionTs || a.orderId - b.orderId);
}

/** Orders resting at the simulated exchange at `t` (arrived, not yet finished, not refused). */
export function restingAt(tl: RunTimeline, t: number): OrderRecord[] {
  return tl.orders.filter(
    (o) => o.exch_arrival_ts !== null && o.exch_arrival_ts <= t && (o.terminal_ts === null || t < o.terminal_ts) && o.outcome !== 'refused',
  );
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
