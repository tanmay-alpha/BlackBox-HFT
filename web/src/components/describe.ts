// Plain-language labels and explanations for configuration values. The wording of each
// execution assumption follows the documentation of the corresponding type in
// `crates/bb-core/src/config.rs`; nothing here claims more than the engine model does.

import type {
  ExchangeModel,
  FeeConfig,
  OrderLatencyConfig,
  OrderOutcome,
  QueueModelConfig,
  RunConfig,
  Severity,
  StrategyKind,
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
    label: 'Risk-adverse',
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

/** Mirrors `QueueModelConfig::label`. */
export function queueModelLabel(q: QueueModelConfig): string {
  return queueModelHasExponent(q) ? `${q.type}(n=${q.n})` : q.type;
}

/** Mirrors `StrategyKind::label`. */
export function strategyLabel(kind: StrategyKind): string {
  return kind.type;
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
  imbalance_taker: {
    label: 'Imbalance taker',
    help: 'Takes liquidity when top-of-book imbalance exceeds a threshold, exits after a holding time.',
  },
  scripted: {
    label: 'Scripted',
    help: 'Places exactly the listed orders. Used for curated forensics demos and tests.',
  },
};

/** `entry / response` order latency, with jitter when configured. */
export function latencyLabel(l: OrderLatencyConfig): string {
  const base = `${fmtDuration(l.entry_ns)} / ${fmtDuration(l.response_ns)}`;
  return l.type === 'jitter' ? `${base} + U[0, ${fmtDuration(l.jitter_ns)}]` : base;
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
    e.exchange_model,
    queueModelLabel(e.queue_model),
    `order latency ${latencyLabel(e.order_latency)}`,
    `fees ${feeLabel(e.fees)}`,
  ];
  if (e.feed_latency_offset_ns !== 0) {
    parts.push(`feed +${fmtDuration(e.feed_latency_offset_ns)}`);
  }
  return parts.join(' \u00b7 ');
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
