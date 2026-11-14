// Prototype B (research workspace): loaders and plain-language presentation of the stored latency
// experiment. Every number shown comes from `story.ts` loaders or the API; this file only chooses
// wording, units and formatting, and states the arithmetic it does (the P&L identity split).

import { api, ApiError } from '../../../api/client';
import type { EquityPage, OrderDetail } from '../../../api/model';
import type { DatasetManifest, OrderPair, PairClass } from '../../../api/types';
import { fmtDuration, fmtDurationExact, fmtInt, fmtNum, fmtPct, fmtPp, fmtSigned } from '../../../components/fmt';
import { fmtSignedDuration } from '../../forensics/labels';
import {
  loadLatencyStory,
  pnlBreakdown,
  PNL_IDENTITY_TOLERANCE,
  type LatencyStory,
  type PnlBreakdown,
  type Scenario,
  type ScenarioMetric,
} from '../story';

export type ScenarioKey = 'fast' | 'slow';
export type Focus = ScenarioKey | 'both';

export interface Workspace {
  story: LatencyStory;
  /** Manifest of the exact dataset version both runs replayed. */
  dataset: DatasetManifest;
}

export async function loadWorkspace(signal: AbortSignal): Promise<Workspace> {
  const story = await loadLatencyStory(signal);
  const ds = story.fast.run.dataset;
  const version = await api.datasetVersion(ds.id, ds.version, { signal });
  if (!version.manifest) throw new Error(`Dataset ${ds.id} version ${ds.version} has no manifest.`);
  return { story, dataset: version.manifest };
}

/** Both runs' equity over the whole run, at most 1,000 points (or buckets) each. */
export async function loadEquity(story: LatencyStory, signal: AbortSignal): Promise<Record<ScenarioKey, EquityPage>> {
  const [fast, slow] = await Promise.all([
    api.equity(story.fast.runId, { max_points: 1000 }, { signal }),
    api.equity(story.slow.runId, { max_points: 1000 }, { signal }),
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
  return fmtDurationExact(s.entryNs);
}

// ----- answer summary ----------------------------------------------------------------------

function result(net: number): string {
  if (net < 0) return `lost ${fmtNum(-net, 2)}`;
  if (net > 0) return `made ${fmtNum(net, 2)}`;
  return 'broke even';
}

export interface AnswerSummary {
  /** The header line: net results, fees and realized P&L, and what the gap consists of. */
  oneLine: string;
  /** Prose form of the net results. */
  headline: string;
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
  let oneLine =
    `${lead}Net result after fees: ${nf} ${fmtSigned(f.net, 2)}, ${ns} ${fmtSigned(s.net, 2)}. ` +
    `Fees ${fmtNum(f.fees, 2)} vs ${fmtNum(s.fees, 2)}; realized trading P&L ${fmtSigned(f.realized, 2)} vs ${fmtSigned(s.realized, 2)}.`;

  const identityHolds = Math.abs(f.residual) <= PNL_IDENTITY_TOLERANCE && Math.abs(s.residual) <= PNL_IDENTITY_TOLERANCE;
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
      oneLine += ` ${better} ${verb}; ${fmtPct(share)} of that is ${top.what}.`;
      split = `${better} ${verb}. ${fmtPct(share)} of that is ${top.what}: ${top.detail}. Otherwise, ${rest.join('; ')}.`;
    } else {
      oneLine += ` ${better} ${verb}, with no single dominant part.`;
      split = `${better} ${verb}, with no single dominant part: ${parts.map((p) => p.detail).join('; ')}.`;
    }
  }
  return { oneLine, headline, split, fast: f, slow: s, identityHolds };
}

// ----- metrics table -----------------------------------------------------------------------

export interface MetricPresentation {
  label: string;
  /** Unit shown in the table, or '' when the value carries it (durations, percentages). */
  unit: string;
  group: string;
  primary: boolean;
  value: (v: number | null) => string;
  delta: (d: number | null) => string;
}

const money = (v: number | null) => fmtNum(v, 2);
const signedMoney = (v: number | null) => fmtSigned(v, 2);

const PLAIN: Record<string, Partial<MetricPresentation>> = {
  net_pnl: { label: 'Net result after fees', group: 'Money', primary: true, value: signedMoney },
  realized_pnl: { label: 'Realized trading P&L', group: 'Money', primary: true, value: signedMoney },
  unrealized_pnl: { label: 'Unrealized P&L on the open position at the end', group: 'Money', primary: true, value: signedMoney },
  fees: { label: 'Fees paid', group: 'Money', primary: true, value: money },
  max_drawdown: { label: 'Largest fall from a previous high', group: 'Money', value: money },
  turnover: { label: 'Traded value', group: 'Activity', value: money },
  orders_submitted: { label: 'Orders sent', group: 'Activity', primary: true },
  fill_count: { label: 'Fills', group: 'Activity', primary: true },
  fill_ratio_orders: { label: 'Share of orders that filled', group: 'Activity', primary: true },
  fill_ratio_qty: { label: 'Share of order quantity that filled', group: 'Activity' },
  volume: { label: 'Quantity traded', group: 'Activity' },
  cancels_sent: { label: 'Cancel requests sent', group: 'Activity', primary: true },
  cancels_rejected: { label: 'Cancel requests the exchange rejected', group: 'Activity', primary: true },
  risk_blocks: { label: 'Orders blocked by risk limits', group: 'Activity' },
  entry_latency_ns: { label: 'Order travel time to the exchange (median)', group: 'Timing', primary: true },
  round_trip_ns: { label: 'Round trip to the exchange and back (median)', group: 'Timing', primary: true },
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
    primary: p.primary ?? false,
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

export const TIF_TEXT: Record<string, string> = {
  gtc: 'good till cancel',
  gtx: 'post-only',
  ioc: 'immediate or cancel',
  fok: 'fill or kill',
};
