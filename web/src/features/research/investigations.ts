// Pure derivations behind Research home (`/research`). Every number an investigation shows is
// read or computed here from one runtime source: the verified study result (`features/study`
// loader) or one Market Data Workbench event detail (`GET /market/datasets/l3-demo/events/{i}`).
// A source that does not have the expected shape yields a `SourceProblem` with the exact reason;
// nothing falls back to a constant. Identity and lineage rules: docs/engineering/RESEARCH_IDENTITY.md.

import type { BookOrder, EventDetail } from '../market/api';
import type { Aggregate, AggregateMetric, AnchorRun, StudyCell, StudyResult } from '../study/contract';
import { latencyLabel, queueLabel, regimeLabel } from '../study/model';

/** A source answered, but not with what the investigation reads. Displayed verbatim. */
export class SourceProblem extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SourceProblem';
  }
}

const problem = (message: string): never => {
  throw new SourceProblem(message);
};

// ----- (a) L2 hides queue information: the workbench's curated cancel-ahead example ----------

/** The workbench demo dataset (synthetic ITCH 5.0 messages, `docs/research/MARKET_DATA_WORKBENCH.md`). */
export const WORKBENCH_DATASET = 'l3-demo';
export const CANCEL_AHEAD_EXAMPLE = 'cancel-ahead';

/**
 * The CLI command that regenerates the workbench source bytes, from the generator settings the
 * dataset summary declares; `null` when they are not the fault-free generator settings it knows.
 */
export function workbenchCommand(generator: unknown, sourceSha256: string): string | null {
  if (typeof generator !== 'object' || generator === null) return null;
  if (!('seed' in generator) || !('events' in generator) || !('symbol' in generator)) return null;
  const { seed, events, symbol } = generator;
  const faults = ('inject_unknown_execute' in generator && generator.inject_unknown_execute) || ('inject_duplicate_add' in generator && generator.inject_duplicate_add);
  if (faults || !/^\d+$/.test(String(seed)) || typeof events !== 'number' || typeof symbol !== 'string' || !/^[A-Z0-9]{1,8}$/.test(symbol)) return null;
  return [
    `blackbox market l3-synth --seed ${seed} --events ${events} --symbol ${symbol} --out data/workbench-synthetic`,
    `# expect source SHA-256 ${sourceSha256} in the written provenance`,
  ].join('\n');
}

export interface QueueShift {
  order_ref: string;
  before: { position: number; shares_ahead: number };
  after: { position: number; shares_ahead: number };
}

export interface CancelAhead {
  eventIndex: number;
  side: 'buy' | 'sell';
  price4: number;
  /** The deleted order. */
  orderRef: string;
  deletedShares: number;
  /** 1-based FIFO position of the deleted order and the level's order count, before the event. */
  position: number;
  levelOrders: number;
  /** Shares of the orders ahead of the deleted one (unchanged by the delete). */
  sharesAheadOfDeleted: number;
  /** What a price-level (L2) feed shows: the level total before and after. */
  l2: { before: number; after: number; delta: number; ordersBefore: number; ordersAfter: number };
  /** Orders ahead of / behind the deleted one, from its position. */
  ordersAhead: number;
  ordersBehind: number;
  /** The order immediately behind the deleted one, when the response lists it in both instants. */
  nextBehind: QueueShift | null;
  /** Listed orders behind the deleted one, matched before and after. */
  behindListed: number;
  /** Every matched order behind moved up one position and `deletedShares` shares. */
  behindConsistent: boolean;
  /** Every listed order ahead kept its position and shares ahead. */
  aheadUnchanged: boolean;
}

/**
 * The L2 and L3 views of the cancel-ahead event. L2: the changed level's total before and after.
 * L3: the deleted order's FIFO position, and per listed order behind it the change of its
 * position and shares ahead (after minus before, matched by order reference).
 */
export function cancelAhead(d: EventDetail): CancelAhead {
  const e = d.event;
  if (e.message_type !== 'D' || e.applied !== 'book') {
    problem(`Event #${e.event_index} is ${e.message_type} (${e.applied}), not an accepted D (delete); the curated example no longer matches its rule.`);
  }
  const ob = d.order_before ?? problem(`Event #${e.event_index} carries no position of the deleted order.`);
  const level =
    d.affected.find((a) => a.side === ob.side && a.price4 === ob.price4) ??
    problem(`Event #${e.event_index} lists no changed level at the deleted order's price.`);
  const after = new Map(level.after.orders.map((o) => [o.order_ref, o]));
  let behindListed = 0;
  let behindConsistent = true;
  let aheadUnchanged = true;
  let nextBehind: QueueShift | null = null;
  for (const b of level.before.orders) {
    if (b.order_ref === ob.order_ref) continue;
    const a: BookOrder | undefined = after.get(b.order_ref);
    if (!a) continue;
    if (b.queue_position < ob.queue_position) {
      if (a.queue_position !== b.queue_position || a.shares_ahead !== b.shares_ahead) aheadUnchanged = false;
      continue;
    }
    behindListed += 1;
    if (a.queue_position !== b.queue_position - 1 || a.shares_ahead !== b.shares_ahead - ob.shares) behindConsistent = false;
    if (b.queue_position === ob.queue_position + 1) {
      nextBehind = {
        order_ref: b.order_ref,
        before: { position: b.queue_position, shares_ahead: b.shares_ahead },
        after: { position: a.queue_position, shares_ahead: a.shares_ahead },
      };
    }
  }
  return {
    eventIndex: e.event_index,
    side: ob.side,
    price4: ob.price4,
    orderRef: ob.order_ref,
    deletedShares: ob.shares,
    position: ob.queue_position,
    levelOrders: ob.level_orders,
    sharesAheadOfDeleted: ob.shares_ahead,
    l2: {
      before: level.before.shares,
      after: level.after.shares,
      delta: level.after.shares - level.before.shares,
      ordersBefore: level.before.order_count,
      ordersAfter: level.after.order_count,
    },
    ordersAhead: ob.queue_position - 1,
    ordersBehind: ob.level_orders - ob.queue_position,
    nextBehind,
    behindListed,
    behindConsistent,
    aheadUnchanged,
  };
}

// ----- study helpers --------------------------------------------------------------------------

/** A metric's value as the study reports it: the deterministic value, or the seed mean. */
export interface Value {
  value: number;
  /** 95 % percentile-bootstrap interval over latency seeds; `null` for a deterministic run. */
  interval: [number, number] | null;
  /** Runs behind the value. */
  n: number;
}

export function valueOf(m: AggregateMetric | undefined): Value | null {
  if (!m) return null;
  if (m.kind === 'deterministic_single') return { value: m.value, interval: null, n: 1 };
  return m.bootstrap ? { value: m.bootstrap.mean, interval: [m.bootstrap.lower_95, m.bootstrap.upper_95], n: m.bootstrap.n } : null;
}

const isBaselineGroup = (s: StudyResult) => (a: Aggregate) =>
  a.regime === s.baseline.regime && a.queue_model === s.baseline.queue_model && a.latency === s.baseline.latency && a.feed_offset_ns === s.baseline.feed_offset_ns;

function baselineGroup(s: StudyResult): Aggregate {
  return s.aggregates.find(isBaselineGroup(s)) ?? problem(`The study lists no aggregate group for its baseline (${s.baseline.regime}, ${s.baseline.queue_model}, ${s.baseline.latency}).`);
}

function need(v: Value | null, what: string): Value {
  return v ?? problem(`The study has no value for ${what}.`);
}

/** What one aggregate group changes against the baseline, in words. */
export function changedAssumption(s: StudyResult, a: Aggregate): string {
  const parts: string[] = [];
  if (a.queue_model !== s.baseline.queue_model) parts.push(`Queue model: ${queueLabel(a.queue_model)}`);
  if (a.latency !== s.baseline.latency) parts.push(`Order latency: ${latencyLabel(a.latency)}`);
  if (a.feed_offset_ns !== s.baseline.feed_offset_ns) parts.push(`Feed latency offset +${a.feed_offset_ns / 1e6} ms`);
  if (a.regime !== s.baseline.regime) parts.push(`Regime: ${regimeLabel(a.regime)}`);
  return parts.length ? parts.join('; ') : 'Baseline';
}

/** The study's single baseline run (the regime-baseline cell whose run id the baseline names). */
export function baselineCell(s: StudyResult): StudyCell {
  return s.cells.find((c) => c.run_id === s.baseline.run_id) ?? problem(`The study lists no cell for its baseline run ${s.baseline.run_id}.`);
}

/** Whether `gross − fees = net` holds to floating-point rounding. */
export function identityHolds(gross: number, fees: number, net: number): boolean {
  return Math.abs(gross - fees - net) <= 1e-9 * Math.max(1, Math.abs(gross), Math.abs(fees), Math.abs(net));
}

// ----- (b) fees: gross vs fees vs net ----------------------------------------------------------

export interface FeeRow {
  regime: string;
  runId: string;
  gross: number;
  fees: number;
  net: number;
}

export interface FeeEffect {
  /** The baseline run's decomposition. */
  baseline: FeeRow & { fills: number; datasetId: string; datasetSha: string };
  /** Maker / taker fee rates of the baseline configuration (fractions of notional), when stated. */
  feeRates: { maker: number; taker: number } | null;
  /** Every regime-baseline run; `feesExceedGross` counts those where fees > |gross|. */
  regimes: FeeRow[];
  feesExceedGross: number;
}

/** `execution.fees` of a run configuration when both rates are numbers (the contract keeps the config opaque). */
function feeRatesOf(config: Record<string, unknown>): { maker: number; taker: number } | null {
  const execution = config.execution;
  if (typeof execution !== 'object' || execution === null || !('fees' in execution)) return null;
  const fees = execution.fees;
  if (typeof fees !== 'object' || fees === null || !('maker' in fees) || !('taker' in fees)) return null;
  return typeof fees.maker === 'number' && typeof fees.taker === 'number' ? { maker: fees.maker, taker: fees.taker } : null;
}

export function feeEffect(s: StudyResult): FeeEffect {
  const cell = baselineCell(s);
  const acc = cell.accounting;
  // The baseline cell is the baseline regime's regime-baseline run, so this checks it too.
  const rows: FeeRow[] = s.cells
    .filter((c) => c.design === 'regime_baseline')
    .map((c) => ({ regime: c.regime, runId: c.run_id, gross: c.accounting.gross_pnl, fees: c.accounting.fees, net: c.accounting.net_pnl }));
  for (const r of rows) {
    if (!identityHolds(r.gross, r.fees, r.net)) {
      problem(`Run ${r.runId}: gross ${r.gross} − fees ${r.fees} ≠ net ${r.net}; the decomposition is inconsistent.`);
    }
  }
  const feeRates = feeRatesOf(s.baseline.config);
  return {
    baseline: {
      regime: cell.regime,
      runId: cell.run_id,
      gross: acc.gross_pnl,
      fees: acc.fees,
      net: acc.net_pnl,
      fills: cell.measures.fills ?? problem(`The baseline run ${cell.run_id} reports no fill count.`),
      datasetId: cell.dataset.id,
      datasetSha: cell.dataset.sha256,
    },
    feeRates,
    regimes: rows,
    feesExceedGross: rows.filter((r) => r.fees > Math.abs(r.gross)).length,
  };
}

// ----- (c) late orders: latency levels on the baseline regime ------------------------------------

export interface LatencyRow {
  /** `?cell=` group key parts. */
  group: Aggregate;
  label: string;
  fillRate: Value;
  net: Value;
  fees: Value;
  reference: boolean;
}

export interface LatencyEffect {
  rows: LatencyRow[];
  /** The deterministic "entry 10 ms" level, the plainest late-arrival contrast. */
  slowEntry: LatencyRow | null;
  /** The feed delay (+offset) group. */
  feedDelay: LatencyRow | null;
  reference: LatencyRow;
}

/**
 * Every latency level of the study, plus the feed-offset group, with the baseline queue model on
 * the baseline regime (the study's one-factor latency design), in the study's level order.
 */
export function latencyEffect(s: StudyResult): LatencyEffect {
  const order = s.latency_models.map((m) => m.name);
  const groups = s.aggregates.filter((a) => a.regime === s.baseline.regime && a.queue_model === s.baseline.queue_model);
  const pos = (a: Aggregate) => (a.feed_offset_ns !== s.baseline.feed_offset_ns ? order.length : order.indexOf(a.latency));
  const rows = groups
    .filter((a) => pos(a) >= 0)
    .sort((x, y) => pos(x) - pos(y))
    .map((a) => {
      const label = a.feed_offset_ns !== s.baseline.feed_offset_ns ? `Feed latency offset +${a.feed_offset_ns / 1e6} ms` : latencyLabel(a.latency);
      return {
        group: a,
        label,
        fillRate: need(valueOf(a.metrics.fill_rate), `fill rate of ${label}`),
        net: need(valueOf(a.metrics.net_pnl), `net P&L of ${label}`),
        fees: need(valueOf(a.metrics.fees), `fees of ${label}`),
        reference: isBaselineGroup(s)(a),
      };
    });
  const reference = rows.find((r) => r.reference) ?? problem('The latency comparison has no baseline row.');
  if (rows.length < 2) problem('The study has no latency level besides the baseline.');
  return {
    rows,
    slowEntry: rows.find((r) => r.group.latency === 'entry_slow' && r.group.feed_offset_ns === s.baseline.feed_offset_ns) ?? null,
    feedDelay: rows.find((r) => r.group.feed_offset_ns !== s.baseline.feed_offset_ns) ?? null,
    reference,
  };
}

// ----- (d) liquidity shock: regime baselines ----------------------------------------------------

export interface RegimeRow {
  regime: string;
  label: string;
  runId: string;
  fillRate: number;
  fills: number;
  net: number;
  reference: boolean;
  /** One of the regimes this question is about (liquidity depletion, widening spreads). */
  shock: boolean;
}

export const SHOCK_REGIMES = ['liquidity_depletion', 'widening_spreads'] as const;

/** A curated demo anchor pair: the normal-liquidity anchor and one shock-regime anchor. */
export interface AnchorPair {
  regime: string;
  reference: AnchorRun;
  shocked: AnchorRun;
}

export interface RegimeEffect {
  rows: RegimeRow[];
  reference: RegimeRow;
  /** Anchor pairs whose runs differ only in the regime of their dataset (checked from fields). */
  anchorPairs: AnchorPair[];
}

export function regimeEffect(s: StudyResult): RegimeEffect {
  const rows: RegimeRow[] = s.cells
    .filter((c) => c.design === 'regime_baseline')
    .map((c) => ({
      regime: c.regime,
      label: regimeLabel(c.regime),
      runId: c.run_id,
      fillRate: c.measures.fill_rate ?? problem(`Regime baseline ${c.run_id} reports no fill rate.`),
      fills: c.measures.fills ?? problem(`Regime baseline ${c.run_id} reports no fill count.`),
      net: c.accounting.net_pnl,
      reference: c.regime === s.baseline.regime,
      shock: (SHOCK_REGIMES as readonly string[]).includes(c.regime),
    }));
  const reference = rows.find((r) => r.reference) ?? problem(`No regime-baseline run on ${s.baseline.regime}.`);
  if (!rows.some((r) => r.shock)) problem('The study has no liquidity-depletion or widening-spreads baseline.');
  const regimeOfDataset = new Map(s.demo_anchors.datasets.map((d) => [d.id, d.regime]));
  const anchors = s.demo_anchors.runs;
  const anchorPairs: AnchorPair[] = [];
  for (const [i, j] of s.demo_anchors.pairs) {
    const a = anchors.find((r) => r.index === i);
    const b = anchors.find((r) => r.index === j);
    if (!a || !b || a.queue_model !== b.queue_model || a.latency !== b.latency || a.feed_offset_ns !== b.feed_offset_ns) continue;
    const ra = regimeOfDataset.get(a.dataset.id);
    const rb = regimeOfDataset.get(b.dataset.id);
    if (ra === null && typeof rb === 'string' && (SHOCK_REGIMES as readonly string[]).includes(rb)) anchorPairs.push({ regime: rb, reference: a, shocked: b });
  }
  return { rows, reference, anchorPairs };
}

// ----- (e) which assumption changes the result most ------------------------------------------

export interface RankedAssumption {
  group: Aggregate;
  label: string;
  value: Value;
  delta: number;
  /** Fill-rate change of the same group, for reading the P&L change. */
  fillRateDelta: number | null;
}

export interface SensitivityRanking {
  metric: string;
  baseline: Value;
  ranked: RankedAssumption[];
  /** One-factor groups left out because the metric has no value (fewer than two seed values). */
  excluded: string[];
  /** Ranked assumptions whose change is zero up to floating-point rounding (1e-9 relative). */
  unchanged: string[];
}

/**
 * The computation: on the baseline regime, every `one_factor` group (exactly one execution
 * assumption differs from the baseline: queue model, order-latency level or feed offset) is
 * compared with the baseline group on `metric`; the value is the deterministic result or, for a
 * stochastic latency level, the seed mean. Rank by |value − baseline| descending; ties keep the
 * study's order. Regimes (different events) and factorial groups (two changes) are not ranked.
 */
export function sensitivityRanking(s: StudyResult, metric = 'net_pnl'): SensitivityRanking {
  const base = baselineGroup(s);
  const baseline = need(valueOf(base.metrics[metric]), `${metric} of the baseline group`);
  const baseFill = valueOf(base.metrics.fill_rate);
  const excluded: string[] = [];
  const ranked: RankedAssumption[] = [];
  for (const g of s.aggregates) {
    if (g.design !== 'one_factor' || g.regime !== s.baseline.regime) continue;
    const label = changedAssumption(s, g);
    const v = valueOf(g.metrics[metric]);
    if (!v) {
      excluded.push(label);
      continue;
    }
    const fill = valueOf(g.metrics.fill_rate);
    ranked.push({ group: g, label, value: v, delta: v.value - baseline.value, fillRateDelta: fill && baseFill ? fill.value - baseFill.value : null });
  }
  if (ranked.length === 0) problem(`The study has no one-factor group with a ${metric} value.`);
  ranked.sort((a, b) => Math.abs(b.delta) - Math.abs(a.delta));
  const tolerance = 1e-9 * Math.max(1, Math.abs(baseline.value));
  const unchanged = ranked.filter((r) => Math.abs(r.delta) <= tolerance).map((r) => r.label);
  return { metric, baseline, ranked, excluded, unchanged };
}
