// Plain-language helpers for the Strategy Lab: why a changed setting matters, a changed value in
// the form's units, the gist of a preset for its card, the modeled execution assumptions for the
// Review step, and a size estimate derived from the dataset span and the interval.

import type { DatasetSummary } from '../../api/model';
import type { OrderLatencyConfig, RunConfig } from '../../api/types';
import {
  EXCHANGE_MODEL_INFO,
  QUEUE_MODEL_INFO,
  STRATEGY_INFO,
  exchangeModelLabel,
  feeLabel,
  fractionToBps,
  latencyLabel,
  queueModelLabel,
} from '../../components/describe';
import { fmtDuration, fmtDurationExact } from '../../components/fmt';
import { LATENCY_MODELS } from './fields';
import type { Field } from './fields';

/** Longest matching prefix wins; the text says what the change does to the simulation. */
const WHY: [string, string][] = [
  ['dataset_id', 'A different market replay: every decision and fill comes from other data.'],
  ['strategy.kind.type', 'A different decision rule; its parameters start from that strategy\u2019s defaults.'],
  ['strategy.kind.order_qty', 'Larger orders take longer to fill from the modeled queue and reach the position limits sooner.'],
  ['strategy.kind.max_position', 'Changes when the strategy stops adding to its inventory on one side.'],
  ['strategy.kind.orders', 'Changes the fixed list of orders the scripted strategy places.'],
  ['strategy.interval_ns', 'Changes how often the strategy decides; also the grid two scenarios are aligned on.'],
  ['strategy.kind', 'Changes what the strategy decides to quote or trade.'],
  ['execution.order_latency', 'Orders and cancels reach the simulated book later or earlier: the queue ahead and the prices on arrival change.'],
  ['execution.feed_latency_offset_ns', 'The strategy sees the market later, so it decides on older information.'],
  ['execution.think_time_ns', 'Requests leave later after each decision.'],
  ['execution.fees', 'Changes the net result of every fill. For strategies that ignore fees, orders and fills stay the same.'],
  ['execution.queue_model', 'Changes the estimated place in the queue, so when resting orders are modeled to fill.'],
  ['execution.exchange_model', 'Changes whether a fill can be partial.'],
  ['risk.max_drawdown', 'Changes when the harness halts the strategy after a fall in equity.'],
  ['risk', 'Changes which orders the harness blocks before they reach the simulated exchange.'],
  ['record', 'Changes only how much the tape keeps, not what is simulated (still part of the run identity).'],
  ['seed', 'A different random draw for seeded latency models; with constant latency the outcome is the same, under a new run id.'],
];

export function whyItMatters(path: string): string {
  let best: [string, string] | null = null;
  for (const entry of WHY) {
    const [prefix] = entry;
    if ((path === prefix || path.startsWith(`${prefix}.`)) && (best === null || prefix.length > best[0].length)) best = entry;
  }
  return best ? best[1] : 'Part of the run configuration and its identity.';
}

/** A changed value in the unit the form uses (ms, bp, option names), or as stored when unknown. */
export function changeValue(field: Field<RunConfig> | undefined, raw: string, path = ''): string {
  if (raw === '(absent)') return raw;
  if (!field) {
    const n = Number(raw);
    return path.endsWith('_ns') && Number.isFinite(n) ? fmtDuration(n) : raw;
  }
  if (field.kind === 'select') {
    const v: unknown = JSON.parse(raw);
    return field.options.find((o) => o.value === v)?.label ?? raw;
  }
  if (field.kind === 'number' || field.kind === 'optional_number') {
    const n = Number(raw);
    if (raw === 'null' || !Number.isFinite(n)) return raw !== 'null' ? raw : field.kind === 'optional_number' ? 'off' : 'blank';
    const shown = field.kind === 'number' && field.convert ? Number(field.convert.toDisplay(n).toPrecision(12)) : n;
    return field.unit ? `${shown} ${field.unit}` : String(shown);
  }
  return raw;
}

/** The signed difference of a numeric change in the form's unit (`+6 ms`); null when not numeric. */
export function changeDelta(field: Field<RunConfig> | undefined, a: string, b: string): string | null {
  if (!field || (field.kind !== 'number' && field.kind !== 'optional_number')) return null;
  const x = Number(a);
  const y = Number(b);
  if (a === 'null' || b === 'null' || !Number.isFinite(x) || !Number.isFinite(y)) return null;
  const shown = (n: number) => (field.kind === 'number' && field.convert ? field.convert.toDisplay(n) : n);
  const d = Number((shown(y) - shown(x)).toPrecision(12));
  if (d === 0) return null;
  const text = `${d > 0 ? '+' : '\u2212'}${Math.abs(d)}`;
  return field.unit ? `${text} ${field.unit}` : text;
}

/** Which side of the book a strategy kind works, for the preset card's tag. */
export type StrategyRole = 'maker' | 'taker' | 'scripted';

export const ROLE_LABEL: Record<StrategyRole, string> = {
  maker: 'Passive maker',
  taker: 'Liquidity taker',
  scripted: 'Scripted orders',
};

const ROLES: Record<string, StrategyRole> = {
  touch_maker: 'maker',
  avellaneda_stoikov: 'maker',
  glft: 'maker',
  imbalance_taker: 'taker',
  scripted: 'scripted',
};

export function strategyRole(type: string): StrategyRole | null {
  return ROLES[type] ?? null;
}

/**
 * A preset title without its strategy prefix ("Touch maker: slower line" → "Slower line"), since
 * the card's tag already names the strategy. Titles without that prefix are kept whole.
 */
export function presetShortTitle(title: string, strategyType: string): string {
  const label = strategyType in STRATEGY_INFO ? STRATEGY_INFO[strategyType as keyof typeof STRATEGY_INFO].label : null;
  const prefix = label ? `${label.toLowerCase()}: ` : null;
  if (!prefix || !title.toLowerCase().startsWith(prefix) || title.length === prefix.length) return title;
  const rest = title.slice(prefix.length);
  return rest.charAt(0).toUpperCase() + rest.slice(1);
}

function latencyChip(l: OrderLatencyConfig): string {
  switch (l.type) {
    case 'constant':
      return l.entry_ns === l.response_ns
        ? `${fmtDurationExact(l.entry_ns)} latency`
        : `${fmtDurationExact(l.entry_ns)} / ${fmtDurationExact(l.response_ns)} latency`;
    case 'jitter':
      return `${fmtDurationExact(l.entry_ns)} latency + jitter \u2264 ${fmtDurationExact(l.jitter_ns)}`;
    case 'log_normal':
      return `Log-normal latency, median ${fmtDurationExact(l.entry.median_ns)}`;
    case 'heavy_tail':
      return `Heavy-tail latency, min ${fmtDurationExact(l.entry.min_ns)}`;
  }
}

export type ChipKey = 'latency' | 'queue' | 'fees';

export interface Chip {
  key: ChipKey;
  text: string;
  /** Differs from the value most of the listed presets share. */
  differs: boolean;
}

/** The three execution assumptions a preset card shows, compactly worded. */
export function assumptionChips(c: RunConfig): { key: ChipKey; text: string }[] {
  const f = c.execution.fees;
  const maker = fractionToBps(f.maker);
  return [
    { key: 'latency', text: latencyChip(c.execution.order_latency) },
    { key: 'queue', text: `${QUEUE_MODEL_INFO[c.execution.queue_model.type].label} queue` },
    { key: 'fees', text: maker < 0 ? `Maker rebate ${-maker} bp, taker ${fractionToBps(f.taker)} bp` : `Fees ${maker} / ${fractionToBps(f.taker)} bp` },
  ];
}

/**
 * Chips for every preset, each marked when it differs from the value most presets share, so the
 * one assumption a variant changes stands out. With fewer than three presets nothing is marked.
 */
export function presetChips(configs: RunConfig[]): Chip[][] {
  const all = configs.map(assumptionChips);
  const common = new Map<ChipKey, string>();
  if (configs.length >= 3) {
    for (const key of ['latency', 'queue', 'fees'] as const) {
      const counts = new Map<string, number>();
      for (const chips of all) {
        const t = chips.find((x) => x.key === key)?.text ?? '';
        counts.set(t, (counts.get(t) ?? 0) + 1);
      }
      const [top] = [...counts.entries()].sort((a, b) => b[1] - a[1]);
      if (top && top[1] > configs.length / 2) common.set(key, top[0]);
    }
  }
  return all.map((chips) => chips.map((x) => ({ ...x, differs: common.has(x.key) && common.get(x.key) !== x.text })));
}

export interface AssumptionRow {
  key: string;
  label: string;
  value: string;
  help: string;
  /** Differs from the starting point's value. */
  changed: boolean;
}

/**
 * The execution assumptions of a configuration (all modeled: the simulator's estimate of how the
 * exchange would have treated the orders), each flagged when it differs from `reference`.
 */
export function assumptionRows(c: RunConfig, reference: RunConfig | null): AssumptionRow[] {
  const e = c.execution;
  const r = reference?.execution ?? null;
  const differs = (a: unknown, b: unknown) => r !== null && JSON.stringify(a) !== JSON.stringify(b);
  const rows: AssumptionRow[] = [
    {
      key: 'queue',
      label: 'Queue model',
      value: queueModelLabel(e.queue_model),
      help: QUEUE_MODEL_INFO[e.queue_model.type].help,
      changed: differs(e.queue_model, r?.queue_model),
    },
    {
      key: 'latency',
      label: 'Order latency (entry / response)',
      value: latencyLabel(e.order_latency),
      help: LATENCY_MODELS.find((m) => m.value === e.order_latency.type)?.help ?? '',
      changed: differs(e.order_latency, r?.order_latency),
    },
    {
      key: 'fill',
      label: 'Fill model',
      value: exchangeModelLabel(e.exchange_model),
      help: EXCHANGE_MODEL_INFO[e.exchange_model].help,
      changed: differs(e.exchange_model, r?.exchange_model),
    },
    {
      key: 'fees',
      label: 'Fees (maker / taker)',
      value: feeLabel(e.fees),
      help: 'A share of traded value charged on every fill; a negative maker fee is a rebate.',
      changed: differs(e.fees, r?.fees),
    },
  ];
  if (e.feed_latency_offset_ns !== 0 || (r !== null && r.feed_latency_offset_ns !== 0)) {
    rows.push({
      key: 'feed',
      label: 'Feed latency offset',
      value: fmtDuration(e.feed_latency_offset_ns),
      help: 'Market data reaches the strategy this much later than it was recorded.',
      changed: differs(e.feed_latency_offset_ns, r?.feed_latency_offset_ns),
    });
  }
  if (e.think_time_ns || r?.think_time_ns) {
    rows.push({
      key: 'think',
      label: 'Think time',
      value: fmtDuration(e.think_time_ns ?? 0),
      help: 'Requests leave this long after each decision.',
      changed: differs(e.think_time_ns ?? 0, r?.think_time_ns ?? 0),
    });
  }
  return rows;
}

export interface SizeEstimate {
  /** Strategy evaluations across the dataset span. */
  ticks: number;
  spanNs: number;
  rows: number;
}

/** Derived before submission; the server returns its own estimate with the experiment. */
export function sizeEstimate(c: RunConfig, dataset: DatasetSummary | null): SizeEstimate | null {
  const v = dataset?.current_version;
  if (!v || !(c.strategy.interval_ns > 0)) return null;
  const spanNs = v.end_ns - v.start_ns;
  if (!(spanNs > 0)) return null;
  return { ticks: Math.floor(spanNs / c.strategy.interval_ns), spanNs, rows: v.rows };
}
