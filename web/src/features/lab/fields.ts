// Typed field descriptors for the Strategy Laboratory form. The form is generated from these:
// each descriptor names one `RunConfig` field, how to read and write it, the Rust type's bounds,
// and any unit conversion between what the user types and what is stored. Conversions are
// explicit functions so milliseconds never silently become nanoseconds, nor basis points
// fractions.

import {
  baseEntryNs,
  baseResponseNs,
  type AvellanedaStoikovParams,
  type CubicLeg,
  type ExchangeModel,
  type ExecutionConfig,
  type GlftParams,
  type ImbalanceTakerParams,
  type InstrumentMeta,
  type LogNormalLeg,
  type OrdType,
  type OrderLatencyConfig,
  type Preset,
  type QueueModelConfig,
  type RiskConfig,
  type RunConfig,
  type ScriptedOrder,
  type Side,
  type SigmaSource,
  type StrategyKind,
  type Tif,
  type TouchMakerParams,
} from '../../api/types';
import {
  EXCHANGE_MODELS,
  EXCHANGE_MODEL_INFO,
  QUEUE_MODEL_INFO,
  QUEUE_MODEL_TYPES,
  SIGMA_SOURCES,
  SIGMA_SOURCE_INFO,
  STRATEGY_INFO,
  bpsToFraction,
  fractionToBps,
  queueModelHasExponent,
} from '../../components/describe';
import type { QueueModelType } from '../../components/describe';
import { MAX_INTERVAL_NS, MAX_LATENCY_NS, MAX_LOG_NORMAL_SIGMA, MIN_INTERVAL_NS } from './validate';

// ----- descriptor types -------------------------------------------------------------------

export interface UnitConversion {
  /** Unit of the value stored in the configuration. */
  storedUnit: string;
  toStored: (display: number) => number;
  toDisplay: (stored: number) => number;
}

interface FieldCommon {
  /** Dotted path of the field in the serialized `RunConfig` (unique within a form). */
  id: string;
  label: string;
  /** One line: what the field means. */
  help: string;
}

export interface NumberField<T> extends FieldCommon {
  kind: 'number';
  /** The Rust field is an integer type. */
  integer: boolean;
  /** Unit of the value as typed by the user. */
  unit?: string;
  convert?: UnitConversion;
  /** Inclusive bounds of the Rust type, in stored units (0 for unsigned integers). */
  min?: number;
  max?: number;
  get: (obj: T) => number;
  set: (obj: T, stored: number) => T;
}

export interface OptionalNumberField<T> extends FieldCommon {
  kind: 'optional_number';
  integer: boolean;
  unit?: string;
  min?: number;
  max?: number;
  /** Label of the checkbox that switches the value on. */
  enableLabel: string;
  get: (obj: T) => number | null;
  set: (obj: T, stored: number | null) => T;
}

export interface SelectOption {
  value: string;
  label: string;
  /** One line: what this choice models. */
  help?: string;
  /** Shown but not selectable; `help` says why. */
  disabled?: boolean;
}

export interface SelectField<T> extends FieldCommon {
  kind: 'select';
  options: SelectOption[];
  get: (obj: T) => string;
  set: (obj: T, value: string) => T;
}

export interface TextField<T> extends FieldCommon {
  kind: 'text';
  get: (obj: T) => string;
  set: (obj: T, value: string) => T;
}

export type Field<T> = NumberField<T> | OptionalNumberField<T> | SelectField<T> | TextField<T>;

export interface Section {
  id: string;
  title: string;
  /** One line under the title. */
  description: string;
  /** The section is an execution assumption: its outputs are estimates, not observations. */
  modeled: boolean;
  fields: Field<RunConfig>[];
}

// ----- unit conversions -------------------------------------------------------------------

/** Milliseconds as typed, whole nanoseconds as stored (1 ms = 1,000,000 ns). */
export const MS_TO_NS: UnitConversion = {
  storedUnit: 'ns',
  toStored: (ms) => Math.round(ms * 1_000_000),
  toDisplay: (ns) => ns / 1_000_000,
};

/** Basis points as typed, fraction of traded value as stored (1 bp = 0.0001). */
export const BPS_TO_FRACTION: UnitConversion = {
  storedUnit: 'fraction of traded value',
  toStored: bpsToFraction,
  toDisplay: fractionToBps,
};

const U8_MAX = 255;
const U32_MAX = 4_294_967_295;
/** u64/i64 fields travel as JSON numbers; beyond 2^53 - 1 they would lose precision. */
const SAFE_MAX = Number.MAX_SAFE_INTEGER;

/**
 * Type-level problem of one field's current value (not a number, not whole, outside the Rust
 * type's range), or `null`. Semantic rules live in `validate.ts`.
 */
export function fieldProblem<T>(field: Field<T>, obj: T): string | null {
  if (field.kind === 'select' || field.kind === 'text') return null;
  const v = field.get(obj);
  if (v === null) return null;
  if (!Number.isFinite(v)) return 'Enter a number.';
  if (field.integer && !Number.isInteger(v)) return 'Must be a whole number.';
  const shown = (stored: number) => {
    const display = field.kind === 'number' && field.convert ? field.convert.toDisplay(stored) : stored;
    return field.unit ? `${display} ${field.unit}` : String(display);
  };
  if (field.min !== undefined && v < field.min) return `Must be at least ${shown(field.min)}.`;
  if (field.max !== undefined && v > field.max) return `Must be at most ${shown(field.max)}.`;
  if (field.integer && Math.abs(v) > SAFE_MAX) return 'Too large to be sent exactly.';
  return null;
}

// ----- RunConfig lenses -------------------------------------------------------------------

function withExecution(c: RunConfig, patch: Partial<ExecutionConfig>): RunConfig {
  return { ...c, execution: { ...c.execution, ...patch } };
}

function withRisk(c: RunConfig, patch: Partial<RiskConfig>): RunConfig {
  return { ...c, risk: { ...c.risk, ...patch } };
}

function withKind(c: RunConfig, kind: StrategyKind): RunConfig {
  return { ...c, strategy: { ...c.strategy, kind } };
}

function patchTouch(c: RunConfig, patch: Partial<TouchMakerParams>): RunConfig {
  const k = c.strategy.kind;
  return k.type === 'touch_maker' ? withKind(c, { ...k, ...patch }) : c;
}

function patchAvellaneda(c: RunConfig, patch: Partial<AvellanedaStoikovParams>): RunConfig {
  const k = c.strategy.kind;
  return k.type === 'avellaneda_stoikov' ? withKind(c, { ...k, ...patch }) : c;
}

function patchImbalance(c: RunConfig, patch: Partial<ImbalanceTakerParams>): RunConfig {
  const k = c.strategy.kind;
  return k.type === 'imbalance_taker' ? withKind(c, { ...k, ...patch }) : c;
}

function patchGlft(c: RunConfig, patch: Partial<GlftParams>): RunConfig {
  const k = c.strategy.kind;
  return k.type === 'glft' ? withKind(c, { ...k, ...patch }) : c;
}

function withLatency(c: RunConfig, patch: { entry_ns?: number; response_ns?: number; jitter_ns?: number }): RunConfig {
  const l = c.execution.order_latency;
  if (l.type !== 'constant' && l.type !== 'jitter') return c;
  const entry_ns = patch.entry_ns ?? l.entry_ns;
  const response_ns = patch.response_ns ?? l.response_ns;
  const next: OrderLatencyConfig =
    l.type === 'jitter'
      ? { type: 'jitter', entry_ns, response_ns, jitter_ns: patch.jitter_ns ?? l.jitter_ns }
      : { type: 'constant', entry_ns, response_ns };
  return withExecution(c, { order_latency: next });
}

type LegName = 'entry' | 'response' | 'cancel';

function patchLogNormalLeg(c: RunConfig, leg: LegName, patch: Partial<LogNormalLeg>): RunConfig {
  const l = c.execution.order_latency;
  if (l.type !== 'log_normal') return c;
  const prev = l[leg];
  if (!prev) return c;
  const next = { ...l };
  next[leg] = { ...prev, ...patch };
  return withExecution(c, { order_latency: next });
}

function patchCubicLeg(c: RunConfig, leg: LegName, patch: Partial<CubicLeg>): RunConfig {
  const l = c.execution.order_latency;
  if (l.type !== 'heavy_tail') return c;
  const prev = l[leg];
  if (!prev) return c;
  const next = { ...l };
  next[leg] = { ...prev, ...patch };
  return withExecution(c, { order_latency: next });
}

/** A new log-normal leg at `median_ns`; the spread starts blank so it has to be chosen. */
function logNormalLeg(median_ns: number): LogNormalLeg {
  return { median_ns, sigma: Number.NaN };
}

/** A new cubic leg at `min_ns`; the shape parameters start blank so they have to be chosen. */
function cubicLeg(min_ns: number): CubicLeg {
  return { min_ns, jitter: Number.NaN, clip: Number.NaN, unit: Number.NaN };
}

/** The cancel leg's location parameter, if the model has its own cancel leg. */
function cancelBaseNs(l: OrderLatencyConfig): number | null {
  if (l.type === 'log_normal') return l.cancel ? l.cancel.median_ns : null;
  if (l.type === 'heavy_tail') return l.cancel ? l.cancel.min_ns : null;
  return null;
}

/**
 * The latency model `type` replacing `prev`. Entry and response keep their location (latency,
 * median or minimum, in ns), as does a separate cancel leg; the new model's dimensionless
 * parameters start blank.
 */
function switchLatencyModel(prev: OrderLatencyConfig, type: OrderLatencyConfig['type']): OrderLatencyConfig {
  const entry_ns = baseEntryNs(prev);
  const response_ns = baseResponseNs(prev);
  const cancel_ns = cancelBaseNs(prev);
  switch (type) {
    case 'constant':
      return { type, entry_ns, response_ns };
    case 'jitter':
      return { type, entry_ns, response_ns, jitter_ns: prev.type === 'jitter' ? prev.jitter_ns : 0 };
    case 'log_normal':
      return {
        type,
        entry: logNormalLeg(entry_ns),
        response: logNormalLeg(response_ns),
        cancel: cancel_ns === null ? null : logNormalLeg(cancel_ns),
      };
    case 'heavy_tail':
      return {
        type,
        entry: cubicLeg(entry_ns),
        response: cubicLeg(response_ns),
        cancel: cancel_ns === null ? null : cubicLeg(cancel_ns),
      };
  }
}

// ----- defaults ---------------------------------------------------------------------------

/** `lots` lot sizes, free of binary-float noise (10 x 0.001 is 0.01, not 0.010000000000000002). */
function lotsOf(inst: InstrumentMeta | null, lots: number): number {
  if (!inst) return Number.NaN;
  return Number((inst.lot_size * lots).toPrecision(12));
}

/**
 * Parameters for a strategy kind the user just switched to. They come from the first server
 * preset of that kind when there is one. Otherwise sizes start at the smallest valid values for
 * the instrument and model parameters start blank (NaN) so the user has to choose them.
 */
export function strategyKindDefaults(
  type: StrategyKind['type'],
  inst: InstrumentMeta | null,
  presets: Preset[],
): StrategyKind {
  for (const p of presets) {
    if (p.config.strategy.kind.type === type) return p.config.strategy.kind;
  }
  const order_qty = lotsOf(inst, 1);
  const max_position = lotsOf(inst, 10);
  switch (type) {
    case 'touch_maker':
      return { type, order_qty, max_position, offset_ticks: 0, skew_ticks_per_unit: 0 };
    case 'avellaneda_stoikov':
      return {
        type,
        order_qty,
        max_position,
        gamma: Number.NaN,
        kappa: Number.NaN,
        horizon_s: Number.NaN,
        vol_halflife_ticks: Number.NaN,
      };
    case 'glft':
      // The sigma source is a choice among three series and cannot be blank; `mid` is listed first.
      return {
        type,
        order_qty,
        max_position,
        gamma: Number.NaN,
        kappa: Number.NaN,
        intensity_a: Number.NaN,
        sigma_source: 'mid',
        vol_halflife_ticks: Number.NaN,
      };
    case 'imbalance_taker':
      return {
        type,
        order_qty,
        max_position,
        threshold: Number.NaN,
        hold_ticks: Number.NaN,
        cooldown_ticks: 0,
      };
    case 'scripted':
      return { type, orders: [] };
  }
}

/**
 * Starting form values when no preset or run has been loaded: a touch maker with the smallest
 * valid sizes for the instrument and the most conservative execution assumptions (risk-adverse
 * queue, no fees, 1 ms order latency). These are form defaults to edit, not a recommendation.
 */
export function startingConfig(datasetId: string, inst: InstrumentMeta | null): RunConfig {
  return {
    dataset_id: datasetId,
    strategy: {
      interval_ns: 100_000_000,
      kind: strategyKindDefaults('touch_maker', inst, []),
    },
    execution: {
      exchange_model: 'partial_fill',
      queue_model: { type: 'risk_adverse' },
      order_latency: { type: 'constant', entry_ns: 1_000_000, response_ns: 1_000_000 },
      feed_latency_offset_ns: 0,
      fees: { maker: 0, taker: 0 },
    },
    risk: {
      max_position: lotsOf(inst, 10),
      max_order_qty: lotsOf(inst, 1),
      max_open_orders: 10,
      max_drawdown: null,
    },
    // Mirrors `RecordConfig::default()`.
    record: { sample_every_ticks: 1, max_queue_updates_per_order: 2_000 },
    seed: 1,
  };
}

/** A new scripted order row. Quantity starts blank so it has to be chosen. */
export function newScriptedOrder(orders: ScriptedOrder[]): ScriptedOrder {
  const lastTick = orders.reduce((m, o) => (Number.isFinite(o.at_tick) ? Math.max(m, o.at_tick) : m), -1);
  return {
    at_tick: lastTick + 1,
    slot: 0,
    side: 'buy',
    price: { type: 'from_touch', offset: 0 },
    qty: Number.NaN,
    tif: 'gtc',
    ord_type: 'limit',
    cancel_after_ticks: null,
    label: '',
  };
}

// ----- sections ---------------------------------------------------------------------------

export interface SectionContext {
  instrument: InstrumentMeta | null;
  presets: Preset[];
}

const STRATEGY_TYPES: StrategyKind['type'][] = [
  'touch_maker',
  'avellaneda_stoikov',
  'glft',
  'imbalance_taker',
  'scripted',
];

function isStrategyType(v: string): v is StrategyKind['type'] {
  return (STRATEGY_TYPES as string[]).includes(v);
}

function isQueueModelType(v: string): v is QueueModelType {
  return (QUEUE_MODEL_TYPES as string[]).includes(v);
}

function isExchangeModel(v: string): v is ExchangeModel {
  return (EXCHANGE_MODELS as string[]).includes(v);
}

function isSigmaSource(v: string): v is SigmaSource {
  return (SIGMA_SOURCES as string[]).includes(v);
}

function strategyFields(c: RunConfig, ctx: SectionContext): Field<RunConfig>[] {
  const fields: Field<RunConfig>[] = [
    {
      kind: 'number',
      id: 'strategy.interval_ns',
      label: 'Evaluation interval',
      help: 'Local-clock time between strategy evaluations, from 1 ms to 1 h. Every strategy runs on this fixed grid, which is what lets two scenarios be aligned decision by decision.',
      integer: true,
      unit: 'ms',
      convert: MS_TO_NS,
      min: MIN_INTERVAL_NS,
      max: MAX_INTERVAL_NS,
      get: (x) => x.strategy.interval_ns,
      set: (x, v) => ({ ...x, strategy: { ...x.strategy, interval_ns: v } }),
    },
    {
      kind: 'select',
      id: 'strategy.kind.type',
      label: 'Strategy',
      help: STRATEGY_INFO[c.strategy.kind.type].help,
      options: STRATEGY_TYPES.map((t) => ({
        value: t,
        label: STRATEGY_INFO[t].label,
        help: STRATEGY_INFO[t].help,
      })),
      get: (x) => x.strategy.kind.type,
      set: (x, v) =>
        isStrategyType(v) && v !== x.strategy.kind.type
          ? withKind(x, strategyKindDefaults(v, ctx.instrument, ctx.presets))
          : x,
    },
  ];

  const kind = c.strategy.kind;
  switch (kind.type) {
    case 'touch_maker':
      fields.push(
        {
          kind: 'number',
          id: 'strategy.kind.order_qty',
          label: 'Order quantity',
          help: 'Size of each quote, in base units. Must be a multiple of the lot size.',
          integer: false,
          unit: 'base units',
          get: (x) => (x.strategy.kind.type === 'touch_maker' ? x.strategy.kind.order_qty : Number.NaN),
          set: (x, v) => patchTouch(x, { order_qty: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.max_position',
          label: 'Strategy position limit',
          help: 'Stop quoting the side that would grow |position| beyond this.',
          integer: false,
          unit: 'base units',
          get: (x) =>
            x.strategy.kind.type === 'touch_maker' ? x.strategy.kind.max_position : Number.NaN,
          set: (x, v) => patchTouch(x, { max_position: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.offset_ticks',
          label: 'Offset from touch',
          help: 'Quote this many ticks behind the touch (0 joins the best price).',
          integer: true,
          unit: 'ticks',
          get: (x) =>
            x.strategy.kind.type === 'touch_maker' ? x.strategy.kind.offset_ticks : Number.NaN,
          set: (x, v) => patchTouch(x, { offset_ticks: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.skew_ticks_per_unit',
          label: 'Inventory skew',
          help: 'Shift both quotes by -skew x position ticks (rounded), so inventory pushes quotes toward flattening.',
          integer: false,
          unit: 'ticks per unit',
          get: (x) =>
            x.strategy.kind.type === 'touch_maker'
              ? x.strategy.kind.skew_ticks_per_unit
              : Number.NaN,
          set: (x, v) => patchTouch(x, { skew_ticks_per_unit: v }),
        },
      );
      break;
    case 'avellaneda_stoikov':
      fields.push(
        {
          kind: 'number',
          id: 'strategy.kind.order_qty',
          label: 'Order quantity',
          help: 'Size of each quote, in base units. Must be a multiple of the lot size.',
          integer: false,
          unit: 'base units',
          get: (x) =>
            x.strategy.kind.type === 'avellaneda_stoikov' ? x.strategy.kind.order_qty : Number.NaN,
          set: (x, v) => patchAvellaneda(x, { order_qty: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.max_position',
          label: 'Strategy position limit',
          help: 'Largest |position| the strategy quotes toward.',
          integer: false,
          unit: 'base units',
          get: (x) =>
            x.strategy.kind.type === 'avellaneda_stoikov'
              ? x.strategy.kind.max_position
              : Number.NaN,
          set: (x, v) => patchAvellaneda(x, { max_position: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.gamma',
          label: 'Risk aversion gamma',
          help: 'Risk aversion (per price unit) in the reservation price and spread.',
          integer: false,
          get: (x) =>
            x.strategy.kind.type === 'avellaneda_stoikov' ? x.strategy.kind.gamma : Number.NaN,
          set: (x, v) => patchAvellaneda(x, { gamma: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.kappa',
          label: 'Arrival decay kappa',
          help: 'Order-arrival decay (per price unit) in lambda(delta) = A * exp(-kappa * delta).',
          integer: false,
          get: (x) =>
            x.strategy.kind.type === 'avellaneda_stoikov' ? x.strategy.kind.kappa : Number.NaN,
          set: (x, v) => patchAvellaneda(x, { kappa: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.horizon_s',
          label: 'Horizon',
          help: 'Remaining horizon T - t, held constant (the common stationary variant).',
          integer: false,
          unit: 's',
          get: (x) =>
            x.strategy.kind.type === 'avellaneda_stoikov' ? x.strategy.kind.horizon_s : Number.NaN,
          set: (x, v) => patchAvellaneda(x, { horizon_s: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.vol_halflife_ticks',
          label: 'Volatility half-life',
          help: 'Volatility is an exponentially weighted estimate of mid-price changes with this half-life.',
          integer: true,
          unit: 'strategy ticks',
          min: 0,
          max: U32_MAX,
          get: (x) =>
            x.strategy.kind.type === 'avellaneda_stoikov'
              ? x.strategy.kind.vol_halflife_ticks
              : Number.NaN,
          set: (x, v) => patchAvellaneda(x, { vol_halflife_ticks: v }),
        },
      );
      break;
    case 'glft':
      fields.push(
        {
          kind: 'number',
          id: 'strategy.kind.order_qty',
          label: 'Order quantity',
          help: 'Quote size Delta, in base units; also the trade size in the GLFT formulas. Must be a multiple of the lot size.',
          integer: false,
          unit: 'base units',
          get: (x) => (x.strategy.kind.type === 'glft' ? x.strategy.kind.order_qty : Number.NaN),
          set: (x, v) => patchGlft(x, { order_qty: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.max_position',
          label: 'Strategy position limit',
          help: 'Inventory bound Q: a side is not quoted when one more fill would take |position| past it.',
          integer: false,
          unit: 'base units',
          get: (x) => (x.strategy.kind.type === 'glft' ? x.strategy.kind.max_position : Number.NaN),
          set: (x, v) => patchGlft(x, { max_position: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.gamma',
          label: 'Risk aversion gamma',
          help: 'Absolute risk aversion of the CARA objective, per unit of wealth (1 / (price unit x base unit)). Finite and positive.',
          integer: false,
          get: (x) => (x.strategy.kind.type === 'glft' ? x.strategy.kind.gamma : Number.NaN),
          set: (x, v) => patchGlft(x, { gamma: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.kappa',
          label: 'Intensity decay kappa',
          help: 'Decay k (per price unit) in Lambda(delta) = A * exp(-k * delta). Finite and positive.',
          integer: false,
          get: (x) => (x.strategy.kind.type === 'glft' ? x.strategy.kind.kappa : Number.NaN),
          set: (x, v) => patchGlft(x, { kappa: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.intensity_a',
          label: 'Intensity scale A',
          help: 'Fills per second of a quote at the reference price. An assumption, not estimated from the data. Finite and positive.',
          integer: false,
          unit: 'fills/s',
          get: (x) => (x.strategy.kind.type === 'glft' ? x.strategy.kind.intensity_a : Number.NaN),
          set: (x, v) => patchGlft(x, { intensity_a: v }),
        },
        {
          kind: 'select',
          id: 'strategy.kind.sigma_source',
          label: 'Volatility source',
          help: 'Observed price series whose clock-sampled changes estimate sigma.',
          options: SIGMA_SOURCES.map((s) => ({ value: s, label: `${SIGMA_SOURCE_INFO[s].label} (${s})`, help: SIGMA_SOURCE_INFO[s].help })),
          get: (x) => (x.strategy.kind.type === 'glft' ? x.strategy.kind.sigma_source : ''),
          set: (x, v) => (isSigmaSource(v) ? patchGlft(x, { sigma_source: v }) : x),
        },
        {
          kind: 'number',
          id: 'strategy.kind.vol_halflife_ticks',
          label: 'Volatility half-life',
          help: 'Half-life of the exponentially weighted variance estimate. At least 1.',
          integer: true,
          unit: 'strategy ticks',
          min: 0,
          max: U32_MAX,
          get: (x) => (x.strategy.kind.type === 'glft' ? x.strategy.kind.vol_halflife_ticks : Number.NaN),
          set: (x, v) => patchGlft(x, { vol_halflife_ticks: v }),
        },
      );
      break;
    case 'imbalance_taker':
      fields.push(
        {
          kind: 'number',
          id: 'strategy.kind.order_qty',
          label: 'Order quantity',
          help: 'Size of each entry, in base units. Must be a multiple of the lot size.',
          integer: false,
          unit: 'base units',
          get: (x) =>
            x.strategy.kind.type === 'imbalance_taker' ? x.strategy.kind.order_qty : Number.NaN,
          set: (x, v) => patchImbalance(x, { order_qty: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.max_position',
          label: 'Strategy position limit',
          help: 'Largest |position| the strategy will build.',
          integer: false,
          unit: 'base units',
          get: (x) =>
            x.strategy.kind.type === 'imbalance_taker' ? x.strategy.kind.max_position : Number.NaN,
          set: (x, v) => patchImbalance(x, { max_position: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.threshold',
          label: 'Imbalance threshold',
          help: 'Enter when (bid_qty - ask_qty) / (bid_qty + ask_qty) exceeds this in absolute value. Between 0 and 1, exclusive.',
          integer: false,
          get: (x) =>
            x.strategy.kind.type === 'imbalance_taker' ? x.strategy.kind.threshold : Number.NaN,
          set: (x, v) => patchImbalance(x, { threshold: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.hold_ticks',
          label: 'Holding time',
          help: 'Close the position this many strategy ticks after entry.',
          integer: true,
          unit: 'strategy ticks',
          min: 0,
          max: U32_MAX,
          get: (x) =>
            x.strategy.kind.type === 'imbalance_taker' ? x.strategy.kind.hold_ticks : Number.NaN,
          set: (x, v) => patchImbalance(x, { hold_ticks: v }),
        },
        {
          kind: 'number',
          id: 'strategy.kind.cooldown_ticks',
          label: 'Cooldown',
          help: 'Minimum strategy ticks between entries.',
          integer: true,
          unit: 'strategy ticks',
          min: 0,
          max: U32_MAX,
          get: (x) =>
            x.strategy.kind.type === 'imbalance_taker' ? x.strategy.kind.cooldown_ticks : Number.NaN,
          set: (x, v) => patchImbalance(x, { cooldown_ticks: v }),
        },
      );
      break;
    case 'scripted':
      // The order list is edited by `ScriptedOrdersEditor` from `SCRIPTED_ORDER_FIELDS`.
      break;
  }
  return fields;
}

function queueFields(c: RunConfig): Field<RunConfig>[] {
  const q = c.execution.queue_model;
  const fields: Field<RunConfig>[] = [
    {
      kind: 'select',
      id: 'execution.queue_model.type',
      label: 'Queue-position model',
      help: QUEUE_MODEL_INFO[q.type].help,
      options: QUEUE_MODEL_TYPES.map((t) => ({
        value: t,
        label: `${QUEUE_MODEL_INFO[t].label} (${t})`,
        help: QUEUE_MODEL_INFO[t].help,
      })),
      get: (x) => x.execution.queue_model.type,
      set: (x, v) => {
        if (!isQueueModelType(v)) return x;
        const prev = x.execution.queue_model;
        let next: QueueModelConfig;
        if (v === 'power_prob' || v === 'power_prob2' || v === 'power_prob3') {
          // Keep an exponent already chosen; otherwise leave it blank for the user to set.
          next = { type: v, n: queueModelHasExponent(prev) ? prev.n : Number.NaN };
        } else {
          next = { type: v };
        }
        return withExecution(x, { queue_model: next });
      },
    },
  ];
  if (queueModelHasExponent(q)) {
    fields.push({
      kind: 'number',
      id: 'execution.queue_model.n',
      label: 'Exponent n',
      help: 'Exponent of the probability function above. Must be finite and positive.',
      integer: false,
      get: (x) =>
        queueModelHasExponent(x.execution.queue_model) ? x.execution.queue_model.n : Number.NaN,
      set: (x, v) => {
        const prev = x.execution.queue_model;
        return queueModelHasExponent(prev)
          ? withExecution(x, { queue_model: { type: prev.type, n: v } })
          : x;
      },
    });
  }
  return fields;
}

const LATENCY_MODELS: { value: OrderLatencyConfig['type']; label: string; help: string }[] = [
  {
    value: 'constant',
    label: 'Constant (constant)',
    help: 'Every request takes exactly the entry latency to reach the exchange, and every response the response latency to come back.',
  },
  {
    value: 'jitter',
    label: 'Constant plus seeded jitter (jitter)',
    help: 'Each request\u2019s latency is the base plus a seeded uniform draw from [0, jitter]. The engine\u2019s order bus keeps requests in send order, so jitter never reorders them.',
  },
  {
    value: 'log_normal',
    label: 'Seeded log-normal (log_normal)',
    help: 'Every leg is an independent seeded draw median \u00d7 exp(sigma \u00d7 Z), Z ~ N(0, 1). Cancels may have their own entry distribution. Messages never overtake.',
  },
  {
    value: 'heavy_tail',
    label: 'Seeded heavy tail, ABIDES cubic (heavy_tail)',
    help: 'Every leg is min + (jitter / x\u00b3) \u00d7 (min / unit) with x uniform on (clip, 1]: a clipped power-law tail where rare messages are very slow. Messages never overtake.',
  },
];

function isLatencyModel(v: string): v is OrderLatencyConfig['type'] {
  return LATENCY_MODELS.some((m) => m.value === v);
}

const LEG_TEXT: Record<LegName, string> = {
  entry: 'Entry (strategy host to exchange) of new orders',
  response: 'Response (exchange to strategy host: acknowledgement, fill, cancel)',
  cancel: 'Entry of cancel requests',
};

const LEG_LABEL: Record<LegName, string> = { entry: 'Entry', response: 'Response', cancel: 'Cancel entry' };

function logNormalLegFields(leg: LegName): Field<RunConfig>[] {
  const get = (x: RunConfig): LogNormalLeg | null => {
    const l = x.execution.order_latency;
    return l.type === 'log_normal' ? l[leg] : null;
  };
  return [
    {
      kind: 'number',
      id: `execution.order_latency.${leg}.median_ns`,
      label: `${LEG_LABEL[leg]} median`,
      help: `${LEG_TEXT[leg]}: median latency, from 1 ns to 60 s.`,
      integer: true,
      unit: 'ms',
      convert: MS_TO_NS,
      min: 1,
      max: MAX_LATENCY_NS,
      get: (x) => get(x)?.median_ns ?? Number.NaN,
      set: (x, v) => patchLogNormalLeg(x, leg, { median_ns: v }),
    },
    {
      kind: 'number',
      id: `execution.order_latency.${leg}.sigma`,
      label: `${LEG_LABEL[leg]} sigma`,
      help: `${LEG_TEXT[leg]}: standard deviation of ln(latency), from 0 to ${MAX_LOG_NORMAL_SIGMA}. Zero makes the leg constant at the median.`,
      integer: false,
      min: 0,
      max: MAX_LOG_NORMAL_SIGMA,
      get: (x) => get(x)?.sigma ?? Number.NaN,
      set: (x, v) => patchLogNormalLeg(x, leg, { sigma: v }),
    },
  ];
}

function cubicLegFields(leg: LegName): Field<RunConfig>[] {
  const get = (x: RunConfig): CubicLeg | null => {
    const l = x.execution.order_latency;
    return l.type === 'heavy_tail' ? l[leg] : null;
  };
  return [
    {
      kind: 'number',
      id: `execution.order_latency.${leg}.min_ns`,
      label: `${LEG_LABEL[leg]} minimum`,
      help: `${LEG_TEXT[leg]}: minimum latency, also the scale of the tail. The largest latency, min + jitter \u00d7 min / (unit \u00d7 clip\u00b3), must be at most 60 s.`,
      integer: true,
      unit: 'ms',
      convert: MS_TO_NS,
      min: 0,
      max: MAX_LATENCY_NS,
      get: (x) => get(x)?.min_ns ?? Number.NaN,
      set: (x, v) => patchCubicLeg(x, leg, { min_ns: v }),
    },
    {
      kind: 'number',
      id: `execution.order_latency.${leg}.jitter`,
      label: `${LEG_LABEL[leg]} jitter`,
      help: `${LEG_TEXT[leg]}: ABIDES jitter, the shape of the cubic curve, in [0, 1]. Zero makes the leg constant at the minimum.`,
      integer: false,
      min: 0,
      max: 1,
      get: (x) => get(x)?.jitter ?? Number.NaN,
      set: (x, v) => patchCubicLeg(x, leg, { jitter: v }),
    },
    {
      kind: 'number',
      id: `execution.order_latency.${leg}.clip`,
      label: `${LEG_LABEL[leg]} clip`,
      help: `${LEG_TEXT[leg]}: ABIDES jitter_clip; x is drawn from (clip, 1], so clip must be strictly between 0 and 1. Smaller values allow longer tails.`,
      integer: false,
      min: 0,
      max: 1,
      get: (x) => get(x)?.clip ?? Number.NaN,
      set: (x, v) => patchCubicLeg(x, leg, { clip: v }),
    },
    {
      kind: 'number',
      id: `execution.order_latency.${leg}.unit`,
      label: `${LEG_LABEL[leg]} unit`,
      help: `${LEG_TEXT[leg]}: ABIDES jitter_unit; the tail is measured in units of min / unit. Finite and positive.`,
      integer: false,
      min: 0,
      get: (x) => get(x)?.unit ?? Number.NaN,
      set: (x, v) => patchCubicLeg(x, leg, { unit: v }),
    },
  ];
}

function latencyFields(c: RunConfig): Field<RunConfig>[] {
  const l = c.execution.order_latency;
  const fields: Field<RunConfig>[] = [
    {
      kind: 'select',
      id: 'execution.order_latency.type',
      label: 'Order latency model',
      help: LATENCY_MODELS.find((m) => m.value === l.type)?.help ?? '',
      options: LATENCY_MODELS,
      get: (x) => x.execution.order_latency.type,
      set: (x, v) => {
        const prev = x.execution.order_latency;
        if (v === prev.type || !isLatencyModel(v)) return x;
        return withExecution(x, { order_latency: switchLatencyModel(prev, v) });
      },
    },
  ];
  switch (l.type) {
    case 'constant':
    case 'jitter':
      fields.push(
        {
          kind: 'number',
          id: 'execution.order_latency.entry_ns',
          label: 'Entry latency',
          help: 'Time for a request to travel from the strategy host to the exchange, at most 60 s. Applies to new orders and cancels alike.',
          integer: true,
          unit: 'ms',
          convert: MS_TO_NS,
          min: 0,
          max: MAX_LATENCY_NS,
          get: (x) => baseEntryNs(x.execution.order_latency),
          set: (x, v) => withLatency(x, { entry_ns: v }),
        },
        {
          kind: 'number',
          id: 'execution.order_latency.response_ns',
          label: 'Response latency',
          help: 'Time for the exchange\u2019s response (acknowledgement, fill, cancel) to reach the strategy host, at most 60 s.',
          integer: true,
          unit: 'ms',
          convert: MS_TO_NS,
          min: 0,
          max: MAX_LATENCY_NS,
          get: (x) => baseResponseNs(x.execution.order_latency),
          set: (x, v) => withLatency(x, { response_ns: v }),
        },
      );
      if (l.type === 'jitter') {
        fields.push({
          kind: 'number',
          id: 'execution.order_latency.jitter_ns',
          label: 'Jitter range',
          help: 'Upper end of the uniform draw added to each request\u2019s latency, at most 60 s. Draws are seeded, so a run is reproducible.',
          integer: true,
          unit: 'ms',
          convert: MS_TO_NS,
          min: 0,
          max: MAX_LATENCY_NS,
          get: (x) =>
            x.execution.order_latency.type === 'jitter'
              ? x.execution.order_latency.jitter_ns
              : Number.NaN,
          set: (x, v) => withLatency(x, { jitter_ns: v }),
        });
      }
      break;
    case 'log_normal':
    case 'heavy_tail': {
      const legFields = l.type === 'log_normal' ? logNormalLegFields : cubicLegFields;
      fields.push(...legFields('entry'), ...legFields('response'), {
        kind: 'select',
        id: 'execution.order_latency.cancel',
        label: 'Cancel entry latency',
        help: 'Cancel requests either draw from the entry parameters (on their own seeded sub-stream) or from a distribution of their own.',
        options: [
          { value: 'as_entry', label: 'Same parameters as entry (cancel: null)' },
          { value: 'own', label: 'Own distribution' },
        ],
        get: (x) => (cancelBaseNs(x.execution.order_latency) === null ? 'as_entry' : 'own'),
        set: (x, v) => {
          const prev = x.execution.order_latency;
          if (prev.type === 'log_normal') {
            const cancel = v === 'own' ? (prev.cancel ?? logNormalLeg(prev.entry.median_ns)) : null;
            return withExecution(x, { order_latency: { ...prev, cancel } });
          }
          if (prev.type === 'heavy_tail') {
            const cancel = v === 'own' ? (prev.cancel ?? cubicLeg(prev.entry.min_ns)) : null;
            return withExecution(x, { order_latency: { ...prev, cancel } });
          }
          return x;
        },
      });
      if (l.cancel) fields.push(...legFields('cancel'));
      break;
    }
  }
  fields.push({
    kind: 'number',
    id: 'execution.think_time_ns',
    label: 'Think time',
    help: 'Strategy compute time: requests decided on a tick are handed to the order path this long after the decision. Must be below the evaluation interval (and at most 60 s); zero sends at the tick.',
    integer: true,
    unit: 'ms',
    convert: MS_TO_NS,
    min: 0,
    max: MAX_LATENCY_NS,
    get: (x) => x.execution.think_time_ns ?? 0,
    // Zero is the server's default and is omitted from the configuration, as the server does.
    set: (x, v) => withExecution(x, { think_time_ns: v === 0 ? undefined : v }),
  });
  return fields;
}

/** Every editable section of a `RunConfig`, in display order. */
export function buildSections(c: RunConfig, ctx: SectionContext): Section[] {
  return [
    {
      id: 'strategy',
      title: 'Strategy',
      description:
        'What decides the orders. A strategy only sees information that had reached the strategy host by each evaluation.',
      modeled: false,
      fields: strategyFields(c, ctx),
    },
    {
      id: 'exchange_model',
      title: 'Exchange model',
      description: 'How the simulated matching engine turns a met fill condition into a fill.',
      modeled: true,
      fields: [
        {
          kind: 'select',
          id: 'execution.exchange_model',
          label: 'Exchange model',
          help: EXCHANGE_MODEL_INFO[c.execution.exchange_model].help,
          options: EXCHANGE_MODELS.map((m) => ({
            value: m,
            label: `${EXCHANGE_MODEL_INFO[m].label} (${m})`,
            help: EXCHANGE_MODEL_INFO[m].help,
          })),
          get: (x) => x.execution.exchange_model,
          set: (x, v) => (isExchangeModel(v) ? withExecution(x, { exchange_model: v }) : x),
        },
      ],
    },
    {
      id: 'queue_model',
      title: 'Queue position',
      description:
        'Level-2 data does not show individual orders, so a resting order\u2019s place in the queue is never observed. This model estimates it.',
      modeled: true,
      fields: queueFields(c),
    },
    {
      id: 'order_latency',
      title: 'Order latency',
      description: 'Delay between the strategy\u2019s decision and the simulated exchange, in both directions.',
      modeled: true,
      fields: latencyFields(c),
    },
    {
      id: 'feed_latency',
      title: 'Feed latency',
      description: 'How late market data reaches the strategy, on top of what the dataset recorded.',
      modeled: true,
      fields: [
        {
          kind: 'number',
          id: 'execution.feed_latency_offset_ns',
          label: 'Feed latency offset',
          help: 'Added to every feed event\u2019s local receipt time, at most 60 s. Zero keeps the dataset\u2019s own exchange-to-local latency. Cannot be negative: the strategy must never see an event before the dataset says it was received.',
          integer: true,
          unit: 'ms',
          convert: MS_TO_NS,
          min: 0,
          max: MAX_LATENCY_NS,
          get: (x) => x.execution.feed_latency_offset_ns,
          set: (x, v) => withExecution(x, { feed_latency_offset_ns: v }),
        },
      ],
    },
    {
      id: 'fees',
      title: 'Fees',
      description: 'Charged on traded value for every simulated fill. A negative fee is a rebate.',
      modeled: true,
      fields: [
        {
          kind: 'number',
          id: 'execution.fees.maker',
          label: 'Maker fee',
          help: 'Applied to fills of resting orders.',
          integer: false,
          unit: 'bp',
          convert: BPS_TO_FRACTION,
          get: (x) => x.execution.fees.maker,
          set: (x, v) => withExecution(x, { fees: { ...x.execution.fees, maker: v } }),
        },
        {
          kind: 'number',
          id: 'execution.fees.taker',
          label: 'Taker fee',
          help: 'Applied to fills of orders that take liquidity on arrival.',
          integer: false,
          unit: 'bp',
          convert: BPS_TO_FRACTION,
          get: (x) => x.execution.fees.taker,
          set: (x, v) => withExecution(x, { fees: { ...x.execution.fees, taker: v } }),
        },
      ],
    },
    {
      id: 'risk',
      title: 'Risk limits',
      description:
        'Pre-trade rules enforced by the strategy harness before a request reaches the engine. A blocked order is recorded on the tape with the rule that stopped it.',
      modeled: false,
      fields: [
        {
          kind: 'number',
          id: 'risk.max_position',
          label: 'Max position',
          help: 'Reject an order whose fill would take |position| above this, counting the locally known position plus all working orders on the same side (worst case).',
          integer: false,
          unit: 'base units',
          get: (x) => x.risk.max_position,
          set: (x, v) => withRisk(x, { max_position: v }),
        },
        {
          kind: 'number',
          id: 'risk.max_order_qty',
          label: 'Max order quantity',
          help: 'Reject any single order larger than this.',
          integer: false,
          unit: 'base units',
          get: (x) => x.risk.max_order_qty,
          set: (x, v) => withRisk(x, { max_order_qty: v }),
        },
        {
          kind: 'number',
          id: 'risk.max_open_orders',
          label: 'Max open orders',
          help: 'Reject new orders while this many are already working.',
          integer: true,
          min: 0,
          max: U32_MAX,
          get: (x) => x.risk.max_open_orders,
          set: (x, v) => withRisk(x, { max_open_orders: v }),
        },
        {
          kind: 'optional_number',
          id: 'risk.max_drawdown',
          label: 'Max drawdown',
          help: 'Halt (cancel everything, stop quoting) once equity falls this far below its running peak.',
          integer: false,
          unit: 'quote currency',
          enableLabel: 'Enforce a drawdown halt',
          get: (x) => x.risk.max_drawdown,
          set: (x, v) => withRisk(x, { max_drawdown: v }),
        },
      ],
    },
    {
      id: 'record',
      title: 'Flight recorder',
      description: 'What the tape keeps. These settings change how much is recorded, not what is simulated; they are still part of the run configuration and its hash.',
      modeled: false,
      fields: [
        {
          kind: 'number',
          id: 'record.sample_every_ticks',
          label: 'Sample every',
          help: 'Equity, position and market are sampled every this many strategy ticks.',
          integer: true,
          unit: 'strategy ticks',
          min: 0,
          max: U32_MAX,
          get: (x) => x.record.sample_every_ticks,
          set: (x, v) => ({ ...x, record: { ...x.record, sample_every_ticks: v } }),
        },
        {
          kind: 'number',
          id: 'record.max_queue_updates_per_order',
          label: 'Queue updates kept per order',
          help: 'Beyond this, the recorder keeps only updates that move the estimate by at least one lot and writes an explicit truncation marker with the number dropped.',
          integer: true,
          min: 0,
          max: U32_MAX,
          get: (x) => x.record.max_queue_updates_per_order,
          set: (x, v) => ({ ...x, record: { ...x.record, max_queue_updates_per_order: v } }),
        },
      ],
    },
    {
      id: 'seed',
      title: 'Seed',
      description: 'Makes every stochastic component reproducible.',
      modeled: false,
      fields: [
        {
          kind: 'number',
          id: 'seed',
          label: 'Seed',
          help: 'Seed for every stochastic component (currently only jittered order latency). The same seed, configuration and dataset reproduce the same result hash.',
          integer: true,
          min: 0,
          max: SAFE_MAX,
          get: (x) => x.seed,
          set: (x, v) => ({ ...x, seed: v }),
        },
      ],
    },
  ];
}

// ----- scripted orders --------------------------------------------------------------------

const SIDES: Side[] = ['buy', 'sell'];
const TIFS: Tif[] = ['gtc', 'gtx', 'ioc', 'fok'];
const ORD_TYPES: OrdType[] = ['limit', 'market'];

const TIF_HELP: Record<Tif, string> = {
  gtc: 'rests until filled or cancelled; takes liquidity if it crosses on arrival',
  gtx: 'post-only; expired by the exchange model if it would cross on arrival',
  ioc: 'executes what it can on arrival, remainder expires',
  fok: 'executes fully on arrival or expires',
};

/** One descriptor per `ScriptedOrder` field (the price is edited as its type plus its value). */
export const SCRIPTED_ORDER_FIELDS: Field<ScriptedOrder>[] = [
  {
    kind: 'number',
    id: 'at_tick',
    label: 'At tick',
    help: 'Strategy tick at which to place the order.',
    integer: true,
    min: 0,
    max: SAFE_MAX,
    get: (o) => o.at_tick,
    set: (o, v) => ({ ...o, at_tick: v }),
  },
  {
    kind: 'number',
    id: 'slot',
    label: 'Slot',
    help: 'Slot within the tick; (at_tick, slot) must be unique across the script.',
    integer: true,
    min: 0,
    max: U8_MAX,
    get: (o) => o.slot,
    set: (o, v) => ({ ...o, slot: v }),
  },
  {
    kind: 'select',
    id: 'side',
    label: 'Side',
    help: 'Buy or sell.',
    options: SIDES.map((s) => ({ value: s, label: s })),
    get: (o) => o.side,
    set: (o, v) => (v === 'buy' || v === 'sell' ? { ...o, side: v } : o),
  },
  {
    kind: 'select',
    id: 'price.type',
    label: 'Price is',
    help: 'Absolute tick, or relative to the same-side best price seen locally at placement (buy: best_bid + offset; sell: best_ask - offset; positive is more aggressive).',
    options: [
      { value: 'from_touch', label: 'offset from touch' },
      { value: 'tick', label: 'absolute tick' },
    ],
    get: (o) => o.price.type,
    set: (o, v) => {
      if (v === o.price.type) return o;
      // The number means something different under the other type, so it starts blank.
      if (v === 'tick') return { ...o, price: { type: 'tick', tick: Number.NaN } };
      if (v === 'from_touch') return { ...o, price: { type: 'from_touch', offset: 0 } };
      return o;
    },
  },
  {
    kind: 'number',
    id: 'price.value',
    label: 'Tick / offset',
    help: 'The absolute price tick, or the offset in ticks from the touch.',
    integer: true,
    unit: 'ticks',
    get: (o) => (o.price.type === 'tick' ? o.price.tick : o.price.offset),
    set: (o, v) =>
      o.price.type === 'tick'
        ? { ...o, price: { type: 'tick', tick: v } }
        : { ...o, price: { type: 'from_touch', offset: v } },
  },
  {
    kind: 'number',
    id: 'qty',
    label: 'Qty',
    help: 'Order quantity in base units; a positive multiple of the lot size.',
    integer: false,
    get: (o) => o.qty,
    set: (o, v) => ({ ...o, qty: v }),
  },
  {
    kind: 'select',
    id: 'tif',
    label: 'TIF',
    help: TIFS.map((t) => `${t}: ${TIF_HELP[t]}`).join('; '),
    options: TIFS.map((t) => ({ value: t, label: t, help: TIF_HELP[t] })),
    get: (o) => o.tif,
    set: (o, v) => {
      const tif = TIFS.find((t) => t === v);
      return tif ? { ...o, tif } : o;
    },
  },
  {
    kind: 'select',
    id: 'ord_type',
    label: 'Type',
    help: 'Limit order. Market orders are listed but not available: hftbacktest 0.9.4 mishandles them (docs/EXECUTION_MODEL.md, defect 5); an immediate-or-cancel limit order priced through the touch takes liquidity instead.',
    options: ORD_TYPES.map((t) =>
      t === 'market'
        ? {
            value: t,
            label: 'market (not supported)',
            help: 'Rejected by the server: market orders are not supported on hftbacktest 0.9.4 (defect 5). Use an IOC limit order.',
            disabled: true,
          }
        : { value: t, label: t },
    ),
    get: (o) => o.ord_type,
    set: (o, v) => {
      const ord_type = ORD_TYPES.find((t) => t === v && t !== 'market');
      return ord_type ? { ...o, ord_type } : o;
    },
  },
  {
    kind: 'optional_number',
    id: 'cancel_after_ticks',
    label: 'Cancel after',
    help: 'Request cancellation this many ticks after placement if still working.',
    integer: true,
    unit: 'ticks',
    min: 0,
    max: SAFE_MAX,
    enableLabel: 'cancel',
    get: (o) => o.cancel_after_ticks,
    set: (o, v) => ({ ...o, cancel_after_ticks: v }),
  },
  {
    kind: 'text',
    id: 'label',
    label: 'Label',
    help: 'Free text shown as the order\u2019s reason in the forensics view.',
    get: (o) => o.label,
    set: (o, v) => ({ ...o, label: v }),
  },
];
