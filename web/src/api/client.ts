// Typed client for the BlackBox HTTP API (docs/API.md). Every method returns exactly what the
// server sent; nothing is synthesized, cached or retried here.

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

/**
 * A failed API call. `code` is the server's snake_case error code (`not_found`,
 * `invalid_request`, `invalid_config`, `simulation_failed`, `internal`), or one of the
 * client-side codes `network` (no response; `status` is 0) and `bad_response` (the server
 * answered with something that is not the documented JSON).
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

export const api = {
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
    if (datasetId === undefined) return presets;
    return presets.map((p) => ({ ...p, config: { ...p.config, dataset_id: datasetId } }));
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

export type Api = typeof api;

/** Download address of a run's research report (Markdown attachment). */
export function reportUrl(runId: string): string {
  return `${BASE}/runs/${enc(runId)}/report`;
}

/** Download address of a complete run (`RunResult` JSON attachment). */
export function exportUrl(runId: string): string {
  return `${BASE}/runs/${enc(runId)}/export`;
}
