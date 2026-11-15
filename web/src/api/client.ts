// Typed client for `/api/v1` (docs/engineering/API_CONTRACT.md §8). Every method returns exactly
// what the server sent, with every nanosecond field checked and converted by `normalize`
// (components/ns.ts). Nothing is synthesized, cached or retried here. Every method takes an
// optional `AbortSignal` so a superseded view cancels its request.

import { getApiKey } from './auth';
import {
  collectAll,
  request,
  requestDownload,
  requestText,
  type Collected,
  type Download,
} from './http';
import type * as M from './model';
import { normalize, toWire } from './normalize';
import type {
  DatasetManifest,
  FillRecord,
  FlightEvent,
  OrderOutcome,
  OrderPair,
  OrderRecord,
  PairClass,
  Preset,
  ReplayWindow,
  RunConfig,
  SweepSpec,
  Side,
} from './types';
import type * as W from './v1';

export { ApiError, API_BASE, collectAll, describeError, isAbort } from './http';
export type { Collected, Download, ErrorView } from './http';

export interface Opts {
  signal?: AbortSignal;
}

type Cursor = { cursor?: string | null; limit?: number };

const enc = encodeURIComponent;

/** Slug of the public demo workspace (migration 0005); any caller may list it with `workspace=demo`. */
export const DEMO_WORKSPACE = 'demo';

/** Relative ns query values travel as decimal strings. */
const nsQ = (v: number | string | null | undefined) => (v === null || v === undefined ? undefined : toWire.ns(v));

export interface ReplayQuery {
  t0?: number | null;
  t1?: number | null;
  step_ns?: number | null;
  levels?: number | null;
  max_trades?: number | null;
}

export interface ListQuery extends Cursor {
  workspace?: string;
  dataset_id?: string;
  strategy_kind?: string;
  status?: readonly string[];
  sweep_id?: string;
  label_prefix?: string;
  mine?: boolean;
  run_id?: string;
}

export interface RunsQuery extends ListQuery {
  sort?: 'created_desc' | 'net_pnl_desc' | 'net_pnl_asc' | 'max_drawdown_asc';
}

export interface JobsQuery extends Cursor {
  workspace?: string;
  state?: readonly W.JobState[];
  mine?: boolean;
}

export interface OrdersQuery {
  outcome?: OrderOutcome | null;
  side?: Side | null;
  offset?: number;
  limit?: number;
}

/** How many list items a "load everything" view reads before saying the list is incomplete. */
export const LIST_CAP = 1000;

/** Items per list page the console requests (the server accepts at most 100). */
export const PAGE_LIMIT = 50;

export const api = {
  // ----- health, identity ----------------------------------------------------------------------
  status: (o: Opts = {}) => request<W.Status>('GET', '/status', o).then(normalize.status),
  /** With `key`, validates that key without storing it. */
  whoami: (o: Opts & { key?: string } = {}) => request<W.Whoami>('GET', '/whoami', o).then(normalize.whoami),
  /** Database and storage readiness (`GET /readyz`, unversioned). Wakes the database: on demand only. */
  readyz: (o: Opts = {}) => request<W.Readiness>('GET', '/readyz', { ...o, root: true }),

  // ----- datasets ------------------------------------------------------------------------------
  datasetsPage: (q: Cursor = {}, o: Opts = {}) =>
    request<W.CursorPage<W.DatasetSummary>>('GET', '/datasets', { query: q, ...o }).then(normalize.datasets),
  datasets: (o: Opts = {}): Promise<Collected<M.DatasetSummary>> =>
    collectAll((cursor, signal) => api.datasetsPage({ cursor, limit: PAGE_LIMIT }, { signal }), { maxItems: LIST_CAP, signal: o.signal }),
  dataset: (id: string, o: Opts = {}) => request<W.Dataset>('GET', `/datasets/${enc(id)}`, o).then(normalize.dataset),
  /** The current version's manifest (`GET /datasets/{id}` embeds it). */
  async manifest(id: string, o: Opts = {}) {
    const d = await api.dataset(id, o);
    if (!d.current_version) throw new Error(`Dataset ${id} has no current version.`);
    return d.current_version.manifest;
  },
  datasetVersions: (id: string, q: Cursor = {}, o: Opts = {}) =>
    request<W.CursorPage<W.DatasetVersion>>('GET', `/datasets/${enc(id)}/versions`, { query: q, ...o }).then(normalize.datasetVersions),
  /** One version with its full manifest (the exact version a run used). */
  datasetVersion: (id: string, version: number, o: Opts = {}) =>
    request<W.DatasetVersion>('GET', `/datasets/${enc(id)}/versions/${version}`, o).then(
      (v) => normalize.datasetVersion(v) as M.DatasetVersion & { manifest: DatasetManifest },
    ),
  datasetQuality: (id: string, version: number, o: Opts = {}) =>
    request<W.DatasetQuality>('GET', `/datasets/${enc(id)}/versions/${version}/quality`, o),
  /** Replay of the current version, or of `version` (the exact version a run used). */
  replay: (id: string, q: ReplayQuery = {}, o: Opts & { version?: number } = {}) =>
    request<ReplayWindow>('GET', o.version === undefined ? `/datasets/${enc(id)}/replay` : `/datasets/${enc(id)}/versions/${o.version}/replay`, {
      query: { t0_ns: nsQ(q.t0), t1_ns: nsQ(q.t1), step_ns: nsQ(q.step_ns), levels: q.levels, max_trades: q.max_trades },
      signal: o.signal,
    }).then(normalize.replay),
  /** Presets written for `datasetId` (the server rebinds `config.dataset_id`). */
  presets: (datasetId?: string, o: Opts = {}) =>
    request<Preset[]>('GET', '/presets', { query: { dataset_id: datasetId }, ...o })
      .then(normalize.presets)
      .then((list) => (datasetId === undefined ? list : list.map((p) => ({ ...p, config: { ...p.config, dataset_id: datasetId } })))),
  demos: (datasetId?: string, o: Opts = {}) =>
    request<W.Demo[]>('GET', '/demos', { query: { dataset_id: datasetId }, ...o }).then(normalize.demos),

  // ----- experiments and jobs ------------------------------------------------------------------
  /** Enqueues a job, or returns the live job / stored run of the same experiment (idempotent). */
  submitExperiment: (config: RunConfig, label: string | null, o: Opts = {}) =>
    request<W.SubmitExperimentResponse>('POST', '/experiments', { body: { config: toWire.config(config), label }, ...o }).then(normalize.submitted),
  experimentsPage: (q: ListQuery = {}, o: Opts = {}) =>
    request<W.CursorPage<W.Experiment>>('GET', '/experiments', { query: q, ...o }).then(normalize.experiments),
  experiment: (id: string, o: Opts = {}) => request<W.Experiment>('GET', `/experiments/${enc(id)}`, o).then(normalize.experiment),
  deleteExperiment: (id: string, o: Opts = {}) => request<void>('DELETE', `/experiments/${enc(id)}`, o),
  jobsPage: (q: JobsQuery = {}, o: Opts = {}) => request<W.CursorPage<W.Job>>('GET', '/jobs', { query: q, ...o }).then(normalize.jobs),
  job: (id: string, o: Opts = {}) => request<W.Job>('GET', `/jobs/${enc(id)}`, o).then(normalize.job),
  cancelJob: (id: string, o: Opts = {}) => request<W.Job>('POST', `/jobs/${enc(id)}/cancel`, { body: {}, ...o }).then(normalize.job),

  // ----- runs ----------------------------------------------------------------------------------
  runsPage: (q: RunsQuery = {}, o: Opts = {}) =>
    request<W.CursorPage<W.RunSummary>>('GET', '/runs', { query: q, ...o }).then(normalize.runs),
  runs: (q: RunsQuery = {}, o: Opts = {}): Promise<Collected<M.Run>> =>
    collectAll((cursor, signal) => api.runsPage({ ...q, cursor, limit: PAGE_LIMIT }, { signal }), { maxItems: LIST_CAP, signal: o.signal }),
  run: (id: string, o: Opts = {}) => request<W.RunSummary>('GET', `/runs/${enc(id)}`, o).then((r) => normalize.run(r) as M.RunDetail),
  engineCheck: (id: string, o: Opts = {}) => request<M.Run['engine_check']>('GET', `/runs/${enc(id)}/engine-check`, o),
  /** bb-analytics execution and risk reports of a run (`GET /runs/{id}/analytics`). */
  runAnalytics: (id: string, o: Opts = {}) => request<W.RunAnalytics>('GET', `/runs/${enc(id)}/analytics`, o).then(normalize.analytics),
  orders: (id: string, q: OrdersQuery = {}, o: Opts = {}) =>
    request<W.OffsetPage<OrderRecord>>('GET', `/runs/${enc(id)}/orders`, { query: q, ...o }).then(normalize.orders),
  orderDetail: (id: string, orderId: number, o: Opts = {}) =>
    request<W.OrderDetail>('GET', `/runs/${enc(id)}/orders/${orderId}`, o).then(normalize.orderDetail),
  fillsPage: (id: string, q: Cursor & { t0?: number; t1?: number } = {}, o: Opts = {}) =>
    request<W.CursorPage<FillRecord>>('GET', `/runs/${enc(id)}/fills`, {
      query: { t0_ns: nsQ(q.t0), t1_ns: nsQ(q.t1), cursor: q.cursor, limit: q.limit },
      ...o,
    }).then(normalize.fills),
  fills: (id: string, o: Opts & { maxItems?: number } = {}): Promise<Collected<FillRecord>> =>
    collectAll((cursor, signal) => api.fillsPage(id, { cursor, limit: 5000 }, { signal }), { maxItems: o.maxItems ?? 50_000, signal: o.signal }),
  equity: (id: string, q: { t0?: number; t1?: number; max_points?: number; cursor?: string | null } = {}, o: Opts = {}) =>
    request<W.EquityPage>('GET', `/runs/${enc(id)}/equity`, {
      query: { t0_ns: nsQ(q.t0), t1_ns: nsQ(q.t1), max_points: q.max_points, cursor: q.cursor },
      ...o,
    }).then(normalize.equity),
  tape: (id: string, q: { from_seq?: number; order_id?: number; limit?: number; cursor?: string | null } = {}, o: Opts = {}) =>
    request<W.CursorPage<FlightEvent>>('GET', `/runs/${enc(id)}/tape`, { query: q, ...o }).then(normalize.tape),
  /** One page of a window's timeline; follow `next_cursor` (same `t0`/`t1`) for the rest of its events. */
  timeline: (id: string, q: { t0?: number | null; t1?: number | null; max_events?: number; cursor?: string | null } = {}, o: Opts = {}) =>
    request<W.RunTimeline>('GET', `/runs/${enc(id)}/timeline`, {
      query: { t0_ns: nsQ(q.t0), t1_ns: nsQ(q.t1), max_events: q.max_events, cursor: q.cursor },
      ...o,
    }).then(normalize.timeline),
  /** The deterministic research report of a run (`text/markdown`). */
  runReport: (id: string, o: Opts = {}) => requestText(`/runs/${enc(id)}/report`, o),
  /** The verified run bundle (`.bbz`, gzip NDJSON). */
  runExport: (id: string, o: Opts = {}): Promise<Download> => requestDownload(`/runs/${enc(id)}/export`, `run-${id}.bbz`, o),

  // ----- comparisons ---------------------------------------------------------------------------
  createComparison: (runA: string, runB: string, label?: string, o: Opts = {}) =>
    request<W.Comparison>('POST', '/comparisons', { body: { run_a: runA, run_b: runB, label }, ...o }).then(normalize.comparison),
  comparisonsPage: (q: Cursor & { workspace?: string } = {}, o: Opts = {}) =>
    request<W.CursorPage<W.Comparison>>('GET', '/comparisons', { query: q, ...o }).then(normalize.comparisons),
  comparisons: (o: Opts & { workspace?: string } = {}): Promise<Collected<M.Comparison>> =>
    collectAll((cursor, signal) => api.comparisonsPage({ cursor, limit: PAGE_LIMIT, workspace: o.workspace }, { signal }), {
      maxItems: LIST_CAP,
      signal: o.signal,
    }),
  /**
   * Every stored comparison the caller can open: its own workspace's, plus, for a keyed caller
   * (whose default list holds only its own workspace), the public demo's curated ones. Used to
   * find the comparison of two runs, which may live in either workspace.
   */
  async readableComparisons(o: Opts = {}): Promise<Collected<M.Comparison>> {
    const own = await api.comparisons(o);
    if (getApiKey() === null) return own;
    const demo = await api.comparisons({ ...o, workspace: DEMO_WORKSPACE });
    const seen = new Set(own.items.map((c) => c.id));
    return {
      items: [...own.items, ...demo.items.filter((c) => !seen.has(c.id))],
      incomplete: own.incomplete || demo.incomplete,
      pages: own.pages + demo.pages,
    };
  },
  comparison: (id: string, o: Opts = {}) => request<W.Comparison>('GET', `/comparisons/${enc(id)}`, o).then(normalize.comparison),
  comparisonPairs: (id: string, q: Cursor & { class?: PairClass } = {}, o: Opts = {}) =>
    request<W.CursorPage<OrderPair>>('GET', `/comparisons/${enc(id)}/pairs`, { query: q, ...o }).then(normalize.pairs),
  comparisonReport: (id: string, o: Opts = {}) => requestText(`/comparisons/${enc(id)}/report`, o),

  // ----- sweeps --------------------------------------------------------------------------------
  /** Submits a `SweepSpec` (grid, random or seeds design, optional IS/OOS partition). Idempotent per spec. */
  createSweep: (spec: SweepSpec, label: string | null, o: Opts = {}) =>
    request<W.SubmitSweepResponse>('POST', '/sweeps', { body: { spec: toWire.sweepSpec(spec), label }, ...o }).then((r) => ({
      created: r.created,
      sweep: normalize.sweep(r.sweep),
    })),
  sweepsPage: (q: Cursor & { workspace?: string } = {}, o: Opts = {}) =>
    request<W.CursorPage<W.Sweep>>('GET', '/sweeps', { query: q, ...o }).then(normalize.sweeps),
  sweep: (id: string, o: Opts = {}) => request<W.Sweep>('GET', `/sweeps/${enc(id)}`, o).then(normalize.sweep),
  cancelSweep: (id: string, o: Opts = {}) => request<W.Sweep>('POST', `/sweeps/${enc(id)}/cancel`, { body: {}, ...o }).then(normalize.sweep),
  /** The analysis of a finished sweep (409 `conflict` while a run is still live). */
  sweepReport: (id: string, o: Opts = {}) =>
    request<W.SweepReport>('GET', `/sweeps/${enc(id)}/report`, o).then(normalize.sweepReport),
  sweepReportMarkdown: (id: string, o: Opts = {}) => requestText(`/sweeps/${enc(id)}/report?format=markdown`, o),

  // ----- saved reports -------------------------------------------------------------------------
  createReport: (kind: W.ReportKind, subjectId: string, title: string, o: Opts = {}) =>
    request<W.Report>('POST', '/reports', { body: { kind, subject_id: subjectId, title }, ...o }).then(normalize.report),
  reportsPage: (q: Cursor & { workspace?: string } = {}, o: Opts = {}) =>
    request<W.CursorPage<W.Report>>('GET', '/reports', { query: q, ...o }).then(normalize.reports),
  report: (id: string, o: Opts = {}) => request<W.Report>('GET', `/reports/${enc(id)}`, o).then(normalize.report),
  reportContent: (id: string, o: Opts = {}) => requestText(`/reports/${enc(id)}/content`, o),
  deleteReport: (id: string, o: Opts = {}) => request<void>('DELETE', `/reports/${enc(id)}`, o),

  // ----- system (admin) ------------------------------------------------------------------------
  systemWorkers: (o: Opts = {}) => request<W.WorkerHeartbeat[]>('GET', '/system/workers', o),
  systemQueue: (o: Opts = {}) => request<W.QueueSummary>('GET', '/system/queue', o),
  systemUsage: (o: Opts = {}) => request<W.Usage>('GET', '/system/usage', o),
  systemEvents: (q: { kind?: string; level?: string; limit?: number } = {}, o: Opts = {}) =>
    request<W.SystemEvent[]>('GET', '/system/events', { query: q, ...o }).then(normalize.events),
  systemApiKeys: (o: Opts = {}) => request<W.ApiKeyInfo[]>('GET', '/system/api-keys', o).then(normalize.apiKeys),
};

export type Api = typeof api;

/** Hands a downloaded body to the browser as a file (the key never appears in a URL). */
export function saveDownload({ blob, filename }: Download): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.rel = 'noreferrer';
  document.body.append(a);
  a.click();
  a.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** Saves text (a Markdown report) under `filename`. */
export function saveText(text: string, filename: string, type = 'text/markdown'): void {
  saveDownload({ blob: new Blob([text], { type: `${type};charset=utf-8` }), filename });
}
