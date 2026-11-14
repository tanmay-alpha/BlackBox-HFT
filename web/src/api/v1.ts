// Wire types of `/api/v1` (docs/engineering/API_CONTRACT.md §1-§9), exactly as the server sends
// them: every nanosecond quantity and `seed` are decimal strings, absent optionals are `null`.
//
// This is the only module that declares v1 resource shapes. When the generated OpenAPI types
// (`v1.gen.ts`, npm script `gen:api`) land, every declaration below becomes an alias of the
// generated schema (`export type Job = components['schemas']['Job']`), and nothing else changes:
// `normalize.ts` converts these wire shapes into the view types the workspaces use.
//
// Nested bb-core types (`RunConfig`, `Metrics`, `OrderRecord`, …) are reused from `types.ts`,
// whose ns fields are typed as the numbers `normalize.ts` produces; on the wire they are strings.

import type {
  Comparison as CoreComparison,
  DatasetManifest,
  Demo as CoreDemo,
  DemoRun as CoreDemoRun,
  EngineInfo,
  EngineStateCheck,
  EquitySample,
  Explanation,
  FillRecord,
  FlightEvent,
  GridSpec,
  InstrumentMeta,
  Metrics,
  OrderPair,
  OrderRecord,
  QualityReport,
  RunConfig,
  RunTimeline as CoreRunTimeline,
} from './types';

/** A decimal i64 nanosecond value (`"1700000000000000000"`, `"-5000"`). */
export type NsString = string;

// ----- envelopes -------------------------------------------------------------------------------

/** `byte` when the 4 MiB response cap ended the page early (§7). */
export type TruncatedBy = 'limit' | 'bytes' | null;

/** Cursor page (§7): every DB list and append-only run section. */
export interface CursorPage<T> {
  items: T[];
  next_cursor: string | null;
  truncated_by: TruncatedBy;
}

/** Offset page (§7): `orders` only. */
export interface OffsetPage<T> {
  items: T[];
  offset: number;
  total: number;
  next_offset: number | null;
  truncated_by: TruncatedBy;
}

export type ErrorCode =
  | 'invalid_request'
  | 'unauthorized'
  | 'forbidden'
  | 'not_found'
  | 'conflict'
  | 'invalid_config'
  | 'budget_exceeded'
  | 'rate_limited'
  | 'quota_exceeded'
  | 'integrity_failed'
  | 'platform_mismatch'
  | 'internal'
  | 'unavailable';

/** Body of every non-2xx response (§2). */
export interface ErrorEnvelope {
  error: {
    code: ErrorCode | string;
    message: string;
    request_id: string;
    details: Record<string, unknown> | null;
  };
}

// ----- status and identity (§9.1, §9.2) --------------------------------------------------------

export type WorkerState = 'starting' | 'idle' | 'busy' | 'draining' | 'stopped';

export interface Status {
  status: 'ok' | 'draining';
  api_version: string;
  contract_version: number;
  blackbox_version: string;
  git_sha: string;
  engine: EngineInfo;
  server_time_ns: NsString;
  worker: {
    mode: string;
    state: WorkerState;
    slots: number;
    busy: number;
    last_heartbeat_age_ms: number | null;
    heartbeat_interval_ms: number;
  };
  queue: { depth: number; running: number; as_of_ns: NsString | null };
  limits: {
    profile: 'hosted' | 'local';
    max_ticks: number;
    max_tape_events: number;
    max_output_bytes: number;
    max_sweep_cells: number;
    deadline_ms: number;
  };
  disclaimer: string;
}

export type Scope = 'read' | 'run' | 'admin';

/** `GET /readyz` (200 only; a 503 is the `unavailable` error envelope). */
export interface Readiness {
  status: string;
  db: string;
  migrations: string;
  blob: string;
}

export interface Quota {
  limit: number;
  used: number;
}

export interface Whoami {
  mode: 'demo' | 'research';
  workspace: { slug: string; name: string; kind: 'demo' | 'research' | string };
  key: {
    id: string;
    label: string;
    display_prefix: string;
    scopes: Scope[];
    expires_at_ns: NsString | null;
  } | null;
  quotas: {
    experiments_per_day: Quota;
    sweeps_per_day: Quota;
    active_jobs: Quota;
  } | null;
}

// ----- jobs (§8.5, §9.3) -----------------------------------------------------------------------

export type JobState = 'created' | 'queued' | 'running' | 'cancelling' | 'completed' | 'failed' | 'cancelled' | 'expired';

export type JobFailureCategory =
  | 'invalid_config'
  | 'budget_exceeded'
  | 'simulation_failed'
  | 'lease_expired'
  | 'integrity_failed'
  | 'storage_failed'
  | 'internal';

export interface WorkEstimate {
  ticks: number;
  tape_events_upper: number;
  output_bytes_upper: number;
}

export interface Job {
  id: string;
  kind: string;
  state: JobState;
  version: number;
  experiment_fingerprint: string;
  platform: string;
  priority: number;
  attempt: number;
  max_attempts: number;
  lease_losses: number;
  created_at_ns: NsString;
  queued_at_ns: NsString | null;
  started_at_ns: NsString | null;
  finished_at_ns: NsString | null;
  queue_position: number | null;
  queue_depth: number | null;
  progress: { done: number; total: number; unit: string; tape_events: number | null } | null;
  last_heartbeat_at_ns: NsString | null;
  heartbeat_interval_ms: number | null;
  cancel_requested_at_ns: NsString | null;
  error: { category: JobFailureCategory | string; code: string; message: string } | null;
  expired_reason: string | null;
  run_id: string | null;
  run_ids: string[];
  estimate: WorkEstimate | null;
  /** Only on a cancel response that detached the caller's experiment from a shared job. */
  detached?: boolean;
}

// ----- runs (§8.4, §9.4) -----------------------------------------------------------------------

export interface RunDatasetRef {
  id: string;
  version: number;
  events_sha256: string;
  origin_ns: NsString;
}

export interface RunSummary {
  run_id: string;
  fingerprint: string;
  platform: string;
  canonical: boolean;
  dataset: RunDatasetRef;
  strategy_kind: string;
  interval_ns: NsString;
  contract_version: number;
  simulation_revision: number;
  engine: EngineInfo;
  blackbox_version: string;
  build_profile: string;
  result_sha256: string;
  summary_sha256: string;
  bundle: { format_version: number; sha256: string; size_bytes: number };
  counts: { ticks: number; orders: number; fills: number; tape_events: number; equity_samples: number };
  metrics: Metrics;
  engine_check: EngineStateCheck;
  wall_time_ms: number;
  peak_rss_bytes: number | null;
  created_at_ns: NsString;
  /** The caller-workspace experiment label (the demo label when only visible through demo). */
  label: string | null;
  experiment_id: string | null;
  /** `GET /runs/{id}` only. */
  config?: RunConfig;
}

export interface OrderDetail {
  order: OrderRecord;
  events: FlightEvent[];
  fills: FillRecord[];
  explanation: Explanation;
  instrument: InstrumentMeta;
}

export interface EquityBucket {
  t0_ns: NsString;
  t1_ns: NsString;
  first: number;
  last: number;
  min_equity: number;
  max_equity: number;
  n: number;
}

export interface EquityPage {
  items: EquitySample[] | EquityBucket[];
  downsampled: boolean;
  bucket_ns: NsString | null;
  next_cursor: string | null;
  truncated_by: TruncatedBy;
}

export interface RunTimeline extends CoreRunTimeline {
  truncated_by: TruncatedBy;
}

// ----- experiments (§8.3, §9.4) ---------------------------------------------------------------

export type ExperimentStatus = JobState | 'detached';

export interface Experiment {
  id: string;
  workspace: string;
  label: string;
  version: number;
  pinned: boolean;
  run_id: string;
  fingerprint: string;
  dataset: { id: string; version: number };
  strategy_kind: string;
  interval_ns: NsString;
  platform: string;
  sweep: { id: string; x: number; y: number } | null;
  status: ExperimentStatus;
  job: Job | null;
  run: RunSummary | null;
  estimate: WorkEstimate | null;
  created_by: { display_prefix: string; label: string } | null;
  created_at_ns: NsString;
  /** `GET /experiments/{id}` only. */
  config?: RunConfig;
}

export interface SubmitExperimentResponse {
  created: boolean;
  experiment: Experiment;
  job: Job | null;
  run: RunSummary | null;
}

// ----- datasets (§8.2, §9.4) -------------------------------------------------------------------

export interface DatasetVersion {
  dataset_id: string;
  version: number;
  events_sha256: string;
  normalization_version: number;
  contract_version: number;
  instrument: InstrumentMeta & { exchange: string; symbol: string };
  origin_ns: NsString;
  start_ns: NsString;
  end_ns: NsString;
  timestamp_unit: string;
  rows: number;
  events_bytes: number;
  source: { id: string; vendor: string; licence: string; hostable: boolean };
  quality: { checker_version: number; passed: boolean; error_count: number; warning_count: number };
  retired_at_ns: NsString | null;
  /** Full manifest on detail reads; `null` in lists. */
  manifest: DatasetManifest | null;
}

export interface DatasetSummary {
  id: string;
  title: string;
  kind: 'synthetic' | 'recorded';
  level: string;
  owner: 'global' | 'workspace';
  /** Without `manifest` in the list. */
  current_version: DatasetVersion | null;
  created_at_ns: NsString;
}

/** `GET /datasets/{id}`: the summary with the current version's full manifest. */
export interface Dataset extends DatasetSummary {
  current_version: (DatasetVersion & { manifest: DatasetManifest }) | null;
}

export interface DatasetQuality {
  checker_version: number;
  passed: boolean;
  error_count: number;
  warning_count: number;
  book_valid_fraction: number | null;
  report: QualityReport;
  report_sha256: string;
}

export interface DemoRun extends CoreDemoRun {
  run_id: string | null;
  experiment_id: string | null;
}

export interface Demo extends Omit<CoreDemo, 'runs'> {
  runs: DemoRun[];
}

// ----- comparisons (§8.6) ----------------------------------------------------------------------

/** bb-core `Comparison` plus the resource fields. In `GET /comparisons/{id}` `alignment.pairs` is
 * always empty (pairs are paged by `/pairs`); the list omits `alignment` (`null`). */
export interface Comparison extends CoreComparison {
  id: string;
  label: string;
  version: number;
  /** `aligned` ⇔ `eligibility.aligned`. */
  status: 'aligned' | 'metrics_only';
  comparator_version: number;
  created_at_ns: NsString;
}

export type ComparisonPair = OrderPair;

// ----- sweeps (§8.7) ---------------------------------------------------------------------------

export type SweepState = 'queued' | 'running' | 'completed' | 'partial' | 'cancelled';

export interface SweepCell {
  x: number;
  y: number;
  experiment_id: string;
  job_id: string | null;
  state: ExperimentStatus;
  run_id: string | null;
  metrics: { net_pnl: number; fill_count: number; fees: number; max_drawdown: number } | null;
}

export interface Sweep {
  id: string;
  label: string;
  version: number;
  x: { axis: string; labels: string[] };
  y: { axis: string; labels: string[] };
  state: SweepState;
  progress: {
    cells_total: number;
    completed: number;
    running: number;
    queued: number;
    failed: number;
    cancelled: number;
    expired: number;
  };
  /** `GET /sweeps/{id}` only. */
  cells?: SweepCell[];
  /** `GET /sweeps/{id}` only, when the server returns it. */
  spec?: GridSpec;
  created_at_ns: NsString;
}

export interface SubmitSweepResponse {
  created: boolean;
  sweep: Sweep;
}

// ----- reports (§8.8) --------------------------------------------------------------------------

export type ReportKind = 'run' | 'comparison' | 'sweep';

export interface Report {
  id: string;
  kind: ReportKind;
  subject_id: string;
  title: string;
  generator_version: number;
  format: string;
  body_sha256: string;
  size_bytes: number;
  created_at_ns: NsString;
}

// ----- system (§8.9, admin) --------------------------------------------------------------------

export interface WorkerHeartbeat {
  worker_id: string;
  mode: string;
  platform: string;
  version: string;
  state: string;
  current_job_id: string | null;
  last_heartbeat_age_ms: number;
}

export interface QueueSummary {
  depth_by_platform: Record<string, number>;
  running: number;
  oldest_queued_age_ms: number | null;
  lease_reclaims_24h: number;
}

export interface SystemEvent {
  id: string;
  source: 'system' | 'audit' | string;
  kind: string;
  level: string;
  message: string;
  created_at_ns: NsString;
  details: Record<string, unknown> | null;
}

export interface Usage {
  blob: {
    month: string;
    uploads: number;
    uploads_limit: number;
    reads: number;
    reads_limit: number;
    bytes_stored: number;
    bytes_limit: number;
  };
  db: { size_bytes: number; limit_bytes: number };
}

export interface ApiKeyInfo {
  id: string;
  workspace: string;
  label: string;
  display_prefix: string;
  scopes: Scope[];
  created_at_ns: NsString;
  expires_at_ns: NsString | null;
  revoked_at_ns: NsString | null;
  last_used_at_ns: NsString | null;
}

// ----- analytics (bb-analytics, `GET /runs/{id}/analytics`) -----------------------------------
//
// Integer ns fields (`horizon_ns`, `elapsed_ns`, `sent_ns`, `observation_end_ns`, the first
// element of `opportunities`) are decimal strings and stay strings in the views (formatted
// exactly by `fmtNsDuration`). `AnalyticsValue.value` is an f64 statistic in `unit`.

/** Every analytics number: available with its unit and counts, or unavailable with the reason. */
export type AnalyticsValue =
  | { status: 'available'; value: number; unit: string; n: number; excluded: number }
  | { status: 'unavailable'; reason: string; unit: string; n: number; excluded: number };

export interface PriceMetric {
  price: AnalyticsValue;
  ticks: AnalyticsValue;
  bps: AnalyticsValue;
}

export interface ConfidenceBand {
  lower: AnalyticsValue;
  upper: AnalyticsValue;
  method: string;
}

export interface HorizonMetrics {
  horizon_ns: NsString;
  matched_effective_half: PriceMetric;
  realised_half: PriceMetric;
  markout: PriceMetric;
  adverse_selection: PriceMetric;
  adverse_selection_95: ConfidenceBand;
}

export interface SpreadMetrics {
  filled_quantity: AnalyticsValue;
  effective_half: PriceMetric;
  effective_full: PriceMetric;
  arrival_slippage: PriceMetric;
  decision_slippage: PriceMetric;
  latency_drift: PriceMetric;
  horizons: HorizonMetrics[];
}

export interface LiquiditySplit<T> {
  all: T;
  maker: T;
  taker: T;
}

export interface Distribution {
  total: AnalyticsValue;
  median: AnalyticsValue;
  p05: AnalyticsValue;
  p95: AnalyticsValue;
}

export interface ShortfallComponents {
  execution: Distribution;
  delay: Distribution;
  trading: Distribution;
  opportunity: Distribution;
  fees: Distribution;
}

export interface CancelSummary {
  attempts: number;
  exchange_censored: number;
  acknowledgement_censored: number;
  late_cancels: number;
  to_exchange_ns: Distribution;
  round_trip_ns: Distribution;
  in_flight_quantity: Distribution;
}

export interface SurvivalPoint {
  elapsed_ns: NsString;
  survival: AnalyticsValue;
  fill_probability: AnalyticsValue;
  probability_95: ConfidenceBand;
  at_risk: number;
  events: number;
  censored: number;
}

export interface KaplanMeier {
  n: number;
  excluded: number;
  censored: number;
  curve: SurvivalPoint[];
  fill_probability: AnalyticsValue;
  probability_95: ConfidenceBand;
  median_ns: AnalyticsValue;
}

export interface QueueBucket {
  lower: number | null;
  upper: number | null;
  covariate_unit: string;
  estimate: KaplanMeier;
}

export interface QueueMetrics {
  horizon_ns: NsString;
  modeled: boolean;
  relative: QueueBucket[];
  absolute_lots: QueueBucket[];
  excluded_relative: number;
  excluded_absolute: number;
}

export interface FillTimes {
  observed_ns: Distribution;
  km: KaplanMeier;
}

export interface PnlDecomposition {
  edge: AnalyticsValue;
  drift: AnalyticsValue;
  fees: AnalyticsValue;
  net: AnalyticsValue;
}

export interface ExecutionReport {
  analytics_version: string;
  order_count: number;
  order_details_streamed: boolean;
  spreads: LiquiditySplit<SpreadMetrics>;
  /** Empty in the API: per-order details are not retained. */
  orders: unknown[];
  shortfall: Distribution;
  shortfall_components: ShortfallComponents;
  cancels: CancelSummary;
  opportunities: [NsString, Distribution][];
  queue: QueueMetrics;
  first_fill: FillTimes;
  complete_fill: FillTimes;
  pnl: LiquiditySplit<PnlDecomposition>;
  exchange_end_mid: AnalyticsValue;
  observation_end_ns: NsString | null;
  unassociated_fills: number;
  caveats: string[];
}

export interface RiskReport {
  analytics_version: string;
  mark: string;
  sample_interval_ns: AnalyticsValue;
  bucket_horizon_ns: AnalyticsValue;
  trials: number;
  net_pnl: AnalyticsValue;
  turnover: AnalyticsValue;
  pnl_per_turnover_bps: AnalyticsValue;
  max_drawdown: AnalyticsValue;
  time_under_water_ns: AnalyticsValue;
  mean_absolute_position: AnalyticsValue;
  time_at_position_limit_ns: AnalyticsValue;
  max_long: AnalyticsValue;
  max_short: AnalyticsValue;
  /** Non-annualised mean / sd of bucket P&L; unavailable below 30 buckets. */
  bucket_sharpe: AnalyticsValue;
  sharpe_hac_se: AnalyticsValue;
  caveats: string[];
}

export interface RunAnalytics {
  analytics_version: string;
  /** `null` when the execution report could not be computed; the reason otherwise. */
  execution: ExecutionReport | null;
  risk: RiskReport;
  execution_unavailable: string | null;
}
