// The study's focused views (B2) and its uncertainty breakdown (B4). A view is built only when the
// fields it draws exist and agree with each other; otherwise it says why it is omitted, and the
// page prints that reason instead of a chart. Pure functions, no React.

import { groupCellKey, runCellKey } from './cellKey';
import type { Aggregate, StudyCell, StudyResult } from './contract';
import { labelGroup, referenceGroup, type ComparisonLabel } from './explore';
import { latencyLabel, metricDef, metricPoint, queueLabel, regimeLabel, type MetricPoint } from './model';

export type View<T> = { ok: true; data: T } | { ok: false; reason: string };

const omit = <T>(reason: string): View<T> => ({ ok: false, reason });

function members(study: StudyResult, g: Aggregate): StudyCell[] | null {
  const out = g.members.map((id) => study.cells.find((c) => c.run_id === id));
  return out.every((c): c is StudyCell => c !== undefined) ? out : null;
}

const isBaselineLevels = (study: StudyResult, g: Aggregate) =>
  g.queue_model === study.baseline.queue_model && g.latency === study.baseline.latency && g.feed_offset_ns === study.baseline.feed_offset_ns;

/** Relative tolerance for checking a published mean against its seed values. */
const TOL = 1e-9;

// ---- seed variation strip (B2) ---------------------------------------------------------------

export interface SeedPoint {
  runId: string;
  key: string;
  seed: number;
  value: number;
}

export interface SeedStrip {
  key: string;
  label: string;
  points: SeedPoint[];
  mean: number;
  interval: [number, number];
}

export interface SeedStrips {
  strips: SeedStrip[];
  /** Stochastic groups left out, with the reason. */
  omitted: { label: string; reason: string }[];
}

/**
 * Every seed value of each stochastic group for `metric`, checked against the published bootstrap:
 * the members exist and carry the measure, seeds are distinct study seeds, their count is the
 * bootstrap `n`, their mean is the bootstrap mean, and the interval lies within the seed values.
 */
export function seedStrip(study: StudyResult, g: Aggregate, metric: string): View<SeedStrip> {
  const m = g.metrics[metric];
  if (!m || m.kind !== 'stochastic_seed_mean') return omit('the group has no seed mean for this measure');
  if (!m.bootstrap) return omit('fewer than two seed values, no bootstrap');
  const runs = members(study, g);
  if (!runs) return omit('a member run is missing from the artefact');
  const points: SeedPoint[] = [];
  for (const r of runs) {
    const v = r.measures[metric];
    if (typeof v !== 'number' || !Number.isFinite(v)) return omit(`run ${r.run_id} has no ${metric}`);
    points.push({ runId: r.run_id, key: runCellKey(r.run_id), seed: r.seed, value: v });
  }
  const seeds = new Set(points.map((p) => p.seed));
  if (seeds.size !== points.length) return omit('two member runs share a seed');
  if (points.some((p) => !study.design.latency_seeds.includes(p.seed))) return omit('a member seed is not one of the study latency seeds');
  const b = m.bootstrap;
  if (b.n !== points.length) return omit(`the bootstrap counts ${b.n} seeds, the group has ${points.length} runs`);
  const mean = points.reduce((s, p) => s + p.value, 0) / points.length;
  if (Math.abs(mean - b.mean) > TOL * Math.max(1, Math.abs(mean))) return omit('the published mean is not the mean of the seed values');
  const lo = Math.min(...points.map((p) => p.value));
  const hi = Math.max(...points.map((p) => p.value));
  const slack = TOL * Math.max(1, Math.abs(lo), Math.abs(hi));
  if (!(b.lower_95 <= b.mean + slack && b.mean <= b.upper_95 + slack && lo <= b.lower_95 + slack && b.upper_95 <= hi + slack)) {
    return omit('the published interval is not inside the range of the seed values');
  }
  return {
    ok: true,
    data: {
      key: groupCellKey(g),
      label: `${regimeLabel(g.regime)} \u00b7 ${queueLabel(g.queue_model)} \u00b7 ${latencyLabel(g.latency)}`,
      points: points.sort((x, y) => x.seed - y.seed),
      mean: b.mean,
      interval: [b.lower_95, b.upper_95],
    },
  };
}

export function seedStrips(study: StudyResult, metric: string): View<SeedStrips> {
  const stochastic = study.aggregates.filter((g) => g.members.length > 1);
  if (stochastic.length === 0) return omit('the study has no stochastic multi-seed group');
  const out: SeedStrips = { strips: [], omitted: [] };
  for (const g of stochastic) {
    const v = seedStrip(study, g, metric);
    if (v.ok) out.strips.push(v.data);
    else out.omitted.push({ label: `${regimeLabel(g.regime)} \u00b7 ${queueLabel(g.queue_model)} \u00b7 ${latencyLabel(g.latency)}`, reason: v.reason });
  }
  if (out.strips.length === 0) return omit(`no stochastic group passes the checks (${out.omitted[0]?.reason ?? 'none'})`);
  return { ok: true, data: out };
}

// ---- paired one-factor deltas: cancel delay, feed delay (B2) --------------------------------

export interface DeltaRow {
  metric: string;
  label: string;
  fmt: (v: number) => string;
  reference: MetricPoint;
  changed: MetricPoint;
  delta: number;
}

export interface PairedDelta {
  key: string;
  referenceKey: string;
  title: string;
  /** The changed assumption, in words (from the artefact config). */
  change: string;
  label: ComparisonLabel;
  rows: DeltaRow[];
  /** Mean cancelled orders per run (order counts of the member runs). */
  canceled: { reference: number; changed: number };
}

const DELTA_METRICS = ['fill_rate', 'fills', 'unfilled_orders', 'net_pnl', 'fees', 'time_to_fill_p50_ns', 'mean_abs_position', 'modeled_queue_at_entry_mean'];

/**
 * One group against its reference on the same dataset, measure by measure. Requires every member
 * to be paired with the reference run (aligned, like-for-like, equal dataset hash), the changed
 * configuration to touch only `allowed` factors (config path prefixes), consistent order counts,
 * and each measure present on both sides.
 */
export function pairedDelta(study: StudyResult, g: Aggregate, title: string, change: string, allowed: (path: string) => boolean): View<PairedDelta> {
  const ref = referenceGroup(study, g);
  if (!ref) return omit('no reference group in the artefact');
  const runs = members(study, g);
  const refRuns = members(study, ref);
  if (!runs || !refRuns || refRuns.length !== 1) return omit('a member or reference run is missing from the artefact');
  const refRun = refRuns[0];
  for (const r of runs) {
    const c = r.comparison;
    if (!c || c.reference_run_id !== refRun.run_id) return omit(`run ${r.run_id} is not compared with the reference run ${refRun.run_id}`);
    if (!c.aligned || !c.like_for_like || r.dataset.sha256 !== refRun.dataset.sha256) return omit(`run ${r.run_id} is not paired like-for-like with the reference`);
    const stray = c.config_diffs.find((d) => !allowed(d.path) && !(d.path === 'seed' && g.members.length > 1));
    if (stray) return omit(`run ${r.run_id} also changes ${stray.path}`);
    if (c.config_diffs.length === 0) return omit(`run ${r.run_id} changes nothing against the reference`);
  }
  for (const r of [...runs, refRun]) {
    const o = r.orders;
    if (o.filled + o.canceled > o.submitted) return omit(`run ${r.run_id} reports more filled plus cancelled orders than submitted`);
  }
  const rows: DeltaRow[] = [];
  for (const metric of DELTA_METRICS) {
    const a = metricPoint(ref.metrics[metric]);
    const b = metricPoint(g.metrics[metric]);
    if (!a || !b) continue;
    const def = metricDef(metric);
    rows.push({ metric, label: def.label, fmt: def.fmt, reference: a, changed: b, delta: b.value - a.value });
  }
  if (rows.length === 0) return omit('neither side carries the measures this view compares');
  const label = labelGroup(study, g);
  if (!label) return omit('the group is the reference itself');
  const meanCanceled = (rs: StudyCell[]) => rs.reduce((s, r) => s + r.orders.canceled, 0) / rs.length;
  return {
    ok: true,
    data: { key: groupCellKey(g), referenceKey: groupCellKey(ref), title, change, label, rows, canceled: { reference: refRun.orders.canceled, changed: meanCanceled(runs) } },
  };
}

/** The cancel-entry latency of a latency configuration in ns, when it has its own leg. */
function cancelLegNs(config: Record<string, unknown>): number | null {
  const c = config.cancel;
  if (typeof c !== 'object' || c === null || !('median_ns' in c)) return null;
  return typeof c.median_ns === 'number' ? c.median_ns : null;
}

/** Cancel delay: the latency levels with their own cancel leg, against the baseline, baseline regime. */
export function cancelDelayViews(study: StudyResult): View<PairedDelta[]> {
  const levels = study.latency_models.filter((m) => cancelLegNs(m.config) !== null);
  if (levels.length === 0) return omit('no latency level of this study has a separate cancel-entry latency');
  const out: PairedDelta[] = [];
  for (const level of levels) {
    const g = study.aggregates.find(
      (a) => a.regime === study.baseline.regime && a.queue_model === study.baseline.queue_model && a.latency === level.name && a.feed_offset_ns === study.baseline.feed_offset_ns,
    );
    if (!g) return omit(`no group runs the ${level.name} level on the baseline regime and queue model`);
    const v = pairedDelta(study, g, latencyLabel(level.name), `cancel entry ${(cancelLegNs(level.config) ?? 0) / 1e6} ms`, (p) => p.startsWith('execution.order_latency'));
    if (!v.ok) return v;
    out.push(v.data);
  }
  return { ok: true, data: out };
}

/** Feed delay: the feed-offset groups against the baseline with every other level at baseline. */
export function feedDelayViews(study: StudyResult): View<PairedDelta[]> {
  const b = study.baseline;
  const groups = study.aggregates.filter((a) => a.regime === b.regime && a.queue_model === b.queue_model && a.latency === b.latency && a.feed_offset_ns !== b.feed_offset_ns);
  if (groups.length === 0) return omit('no group changes the feed latency offset alone');
  const out: PairedDelta[] = [];
  for (const g of groups) {
    const runs = members(study, g);
    const declared = runs?.every((r) => r.comparison?.config_diffs.some((d) => d.path === 'execution.feed_latency_offset_ns' && d.b === g.feed_offset_ns));
    if (!declared) return omit(`the configuration diff of the +${g.feed_offset_ns / 1e6} ms group does not carry its feed offset`);
    const v = pairedDelta(study, g, `Feed latency offset +${g.feed_offset_ns / 1e6} ms`, `every local feed receipt ${g.feed_offset_ns / 1e6} ms later`, (p) => p === 'execution.feed_latency_offset_ns');
    if (!v.ok) return v;
    out.push(v.data);
  }
  return { ok: true, data: out };
}

// ---- inventory impact (B2) -------------------------------------------------------------------

export interface InventoryRow {
  key: string;
  group: Aggregate;
  label: string;
  reference: boolean;
  mean: MetricPoint;
  /** Largest absolute inventory over the group's runs. */
  max: number;
  runs: number;
}

export interface InventoryView {
  rows: InventoryRow[];
  /** The baseline risk limit on absolute position, when the configuration states one. */
  limit: number | null;
}

/**
 * Mean and maximum absolute inventory of every baseline-regime group that changes one assumption.
 * Checks that every member run carries both measures, that they are non-negative and finite, and
 * that no run's mean exceeds its maximum.
 */
export function inventoryView(study: StudyResult): View<InventoryView> {
  const groups = study.aggregates.filter((a) => a.regime === study.baseline.regime && a.design !== 'factorial');
  if (groups.length === 0) return omit('no baseline-regime groups');
  const rows: InventoryRow[] = [];
  for (const g of groups) {
    const runs = members(study, g);
    if (!runs) return omit('a member run is missing from the artefact');
    for (const r of runs) {
      const mean = r.measures.mean_abs_position;
      const max = r.measures.max_abs_position;
      if (typeof mean !== 'number' || typeof max !== 'number') return omit(`run ${r.run_id} lacks mean_abs_position or max_abs_position`);
      if (!Number.isFinite(mean) || !Number.isFinite(max) || mean < 0 || max < 0) return omit(`run ${r.run_id} has a negative or non-finite inventory measure`);
      if (mean > max * (1 + TOL)) return omit(`run ${r.run_id} reports a mean absolute inventory above its maximum`);
    }
    const point = metricPoint(g.metrics.mean_abs_position);
    if (!point) return omit('a group has no mean_abs_position aggregate');
    rows.push({
      key: groupCellKey(g),
      group: g,
      label: g.feed_offset_ns !== 0 ? `Feed latency offset +${g.feed_offset_ns / 1e6} ms` : `${queueLabel(g.queue_model)} \u00b7 ${latencyLabel(g.latency)}`,
      reference: isBaselineLevels(study, g),
      mean: point,
      max: Math.max(...runs.map((r) => r.measures.max_abs_position)),
      runs: runs.length,
    });
  }
  const risk = study.baseline.config.risk;
  const limit = typeof risk === 'object' && risk !== null && 'max_position' in risk && typeof risk.max_position === 'number' ? risk.max_position : null;
  return { ok: true, data: { rows, limit } };
}

// ---- uncertainty (B4) ------------------------------------------------------------------------

export interface SpreadRow {
  /** What varies. */
  factor: string;
  levels: number;
  min: number;
  max: number;
  /** `max - min` of the measure across the levels. */
  range: number;
  /** Paired on one dataset (parameter sensitivity) or across datasets. */
  across: 'parameter' | 'dataset';
}

/**
 * How far `metric` moves when one thing varies, everything else at baseline: queue models,
 * latency levels (baseline regime, paired), and regimes (cross-dataset). Values are the group
 * means; a seed mean counts as one level.
 */
export function spreads(study: StudyResult, metric: string): SpreadRow[] {
  const b = study.baseline;
  const value = (g: Aggregate) => metricPoint(g.metrics[metric])?.value;
  const row = (factor: string, across: SpreadRow['across'], groups: Aggregate[]): SpreadRow[] => {
    const vs = groups.map(value).filter((v): v is number => v !== undefined && Number.isFinite(v));
    if (vs.length < 2) return [];
    const min = Math.min(...vs);
    const max = Math.max(...vs);
    return [{ factor, levels: vs.length, min, max, range: max - min, across }];
  };
  const inBase = study.aggregates.filter((a) => a.regime === b.regime);
  return [
    ...row('Queue-position model', 'parameter', inBase.filter((a) => a.latency === b.latency && a.feed_offset_ns === b.feed_offset_ns)),
    ...row('Order-latency model (incl. cancel delay)', 'parameter', inBase.filter((a) => a.queue_model === b.queue_model && a.feed_offset_ns === b.feed_offset_ns)),
    ...row('Feed latency offset', 'parameter', inBase.filter((a) => a.queue_model === b.queue_model && a.latency === b.latency)),
    ...row('Synthetic market regime (dataset)', 'dataset', study.aggregates.filter((a) => a.design === 'regime_baseline')),
  ];
}
