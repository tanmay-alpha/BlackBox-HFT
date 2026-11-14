// View types: the v1 wire shapes (`v1.ts`) after `normalize.ts`. Relative nanosecond values
// (durations, times since a dataset origin) become exact JS numbers checked by
// `components/ns.ts`; absolute epoch nanoseconds (`*_at_ns`, `origin_ns`, `server_time_ns`) stay
// decimal strings and are formatted with `fmtNsUtc`.

import type { DatasetManifest, EquitySample, OrderPair, RunConfig } from './types';
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
  SweepState,
  SystemEvent,
  TruncatedBy,
  Usage,
  Whoami,
  WorkerHeartbeat,
  WorkerState,
} from './v1';

export type Run = NsNum<W.RunSummary, 'interval_ns'>;

/** `GET /runs/{id}`: always carries the configuration. */
export type RunDetail = Run & { config: RunConfig };

export type Experiment = Omit<NsNum<W.Experiment, 'interval_ns'>, 'run'> & { run: Run | null };

export interface SubmitExperimentResult {
  created: boolean;
  experiment: Experiment;
  job: W.Job | null;
  run: Run | null;
}

export type DatasetVersion = NsNum<W.DatasetVersion, 'start_ns' | 'end_ns'>;

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

export type Comparison = W.Comparison;
export type ComparisonPair = OrderPair;

export type Sweep = W.Sweep;
export type Demo = W.Demo;
export type DemoRun = W.DemoRun;

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
