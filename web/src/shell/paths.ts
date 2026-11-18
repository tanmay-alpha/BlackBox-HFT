// URL conventions (UI_DESIGN_SPEC.md §9): the path names the workspace and its primary entity,
// the query holds view parameters, nanosecond values travel as decimal strings. Every link in
// the console is built here so a route change is one edit.

const enc = encodeURIComponent;

type Query = Record<string, string | number | null | undefined>;

function withQuery(path: string, query: Query = {}): string {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v === null || v === undefined || v === '') continue;
    params.set(k, String(v));
  }
  const s = params.toString();
  return s ? `${path}?${s}` : path;
}

export const paths = {
  /** The public latency experiment, the console's landing page. */
  explore: () => '/',
  /** Research home: the investigations, each linking into the workspace holding its evidence. */
  research: () => '/research',
  /** The experiments catalogue (Mission Control). */
  overview: () => '/experiments',
  datasets: () => '/data',
  dataset: (id: string) => `/data/${enc(id)}`,
  book: () => '/book',
  bookDataset: (id: string, query?: { t0?: string | number; len?: string | number; levels?: number }) =>
    withQuery(`/book/data/${enc(id)}`, query),
  bookRun: (runId: string, query?: { t?: string | number | null; order?: number | null }) =>
    withQuery(`/book/runs/${enc(runId)}`, query),
  /** Market Data Workbench: L3 (order-level) datasets, the demo is synthetic. */
  market: () => '/market',
  /**
   * One workbench dataset. `tab` is the step, `event` the selected event index (shared by the
   * book, timeline and metrics), `type` the timeline's ITCH message-type filter, `from` the event
   * index the timeline page starts at, `example` the curated example id (`cancel-ahead`) the book
   * step's picker shows as selected.
   */
  marketDataset: (
    id: string,
    query?: {
      tab?: 'dataset' | 'validation' | 'book' | 'events' | 'metrics';
      event?: number | null;
      type?: string | null;
      from?: number | null;
      example?: string | null;
    },
  ) => withQuery(`/market/${enc(id)}`, query),
  /** Published execution studies (study viewer index). */
  studies: () => '/studies',
  /**
   * One published study. `metric` is a `measures` key of the study contract, `cell` the selected
   * run (`run.<run_id>`) or aggregate group (`group.<regime>.<queue>.<latency>.<feed_ns>`),
   * built with the study feature's `runCellKey` / `groupCellKey`.
   */
  study: (id: string, query?: { metric?: string | null; cell?: string | null }) => withQuery(`/studies/${enc(id)}`, query),
  /**
   * Queue Model & Fill Uncertainty Lab: the published `queue-shadow` study (its study route
   * `/studies/queue-shadow` redirects here, keeping the query). `model` is an L2 queue
   * model id, `side` is `pooled` (contract v1 has no per-side values), `latency`, `tau` and
   * `checkpoint` are design values in nanoseconds, `tab` the phone stage.
   */
  queueLab: (query?: {
    symbol?: string | null;
    model?: string | null;
    side?: string | null;
    latency?: string | number | null;
    tau?: string | number | null;
    checkpoint?: string | number | null;
    tab?: 'overview' | 'fills' | 'queue' | 'hypotheses' | null;
  }) => withQuery('/queue-lab', query),
  strategy: (query?: { dataset?: string; from?: string }) => withQuery('/strategy', query),
  runs: () => '/forensics',
  run: (runId: string) => `/forensics/${enc(runId)}`,
  order: (runId: string, orderId: number | string) => `/forensics/${enc(runId)}/orders/${enc(String(orderId))}`,
  compare: () => '/compare',
  comparePair: (a?: string, b?: string) => withQuery('/compare/pair', { a, b }),
  compareSet: (query?: { runs?: string[]; demo?: string }) =>
    withQuery('/compare/set', { runs: query?.runs?.join(','), demo: query?.demo }),
  /** Recent sweeps list. */
  sweeps: () => '/compare/sweeps',
  /** Result matrix of one sweep. */
  sweep: (sweepId: string) => `/compare/sweeps/${enc(sweepId)}`,
  /** Simulation Workbench job queue; `state` filters, `mine` = created by this key. */
  sim: (query?: { state?: string; mine?: 1 }) => withQuery('/sim', query),
  job: (jobId: string) => `/sim/jobs/${enc(jobId)}`,
  /** Sweep submission (Simulation Workbench), optionally from a base run. */
  sweepForm: (base?: string) => withQuery('/sim/sweep', { base }),
  risk: () => '/risk',
  riskRun: (runId: string) => `/risk/${enc(runId)}`,
  reports: () => '/reports',
  report: (runId: string) => `/reports/${enc(runId)}`,
  savedReport: (reportId: string) => `/reports/saved/${enc(reportId)}`,
  system: (tab?: 'health' | 'queue' | 'storage' | 'access' | 'about' | 'datasets' | 'events') => withQuery('/system', { tab }),
};
