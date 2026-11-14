// Quant-term definitions for header, KPI and field tooltips (UI_DESIGN_SPEC.md §6.14). One module,
// worded to match the linked repository documents; the evidence bases reuse BASIS_MEANING.

import { BASIS_MEANING } from './ui';

export interface GlossaryEntry {
  term: string;
  definition: string;
  /** Repository document that is the source of truth for the wording. */
  source: string;
}

export const GLOSSARY = {
  markout: {
    term: 'Markout (h)',
    definition:
      'Signed move of the mid h after a fill, in ticks: side \u00d7 (mid(t_fill + h) \u2212 fill price). Negative means the market moved against the fill (adverse selection).',
    source: 'docs/PNL_ACCOUNTING.md',
  },
  spread_capture: {
    term: 'Spread capture',
    definition:
      'Half-spread earned at the instant of each passive fill against the dataset mid then; taker fills are excluded. Not additive with net P&L.',
    source: 'docs/PNL_ACCOUNTING.md',
  },
  queue_position: {
    term: 'Queue position',
    definition:
      'Quantity modeled ahead of the order at its price. L2 data does not show individual orders, so it is an estimate.',
    source: 'docs/QUEUE_MODELS.md',
  },
  latency: {
    term: 'Entry / response latency',
    definition:
      "Modeled delay from the strategy's decision to the simulated exchange (entry), and from the exchange back to the strategy (response).",
    source: 'docs/LATENCY_MODEL.md',
  },
  round_trip: {
    term: 'Round trip',
    definition: 'Entry plus response latency for one request, as realised in the run.',
    source: 'docs/LATENCY_MODEL.md',
  },
  feed_latency: {
    term: 'Feed latency',
    definition: 'Local receipt time minus exchange time of market data.',
    source: 'docs/MARKET_DATA.md',
  },
  fill_ratio: {
    term: 'Fill ratio (orders / qty)',
    definition: 'Orders with a fill \u00f7 orders submitted; filled \u00f7 submitted quantity.',
    source: 'docs/PNL_ACCOUNTING.md',
  },
  clocks: {
    term: 'Exchange clock / local clock',
    definition:
      'Time at the simulated matching engine vs time at the strategy host; every event names its clock.',
    source: 'docs/ARCHITECTURE.md',
  },
  fees: {
    term: 'Maker / taker fee, rebate',
    definition:
      'Fee on passive / aggressive fills as a fraction of traded value; a negative maker fee is a rebate.',
    source: 'components/describe.ts',
  },
  observed: { term: 'Observed', definition: BASIS_MEANING.observed, source: 'components/ui.tsx' },
  modeled: { term: 'Modeled', definition: BASIS_MEANING.modeled, source: 'components/ui.tsx' },
  derived: { term: 'Derived', definition: BASIS_MEANING.derived, source: 'components/ui.tsx' },
  unobservable: { term: 'Unobservable', definition: BASIS_MEANING.unobservable, source: 'components/ui.tsx' },
  l2: {
    term: 'L2 (market by price)',
    definition:
      'Aggregate displayed quantity per price level; individual orders and hidden liquidity are not visible.',
    source: 'docs/MARKET_DATA.md',
  },
  drawdown: {
    term: 'Drawdown',
    definition: 'Largest peak-to-trough fall of simulated equity over the run, in quote currency.',
    source: 'docs/PNL_ACCOUNTING.md',
  },
} satisfies Record<string, GlossaryEntry>;

export type GlossaryKey = keyof typeof GLOSSARY;
