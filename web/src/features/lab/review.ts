// Plain-language helpers for the Strategy Lab's Review step: why a changed setting matters, the
// one-line gist of a preset, and a size estimate derived from the dataset span and the interval.

import type { DatasetSummary } from '../../api/model';
import type { RunConfig } from '../../api/types';

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

/** The first sentence of a description (numbers like 0.6 do not end a sentence). */
export function firstSentence(text: string): string {
  const m = /^(.+?[.!?])(\s|$)/.exec(text.trim());
  return m ? m[1] : text.trim();
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
