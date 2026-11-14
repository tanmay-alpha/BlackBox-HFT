// Wire types of `/api/v1`, taken from the generated OpenAPI types (`v1.gen.ts`, npm script
// `gen:api`; never edited by hand). Every ns value and `seed` is a decimal string on the wire.
//
// Two adjustments are made here, and only here:
// - API_CONTRACT §1 "Nulls": an absent optional value is sent as `null`, never omitted, so the
//   generated `field?: T | null` is read as the required `field: T | null` (`Present`).
// - Enumerations the generator emits as plain `string` are narrowed to their documented values,
//   and the few schemas the generator leaves untyped (`Record<string, never>`) are spelled out.
// `normalize.ts` converts these wire shapes into the view types of `model.ts`.

import type { components } from './v1.gen';

type S = components['schemas'];

/** Optional-nullable fields as the server sends them: always present, possibly `null`. */
type Present<T> = { [K in keyof T]-?: Exclude<T[K], undefined> };

/** A decimal i64 nanosecond value (`"1700000000000000000"`, `"-5000"`). */
export type NsString = string;

// ----- envelopes -------------------------------------------------------------------------------

export type TruncatedBy = S['TruncatedBy'] | null;

/** Cursor page (§7): every DB list and append-only run section. */
export interface CursorPage<T> {
  items: T[];
  next_cursor: string | null;
  truncated_by: TruncatedBy;
}

/** Offset page (§7): `orders` only. */
export type OffsetPage<T> = Omit<Present<S['OrdersPage']>, 'items'> & { items: T[] };

/** Body of every non-2xx response (§2); `details` is free-form JSON. */
export interface ErrorEnvelope {
  error: Omit<S['ErrorBody'], 'details'> & { details: Record<string, unknown> | null };
}

// ----- status and identity ---------------------------------------------------------------------

export type WorkerState = 'starting' | 'idle' | 'busy' | 'draining' | 'stopped';
export type Scope = 'read' | 'run' | 'admin';

export type Status = Omit<Present<S['Status']>, 'worker' | 'queue' | 'limits' | 'status'> & {
  status: 'ok' | 'draining';
  worker: Omit<Present<S['WorkerSummary']>, 'state'> & { state: WorkerState };
  queue: Present<S['QueueSummary']>;
  limits: Present<S['LimitsSummary']>;
};

export type Readiness = S['Ready'];
export type Quota = S['QuotaUse'];

export type Whoami = Omit<Present<S['Whoami']>, 'mode' | 'key'> & {
  mode: 'demo' | 'research';
  key: (Omit<Present<S['KeyRef']>, 'scopes'> & { scopes: Scope[] }) | null;
};

// ----- jobs ------------------------------------------------------------------------------------

export type JobState = 'created' | 'queued' | 'running' | 'cancelling' | 'completed' | 'failed' | 'cancelled' | 'expired';

export type JobFailureCategory =
  | 'invalid_config'
  | 'budget_exceeded'
  | 'simulation_failed'
  | 'lease_expired'
  | 'integrity_failed'
  | 'storage_failed'
  | 'internal';

export type WorkEstimate = S['Estimate'];

export type Job = Omit<Present<S['Job']>, 'state' | 'detached' | 'progress' | 'error'> & {
  state: JobState;
  progress: S['JobProgress'] | null;
  error: S['JobErrorBody'] | null;
  /** Only on a cancel response that detached the caller's experiment from a shared job. */
  detached?: boolean | null;
};

// ----- runs ------------------------------------------------------------------------------------

export type RunSummary = Present<S['RunSummary']>;
export type OrderDetail = S['OrderDetail'];
export type RunTimeline = Present<S['RunTimeline']>;

export type EquityBucket = {
  t0_ns: NsString;
  t1_ns: NsString;
  first: number;
  last: number;
  min_equity: number;
  max_equity: number;
  n: number;
};

/** `items` is untyped in the generated schema: raw `EquitySample`s, or buckets when downsampled. */
export type EquityPage = Omit<Present<S['EquityPage']>, 'items'> & { items: S['EquitySample'][] | EquityBucket[] };

// ----- experiments -----------------------------------------------------------------------------

export type ExperimentStatus = JobState | 'detached';

export type Experiment = Omit<Present<S['Experiment']>, 'status' | 'job' | 'run'> & {
  status: ExperimentStatus;
  job: Job | null;
  run: RunSummary | null;
};

export type SubmitExperimentResponse = Omit<Present<S['Submitted']>, 'experiment' | 'job' | 'run'> & {
  experiment: Experiment;
  job: Job | null;
  run: RunSummary | null;
};

// ----- datasets --------------------------------------------------------------------------------

export type DatasetVersion = Present<S['DatasetVersion']>;
export type DatasetSummary = Omit<Present<S['Dataset']>, 'kind' | 'current_version'> & {
  kind: 'synthetic' | 'recorded';
  current_version: DatasetVersion | null;
};
/** `GET /datasets/{id}`: the summary with the current version's full manifest. */
export type Dataset = Omit<DatasetSummary, 'current_version'> & {
  current_version: (DatasetVersion & { manifest: S['DatasetManifest'] }) | null;
};
export type DatasetQuality = S['Quality'];

export type DemoRun = Present<S['DemoRun']>;
export type Demo = Omit<S['Demo'], 'runs'> & { runs: DemoRun[] };

// ----- comparisons, sweeps, reports ------------------------------------------------------------

/** bb-core `Comparison` plus the resource fields (`alignment.pairs` is empty; pairs are paged). */
export type Comparison = Omit<Present<S['Page_Comparison']['items'][number]>, 'status'> & { status: 'aligned' | 'metrics_only' };

export type SweepState = 'queued' | 'running' | 'completed' | 'partial' | 'cancelled';
/** Which part of the data a sweep run covers. */
export type Segment = S['Segment'];
/** A run's state; `not_submitted` when an interrupted submission left it out (resubmit the spec). */
export type SweepRunState = ExperimentStatus | 'not_submitted';
export type SweepCellRun = Omit<Present<S['SweepCellRun']>, 'state' | 'metrics'> & {
  state: SweepRunState;
  metrics: Present<S['CellMetrics']> | null;
};
export type SweepCell = Omit<Present<S['SweepCell']>, 'runs'> & { runs: SweepCellRun[] };
export type Sweep = Omit<Present<S['Sweep']>, 'state' | 'cells' | 'design'> & {
  state: SweepState;
  design: 'grid' | 'random' | 'seeds';
  cells: SweepCell[] | null;
};
export type SubmitSweepResponse = Omit<S['SweepSubmitted'], 'sweep'> & { sweep: Sweep };
/** `GET /sweeps/{id}/report`: `analysis` for grid and random designs, `ensemble` for seeds. */
export type SweepReport = Present<S['SweepReportResponse']>;

export type ReportKind = 'run' | 'comparison' | 'sweep';
export type Report = Omit<Present<S['Report']>, 'kind'> & { kind: ReportKind };

// ----- system (admin) --------------------------------------------------------------------------

export type WorkerHeartbeat = Present<S['WorkerRow']>;
export type QueueSummary = Omit<Present<S['QueueView']>, 'depth_by_platform'> & { depth_by_platform: Record<string, number> };
export type SystemEvent = Omit<Present<S['EventRow']>, 'detail'> & { detail: Record<string, unknown> | null };
export type ApiKeyInfo = Omit<Present<S['KeyRow']>, 'scopes'> & { scopes: Scope[] };

/** `GET /system/usage` is untyped in the generated schema; API_CONTRACT §8.9 shape. */
export interface Usage {
  blob: { month: string; uploads: number; uploads_limit: number; reads: number; reads_limit: number; bytes_stored: number; bytes_limit: number };
  db: { size_bytes: number; limit_bytes: number };
}

// ----- analytics -------------------------------------------------------------------------------

export type AnalyticsValue = S['Value'];
export type PriceMetric = S['PriceMetric'];
export type ConfidenceBand = S['ConfidenceBand'];
export type HorizonMetrics = S['HorizonMetrics'];
export type SpreadMetrics = S['LiquiditySplit_SpreadMetrics']['all'];
export type Distribution = S['Distribution'];
export type KaplanMeier = S['KaplanMeier'];
export type QueueBucket = S['QueueBucket'];
export type PnlDecomposition = S['LiquiditySplit_PnlDecomposition']['all'];
export type ExecutionReport = Omit<S['ExecutionReport'], 'opportunities'> & { opportunities: [NsString, Distribution][] };
export type RiskReport = S['RiskReport'];
export type RunAnalytics = Omit<Present<S['RunAnalytics']>, 'execution'> & { execution: ExecutionReport };
