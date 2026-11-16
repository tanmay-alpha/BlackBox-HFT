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
   * index the timeline page starts at.
   */
  marketDataset: (
    id: string,
    query?: { tab?: 'dataset' | 'validation' | 'book' | 'events' | 'metrics'; event?: number | null; type?: string | null; from?: number | null },
  ) => withQuery(`/market/${enc(id)}`, query),
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
