// TypeScript mirror of the BlackBox type contract.
//
// Source of truth: `crates/bb-core/src/*.rs` (serde JSON form) and the API-only types in
// `docs/API.md`. Names and field names are identical to the Rust side. Mapping rules:
//   unit enum                      -> snake_case string-literal union
//   #[serde(tag = "type")] enum    -> discriminated union on `type` (newtype variants flatten)
//   Option<T>                      -> T | null
//   tuple                          -> fixed-length array
//   u64 / i64 / f64                -> number
//   origin_ns                      -> decimal string (absolute epoch ns exceed 2^53)
//
// Not mirrored because they are never serialized: `LocalView`, the `Strategy` trait, `Ns`.

// ---------------------------------------------------------------------------------------------
// lib.rs
// ---------------------------------------------------------------------------------------------

/** The API contract this console reads (`Status.contract_version`; ns values as strings). */
export const CONTRACT_VERSION = 2;
export const ENGINE_NAME = 'hftbacktest';
export const ENGINE_VERSION = '0.9.4';

/** Nanoseconds relative to a dataset's `origin_ns`. Safe as a JSON number. */
export type RelNs = number;

/** Order side. Also the aggressor side of a market trade. */
export type Side = 'buy' | 'sell';

/** Time in force, with the pinned engine's semantics (`gtx` is post-only). */
export type Tif = 'gtc' | 'gtx' | 'ioc' | 'fok';

export type OrdType = 'limit' | 'market';

/** Order status as reported by the engine. */
export type OrderStatus =
  | 'none'
  | 'new'
  | 'expired'
  | 'filled'
  | 'canceled'
  | 'partially_filled'
  | 'unsupported';

/** Which kind of request a latency or response event refers to. */
export type ReqKind = 'new' | 'cancel' | 'modify' | 'none';

/** Clock domain of a timestamp. */
export type Clock = 'exchange' | 'local';

/** Who produced a recorded fact. */
export type Source = 'engine' | 'strategy' | 'recorder' | 'derived';

/** How strongly a statement is supported. */
export type Basis = 'observed' | 'modeled' | 'derived' | 'unobservable';

/** `serde_json::Value`. */
export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

// ---------------------------------------------------------------------------------------------
// time.rs
// ---------------------------------------------------------------------------------------------

export type TimeUnit = 'seconds' | 'millis' | 'micros' | 'nanos';

export const NS_PER_US = 1_000;
export const NS_PER_MS = 1_000_000;
export const NS_PER_S = 1_000_000_000;

// ---------------------------------------------------------------------------------------------
// config.rs
// ---------------------------------------------------------------------------------------------

/** Complete, self-contained description of one simulation run. */
export interface RunConfig {
  dataset_id: string;
  strategy: StrategyConfig;
  execution: ExecutionConfig;
  risk: RiskConfig;
  record: RecordConfig;
  /** Seed for every stochastic component: the `jitter`, `log_normal` and `heavy_tail` latency models. */
  seed: number;
}

/** The modeled execution assumptions. These are what counterfactual scenarios vary. */
export interface ExecutionConfig {
  exchange_model: ExchangeModel;
  queue_model: QueueModelConfig;
  order_latency: OrderLatencyConfig;
  /**
   * Strategy compute ("think") time: requests decided on a tick are sent this long after the
   * decision. Must be below `strategy.interval_ns`. The server omits it when zero.
   */
  think_time_ns?: number;
  /** Added to every feed event's local receipt time. Zero keeps the dataset's own latency. */
  feed_latency_offset_ns: number;
  fees: FeeConfig;
}

export type ExchangeModel = 'no_partial_fill' | 'partial_fill';

/** Queue-position model for resting orders on Market-By-Price data. All are estimates. */
export type QueueModelConfig =
  | { type: 'risk_adverse' }
  | { type: 'log_prob' }
  | { type: 'log_prob2' }
  | { type: 'power_prob'; n: number }
  | { type: 'power_prob2'; n: number }
  | { type: 'power_prob3'; n: number };

/**
 * Order-entry and order-response latency. `constant` and `jitter` apply one entry latency to new
 * orders and cancels alike; `log_normal` and `heavy_tail` may give cancels their own entry
 * distribution (`cancel: null` draws cancels from `entry`'s parameters).
 */
export type OrderLatencyConfig =
  | { type: 'constant'; entry_ns: number; response_ns: number }
  | { type: 'jitter'; entry_ns: number; response_ns: number; jitter_ns: number }
  | { type: 'log_normal'; entry: LogNormalLeg; response: LogNormalLeg; cancel: LogNormalLeg | null }
  | { type: 'heavy_tail'; entry: CubicLeg; response: CubicLeg; cancel: CubicLeg | null };

/** One leg of the `log_normal` latency model: `median_ns * exp(sigma * Z)`, `Z ~ N(0, 1)`. */
export interface LogNormalLeg {
  /** Median latency, `exp(mu)`, in ns. */
  median_ns: number;
  /** Standard deviation of `ln(latency)`. Zero makes the leg constant at the median. */
  sigma: number;
}

/**
 * One leg of the `heavy_tail` (ABIDES cubic) latency model: `min + (jitter / x^3) * (min / unit)`
 * with `x` uniform on `(clip, 1]`.
 */
export interface CubicLeg {
  /** Minimum latency in ns; also the scale of the jitter. */
  min_ns: number;
  /** ABIDES `jitter`: shape of the cubic curve, in `[0, 1]`. */
  jitter: number;
  /** ABIDES `jitter_clip`: `x` is drawn from `(clip, 1]`; in `(0, 1)`. */
  clip: number;
  /** ABIDES `jitter_unit`: the jitter is measured in units of `min / unit`. */
  unit: number;
}

/** Largest latency a `heavy_tail` leg can produce (`CubicLeg::max_ns`, at `x = clip`), in ns. */
export function cubicMaxNs(l: CubicLeg): number {
  return l.min_ns + (l.jitter / (l.clip * l.clip * l.clip)) * (l.min_ns / l.unit);
}

/** The entry-path location parameter (`OrderLatencyConfig::base_entry_ns`). */
export function baseEntryNs(l: OrderLatencyConfig): number {
  switch (l.type) {
    case 'constant':
    case 'jitter':
      return l.entry_ns;
    case 'log_normal':
      return l.entry.median_ns;
    case 'heavy_tail':
      return l.entry.min_ns;
  }
}

/** The response-path location parameter: `response_ns`, the response median or minimum. */
export function baseResponseNs(l: OrderLatencyConfig): number {
  switch (l.type) {
    case 'constant':
    case 'jitter':
      return l.response_ns;
    case 'log_normal':
      return l.response.median_ns;
    case 'heavy_tail':
      return l.response.min_ns;
  }
}

/** Fees as a fraction of traded value. Negative means a rebate. `0.0002` is 2 basis points. */
export interface FeeConfig {
  maker: number;
  taker: number;
}

/** Pre-trade risk rules enforced by the strategy harness. */
export interface RiskConfig {
  max_position: number;
  max_order_qty: number;
  max_open_orders: number;
  /** `null` disables the rule. */
  max_drawdown: number | null;
}

/** What the flight recorder keeps. */
export interface RecordConfig {
  sample_every_ticks: number;
  max_queue_updates_per_order: number;
}

export interface StrategyConfig {
  /** Local-clock time between strategy evaluations. */
  interval_ns: number;
  kind: StrategyKind;
}

export interface TouchMakerParams {
  order_qty: number;
  max_position: number;
  offset_ticks: number;
  skew_ticks_per_unit: number;
}

export interface AvellanedaStoikovParams {
  order_qty: number;
  max_position: number;
  gamma: number;
  kappa: number;
  horizon_s: number;
  vol_halflife_ticks: number;
}

/** Price series whose clock-sampled changes estimate GLFT's `sigma`. */
export type SigmaSource = 'mid' | 'weighted_mid' | 'trade';

/** Guéant-Lehalle-Fernandez-Tapia market maker. Prices in price units, quantities in base units. */
export interface GlftParams {
  /** Quote size `Delta` (base units); also the trade size in the formulas. */
  order_qty: number;
  /** Inventory bound `Q` (base units). */
  max_position: number;
  /** Absolute risk aversion `gamma`, per unit of wealth. */
  gamma: number;
  /** Intensity decay `k` (per price unit) in `Lambda(delta) = A * exp(-k * delta)`. */
  kappa: number;
  /** Intensity scale `A`: fills per second of a quote at the reference price. */
  intensity_a: number;
  sigma_source: SigmaSource;
  /** Half-life, in strategy ticks, of the exponentially weighted variance estimate. */
  vol_halflife_ticks: number;
}

export interface ImbalanceTakerParams {
  order_qty: number;
  max_position: number;
  threshold: number;
  hold_ticks: number;
  cooldown_ticks: number;
}

export interface ScriptedParams {
  orders: ScriptedOrder[];
}

export interface ScriptedOrder {
  at_tick: number;
  /** `(at_tick, slot)` must be unique across the script. 0..=255. */
  slot: number;
  side: Side;
  price: ScriptedPrice;
  qty: number;
  tif: Tif;
  ord_type: OrdType;
  cancel_after_ticks: number | null;
  label: string;
}

export type ScriptedPrice = { type: 'tick'; tick: number } | { type: 'from_touch'; offset: number };

/** Newtype variants serialize with the parameter struct's fields next to `type`. */
export type StrategyKind =
  | ({ type: 'touch_maker' } & TouchMakerParams)
  | ({ type: 'avellaneda_stoikov' } & AvellanedaStoikovParams)
  | ({ type: 'glft' } & GlftParams)
  | ({ type: 'imbalance_taker' } & ImbalanceTakerParams)
  | ({ type: 'scripted' } & ScriptedParams);

// ---------------------------------------------------------------------------------------------
// dataset.rs
// ---------------------------------------------------------------------------------------------

export interface InstrumentMeta {
  tick_size: number;
  lot_size: number;
  contract_size: number;
}

/** `synthetic` is generated by BlackBox from a seed and is not market data. */
export type DatasetKind = 'synthetic' | 'recorded';

/** Where the local receipt timestamps came from. */
export type LocalTsProvenance = 'measured' | 'synthetic' | 'assumed';

/** Level of market data the dataset actually contains. */
export type DataLevel = 'l2_mbp_with_trades';

export interface SourceInfo {
  vendor: string;
  exchange: string;
  symbol: string;
  inputs: InputFile[];
  license_note: string;
  exch_ts_semantics: string;
}

export interface InputFile {
  name: string;
  /** `"trades"`, `"incremental_book_l2"` or `"synthetic"`. */
  role: string;
  bytes: number;
  sha256: string;
  rows: number;
}

export interface TimestampInfo {
  input_unit: TimeUnit;
  local_ts: LocalTsProvenance;
  local_ts_shift_ns: number;
  note: string;
}

export interface EventCounts {
  rows: number;
  trades: number;
  depth_updates: number;
  snapshot_levels: number;
  snapshots: number;
  depth_clears: number;
}

export type Severity = 'info' | 'warn' | 'error';

export interface QualityCheck {
  id: string;
  title: string;
  severity: Severity;
  /** `true` when no occurrence was found. */
  passed: boolean;
  count: number;
  detail: string;
  examples: string[];
}

export interface Quantiles {
  n: number;
  min: number;
  p50: number;
  p90: number;
  p99: number;
  max: number;
  mean: number;
}

export interface QualityReport {
  checks: QualityCheck[];
  feed_latency_ns: Quantiles | null;
  spread_ticks: Quantiles | null;
  inter_event_gap_ns: Quantiles | null;
  book_valid_fraction: number;
}

export interface DatasetManifest {
  contract_version: number;
  id: string;
  title: string;
  kind: DatasetKind;
  level: DataLevel;
  source: SourceInfo;
  instrument: InstrumentMeta;
  /** Absolute epoch nanoseconds of the dataset origin, as a decimal string. */
  origin_ns: string;
  start: RelNs;
  end: RelNs;
  timestamps: TimestampInfo;
  counts: EventCounts;
  events_file: string;
  /** The dataset hash recorded in run manifests. */
  events_sha256: string;
  events_bytes: number;
  generator: JsonValue | null;
  quality: QualityReport;
}

// ---------------------------------------------------------------------------------------------
// market.rs
// ---------------------------------------------------------------------------------------------

/** Top of book at every change, columnar. Entry `i` holds from `ts[i]` until `ts[i + 1]`. */
export interface BboSeries {
  ts: RelNs[];
  bid_tick: (number | null)[];
  ask_tick: (number | null)[];
  bid_qty: number[];
  ask_qty: number[];
}

/** Market trades, columnar. */
export interface TradeSeries {
  ts: RelNs[];
  price_tick: number[];
  qty: number[];
  /** Aggressor side when the data states it. */
  side: (Side | null)[];
}

/** The reconstructed book at one instant; levels are `[price_tick, qty]`, best first. */
export interface DepthFrame {
  ts: RelNs;
  bids: [number, number][];
  asks: [number, number][];
}

export interface ReplayWindow {
  t0: RelNs;
  t1: RelNs;
  step_ns: number;
  levels: number;
  bbo: BboSeries;
  bbo_downsampled: boolean;
  trades: TradeSeries;
  trades_truncated: boolean;
  depth: DepthFrame[];
}

// ---------------------------------------------------------------------------------------------
// strategy.rs
// ---------------------------------------------------------------------------------------------

export const SLOTS_PER_TICK = 256;

/** Top of book as the strategy host knew it. */
export interface MarketView {
  best_bid_tick: number | null;
  best_ask_tick: number | null;
  best_bid_qty: number;
  best_ask_qty: number;
  feed_exch_ts: RelNs | null;
  feed_local_ts: RelNs | null;
}

export interface OpenOrder {
  order_id: number;
  side: Side;
  price_tick: number;
  qty: number;
  leaves_qty: number;
  status: OrderStatus;
  request_pending: boolean;
}

export interface TradePrint {
  local_ts: RelNs;
  price_tick: number;
  qty: number;
  aggressor: Side | null;
}

export type Intent =
  | {
      type: 'place';
      slot: number;
      side: Side;
      price_tick: number;
      qty: number;
      tif: Tif;
      ord_type: OrdType;
      reason: string;
    }
  | { type: 'cancel'; order_id: number; reason: string };

// ---------------------------------------------------------------------------------------------
// flight.rs
// ---------------------------------------------------------------------------------------------

export interface FlightEvent {
  /** Position on the tape, starting at 0, contiguous. */
  seq: number;
  /** When the event happened, in `clock`'s domain. */
  ts: RelNs;
  clock: Clock;
  source: Source;
  /** `null` for run-level events. */
  order_id: number | null;
  body: FlightBody;
}

/** The market event that moved a queue estimate. */
export type QueueCause =
  | { type: 'trade'; qty: number }
  | { type: 'depth'; prev_qty: number; new_qty: number }
  /** After a partial maker fill the engine model puts the remainder at the queue front. */
  | { type: 'fill_reset' };

export type RiskRule =
  | 'max_position'
  | 'max_order_qty'
  | 'max_open_orders'
  | 'max_drawdown'
  | 'invalid_order'
  | 'halted';

export type FlightBody =
  // ----- source: strategy, clock: local -----
  | {
      type: 'order_intent';
      tick_no: number;
      slot: number;
      side: Side;
      price_tick: number;
      qty: number;
      tif: Tif;
      ord_type: OrdType;
      reason: string;
      market: MarketView;
      known_position: number;
    }
  | {
      type: 'cancel_intent';
      tick_no: number;
      reason: string;
      market: MarketView;
      known_position: number;
    }
  | { type: 'risk_blocked'; tick_no: number; rule: RiskRule; detail: string }
  | { type: 'risk_halt'; rule: RiskRule; detail: string }
  // ----- source: engine -----
  | {
      /** clock: local. Negative `entry_latency_ns` means rejected before the matching engine. */
      type: 'request_sent';
      req: ReqKind;
      entry_latency_ns: number;
      exch_arrival_ts: RelNs | null;
    }
  | { type: 'request_refused'; req: ReqKind; error: string }
  | {
      /** clock: exchange. */
      type: 'exch_queued';
      queue_ahead: number | null;
      level_qty: number;
      best_bid_tick: number | null;
      best_ask_tick: number | null;
    }
  | {
      /** clock: exchange. */
      type: 'queue_update';
      cause: QueueCause;
      ahead_before: number | null;
      ahead_after: number | null;
    }
  | {
      /** clock: exchange. One simulated execution. */
      type: 'exch_fill';
      price_tick: number;
      qty: number;
      maker: boolean;
      fee: number;
      value: number;
      leaves_qty: number;
    }
  | {
      /** clock: exchange. */
      type: 'exch_response';
      status: OrderStatus;
      request_rejected: boolean;
      leaves_qty: number;
      response_latency_ns: number;
      local_arrival_ts: RelNs;
    }
  | {
      /** clock: local. */
      type: 'local_response';
      status: OrderStatus;
      request_rejected: boolean;
      exec_qty: number;
      exec_price_tick: number;
      leaves_qty: number;
      exch_ts: RelNs;
    }
  // ----- source: recorder -----
  | { type: 'queue_truncated'; dropped: number };

/** The serialized `type` tag of a tape event body. */
export type FlightBodyType = FlightBody['type'];

// ---------------------------------------------------------------------------------------------
// result.rs
// ---------------------------------------------------------------------------------------------

/** Horizons at which post-fill mid-price markouts are measured. */
export const MARKOUT_HORIZONS_NS: readonly [number, number, number, number] = [
  100_000_000, 1_000_000_000, 5_000_000_000, 30_000_000_000,
];

export interface EngineInfo {
  name: string;
  version: string;
  features: string[];
}

export interface DatasetRef {
  id: string;
  sha256: string;
  rows: number;
  origin_ns: string;
}

export interface RunManifest {
  contract_version: number;
  /** First 16 hex characters of `config_sha256`. Identical inputs give an identical id. */
  run_id: string;
  blackbox_version: string;
  engine: EngineInfo;
  /** `"<os>-<arch>"`. */
  platform: string;
  /** `"debug"` or `"release"`. */
  profile: string;
  dataset: DatasetRef;
  config: RunConfig;
  config_sha256: string;
  result_sha256: string;
  /** Wall-clock facts. Informational only; excluded from both hashes. */
  created_unix_ms: number;
  wall_time_ms: number;
}

/** One simulated execution with its accounting effect. */
export interface FillRecord {
  seq: number;
  order_id: number;
  /** Exchange clock. */
  ts: RelNs;
  side: Side;
  price_tick: number;
  qty: number;
  maker: boolean;
  fee: number;
  value: number;
  position_after: number;
  realized_pnl: number;
  /** Derived: dataset mid (fractional ticks) at the fill's exchange time. */
  mid_tick: number | null;
  /** Derived: one entry per horizon in `MARKOUT_HORIZONS_NS`; positive is favourable. */
  markout_ticks: (number | null)[];
}

/** Periodic account snapshot on the local clock. */
export interface EquitySample {
  ts: RelNs;
  tick_no: number;
  mid_tick: number | null;
  position: number;
  known_position: number;
  cash: number;
  fees: number;
  realized_pnl: number;
  unrealized_pnl: number;
  equity: number;
  open_orders: number;
}

/** Lifecycle summary of one order, assembled from the tape. */
export interface OrderRecord {
  order_id: number;
  tick_no: number;
  slot: number;
  side: Side;
  price_tick: number;
  qty: number;
  tif: Tif;
  ord_type: OrdType;
  reason: string;
  outcome: OrderOutcome;
  /** Local clock. */
  decision_ts: RelNs;
  /** Exchange clock. */
  exch_arrival_ts: RelNs | null;
  /** Local clock. */
  first_response_ts: RelNs | null;
  queue_ahead_at_entry: number | null;
  min_queue_ahead: number | null;
  traded_at_price_qty: number;
  filled_qty: number;
  maker_qty: number;
  taker_qty: number;
  avg_fill_tick: number | null;
  fees: number;
  fill_count: number;
  first_fill_ts: RelNs | null;
  last_fill_ts: RelNs | null;
  cancel_sent_ts: RelNs | null;
  cancel_reason: string | null;
  cancel_rejected: boolean;
  terminal_ts: RelNs | null;
  final_status: OrderStatus;
  first_seq: number;
  last_seq: number;
}

export type OrderOutcome =
  | 'risk_blocked'
  | 'refused'
  | 'in_flight_at_end'
  | 'filled'
  | 'partially_filled'
  | 'canceled'
  | 'expired'
  | 'working_at_end';

export interface MarkoutStat {
  horizon_ns: number;
  fills: number;
  /** Quantity-weighted mean markout in ticks. Negative means adverse selection. */
  mean_ticks: number | null;
}

/** Run-level measured outcomes. */
export interface Metrics {
  // Orders
  orders_intended: number;
  orders_risk_blocked: number;
  orders_refused: number;
  orders_submitted: number;
  orders_filled: number;
  orders_partially_filled: number;
  orders_canceled: number;
  orders_expired: number;
  orders_working_at_end: number;
  cancels_sent: number;
  cancels_rejected: number;
  // Fills
  fill_count: number;
  maker_fill_count: number;
  taker_fill_count: number;
  fill_ratio_orders: number | null;
  fill_ratio_qty: number | null;
  // Timing (nanoseconds)
  order_age_ns: Quantiles | null;
  time_to_first_fill_ns: Quantiles | null;
  entry_latency_ns: Quantiles | null;
  response_latency_ns: Quantiles | null;
  round_trip_ns: Quantiles | null;
  // Activity
  volume: number;
  turnover: number;
  fees: number;
  // Inventory
  final_position: number;
  max_long: number;
  max_short: number;
  mean_abs_position: number;
  // P&L (quote currency)
  realized_pnl: number;
  unrealized_pnl: number;
  net_pnl: number;
  max_drawdown: number;
  // Execution quality
  spread_capture: number;
  spread_capture_ticks: number | null;
  markouts: MarkoutStat[];
  // Risk
  risk_blocks: number;
  risk_halted: boolean;
}

/** Cross-check of BlackBox's own ledger against the engine's local-side account state. */
export interface EngineStateCheck {
  agrees: boolean;
  ledger_position: number;
  engine_position: number;
  ledger_cash: number;
  engine_balance: number;
  ledger_fees: number;
  engine_fee: number;
  ledger_fills: number;
  engine_num_trades: number;
  note: string;
}

/** A complete run. */
export interface RunResult {
  manifest: RunManifest;
  metrics: Metrics;
  engine_check: EngineStateCheck;
  ticks: number;
  orders: OrderRecord[];
  fills: FillRecord[];
  equity: EquitySample[];
  tape: FlightEvent[];
}

/** Listing entry for a stored run. */
export interface RunSummary {
  manifest: RunManifest;
  metrics: Metrics;
  ticks: number;
  order_count: number;
  fill_count: number;
  tape_len: number;
  /** User-facing label. Not part of any hash. */
  label: string;
}

// ---------------------------------------------------------------------------------------------
// explain.rs
// ---------------------------------------------------------------------------------------------

export type Question = 'why_filled' | 'why_not_filled' | 'why_partially_filled';

/** Machine-readable conclusion. Exactly one applies to an order. */
export type Verdict =
  | 'filled_queue_exhausted'
  | 'filled_price_crossed'
  | 'filled_taker_on_arrival'
  | 'filled_mixed'
  | 'not_filled_queue_not_reached'
  | 'not_filled_no_trade_at_price'
  | 'not_filled_canceled'
  | 'not_filled_post_only_would_cross'
  | 'not_filled_no_liquidity_on_arrival'
  | 'not_filled_risk_blocked'
  | 'not_filled_refused'
  | 'not_filled_data_ended_in_flight'
  | 'not_filled_working_at_end';

export interface ExplainStep {
  basis: Basis;
  statement: string;
  /** Tape `seq` values this statement rests on. */
  event_seqs: number[];
}

export interface LatencyBreakdown {
  feed_staleness_ns: number | null;
  feed_latency_ns: number | null;
  entry_latency_ns: number | null;
  response_latency_ns: number | null;
  time_to_first_fill_ns: number | null;
  resting_time_ns: number | null;
  cancel_entry_latency_ns: number | null;
}

export interface QueuePoint {
  /** Exchange clock. */
  ts: RelNs;
  seq: number;
  ahead: number;
}

export interface Explanation {
  order_id: number;
  question: Question;
  verdict: Verdict;
  headline: string;
  steps: ExplainStep[];
  caveats: string[];
  latency: LatencyBreakdown;
  queue_path: QueuePoint[];
}

// ---------------------------------------------------------------------------------------------
// compare.rs
// ---------------------------------------------------------------------------------------------

/** Hard upper bound on the number of simulations in one sensitivity grid. */
export const MAX_GRID_CELLS = 100;

export interface ConfigDiff {
  /** Dotted path into the serialized `RunConfig`. */
  path: string;
  a: JsonValue;
  b: JsonValue;
}

export interface MetricRow {
  key: string;
  label: string;
  /** `"quote"`, `"qty"`, `"count"`, `"ratio"`, `"ns"` or `"ticks"`. */
  unit: string;
  a: number | null;
  b: number | null;
  /** `b - a` when both exist. */
  delta: number | null;
}

/** What an eligibility difference does to a comparison. */
export type EligibilityEffect = 'no_alignment' | 'not_like_for_like';

export interface EligibilityReason {
  /** e.g. `strategy.interval_ns`, `dataset.sha256`, `instrument.tick_size`. */
  field: string;
  /** `null` when unknown. */
  a: JsonValue;
  b: JsonValue;
  effect: EligibilityEffect;
  explanation: string;
}

/** Checked before any per-order alignment. */
export interface Eligibility {
  /** Per-order alignment was produced (no `no_alignment` reason). */
  aligned: boolean;
  /** No reason at all: metric deltas are like-for-like. */
  like_for_like: boolean;
  reasons: EligibilityReason[];
}

export interface DecisionOutcome {
  side: Side;
  price_tick: number;
  qty: number;
  tif: Tif;
  ord_type: OrdType;
  decision_ts: RelNs;
  outcome: OrderOutcome;
  final_status: OrderStatus;
  filled_qty: number;
  avg_fill_tick: number | null;
  maker_qty: number;
  taker_qty: number;
  fees: number;
  fill_count: number;
  queue_ahead_at_entry: number | null;
  exch_arrival_ts: RelNs | null;
  first_response_ts: RelNs | null;
  first_fill_ts: RelNs | null;
  last_fill_ts: RelNs | null;
  cancel_sent_ts: RelNs | null;
  terminal_ts: RelNs | null;
}

/** Checked in this order for orders present in both runs; the first group that differs decides. */
export type PairClass =
  | 'identical'
  | 'same_fills_different_history'
  | 'same_fills_different_fees'
  | 'same_outcome_different_timing'
  | 'same_decision_different_outcome'
  | 'different_decision'
  | 'only_a'
  | 'only_b';

/** B minus A in nanoseconds; `null` unless both orders have the timestamp. */
export interface PairTiming {
  decision_ns: number | null;
  exch_arrival_ns: number | null;
  first_response_ns: number | null;
  first_fill_ns: number | null;
  completion_ns: number | null;
}

export interface DeltaStats {
  count: number;
  nonzero: number;
  min: number | null;
  median: number | null;
  max: number | null;
}

export interface TimingSummary {
  decision: DeltaStats;
  exch_arrival: DeltaStats;
  first_response: DeltaStats;
  first_fill: DeltaStats;
  completion: DeltaStats;
}

export interface OrderPair {
  order_id: number;
  tick_no: number;
  /** Decision time in A (in B for `only_b`). */
  decision_ts: RelNs;
  class: PairClass;
  /** Every compared field that differs, most significant first. */
  differences: string[];
  timing: PairTiming | null;
  a: DecisionOutcome | null;
  b: DecisionOutcome | null;
}

export interface Alignment {
  identical: number;
  same_fills_different_history: number;
  same_fills_different_fees: number;
  same_outcome_different_timing: number;
  same_decision_different_outcome: number;
  different_decision: number;
  only_a: number;
  only_b: number;
  /** Earliest non-identical pair; every pair before it is identical, timing included. */
  first_divergence: OrderPair | null;
  /** Earliest pair whose decision or fills differ. */
  first_outcome_divergence: OrderPair | null;
  timing: TimingSummary;
  pairs: OrderPair[];
  pairs_cap: number;
  pairs_truncated: boolean;
}

export interface InterpretationNote {
  basis: Basis;
  statement: string;
}

export interface Comparison {
  run_a: string;
  run_b: string;
  label_a: string;
  label_b: string;
  eligibility: Eligibility;
  config_diffs: ConfigDiff[];
  /** Returned even when not like-for-like. */
  metrics: MetricRow[];
  /** `null` when `eligibility.aligned` is false. */
  alignment: Alignment | null;
  interpretation: InterpretationNote[];
}

/** A parameter swept along one axis of a sensitivity grid. */
export type GridAxis =
  | { type: 'order_latency_ns'; values: number[] }
  | { type: 'entry_latency_ns'; values: number[] }
  | { type: 'feed_latency_offset_ns'; values: number[] }
  | { type: 'queue_model'; values: QueueModelConfig[] }
  | { type: 'maker_fee'; values: number[] }
  | { type: 'taker_fee'; values: number[] };

export interface GridSpec {
  base: RunConfig;
  x: GridAxis;
  y: GridAxis;
}

/** A cell with `metrics: null` did not complete and must be rendered as missing. */
export interface GridCell {
  xi: number;
  yi: number;
  run_id: string | null;
  result_sha256: string | null;
  metrics: Metrics | null;
  error: string | null;
}

export interface GridResult {
  spec: GridSpec;
  x_labels: string[];
  y_labels: string[];
  /** Row-major: `cells[yi * x_len + xi]`. */
  cells: GridCell[];
  wall_time_ms: number;
}

// ---------------------------------------------------------------------------------------------
// API-only types (docs/API.md, crates/bb-app/src/api.rs)
//
// Request-side `Option` fields are optional as well as nullable: serde treats a missing key and
// `null` identically, and query parameters are simply omitted.
// ---------------------------------------------------------------------------------------------

export interface Health {
  status: string;
  blackbox_version: string;
  contract_version: number;
  engine: EngineInfo;
  disclaimer: string;
}

export interface ReplayQuery {
  /** Default: dataset start. */
  t0?: number | null;
  /** Default: min(dataset end, t0 + 60 s). */
  t1?: number | null;
  /** Default: (t1 - t0) / 300, at least 1 ms. */
  step_ns?: number | null;
  /** Default 25, max 100. */
  levels?: number | null;
  /** Default 5000, max 50000. */
  max_trades?: number | null;
}

export interface Preset {
  id: string;
  title: string;
  description: string;
  /** What the preset is meant to demonstrate, in one sentence. */
  demonstrates: string;
  config: RunConfig;
}

export interface RunRequest {
  config: RunConfig;
  label?: string | null;
}

export interface OrdersQuery {
  outcome?: OrderOutcome | null;
  side?: Side | null;
  /** Default 0. */
  offset?: number | null;
  /** Default 200, max 2000. */
  limit?: number | null;
}

export interface OrdersPage {
  /** After filtering. */
  total: number;
  offset: number;
  orders: OrderRecord[];
}

export interface OrderDetail {
  order: OrderRecord;
  /** Every tape event of this order, in `seq` order. */
  events: FlightEvent[];
  fills: FillRecord[];
  explanation: Explanation;
  instrument: InstrumentMeta;
}

export interface TapeQuery {
  /** Default 0. */
  from_seq?: number | null;
  /** Default 500, max 5000. */
  limit?: number | null;
  order_id?: number | null;
}

export interface TapePage {
  /** Tape length (after `order_id` filtering). */
  total: number;
  events: FlightEvent[];
}

export interface CompareRequest {
  a: string;
  b: string;
}

export interface TimelineQuery {
  /** Default: run start (first equity sample ts, else 0). */
  t0?: number | null;
  /** Default: min(run end, t0 + 60 s). */
  t1?: number | null;
  /** Default 5000, max 50000. */
  max_events?: number | null;
}

/** Everything the run-level replay terminal overlays on the dataset's `ReplayWindow` for [t0, t1]. */
export interface RunTimeline {
  run_id: string;
  dataset_id: string;
  t0: RelNs;
  t1: RelNs;
  /** Tape events whose `ts` is in [t0, t1] on either clock, in `seq` order, capped (earliest kept). */
  events: FlightEvent[];
  /** In the window before capping. */
  events_total: number;
  events_truncated: boolean;
  /** Fills with exchange ts in [t0, t1]. */
  fills: FillRecord[];
  /** Samples with ts in [t0, t1], plus the last sample before t0 when one exists. */
  equity: EquitySample[];
  /** Orders alive at some point in [t0, t1]. Ascending `order_id`. */
  orders: OrderRecord[];
  /** Run span: first and last equity sample ts. */
  run_start: RelNs;
  run_end: RelNs;
}

/** A canonical, reproducible experiment: run configurations that differ only in `varied`. */
export interface Demo {
  /** `"cost_of_latency"`, `"queue_position"` or `"gross_vs_net"`. */
  id: string;
  title: string;
  /** The research question in one sentence. */
  question: string;
  /** Dotted `RunConfig` paths that differ between the runs. */
  varied: string[];
  /** Plain-language list of what is held identical. */
  controlled: string[];
  /** How to read the result, without asserting what it will be. */
  how_to_read: string;
  runs: DemoRun[];
  /** CLI commands that reproduce every run of the demo. */
  reproduce: string[];
}

export interface DemoRun {
  label: string;
  config: RunConfig;
}

/** Body of every non-2xx response. */
export interface ApiErrorBody {
  error: { code: string; message: string };
}
