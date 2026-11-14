// Data layer of the public latency experiment ("What changes when a market-making strategy
// receives and executes orders more slowly?"). Everything here is read from stored, verified
// results of the public demo workspace: the `cost_of_latency` demo's two runs and the stored
// comparison between them. Nothing is simulated or recomputed in the browser; the only
// arithmetic is the explicit P&L identity check in `pnlBreakdown`, which states its inputs.

import { api } from '../../api/client';
import type { Comparison, Demo, OrderDetail, RunDetail, RunTimeline } from '../../api/model';
import { fetchTimelineWindow } from '../terminal/timelinePaging';
import type { MetricRow, OrderPair, ReplayWindow } from '../../api/types';
import { baseEntryNs, baseResponseNs } from '../../api/types';

export const LATENCY_DEMO_ID = 'cost_of_latency';

export interface Scenario {
  runId: string;
  /** Demo label, e.g. "1 ms entry and response latency". */
  label: string;
  /** One-way order entry latency (location parameter), ns. */
  entryNs: number;
  /** Exchange-to-strategy response latency (location parameter), ns. */
  responseNs: number;
  run: RunDetail;
}

export interface LatencyStory {
  demo: Demo;
  /** The lower-latency scenario; comparison side `a` when the stored comparison is oriented so. */
  fast: Scenario;
  slow: Scenario;
  /** The stored comparison of the two runs, oriented with `fast` as `a`; `null` when none is stored. */
  comparison: Comparison | null;
  comparisonId: string | null;
  /** Whether the stored comparison is fast-vs-slow (`a` = fast). Metric deltas are `b - a`. */
  oriented: boolean;
}

export class StoryUnavailable extends Error {}

/** Loads the latency experiment from the public demo. Throws `StoryUnavailable` with a reason. */
export async function loadLatencyStory(signal: AbortSignal): Promise<LatencyStory> {
  const demos = await api.demos(undefined, { signal });
  const demo = demos.find((d) => d.id === LATENCY_DEMO_ID);
  if (!demo) throw new StoryUnavailable('The latency experiment is not published by this API.');
  const ids = demo.runs.map((r) => r.run_id);
  if (demo.runs.length !== 2 || ids.some((id) => !id)) {
    throw new StoryUnavailable('The latency experiment has no stored runs yet.');
  }
  const runs = await Promise.all(ids.map((id) => api.run(id as string, { signal })));
  const scenarios: Scenario[] = demo.runs.map((r, i) => ({
    runId: ids[i] as string,
    label: r.label,
    entryNs: baseEntryNs(r.config.execution.order_latency),
    responseNs: baseResponseNs(r.config.execution.order_latency),
    run: runs[i],
  }));
  scenarios.sort((x, y) => x.entryNs + x.responseNs - (y.entryNs + y.responseNs));
  const [fast, slow] = scenarios;

  const list = await api.comparisons({ signal });
  const stored = list.items.find(
    (c) => (c.run_a === fast.runId && c.run_b === slow.runId) || (c.run_a === slow.runId && c.run_b === fast.runId),
  );
  let comparison: Comparison | null = null;
  if (stored) comparison = await api.comparison(stored.id, { signal });
  return {
    demo,
    fast,
    slow,
    comparison,
    comparisonId: stored?.id ?? null,
    oriented: comparison === null || comparison.run_a === fast.runId,
  };
}

/** A comparison metric seen from the fast/slow orientation (`delta` = slow − fast). */
export interface ScenarioMetric {
  key: string;
  label: string;
  unit: string;
  fast: number | null;
  slow: number | null;
  delta: number | null;
}

export function scenarioMetrics(story: LatencyStory): ScenarioMetric[] {
  const rows: MetricRow[] = story.comparison?.metrics ?? [];
  return rows.map((m) => {
    const fast = story.oriented ? m.a : m.b;
    const slow = story.oriented ? m.b : m.a;
    return { key: m.key, label: m.label, unit: m.unit, fast, slow, delta: fast !== null && slow !== null ? slow - fast : null };
  });
}

export function metric(rows: ScenarioMetric[], key: string): ScenarioMetric | null {
  return rows.find((r) => r.key === key) ?? null;
}

/**
 * Net P&L split into its accounting parts for one scenario, read from the stored run metrics:
 * net = realized + unrealized − fees (docs/PNL_ACCOUNTING.md). `residual` is the difference
 * between the stored net P&L and that sum; a non-negligible residual means the identity does not
 * hold for this run and the split must not be shown as an explanation.
 */
export interface PnlBreakdown {
  realized: number;
  unrealized: number;
  fees: number;
  net: number;
  residual: number;
}

export function pnlBreakdown(run: RunDetail): PnlBreakdown {
  const m = run.metrics;
  const residual = m.net_pnl - (m.realized_pnl + m.unrealized_pnl - m.fees);
  return { realized: m.realized_pnl, unrealized: m.unrealized_pnl, fees: m.fees, net: m.net_pnl, residual };
}

/** The identity holds to within this many quote units (floating-point summation noise). */
export const PNL_IDENTITY_TOLERANCE = 1e-6;

/** The order pair where the two runs first differ, and the first pair whose decision or fills differ. */
export function divergences(story: LatencyStory): { first: OrderPair | null; firstOutcome: OrderPair | null } {
  const al = story.comparison?.alignment ?? null;
  return { first: al?.first_divergence ?? null, firstOutcome: al?.first_outcome_divergence ?? null };
}

/** Run id holding the order of a pair side (`a`/`b` are comparison sides, not fast/slow). */
export function pairRun(story: LatencyStory, side: 'a' | 'b'): Scenario {
  const isFast = (side === 'a') === story.oriented;
  return isFast ? story.fast : story.slow;
}

export interface MomentWindow {
  t0: number;
  t1: number;
}

/** A replay window around `t` (dataset-relative ns); never before the dataset origin. */
export function momentWindow(t: number, beforeNs = 250_000_000, afterNs = 750_000_000): MomentWindow {
  return { t0: Math.max(0, t - beforeNs), t1: t + afterNs };
}

export interface Moment {
  window: MomentWindow;
  /** Observed historical book and trades (identical for both scenarios: same dataset version). */
  market: ReplayWindow;
  fast: RunTimeline;
  slow: RunTimeline;
}

/**
 * The market and both runs' activity in a short window. Run timelines are loaded completely
 * (every cursor page, checked against `events_total`; `fetchTimelineWindow` throws otherwise).
 * `complete` is false only when the market replay's trade list was capped, and callers must say so
 * rather than present a partial window.
 */
export async function loadMoment(story: LatencyStory, w: MomentWindow, signal: AbortSignal): Promise<Moment & { complete: boolean }> {
  const ds = story.fast.run.dataset;
  const timeline = (runId: string) =>
    fetchTimelineWindow(w, (cursor, s) => api.timeline(runId, { t0: w.t0, t1: w.t1, max_events: 5000, cursor }, { signal: s }), signal);
  const [market, fast, slow] = await Promise.all([
    // Six levels a side: what the page's book shows; more depth would only cost the visitor bytes.
    api.replay(ds.id, { t0: w.t0, t1: w.t1, levels: 6, max_trades: 5000 }, { signal, version: ds.version }),
    timeline(story.fast.runId),
    timeline(story.slow.runId),
  ]);
  return { window: w, market, fast, slow, complete: !market.trades_truncated };
}

/** One order's full lifecycle in both scenarios (same order id = same decision slot). */
export async function loadOrderPair(
  story: LatencyStory,
  pair: OrderPair,
  signal: AbortSignal,
): Promise<{ fast: OrderDetail | null; slow: OrderDetail | null }> {
  const get = async (side: 'a' | 'b') => {
    if (pair[side] === null) return null;
    return api.orderDetail(pairRun(story, side).runId, pair.order_id, { signal });
  };
  const [a, b] = await Promise.all([get('a'), get('b')]);
  return story.oriented ? { fast: a, slow: b } : { fast: b, slow: a };
}
