// Plain-language labels and explanations for configuration values. The wording of each
// execution assumption follows the documentation of the corresponding type in
// `crates/bb-core/src/config.rs`; nothing here claims more than the engine model does.

import {
  cubicMaxNs,
  type CubicLeg,
  type ExchangeModel,
  type FeeConfig,
  type LogNormalLeg,
  type OrderLatencyConfig,
  type OrderOutcome,
  type QueueModelConfig,
  type RunConfig,
  type Severity,
  type SigmaSource,
  type StrategyKind,
} from '../api/types';
import { fmtDuration } from './fmt';
import type { BadgeKind } from './ui';

export interface OptionInfo {
  label: string;
  /** One line: what this choice models. */
  help: string;
}

export const EXCHANGE_MODELS: ExchangeModel[] = ['no_partial_fill', 'partial_fill'];

export const EXCHANGE_MODEL_INFO: Record<ExchangeModel, OptionInfo> = {
  no_partial_fill: {
    label: 'No partial fills',
    help: 'A resting order fills completely as soon as its fill condition is met. Never fills in part; optimistic about size.',
  },
  partial_fill: {
    label: 'Partial fills',
    help: 'A resting order at the front of the queue fills by the remaining traded quantity, so it can fill in part; a taking order consumes displayed depth level by level.',
  },
};

export type QueueModelType = QueueModelConfig['type'];

export const QUEUE_MODEL_TYPES: QueueModelType[] = [
  'risk_adverse',
  'log_prob',
  'log_prob2',
  'power_prob',
  'power_prob2',
  'power_prob3',
];

export const QUEUE_MODEL_INFO: Record<QueueModelType, OptionInfo> = {
  risk_adverse: {
    label: 'Risk-averse',
    help: 'The queue ahead shrinks only when trades print at the order\u2019s price. Cancellations ahead are never credited. Most conservative.',
  },
  log_prob: {
    label: 'Log probability',
    help: 'Trades at the price reduce the queue ahead; a decrease in displayed quantity is split between ahead and behind with p(behind) = f(back) / (f(back) + f(front)), f(x) = ln(1 + x).',
  },
  log_prob2: {
    label: 'Log probability 2',
    help: 'Trades at the price reduce the queue ahead; a decrease in displayed quantity is split with p(behind) = f(back) / f(back + front), f(x) = ln(1 + x).',
  },
  power_prob: {
    label: 'Power probability',
    help: 'Trades at the price reduce the queue ahead; a decrease in displayed quantity is split with p(behind) = f(back) / (f(back) + f(front)), f(x) = x^n.',
  },
  power_prob2: {
    label: 'Power probability 2',
    help: 'Trades at the price reduce the queue ahead; a decrease in displayed quantity is split with p(behind) = f(back) / f(back + front), f(x) = x^n.',
  },
  power_prob3: {
    label: 'Power probability 3',
    help: 'Trades at the price reduce the queue ahead; a decrease in displayed quantity is split with p(behind) = 1 - (front / (front + back))^n.',
  },
};

export function queueModelHasExponent(
  q: QueueModelConfig,
): q is Extract<QueueModelConfig, { n: number }> {
  return q.type === 'power_prob' || q.type === 'power_prob2' || q.type === 'power_prob3';
}

/** Plain name of a queue model, e.g. "Power probability 3 (n = 3)". */
export function queueModelLabel(q: QueueModelConfig): string {
  const name = QUEUE_MODEL_INFO[q.type].label;
  return queueModelHasExponent(q) ? `${name} (n = ${q.n})` : name;
}

/** Plain name of an exchange model, e.g. "No partial fills". */
export function exchangeModelLabel(m: ExchangeModel): string {
  return EXCHANGE_MODEL_INFO[m].label;
}

/** Plain name of a strategy, e.g. "Touch maker". */
export function strategyLabel(kind: StrategyKind): string {
  return strategyName(kind.type);
}

/** Plain name for a strategy kind id as the run list reports it; unknown ids pass through. */
export function strategyName(type: string | null): string {
  if (type === null) return '\u2014';
  return (STRATEGY_INFO as Record<string, OptionInfo | undefined>)[type]?.label ?? type;
}

export const STRATEGY_INFO: Record<StrategyKind['type'], OptionInfo> = {
  touch_maker: {
    label: 'Touch maker',
    help: 'Joins the best bid and best ask with post-only orders and skews away from inventory.',
  },
  avellaneda_stoikov: {
    label: 'Avellaneda-Stoikov',
    help: 'Avellaneda-Stoikov (2008) reservation price and spread.',
  },
  glft: {
    label: 'GLFT market maker',
    help: 'Gu\u00e9ant-Lehalle-Fernandez-Tapia (2013) asymptotic quotes with Gu\u00e9ant (2017) trade size; volatility estimated from a chosen price series.',
  },
  imbalance_taker: {
    label: 'Imbalance taker',
    help: 'Takes liquidity when top-of-book imbalance exceeds a threshold, exits after a holding time.',
  },
  scripted: {
    label: 'Scripted',
    help: 'Places exactly the listed orders. Used for curated forensics demos and tests.',
  },
};

export const SIGMA_SOURCES: SigmaSource[] = ['mid', 'weighted_mid', 'trade'];

export const SIGMA_SOURCE_INFO: Record<SigmaSource, OptionInfo> = {
  mid: {
    label: 'Mid',
    help: 'Mid (bid + ask) / 2. Jumps by half the spread whenever one side of a wide book moves.',
  },
  weighted_mid: {
    label: 'Size-weighted mid',
    help: '(bid \u00d7 ask_qty + ask \u00d7 bid_qty) / (bid_qty + ask_qty) (Stoikov 2018, eqs. 2-3; not his calibrated micro-price).',
  },
  trade: {
    label: 'Last trade',
    help: 'Last trade price received locally, carried forward between trades.',
  },
};

function logNormalLegLabel(l: LogNormalLeg): string {
  return `LN(median ${fmtDuration(l.median_ns)}, \u03c3 ${l.sigma})`;
}

function cubicLegLabel(l: CubicLeg): string {
  return `cubic(min ${fmtDuration(l.min_ns)}, jitter ${l.jitter}, clip ${l.clip}, unit ${l.unit}; max ${fmtDuration(Math.round(cubicMaxNs(l)))})`;
}

/**
 * `entry / response` order latency, with jitter when configured; for the distribution models,
 * each leg's parameters and a separate cancel-entry leg when there is one.
 */
export function latencyLabel(l: OrderLatencyConfig): string {
  switch (l.type) {
    case 'constant':
      return `${fmtDuration(l.entry_ns)} / ${fmtDuration(l.response_ns)}`;
    case 'jitter':
      return `${fmtDuration(l.entry_ns)} / ${fmtDuration(l.response_ns)} + U[0, ${fmtDuration(l.jitter_ns)}]`;
    case 'log_normal': {
      const base = `${logNormalLegLabel(l.entry)} / ${logNormalLegLabel(l.response)}`;
      return l.cancel ? `${base}, cancel ${logNormalLegLabel(l.cancel)}` : base;
    }
    case 'heavy_tail': {
      const base = `${cubicLegLabel(l.entry)} / ${cubicLegLabel(l.response)}`;
      return l.cancel ? `${base}, cancel ${cubicLegLabel(l.cancel)}` : base;
    }
  }
}

/** Fraction of traded value expressed in basis points (1 bp = 0.0001), free of float noise. */
export function fractionToBps(fraction: number): number {
  return Number((fraction * 10_000).toPrecision(12));
}

/**
 * Basis points to a fraction of traded value. Division keeps `7 bp` exactly `0.0007`; the
 * precision round trims results such as `0.3 / 10000 = 0.000029999999999999997` to `0.00003`.
 */
export function bpsToFraction(bps: number): number {
  return Number((bps / 10_000).toPrecision(12));
}

export function feeLabel(f: FeeConfig): string {
  return `${fractionToBps(f.maker)} / ${fractionToBps(f.taker)} bp`;
}

/** The execution assumptions of a run on one line, for tables. */
export function assumptionsLabel(c: RunConfig): string {
  const e = c.execution;
  const parts = [
    exchangeModelLabel(e.exchange_model),
    queueModelLabel(e.queue_model),
    `order latency ${latencyLabel(e.order_latency)}`,
    `fees ${feeLabel(e.fees)}`,
  ];
  if (e.feed_latency_offset_ns !== 0) {
    parts.push(`feed +${fmtDuration(e.feed_latency_offset_ns)}`);
  }
  if (e.think_time_ns) {
    parts.push(`think ${fmtDuration(e.think_time_ns)}`);
  }
  return parts.join(' \u00b7 ');
}

const TIF_TEXT: Record<string, string> = {
  gtc: 'good till cancel',
  gtx: 'post-only',
  ioc: 'immediate or cancel',
  fok: 'fill or kill',
};

/** Plain words for a time in force (`gtx` → "post-only"); an unknown code passes through. */
export function tifLabel(tif: string): string {
  return TIF_TEXT[tif] ?? tif;
}

export const ORDER_OUTCOMES: OrderOutcome[] = [
  'filled',
  'partially_filled',
  'canceled',
  'expired',
  'working_at_end',
  'in_flight_at_end',
  'risk_blocked',
  'refused',
];

export const OUTCOME_MEANING: Record<OrderOutcome, string> = {
  risk_blocked: 'Stopped by a risk rule; never sent.',
  refused: 'The engine refused the request synchronously; never sent.',
  in_flight_at_end: 'Sent, but the data ended before the exchange processed it.',
  filled: 'Filled completely (simulated).',
  partially_filled: 'Partly filled, then cancelled, expired or still working at the end.',
  canceled: 'Cancelled with no fill.',
  expired:
    'Expired by the exchange model with no fill (post-only would cross, IOC/FOK found nothing).',
  working_at_end: 'Still resting, unfilled, when the data ended.',
};

export function severityBadge(s: Severity): BadgeKind {
  switch (s) {
    case 'info':
      return 'neutral';
    case 'warn':
      return 'warn';
    case 'error':
      return 'error';
  }
}
