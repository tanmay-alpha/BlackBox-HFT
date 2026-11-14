// View types: the v1 wire shapes (`v1.ts`) after `normalize.ts`. Relative nanosecond values
// (durations, times since a dataset origin) become exact JS numbers checked by
// `components/ns.ts`; absolute epoch nanoseconds (`*_at_ns`, `origin_ns`, `server_time_ns`) stay
// decimal strings and are formatted with `fmtNsUtc`.

import type {
  Comparison as CoreComparison,
  DatasetManifest,
  EngineStateCheck,
  EquitySample,
  Metrics,
  OrderDetail as CoreOrderDetail,
  OrderPair,
  Partition,
  RunConfig,
  RunTimeline as CoreRunTimeline,
  SweepSpec,
} from './types';
import type * as W from './v1';

/** `T` with the decimal-string ns fields `K` as exact numbers. */
type NsNum<T, K extends keyof T> = Omit<T, K> & { [P in K]: null extends T[P] ? number | null : number };

export type {
  AnalyticsValue,
  ApiKeyInfo,
  ConfidenceBand,
  Distribution,
  ExecutionReport,
  HorizonMetrics,
  KaplanMeier,
  PnlDecomposition,
  PriceMetric,
  QueueBucket,
  RiskReport,
  RunAnalytics,
  SpreadMetrics,
  CursorPage,
  DatasetQuality,
  ExperimentStatus,
  Job,
  JobFailureCategory,
  JobState,
  OffsetPage,
  QueueSummary,
  Report,
  ReportKind,
  Scope,
  Status,
  SubmitSweepResponse as WireSubmitSweepResponse,
  SweepCell,
  SweepCellRun,
  SweepRunState,
  SweepState,
  SystemEvent,
  TruncatedBy,
  Usage,
  Whoami,
  WorkerHeartbeat,
  WorkerState,
} from './v1';

/** A completed run as the views read it: nested bb-core values with exact ns numbers. */
export type Run = Omit<W.RunSummary, 'interval_ns' | 'metrics' | 'engine_check' | 'config' | 'strategy_kind' | 'label'> & {
  interval_ns: number | null;
  strategy_kind: string | null;
  label: string | null;
  metrics: Metrics;
  engine_check: EngineStateCheck;
  config: RunConfig | null;
};

/** `GET /runs/{id}`: always carries the configuration. */
export type RunDetail = Omit<Run, 'config'> & { config: RunConfig };

export type Experiment = Omit<W.Experiment, 'interval_ns' | 'run' | 'config'> & {
  interval_ns: number | null;
  run: Run | null;
  config: RunConfig | null;
};

export interface SubmitExperimentResult {
  created: boolean;
  experiment: Experiment;
  job: W.Job | null;
  run: Run | null;
}

export type DatasetVersion = Omit<W.DatasetVersion, 'start_ns' | 'end_ns' | 'manifest'> & {
  start_ns: number;
  end_ns: number;
  manifest: DatasetManifest | null;
};

export type DatasetSummary = Omit<W.DatasetSummary, 'current_version'> & { current_version: DatasetVersion | null };

/** `GET /datasets/{id}`: the summary with the current version's full manifest. */
export type Dataset = Omit<DatasetSummary, 'current_version'> & {
  current_version: (DatasetVersion & { manifest: DatasetManifest }) | null;
};

export type EquityBucket = NsNum<W.EquityBucket, 't0_ns' | 't1_ns'>;

interface EquityPageCommon {
  next_cursor: string | null;
  truncated_by: W.TruncatedBy;
}

/** Raw samples, or (when the window holds more than `max_points`) downsampled buckets. */
export type EquityPage = EquityPageCommon &
  ({ downsampled: false; items: EquitySample[]; bucket_ns: null } | { downsampled: true; items: EquityBucket[]; bucket_ns: number | null });

/** The resource fields of `W.Comparison` around the bb-core comparison (exact ns numbers). */
export type Comparison = Omit<W.Comparison, keyof CoreComparison> & CoreComparison;
export type ComparisonPair = OrderPair;

export type Sweep = Omit<W.Sweep, 'spec' | 'partition'> & { spec: SweepSpec | null; partition: Partition | null };
export type SweepReport = Omit<W.SweepReport, 'partition'> & { partition: Partition | null };
export type DemoRun = Omit<W.DemoRun, 'config'> & { config: RunConfig };
export type Demo = Omit<W.Demo, 'runs'> & { runs: DemoRun[] };

/** Dataset-relative timeline, bb-core names (`t0`, `run_start`, …) with exact ns numbers. */
export type RunTimeline = CoreRunTimeline & { truncated_by: W.TruncatedBy };
export type OrderDetail = CoreOrderDetail;

/** Whether `items` are downsampled buckets rather than raw samples. */
export function isBucketed(page: EquityPage): page is Extract<EquityPage, { downsampled: true }> {
  return page.downsampled;
}

export const TERMINAL_JOB_STATES: Record<W.JobState, boolean> = {
  created: false,
  queued: false,
  running: false,
  cancelling: false,
  completed: true,
  failed: true,
  cancelled: true,
  expired: true,
};
export const LIVE_JOB_STATES: readonly W.JobState[] = ['created', 'queued', 'running', 'cancelling'];
