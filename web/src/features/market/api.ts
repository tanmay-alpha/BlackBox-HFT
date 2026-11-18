// Client for the Market Data Workbench (`/api/v1/market`, contract: docs/research/
// MARKET_DATA_WORKBENCH.md). Every method returns exactly what the server sent; nothing is
// synthesized, cached or retried. Timestamps are ITCH session-relative nanoseconds (since the
// session's midnight) and stay decimal strings here; order references are u64 decimal strings.
// Prices are `price4` integers (quote currency x 10,000), quantities are whole shares.

import { request } from '../../api/http';
import type { components } from '../../api/v1.gen';
import type { QualityCheck } from '../../api/types';

const enc = encodeURIComponent;

export interface Opts {
  signal?: AbortSignal;
}

/** Decimal nanoseconds since the session's midnight (`"34200000000000"`). */
export type SessionNs = string;

export interface MarketQuality {
  passed: boolean;
  quarantined_total: number;
  /** The reconstruction's own quality checks, unchanged. */
  checks: QualityCheck[];
}

export interface MarketSummary {
  dataset_id: string;
  /** SHA-256 of the source bytes. */
  dataset_version: string;
  data_class: 'synthetic' | string;
  symbol: string;
  venue: string;
  session: string;
  timestamp_basis: string;
  /** Number of source frames (event indices run `0..events-1`). */
  events: number;
  first_ns: SessionNs | null;
  last_ns: SessionNs | null;
  messages_by_type: Record<string, number>;
  quality: MarketQuality;
  generator: unknown;
  license_note: string;
}

export type ApplyOutcome = 'book' | 'print' | 'no_effect' | 'quarantined' | 'invalid';

export interface MarketEvent {
  /** Zero-based physical frame sequence of the source; gaps belong to other symbols. */
  event_index: number;
  message_type: string;
  ts_ns: SessionNs | null;
  order_ref: string | null;
  new_order_ref: string | null;
  side: 'buy' | 'sell' | null;
  price4: number | null;
  shares: number | null;
  /** Best bid and ask after this event. */
  best_bid_price4: number | null;
  best_ask_price4: number | null;
  applied: ApplyOutcome | string;
}

export interface EventsPage {
  items: MarketEvent[];
  next_cursor: string | null;
  truncated_by: 'limit' | 'bytes' | null;
}

export interface BookOrder {
  order_ref: string;
  shares: number;
  /** 1-based FIFO position at its price, in reconstruction arrival order. */
  queue_position: number;
  shares_ahead: number;
  entered_ns: SessionNs | null;
  entered_event_index: number;
}

export interface BookLevel {
  price4: number;
  /** Total of ALL resting orders at the price (the L2 aggregate), not just the listed ones. */
  shares: number;
  order_count: number;
  orders: BookOrder[];
  orders_truncated: boolean;
}

export interface MarketBook {
  event_index: number;
  ts_ns: SessionNs | null;
  levels: number;
  bids: BookLevel[];
  asks: BookLevel[];
}

/** One resting order in a compact FIFO depth view; its queue position is its array index + 1. */
export interface QueueSlot {
  order_ref: string;
  shares: number;
}

export interface DepthLevel {
  price4: number;
  /** Total of ALL resting orders at the price (the L2 aggregate). */
  shares: number;
  order_count: number;
  orders: QueueSlot[];
  orders_truncated: boolean;
}

export interface Depth {
  /** Best first. */
  bids: DepthLevel[];
  asks: DepthLevel[];
}

export type LevelRole = 'order' | 'replaced' | 'replaced_from' | 'replaced_to';

/** A price level the event changed, complete before and after it (the selected orders are always listed). */
export interface AffectedLevel {
  side: 'buy' | 'sell';
  price4: number;
  role: LevelRole | string;
  before: BookLevel;
  after: BookLevel;
}

export interface OrderPosition {
  order_ref: string;
  side: 'buy' | 'sell';
  price4: number;
  shares: number;
  /** 1-based FIFO position at its price. */
  queue_position: number;
  level_orders: number;
  /** Shares of the orders ahead of it at its price, excluding itself. */
  shares_ahead: number;
  level_shares: number;
  entered_ns: SessionNs | null;
  entered_event_index: number;
  /** Event timestamp minus the accepted add/replace timestamp, decimal ns. */
  age_ns: string | null;
}

export interface LevelChange {
  side: 'buy' | 'sell';
  price4: number;
  before_shares: number;
  after_shares: number;
  before_orders: number;
  after_orders: number;
}

export interface SameTimestampEvent {
  event_index: number;
  message_type: string;
  applied: ApplyOutcome | string;
  level_changes: LevelChange[];
}

export interface SameTimestamp {
  ts_ns: SessionNs | null;
  /** 1-based position of the selected event among the events sharing its timestamp, up to it. */
  position: number;
  first_event_index: number;
  /** Up to and including the selected event, in source order (never a later one). */
  events: SameTimestampEvent[];
  events_truncated: boolean;
}

/** The book immediately before (after event `before_event_index`) and after one event. */
export interface EventDetail {
  event: MarketEvent;
  /** `null`: the event is the first frame, the before-state is the empty book. */
  before_event_index: number | null;
  levels: number;
  orders_per_level: number;
  before: Depth;
  after: Depth;
  affected: AffectedLevel[];
  order_before: OrderPosition | null;
  order_after: OrderPosition | null;
  /** Completed lifetime when this event removed or replaced the referenced order. */
  lifetime_ns: string | null;
  same_timestamp: SameTimestamp;
}

export interface CuratedExample {
  id: string;
  title: string;
  event_index: number;
  /** The deterministic selection rule the event satisfies. */
  rule: string;
}

export interface MarketComparison {
  basis: string;
  events: number;
  order_identity_observations: number;
  touch_queue_observations: number;
  execution_messages: number;
  executions_with_order_identity: number;
  cancel_messages: number;
  cancels_ahead_of_reference: number;
  cancels_at_reference: number;
  cancels_behind_reference: number;
  same_timestamp_add_cancel_level_groups: number;
  l2_unobservable: string[];
  definitions: Record<string, string>;
}

export interface MetricPoint {
  event_index: number;
  ts_ns: SessionNs | null;
  live_orders: number;
  /** Whole-book totals of the symbol. */
  bid_shares: number;
  ask_shares: number;
  order_count_imbalance: number | null;
  /** Per event (not cumulative); buy positive, sell negative. */
  signed_added_shares: number;
  signed_cancelled_shares: number;
  signed_executed_shares: number;
  touch_depleted_shares: number;
  touch_replenished_shares: number;
  best_bid_price4: number | null;
  best_ask_price4: number | null;
}

export interface Distribution<T> {
  n: number;
  min: T | null;
  p50: T | null;
  p90: T | null;
  max: T | null;
}

export type MarketReport = components['schemas']['MarketReport'];

export interface MarketMetrics {
  basis: string;
  definitions: Record<string, string>;
  counts: Record<string, number>;
  rates_per_second: Record<string, number | null>;
  order_lifetime_ns: Distribution<SessionNs>;
  queue_position_at_execution: Distribution<number>;
  series: MetricPoint[];
  /** The existing bb-analytics L2 report (M1–M9) over the reconstructed L2 rows. */
  l2: MarketReport | null;
}

export interface EventsQuery {
  cursor?: string | null;
  /** 1..500 */
  limit?: number;
  /** Exactly one ITCH message-type letter. */
  message_type?: string | null;
  from_event_index?: number | null;
}

export interface BookQuery {
  event_index: number;
  /** 1..20 */
  levels?: number;
  /** 1..100 */
  orders_per_level?: number;
}

const base = (id: string) => `/market/datasets/${enc(id)}`;

export const marketApi = {
  datasets: (o: Opts = {}) => request<{ items: MarketSummary[] }>('GET', '/market/datasets', o),
  dataset: (id: string, o: Opts = {}) => request<MarketSummary>('GET', base(id), o),
  events: (id: string, q: EventsQuery = {}, o: Opts = {}) =>
    request<EventsPage>('GET', `${base(id)}/events`, {
      query: {
        cursor: q.cursor ?? undefined,
        limit: q.limit,
        message_type: q.message_type ?? undefined,
        from_event_index: q.from_event_index ?? undefined,
      },
      signal: o.signal,
    }),
  book: (id: string, q: BookQuery, o: Opts = {}) => request<MarketBook>('GET', `${base(id)}/book`, { query: q, signal: o.signal }),
  comparison: (id: string, o: Opts = {}) => request<MarketComparison>('GET', `${base(id)}/comparison`, o),
  eventDetail: (id: string, index: number, q: { levels?: number; orders_per_level?: number } = {}, o: Opts = {}) =>
    request<EventDetail>('GET', `${base(id)}/events/${index}`, { query: q, signal: o.signal }),
  examples: (id: string, o: Opts = {}) => request<{ items: CuratedExample[] }>('GET', `${base(id)}/examples`, o),
  metrics: (id: string, q: { max_points?: number } = {}, o: Opts = {}) =>
    request<MarketMetrics>('GET', `${base(id)}/metrics`, { query: q, signal: o.signal }),
};
