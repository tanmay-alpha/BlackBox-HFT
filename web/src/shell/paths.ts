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
  overview: () => '/',
  datasets: () => '/data',
  dataset: (id: string) => `/data/${enc(id)}`,
  book: () => '/book',
  bookDataset: (id: string, query?: { t0?: string | number; len?: string | number; levels?: number }) =>
    withQuery(`/book/data/${enc(id)}`, query),
  bookRun: (runId: string, query?: { t?: string | number | null; order?: number | null }) =>
    withQuery(`/book/runs/${enc(runId)}`, query),
  strategy: (query?: { dataset?: string; from?: string }) => withQuery('/strategy', query),
  runs: () => '/forensics',
  run: (runId: string) => `/forensics/${enc(runId)}`,
  order: (runId: string, orderId: number | string) => `/forensics/${enc(runId)}/orders/${enc(String(orderId))}`,
  compare: () => '/compare',
  comparePair: (a?: string, b?: string) => withQuery('/compare/pair', { a, b }),
  compareSet: (query?: { runs?: string[]; demo?: string }) =>
    withQuery('/compare/set', { runs: query?.runs?.join(','), demo: query?.demo }),
  sweep: (base?: string) => withQuery('/compare/sweep', { base }),
};
