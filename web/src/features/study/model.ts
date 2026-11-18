// Read-only views over a verified study result: the comparisons the viewer draws, each value's
// data class, and plain-language names for the study's levels. Pure functions, no React.

import { groupCellKey } from './cellKey';
import type { Aggregate, AggregateMetric, StudyCell, StudyResult } from './contract';

/**
 * What a displayed value is (E4). Every study value is also `not calibrated to real markets`,
 * which the page states once for all of them.
 */
export type DataClass = 'deterministic' | 'sensitivity' | 'seed_variation';

export const DATA_CLASS_LABEL: Record<DataClass, string> = {
  deterministic: 'Deterministic outcome',
  sensitivity: 'Model sensitivity',
  seed_variation: 'Seed variation',
};

export const DATA_CLASS_MEANING: Record<DataClass, string> = {
  deterministic:
    'One simulated run of a fixed configuration on a fixed synthetic dataset. Rerunning it gives the identical value; it has no sampling error and no interval.',
  sensitivity:
    'The difference between runs that change one modelling assumption. It shows how much the simulated outcome depends on that assumption, not which assumption is true.',
  seed_variation:
    'Mean and 95% percentile-bootstrap interval over the random seeds of a stochastic latency model, on one generated market. It is Monte Carlo dispersion of the model, not market uncertainty.',
};

export const NOT_CALIBRATED =
  'Not calibrated to real markets: synthetic data, modelled queue position, latency and fills. No value here is a forecast, an expected return or an annualised figure.';

export interface MetricDef {
  key: string;
  label: string;
  /** How to print one value. */
  fmt: (v: number) => string;
}

const fixed = (digits: number) => (v: number) => v.toFixed(digits);

/** The measures the viewer offers, in menu order. Keys are `measures` keys of the contract. */
export const METRICS: MetricDef[] = [
  { key: 'net_pnl', label: 'Net P&L (quote currency)', fmt: fixed(4) },
  { key: 'fill_rate', label: 'Fill rate (filled / submitted orders)', fmt: fixed(4) },
  { key: 'fills', label: 'Fills', fmt: fixed(1) },
  { key: 'fees', label: 'Fees (quote currency)', fmt: fixed(4) },
  { key: 'unfilled_orders', label: 'Unfilled orders', fmt: fixed(1) },
  { key: 'maker_fill_share', label: 'Maker share of fills', fmt: fixed(4) },
  { key: 'mean_abs_position', label: 'Mean absolute inventory', fmt: fixed(5) },
  { key: 'time_to_fill_p50_ns', label: 'Time to first fill, median (filled orders)', fmt: (v) => `${(v / 1e6).toFixed(3)} ms` },
  { key: 'modeled_queue_at_entry_mean', label: 'Modelled queue ahead at entry, mean', fmt: fixed(3) },
];

export function metricDef(key: string): MetricDef {
  return METRICS.find((m) => m.key === key) ?? { key, label: key, fmt: fixed(4) };
}

const REGIME_LABEL: Record<string, string> = {
  normal_liquidity: 'Normal liquidity',
  widening_spreads: 'Widening spreads',
  liquidity_depletion: 'Liquidity depletion',
  volatility_burst: 'Volatility burst',
  thin_book: 'Thin book',
  feed_staleness: 'Feed staleness',
};

export function regimeLabel(regime: string | null): string {
  if (regime === null) return 'Default generator (no regime intervention)';
  return REGIME_LABEL[regime] ?? regime;
}

const QUEUE_LABEL: Record<string, string> = {
  power_prob3: 'PowerProb3, n = 3 (baseline)',
  risk_adverse: 'Risk-adverse',
  log_prob: 'LogProb',
  log_prob2: 'LogProb2',
  power_prob: 'PowerProb, n = 3',
  power_prob2: 'PowerProb2, n = 3',
};

export function queueLabel(name: string | null): string {
  if (name === null) return 'Not a study level';
  return QUEUE_LABEL[name] ?? name;
}

const LATENCY_LABEL: Record<string, string> = {
  constant: 'Constant 1 ms / 1 ms (baseline)',
  jitter: 'Uniform jitter, median 1 ms',
  log_normal: 'Log-normal, median 1 ms, sigma 0.7',
  heavy_tail: 'Heavy tail (clipped cubic), median 1 ms',
  entry_slow: 'Entry 10 ms / response 1 ms',
  response_slow: 'Entry 1 ms / response 10 ms',
  cancel_delay: 'Cancel entry 20 ms (orders 1 ms / 1 ms)',
};

export function latencyLabel(name: string | null): string {
  if (name === null) return 'Not a study level';
  return LATENCY_LABEL[name] ?? name;
}

export const DESIGN_LABEL: Record<string, string> = {
  regime_baseline: 'Regime baseline',
  one_factor: 'One factor changed',
  factorial: 'Queue x latency factorial',
};

/** One value of a measure with its data class; `interval` only for seed variation. */
export interface MetricPoint {
  value: number;
  interval: [number, number] | null;
  /** Runs behind the value (seeds for stochastic groups). */
  n: number;
  dataClass: 'deterministic' | 'seed_variation';
}

export function metricPoint(metric: AggregateMetric | undefined): MetricPoint | null {
  if (!metric) return null;
  if (metric.kind === 'deterministic_single') return { value: metric.value, interval: null, n: 1, dataClass: 'deterministic' };
  const b = metric.bootstrap;
  if (!b) return null;
  return { value: b.mean, interval: [b.lower_95, b.upper_95], n: b.n, dataClass: 'seed_variation' };
}

/** Short label of an aggregate group for one-factor charts. */
export function groupLabel(a: Aggregate): string {
  if (a.feed_offset_ns !== 0) return `Feed latency offset +${a.feed_offset_ns / 1e6} ms`;
  return `${queueLabel(a.queue_model)} \u00b7 ${latencyLabel(a.latency)}`;
}

export interface ChartRow {
  /** The group's `?cell=` key. */
  key: string;
  group: Aggregate;
  label: string;
  point: MetricPoint | null;
  /** The comparison's reference row (drawn as such, not as better or worse). */
  reference: boolean;
}

function rows(study: StudyResult, metric: string, pick: (a: Aggregate) => boolean, label: (a: Aggregate) => string, isRef: (a: Aggregate) => boolean): ChartRow[] {
  return study.aggregates.filter(pick).map((a) => ({
    key: groupCellKey(a),
    group: a,
    label: label(a),
    point: metricPoint(a.metrics[metric]),
    reference: isRef(a),
  }));
}

const isBaselineOn = (study: StudyResult) => (a: Aggregate) =>
  a.queue_model === study.baseline.queue_model && a.latency === study.baseline.latency && a.feed_offset_ns === study.baseline.feed_offset_ns;

/** Queue models with every other assumption at the baseline (baseline regime). */
export function queueComparison(study: StudyResult, metric: string): ChartRow[] {
  const order = study.queue_models.map((m) => m.name);
  return rows(
    study,
    metric,
    (a) => a.regime === study.baseline.regime && a.latency === study.baseline.latency && a.feed_offset_ns === study.baseline.feed_offset_ns,
    (a) => queueLabel(a.queue_model),
    isBaselineOn(study),
  ).sort((x, y) => order.indexOf(x.group.queue_model) - order.indexOf(y.group.queue_model));
}

/** Latency levels and the feed offset with the baseline queue model (baseline regime). */
export function latencyComparison(study: StudyResult, metric: string): ChartRow[] {
  const order = study.latency_models.map((m) => m.name);
  const pos = (r: ChartRow) => (r.group.feed_offset_ns !== 0 ? order.length : order.indexOf(r.group.latency));
  return rows(
    study,
    metric,
    (a) => a.regime === study.baseline.regime && a.queue_model === study.baseline.queue_model,
    (a) => (a.feed_offset_ns !== 0 ? groupLabel(a) : latencyLabel(a.latency)),
    isBaselineOn(study),
  ).sort((x, y) => pos(x) - pos(y));
}

/** Each regime's baseline run. Regimes replay different events: metrics only, not like-for-like. */
export function regimeComparison(study: StudyResult, metric: string): ChartRow[] {
  return rows(
    study,
    metric,
    (a) => a.design === 'regime_baseline',
    (a) => regimeLabel(a.regime),
    (a) => a.regime === study.baseline.regime,
  );
}

/** Every group of the study, for the fill-rate overview. */
export function allGroups(study: StudyResult, metric: string): ChartRow[] {
  return rows(
    study,
    metric,
    () => true,
    (a) => `${regimeLabel(a.regime)} \u00b7 ${groupLabel(a)}`,
    (a) => a.regime === study.baseline.regime && isBaselineOn(study)(a),
  );
}

export interface MatrixView {
  regime: string;
  queues: string[];
  latencies: string[];
  /** `cells[q][l]`, `null` where the design has no such group. */
  cells: ({ key: string; group: Aggregate; point: MetricPoint | null } | null)[][];
}

/** The queue x latency sensitivity surfaces, one per regime, for `metric`. */
export function sensitivityMatrices(study: StudyResult, metric: string): MatrixView[] {
  return study.sensitivity_surfaces.map((s) => {
    const queues = [...new Set(s.rows.map((r) => r.queue_model))];
    const latencies = [...new Set(s.rows.map((r) => r.latency))];
    const cells = queues.map((q) =>
      latencies.map((l) => {
        const row = s.rows.find((r) => r.queue_model === q && r.latency === l);
        return row ? { key: groupCellKey(row), group: row, point: metricPoint(row.metrics[metric]) } : null;
      }),
    );
    return { regime: s.stratum.regime, queues, latencies, cells };
  });
}

export interface Decomposition {
  key: string;
  label: string;
  gross: number;
  fees: number;
  net: number;
  /** Runs averaged (seeds of a stochastic latency model); 1 for deterministic groups. */
  n: number;
}

/**
 * Gross P&L, fees and net P&L per group: the mean over the group's member runs (seeds), from the
 * per-run accounting (`net = gross - fees`). Arithmetic over modelled values.
 */
export function decomposition(study: StudyResult, pick: (a: Aggregate) => boolean = () => true): Decomposition[] {
  const byId = new Map<string, StudyCell>(study.cells.map((c) => [c.run_id, c]));
  const out: Decomposition[] = [];
  for (const a of study.aggregates.filter(pick)) {
    const members = a.members.map((id) => byId.get(id)).filter((c): c is StudyCell => c !== undefined);
    if (members.length === 0) continue;
    const mean = (f: (c: StudyCell) => number) => members.reduce((s, c) => s + f(c), 0) / members.length;
    out.push({
      key: groupCellKey(a),
      label: `${regimeLabel(a.regime)} \u00b7 ${groupLabel(a)}`,
      gross: mean((c) => c.accounting.gross_pnl),
      fees: mean((c) => c.accounting.fees),
      net: mean((c) => c.accounting.net_pnl),
      n: members.length,
    });
  }
  return out;
}

/** Plain description of an order-latency model configuration (as `latency_models[*].config`). */
export function describeLatency(config: Record<string, unknown>): string {
  const n = (v: unknown) => (typeof v === 'number' ? `${v / 1e6} ms` : '?');
  const leg = (v: unknown): string => {
    if (typeof v !== 'object' || v === null) return '?';
    const l = v as Record<string, unknown>;
    if ('median_ns' in l) return `log-normal median ${n(l.median_ns)}, sigma ${String(l.sigma)}`;
    if ('min_ns' in l) return `cubic min ${n(l.min_ns)}, jitter ${String(l.jitter)}, clip ${String(l.clip)}, unit ${String(l.unit)}`;
    return '?';
  };
  switch (config.type) {
    case 'constant':
      return `entry ${n(config.entry_ns)}, response ${n(config.response_ns)}; cancels use the entry latency`;
    case 'jitter':
      return `entry ${n(config.entry_ns)}, response ${n(config.response_ns)}, each plus uniform [0, ${n(config.jitter_ns)}]; cancels use the entry latency`;
    case 'log_normal':
    case 'heavy_tail':
      return `entry: ${leg(config.entry)}; response: ${leg(config.response)}; cancel: ${config.cancel ? leg(config.cancel) : 'as entry'}`;
    default:
      return JSON.stringify(config);
  }
}

/** The cancel-entry delay of a latency configuration, in words. */
export function cancelDelay(config: Record<string, unknown>): string {
  if ((config.type === 'log_normal' || config.type === 'heavy_tail') && config.cancel) {
    const c = config.cancel as Record<string, unknown>;
    if (typeof c.median_ns === 'number') return `${c.median_ns / 1e6} ms (separate leg${c.sigma === 0 ? ', deterministic' : ''})`;
  }
  return 'same as order entry';
}

/** Held-constant assumptions of the study, from the baseline configuration. */
export function heldConstant(study: StudyResult): { label: string; value: string }[] {
  const c = study.baseline.config as {
    seed?: number;
    strategy?: { interval_ns?: number; kind?: Record<string, unknown> };
    risk?: Record<string, unknown>;
    execution?: { exchange_model?: string; fees?: { maker?: number; taker?: number } };
  };
  const kind = c.strategy?.kind ?? {};
  const out = [
    { label: 'Strategy', value: `${String(kind.type ?? '?')}, order quantity ${String(kind.order_qty ?? '?')}, offset ${String(kind.offset_ticks ?? '?')} ticks, inventory skew ${String(kind.skew_ticks_per_unit ?? '?')} ticks per unit` },
    { label: 'Decision interval', value: c.strategy?.interval_ns ? `${c.strategy.interval_ns / 1e6} ms` : '?' },
    { label: 'Risk limits', value: Object.entries(c.risk ?? {}).map(([k, v]) => `${k} ${v === null ? 'none' : String(v)}`).join(', ') },
    { label: 'Exchange fill model', value: String(c.execution?.exchange_model ?? '?') },
    { label: 'Fees', value: `maker ${(c.execution?.fees?.maker ?? 0) * 1e4} bp, taker ${(c.execution?.fees?.taker ?? 0) * 1e4} bp of traded value` },
    { label: 'Strategy seed', value: String(c.seed ?? '?') },
    { label: 'Synthetic generator', value: `seed ${study.design.generator_seed}, ${study.design.duration_ns / 1e9} s session per regime, same instrument and flow parameters` },
  ];
  return out;
}

export interface DesignSummary {
  design: string;
  label: string;
  varied: string;
  groups: number;
  runs: number;
}

/** What each design block varies, with its group and run counts. */
export function designSummary(study: StudyResult): DesignSummary[] {
  const varied: Record<string, string> = {
    regime_baseline: 'Synthetic market regime only (each regime replays different events)',
    one_factor: 'One of queue model, latency shape, cancel delay or feed latency, on normal liquidity',
    factorial: 'Queue model x latency shape within each stressed regime',
  };
  return Object.keys(DESIGN_LABEL)
    .map((design) => {
      const groups = study.aggregates.filter((a) => a.design === design);
      return {
        design,
        label: DESIGN_LABEL[design],
        varied: varied[design],
        groups: groups.length,
        runs: groups.reduce((s, a) => s + a.members.length, 0),
      };
    })
    .filter((d) => d.groups > 0);
}

/** Prints a point with its interval, e.g. `0.6070 [0.6061, 0.6079]`. */
export function fmtPoint(p: MetricPoint | null, fmt: (v: number) => string): string {
  if (p === null) return 'unavailable';
  return p.interval ? `${fmt(p.value)} [${fmt(p.interval[0])}, ${fmt(p.interval[1])}]` : fmt(p.value);
}
