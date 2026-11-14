// Typed client for the BlackBox HTTP API (docs/API.md). Every method returns exactly what the
// server sent; nothing is synthesized, cached or retried here.
//
// Built with `VITE_STATIC_DATA=1` (`vite build --mode static`), the client is the hosted
// read-only viewer instead: every GET resolves to a file written by `blackbox export-static`
// (served under /static-data/), holding the JSON the server returns for that request, and
// everything that would simulate or change state throws an `ApiError` with code `static_viewer`.

import type {
  ApiErrorBody,
  CompareRequest,
  Comparison,
  DatasetManifest,
  Demo,
  EngineStateCheck,
  EquitySample,
  FillRecord,
  GridResult,
  GridSpec,
  Health,
  OrderDetail,
  OrderRecord,
  OrdersPage,
  OrdersQuery,
  Preset,
  ReplayQuery,
  ReplayWindow,
  RunRequest,
  RunSummary,
  RunTimeline,
  TapePage,
  TapeQuery,
  TimelineQuery,
} from './types';
import { normalize } from './normalize';

/**
 * A failed API call. `code` is the server's snake_case error code (`not_found`,
 * `invalid_request`, `invalid_config`, `simulation_failed`, `internal`), or one of the
 * client-side codes `network` (no response; `status` is 0), `bad_response` (the server
 * answered with something that is not the documented JSON), `unsafe_integer` (a nanosecond
 * field cannot be represented exactly; `status` is 0; see normalize.ts) and `static_viewer`
 * (the hosted read-only viewer cannot do this; `status` is 0).
 */
export class ApiError extends Error {
  code: string;
  status: number;

  constructor(code: string, message: string, status: number) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
    this.status = status;
  }
}

const BASE = '/api';

type QueryValue = string | number | boolean | null | undefined;

function queryString(query: object | undefined): string {
  if (!query) return '';
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query) as [string, QueryValue][]) {
    if (value === null || value === undefined) continue;
    params.set(key, String(value));
  }
  const s = params.toString();
  return s ? `?${s}` : '';
}

function isErrorBody(v: unknown): v is ApiErrorBody {
  if (typeof v !== 'object' || v === null || !('error' in v)) return false;
  const e = v.error;
  if (typeof e !== 'object' || e === null) return false;
  if (!('code' in e) || !('message' in e)) return false;
  return typeof e.code === 'string' && typeof e.message === 'string';
}

async function request<T>(
  method: 'GET' | 'POST' | 'DELETE',
  path: string,
  opts: { query?: object; body?: unknown } = {},
): Promise<T> {
  const url = `${BASE}${path}${queryString(opts.query)}`;
  const init: RequestInit = { method, headers: { Accept: 'application/json' } };
  if (opts.body !== undefined) {
    init.headers = { Accept: 'application/json', 'Content-Type': 'application/json' };
    init.body = JSON.stringify(opts.body);
  }

  let res: Response;
  try {
    res = await fetch(url, init);
  } catch (cause) {
    const why = cause instanceof Error ? cause.message : String(cause);
    throw new ApiError(
      'network',
      `Could not reach the BlackBox server for ${method} ${url} (${why}). Start it with "blackbox serve".`,
      0,
    );
  }

  const text = await res.text();

  if (!res.ok) {
    let parsed: unknown = null;
    try {
      parsed = text ? JSON.parse(text) : null;
    } catch {
      parsed = null;
    }
    if (isErrorBody(parsed)) {
      throw new ApiError(parsed.error.code, parsed.error.message, res.status);
    }
    const snippet = text.trim().slice(0, 200);
    throw new ApiError(
      'bad_response',
      `${method} ${url} failed with HTTP ${res.status} ${res.statusText}` +
        (snippet ? `: ${snippet}` : ' and no error body.'),
      res.status,
    );
  }

  if (res.status === 204 || text.length === 0) {
    return undefined as T;
  }
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new ApiError(
      'bad_response',
      `${method} ${url} returned HTTP ${res.status} with a body that is not JSON.`,
      res.status,
    );
  }
}

const enc = encodeURIComponent;

const liveApi = {
  health(): Promise<Health> {
    return request<Health>('GET', '/health');
  },

  datasets(): Promise<DatasetManifest[]> {
    return request<DatasetManifest[]>('GET', '/datasets');
  },

  dataset(id: string): Promise<DatasetManifest> {
    return request<DatasetManifest>('GET', `/datasets/${enc(id)}`);
  },

  replay(id: string, query: ReplayQuery = {}): Promise<ReplayWindow> {
    return request<ReplayWindow>('GET', `/datasets/${enc(id)}/replay`, { query });
  },

  /**
   * The server's presets. A preset is a complete `RunConfig`, including the dataset it was
   * written for. With `datasetId`, each returned preset's `config.dataset_id` is rebound to that
   * dataset so it can be run there; every other field is left exactly as the server sent it.
   */
  async presets(datasetId?: string): Promise<Preset[]> {
    const presets = await request<Preset[]>('GET', '/presets');
    return rebindPresets(presets, datasetId);
  },

  /** Synchronous and idempotent: an already simulated configuration returns the stored run. */
  createRun(req: RunRequest): Promise<RunSummary> {
    return request<RunSummary>('POST', '/runs', { body: req });
  },

  /** Newest first. */
  runs(): Promise<RunSummary[]> {
    return request<RunSummary[]>('GET', '/runs');
  },

  run(id: string): Promise<RunSummary> {
    return request<RunSummary>('GET', `/runs/${enc(id)}`);
  },

  deleteRun(id: string): Promise<void> {
    return request<void>('DELETE', `/runs/${enc(id)}`);
  },

  orders(runId: string, query: OrdersQuery = {}): Promise<OrdersPage> {
    return request<OrdersPage>('GET', `/runs/${enc(runId)}/orders`, { query });
  },

  orderDetail(runId: string, orderId: number): Promise<OrderDetail> {
    return request<OrderDetail>('GET', `/runs/${enc(runId)}/orders/${enc(String(orderId))}`);
  },

  fills(runId: string): Promise<FillRecord[]> {
    return request<FillRecord[]>('GET', `/runs/${enc(runId)}/fills`);
  },

  equity(runId: string): Promise<EquitySample[]> {
    return request<EquitySample[]>('GET', `/runs/${enc(runId)}/equity`);
  },

  /** Cross-check of BlackBox's ledger against the engine's own account state for a run. */
  engineCheck(runId: string): Promise<EngineStateCheck> {
    return request<EngineStateCheck>('GET', `/runs/${enc(runId)}/engine-check`);
  },

  tape(runId: string, query: TapeQuery = {}): Promise<TapePage> {
    return request<TapePage>('GET', `/runs/${enc(runId)}/tape`, { query });
  },

  compare(a: string, b: string): Promise<Comparison> {
    const body: CompareRequest = { a, b };
    return request<Comparison>('POST', '/compare', { body });
  },

  /** Synchronous; every cell is a real simulation stored as a normal run. */
  grid(spec: GridSpec): Promise<GridResult> {
    return request<GridResult>('POST', '/grids', { body: spec });
  },

  /**
   * Tape events, fills, account samples and live orders of a run for one time window. Callers
   * keep windows short (the server defaults to 60 s) and page as the cursor moves.
   */
  timeline(runId: string, query: TimelineQuery = {}): Promise<RunTimeline> {
    return request<RunTimeline>('GET', `/runs/${enc(runId)}/timeline`, { query });
  },

  /** The canonical experiments for a dataset (default: the server's first dataset). */
  demos(datasetId?: string): Promise<Demo[]> {
    return request<Demo[]>('GET', '/demos', { query: { dataset_id: datasetId } });
  },
};

export type Api = typeof liveApi;

function rebindPresets(presets: Preset[], datasetId: string | undefined): Preset[] {
  if (datasetId === undefined) return presets;
  return presets.map((p) => ({ ...p, config: { ...p.config, dataset_id: datasetId } }));
}

// ---------------------------------------------------------------------------------------------
// Hosted read-only viewer
// ---------------------------------------------------------------------------------------------

const STATIC = import.meta.env.VITE_STATIC_DATA === '1';
/** Where the output of `blackbox export-static` is served. */
const STATIC_ROOT = '/static-data';
/** Written by the exporter next to the bundle root; names that root. */
const MARKER_FILE = 'blackbox-static.json';
/** The UI's replay and timeline window, and the server's default window length. */
const WINDOW_NS = 60_000_000_000;
const DEFAULT_LEVELS = 25;
const MAX_LEVELS = 100;
const DEFAULT_ORDERS_LIMIT = 200;
const MAX_ORDERS_LIMIT = 2000;

/** The explanation every refused action carries in the hosted viewer. */
export const STATIC_VIEWER_MESSAGE =
  'This is the hosted read-only viewer of precomputed synthetic runs. Run BlackBox locally to simulate (see README).';

/** Whether this build is the hosted read-only viewer of precomputed runs. */
export function isStaticViewer(): boolean {
  return STATIC;
}

function staticViewerError(detail: string): ApiError {
  return new ApiError('static_viewer', `${STATIC_VIEWER_MESSAGE} ${detail}`, 0);
}

function notFound(message: string): ApiError {
  return new ApiError('not_found', message, 404);
}

/** `blackbox-static.json`: what the bundle holds. */
interface StaticMarker {
  format: number;
  notice: string;
  blackbox_version: string;
  dataset_id: string;
  /** Directory under /static-data/ holding `api/`, named by the bundle's content hash. */
  root: string;
  runs: string[];
  /** `"<a>__<b>"` per precomputed comparison. */
  compares: string[];
  grids: string[];
}

/** Precomputed windows of one replay or timeline endpoint. */
interface WindowIndex {
  start: number;
  end: number;
  /** Depth levels of every replay file. */
  levels?: number;
  /** Canonical query string to bundle path of the exact server response. */
  exact: Record<string, string>;
  /** 120 s windows every 60 s: any window of at most 60 s lies inside one of them. */
  tiles: { t0: number; t1: number; query: string; file: string }[];
}

async function fetchStaticText(file: string): Promise<string> {
  const url = `${STATIC_ROOT}/${file}`;
  let res: Response;
  try {
    // The marker names the current bundle; it must never come from a stale cache.
    res = await fetch(url, file === MARKER_FILE ? { cache: 'no-cache' } : undefined);
  } catch (cause) {
    const why = cause instanceof Error ? cause.message : String(cause);
    throw new ApiError('network', `Could not load ${url} from the hosted viewer (${why}).`, 0);
  }
  if (!res.ok) {
    throw new ApiError(
      res.status === 404 ? 'not_found' : 'bad_response',
      `${url} could not be loaded from the precomputed bundle (HTTP ${res.status} ${res.statusText}).`,
      res.status,
    );
  }
  const bytes = new Uint8Array(await res.arrayBuffer());
  // Files are gzip-compressed by `blackbox export-static`. A host that sends them with
  // `Content-Encoding: gzip` has already inflated them, so the magic number decides.
  if (bytes.length >= 2 && bytes[0] === 0x1f && bytes[1] === 0x8b) {
    const inflated = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'));
    return new Response(inflated).text();
  }
  return new TextDecoder().decode(bytes);
}

/**
 * Decompressed files, most recently used last. The bundle is immutable; holding the text (not
 * parsed objects) means every caller still gets its own fresh objects, as from the server.
 */
const staticTexts = new Map<string, Promise<string>>();
const STATIC_TEXTS_KEPT = 12;

function staticText(file: string): Promise<string> {
  const hit = staticTexts.get(file);
  if (hit) {
    staticTexts.delete(file);
    staticTexts.set(file, hit);
    return hit;
  }
  const text = fetchStaticText(file);
  staticTexts.set(file, text);
  text.catch(() => staticTexts.delete(file));
  for (const key of staticTexts.keys()) {
    if (staticTexts.size <= STATIC_TEXTS_KEPT) break;
    staticTexts.delete(key);
  }
  return text;
}

async function parseStatic<T>(file: string): Promise<T> {
  const text = await staticText(file);
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new ApiError(
      'bad_response',
      `${STATIC_ROOT}/${file} is not JSON; the precomputed bundle is incomplete or was not deployed.`,
      200,
    );
  }
}

/**
 * The marker is the only file served without a long cache lifetime; it is read once per page
 * load. Every other file lives under its content-hash `root`, so a redeployed bundle has new URLs.
 */
let markerPromise: Promise<StaticMarker> | null = null;
/** The marker's `root` once known, for the synchronous download addresses. */
let bundleRoot = '';

function marker(): Promise<StaticMarker> {
  if (markerPromise === null) {
    const loading = parseStatic<StaticMarker>(MARKER_FILE).then((m) => {
      if (typeof m.root !== 'string' || m.root === '') {
        throw new ApiError(
          'bad_response',
          `${STATIC_ROOT}/${MARKER_FILE} names no bundle root; export the bundle again with this version of blackbox.`,
          200,
        );
      }
      bundleRoot = m.root;
      return m;
    });
    loading.catch(() => {
      markerPromise = null;
    });
    markerPromise = loading;
  }
  return markerPromise;
}

/** A bundle file, by its path under the marker's root (`api/...`). */
async function staticJson<T>(file: string): Promise<T> {
  const m = await marker();
  return parseStatic<T>(`${m.root}/${file}`);
}

// Start reading the marker at once, so download addresses know the root by the time a page
// renders them; a failure is reported by the first API call instead.
if (STATIC) marker().catch(() => undefined);

async function requireDataset(id: string): Promise<StaticMarker> {
  const m = await marker();
  if (id !== m.dataset_id) {
    throw notFound(`dataset '${id}' not found; this hosted viewer holds only '${m.dataset_id}'`);
  }
  return m;
}

async function requireRun(id: string): Promise<StaticMarker> {
  const m = await marker();
  if (!m.runs.includes(id)) {
    throw notFound(`run '${id}' not found among the precomputed runs of this hosted viewer`);
  }
  return m;
}

/** The query as `key=value` pairs, keys sorted, absent values skipped (as the exporter keys it). */
function canonicalQuery(query: object): string {
  return (Object.entries(query) as [string, QueryValue][])
    .filter(([, v]) => v !== null && v !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([k, v]) => `${k}=${String(v)}`)
    .join('&');
}

/** The precomputed tile holding `[t0, t1]`, else the one holding `t0`, else the nearest. */
function tileFor(index: WindowIndex, t0: number, t1: number): string {
  const tiles = index.tiles;
  const tile =
    tiles.find((w) => w.t0 <= t0 && t1 <= w.t1) ??
    tiles.find((w) => w.t0 <= t0 && t0 <= w.t1) ??
    (t0 < tiles[0].t0 ? tiles[0] : tiles[tiles.length - 1]);
  return tile.file;
}

/** Structural equality of two JSON values; key order is irrelevant. */
function jsonEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a) || Array.isArray(b)) {
    return (
      Array.isArray(a) &&
      Array.isArray(b) &&
      a.length === b.length &&
      a.every((v, i) => jsonEqual(v, b[i]))
    );
  }
  const ra = a as Record<string, unknown>;
  const rb = b as Record<string, unknown>;
  const keys = Object.keys(ra);
  return (
    keys.length === Object.keys(rb).length &&
    keys.every((k) => Object.prototype.hasOwnProperty.call(rb, k) && jsonEqual(ra[k], rb[k]))
  );
}

/** Every precomputed sensitivity grid of the hosted viewer; empty outside it. */
export async function precomputedGrids(): Promise<GridResult[]> {
  if (!STATIC) return [];
  const m = await marker();
  return Promise.all(m.grids.map((g) => staticJson<GridResult>(`api/grids/${enc(g)}.json.gz`).then(normalize.grid)));
}

const staticApi: Api = {
  health() {
    return staticJson<Health>('api/health.json.gz');
  },

  datasets() {
    return staticJson<DatasetManifest[]>('api/datasets.json.gz');
  },

  async dataset(id) {
    await requireDataset(id);
    return staticJson<DatasetManifest>(`api/datasets/${enc(id)}.json.gz`);
  },

  /**
   * The exact precomputed response when this query was exported (the Replay page's default and
   * whole-dataset windows); otherwise the precomputed tile holding the requested window. The
   * returned `t0`/`t1`/`step_ns` describe the window actually returned.
   */
  async replay(id, query = {}) {
    await requireDataset(id);
    const index = await staticJson<WindowIndex>(`api/datasets/${enc(id)}/replay/index.json.gz`);
    const exact = index.exact[canonicalQuery(query)];
    if (exact !== undefined) return staticJson<ReplayWindow>(exact);
    const t0 = query.t0 ?? index.start;
    const t1 = query.t1 ?? Math.min(index.end, t0 + WINDOW_NS);
    if (t1 <= t0) {
      throw new ApiError('invalid_request', `t1 (${t1}) must be greater than t0 (${t0})`, 400);
    }
    const levels = Math.min(query.levels ?? DEFAULT_LEVELS, MAX_LEVELS);
    if (index.levels !== undefined && levels > index.levels) {
      throw staticViewerError(
        `Only the top ${index.levels} depth levels per side are precomputed; ${levels} were requested.`,
      );
    }
    return staticJson<ReplayWindow>(tileFor(index, t0, t1));
  },

  async presets(datasetId) {
    return rebindPresets(await staticJson<Preset[]>('api/presets.json.gz'), datasetId);
  },

  /** Resolves to the precomputed run with exactly this configuration, as the server would. */
  async createRun(req) {
    const runs = await staticJson<RunSummary[]>('api/runs.json.gz');
    const stored = runs.find((r) => jsonEqual(r.manifest.config, req.config));
    if (stored === undefined) {
      throw staticViewerError('This configuration was not precomputed, and nothing is simulated here.');
    }
    return stored;
  },

  runs() {
    return staticJson<RunSummary[]>('api/runs.json.gz');
  },

  async run(id) {
    await requireRun(id);
    return staticJson<RunSummary>(`api/runs/${enc(id)}.json.gz`);
  },

  async deleteRun() {
    throw staticViewerError('Precomputed runs cannot be deleted.');
  },

  /** The whole order list is precomputed; filtering and paging follow the server's `OrdersQuery`. */
  async orders(runId, query = {}) {
    await requireRun(runId);
    const all = await staticJson<OrderRecord[]>(`api/runs/${enc(runId)}/orders.json.gz`);
    const offset = query.offset ?? 0;
    const limit = Math.min(query.limit ?? DEFAULT_ORDERS_LIMIT, MAX_ORDERS_LIMIT);
    const matching = all.filter(
      (o) =>
        (query.outcome === null || query.outcome === undefined || o.outcome === query.outcome) &&
        (query.side === null || query.side === undefined || o.side === query.side),
    );
    return { total: matching.length, offset, orders: matching.slice(offset, offset + limit) };
  },

  /** Details are stored 100 orders per file; `orders/index.json` lists each file's first order id. */
  async orderDetail(runId, orderId) {
    await requireRun(runId);
    const missing = notFound(`run '${runId}' has no order ${orderId}`);
    const dir = `api/runs/${enc(runId)}/orders`;
    const firstIds = await staticJson<number[]>(`${dir}/index.json.gz`);
    // The last file whose first order id is at most `orderId`.
    let lo = 0;
    let hi = firstIds.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (firstIds[mid] <= orderId) lo = mid + 1;
      else hi = mid;
    }
    if (lo === 0) throw missing;
    const shard = await staticJson<Record<string, OrderDetail>>(`${dir}/${lo - 1}.json.gz`);
    const detail = shard[String(orderId)];
    if (detail === undefined) throw missing;
    return detail;
  },

  async fills(runId) {
    await requireRun(runId);
    return staticJson<FillRecord[]>(`api/runs/${enc(runId)}/fills.json.gz`);
  },

  async equity(runId) {
    await requireRun(runId);
    return staticJson<EquitySample[]>(`api/runs/${enc(runId)}/equity.json.gz`);
  },

  async engineCheck(runId) {
    await requireRun(runId);
    return staticJson<EngineStateCheck>(`api/runs/${enc(runId)}/engine-check.json.gz`);
  },

  async tape() {
    throw staticViewerError(
      'The paged flight-recorder tape is not part of the bundle; order forensics and the replay terminal carry every event.',
    );
  },

  async compare(a, b) {
    const m = await marker();
    for (const id of [a, b]) {
      if (!m.runs.includes(id)) {
        throw notFound(`run '${id}' not found among the precomputed runs of this hosted viewer`);
      }
    }
    const key = `${a}__${b}`;
    if (!m.compares.includes(key)) {
      throw staticViewerError(
        `Only the comparisons the canonical experiments use (each run of a demo against the demo's first run, either way round) are precomputed; ${a} against ${b} is not one of them.`,
      );
    }
    return staticJson<Comparison>(`api/compare/${key}.json.gz`);
  },

  async grid(spec) {
    for (const result of await precomputedGrids()) {
      if (jsonEqual(result.spec, spec)) return result;
    }
    throw staticViewerError('Only the precomputed sensitivity grid shown on this page is available.');
  },

  /** As `replay`: the exact count-only request at the run start, else the tile holding the window. */
  async timeline(runId, query = {}) {
    await requireRun(runId);
    const index = await staticJson<WindowIndex>(`api/runs/${enc(runId)}/timeline/index.json.gz`);
    const exact = index.exact[canonicalQuery(query)];
    if (exact !== undefined) return staticJson<RunTimeline>(exact);
    const t0 = query.t0 ?? index.start;
    const t1 = query.t1 ?? Math.min(index.end, t0 + WINDOW_NS);
    if (t1 < t0) throw new ApiError('invalid_request', 'timeline t1 must be >= t0', 400);
    return staticJson<RunTimeline>(tileFor(index, t0, t1));
  },

  async demos(datasetId) {
    if (datasetId !== undefined) await requireDataset(datasetId);
    return staticJson<Demo[]>('api/demos.json.gz');
  },
};

/**
 * The API every view uses: the live or static source with every nanosecond field of every
 * response checked and converted by `normalize` (components/ns.ts) before it is returned.
 */
function withExactNs(base: Api): Api {
  return {
    ...base,
    datasets: () => base.datasets().then(normalize.datasets),
    dataset: (id) => base.dataset(id).then((d) => normalize.dataset(d)),
    replay: (id, query) => base.replay(id, query).then(normalize.replay),
    presets: (datasetId) => base.presets(datasetId).then(normalize.presets),
    createRun: (req) => base.createRun(req).then(normalize.run),
    runs: () => base.runs().then(normalize.runs),
    run: (id) => base.run(id).then(normalize.run),
    orders: (runId, query) => base.orders(runId, query).then(normalize.orders),
    orderDetail: (runId, orderId) => base.orderDetail(runId, orderId).then(normalize.orderDetail),
    fills: (runId) => base.fills(runId).then(normalize.fills),
    equity: (runId) => base.equity(runId).then(normalize.equity),
    tape: (runId, query) => base.tape(runId, query).then(normalize.tape),
    compare: (a, b) => base.compare(a, b).then(normalize.compare),
    grid: (spec) => base.grid(spec).then(normalize.grid),
    timeline: (runId, query) => base.timeline(runId, query).then(normalize.timeline),
    demos: (datasetId) => base.demos(datasetId).then(normalize.demos),
  };
}

export const api: Api = withExactNs(STATIC ? staticApi : liveApi);

/** Download address of a run's research report (Markdown attachment). */
export function reportUrl(runId: string): string {
  return STATIC
    ? `${STATIC_ROOT}/${bundleRoot}/api/runs/${enc(runId)}/report.md`
    : `${BASE}/runs/${enc(runId)}/report`;
}

/**
 * Download address of a complete run (`RunResult` JSON attachment). The hosted viewer serves
 * the same JSON gzip-compressed (`.json.gz`).
 */
export function exportUrl(runId: string): string {
  return STATIC
    ? `${STATIC_ROOT}/${bundleRoot}/api/runs/${enc(runId)}/export.json.gz`
    : `${BASE}/runs/${enc(runId)}/export`;
}
