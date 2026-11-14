// Transport for `/api/v1` (API_CONTRACT.md §1-§7): base URL, the `Authorization` header from the
// session key, request cancellation, the error envelope, and cold-start (WAKING) detection.
// Nothing is retried or cached here; callers decide.

import { clearApiKey, getApiKey } from './auth';
import type { CursorPage, ErrorEnvelope } from './v1';

/**
 * Base of every v1 request. `VITE_API_BASE_URL` (the hosted API, e.g.
 * `https://blackbox-hft-api.onrender.com/api/v1`) is inlined at build time; unset means the
 * same-origin `/api/v1` of a local `blackbox serve`.
 */
export const API_BASE = resolveBase(import.meta.env.VITE_API_BASE_URL);

/** The server root, for the unversioned health endpoints (`/healthz`, `/readyz`). */
export const API_ROOT = API_BASE.replace(/\/api\/v1$/, '');

export function resolveBase(configured: string | undefined): string {
  const v = (configured ?? '').trim();
  return (v === '' ? '/api/v1' : v).replace(/\/+$/, '');
}

/** Whether the API lives on another origin (a hosted API on a host that sleeps when idle). */
export function isCrossOrigin(base: string = API_BASE): boolean {
  if (!/^https?:\/\//i.test(base)) return false;
  try {
    return typeof window === 'undefined' || new URL(base).origin !== window.location.origin;
  } catch {
    return false;
  }
}

/**
 * Every failure the UI shows. Server errors carry the envelope's `code`, `message`,
 * `request_id` and `details` verbatim (§2). Client-side codes: `network` (no response),
 * `waking` (the host's loading page or a gateway error while the free instance spins up),
 * `bad_response` (an answer that is not the documented JSON), `aborted` (superseded request) and
 * `unsafe_integer` (a ns value that cannot be represented exactly; see normalize.ts).
 */
export class ApiError extends Error {
  code: string;
  status: number;
  requestId: string | null;
  details: Record<string, unknown> | null;
  /** `Retry-After` in seconds, when the server sent one. */
  retryAfterS: number | null;

  constructor(
    code: string,
    message: string,
    status: number,
    extra: { requestId?: string | null; details?: Record<string, unknown> | null; retryAfterS?: number | null } = {},
  ) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
    this.status = status;
    this.requestId = extra.requestId ?? null;
    this.details = extra.details ?? null;
    this.retryAfterS = extra.retryAfterS ?? null;
  }
}

export function isAbort(e: unknown): boolean {
  return (e instanceof ApiError && e.code === 'aborted') || (e instanceof DOMException && e.name === 'AbortError');
}

function isEnvelope(v: unknown): v is ErrorEnvelope {
  if (typeof v !== 'object' || v === null || !('error' in v)) return false;
  const e = v.error;
  return typeof e === 'object' && e !== null && 'code' in e && 'message' in e && typeof e.code === 'string' && typeof e.message === 'string';
}

const GATEWAY = new Set([502, 503, 504]);

function retryAfter(res: Response): number | null {
  const h = res.headers.get('Retry-After');
  if (h === null) return null;
  const n = Number(h);
  return Number.isFinite(n) && n >= 0 ? n : null;
}

function looksLikeJson(res: Response, text: string): boolean {
  const type = res.headers.get('Content-Type') ?? '';
  if (type.includes('json')) return true;
  const t = text.trimStart();
  return t.startsWith('{') || t.startsWith('[');
}

/**
 * The error for a non-2xx (or undecodable 2xx) response. A body that is not JSON while the
 * status is 2xx or 502/503/504 is the free host's loading page or gateway: `waking`
 * (UI_DESIGN_SPEC §10.1). A JSON 503 is the API's own `unavailable` and is kept as such.
 */
export function errorFromResponse(res: Response, text: string, what: string): ApiError {
  const requestId = res.headers.get('X-Request-Id');
  const retryAfterS = retryAfter(res);
  let parsed: unknown = null;
  if (looksLikeJson(res, text)) {
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = null;
    }
  }
  if (isEnvelope(parsed)) {
    const e = parsed.error;
    return new ApiError(e.code, e.message, res.status, {
      requestId: e.request_id ?? requestId,
      details: e.details ?? null,
      retryAfterS,
    });
  }
  if (res.ok || GATEWAY.has(res.status)) {
    return new ApiError(
      'waking',
      `${what} answered HTTP ${res.status} with ${text.trim() === '' ? 'no body' : 'a page that is not the API'}: the API host is waking up (a free instance can take about a minute to start).`,
      res.status,
      { requestId, retryAfterS },
    );
  }
  const snippet = text.trim().slice(0, 200);
  return new ApiError(
    'bad_response',
    `${what} failed with HTTP ${res.status} ${res.statusText}${snippet ? `: ${snippet}` : ' and no error body.'}`,
    res.status,
    { requestId, retryAfterS },
  );
}

type QueryValue = string | number | boolean | null | undefined | readonly (string | number)[];

/** `?a=1&state=queued&state=running`; arrays repeat the key, absent values are skipped. */
export function queryString(query: object | undefined): string {
  if (!query) return '';
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query) as [string, QueryValue][]) {
    if (value === null || value === undefined) continue;
    if (Array.isArray(value)) for (const v of value) params.append(key, String(v));
    else params.append(key, String(value));
  }
  const s = params.toString();
  return s ? `?${s}` : '';
}

export type Method = 'GET' | 'POST' | 'PATCH' | 'DELETE';

export interface RequestOptions {
  query?: object;
  body?: unknown;
  signal?: AbortSignal;
  ifMatch?: string;
  /** Use this key instead of the session key (validating a key before it is stored). */
  key?: string | null;
  /** `path` is under the server root (`/readyz`), not under `/api/v1`. */
  root?: boolean;
}

export interface Raw {
  res: Response;
  url: string;
}

/** Called after a 401 cleared the session key (the shell opens the key dialog). */
let onUnauthorized: (() => void) | null = null;
export function setUnauthorizedHandler(fn: (() => void) | null): void {
  onUnauthorized = fn;
}

async function send(method: Method, path: string, opts: RequestOptions, accept: string): Promise<Raw> {
  const base = opts.root ? API_ROOT : API_BASE;
  const url = `${base}${path}${queryString(opts.query)}`;
  const headers: Record<string, string> = { Accept: accept };
  const key = opts.key !== undefined ? opts.key : getApiKey();
  if (key) headers.Authorization = `Bearer ${key}`;
  if (opts.ifMatch) headers['If-Match'] = opts.ifMatch;
  const init: RequestInit = { method, headers, signal: opts.signal, credentials: 'omit' };
  if (opts.body !== undefined) {
    headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(opts.body);
  }
  const what = `${method} ${base}${path}`;
  let res: Response;
  try {
    res = await fetch(url, init);
  } catch (cause) {
    if (opts.signal?.aborted) throw new ApiError('aborted', `${what} was cancelled.`, 0);
    const why = cause instanceof Error ? cause.message : String(cause);
    throw new ApiError(
      'network',
      `Could not reach the BlackBox API for ${what} (${why}).${isCrossOrigin() ? ' A sleeping free host answers without CORS headers while it wakes.' : ' Start it with "blackbox serve".'}`,
      0,
    );
  }
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    const err = errorFromResponse(res, text, what);
    // A sent key that the server refuses is cleared (UI_DESIGN_SPEC §10.2); never for an explicit
    // candidate key, which is not stored yet.
    if (res.status === 401 && key && opts.key === undefined) {
      clearApiKey();
      onUnauthorized?.();
    }
    throw err;
  }
  return { res, url: what };
}

/** JSON request. `204` resolves to `undefined`. */
export async function request<T>(method: Method, path: string, opts: RequestOptions = {}): Promise<T> {
  const { res, url } = await send(method, path, opts, 'application/json');
  if (res.status === 204) return undefined as T;
  const text = await readText(res, opts.signal, url);
  if (!looksLikeJson(res, text)) throw errorFromResponse(res, text, url);
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new ApiError('bad_response', `${url} returned HTTP ${res.status} with a body that is not JSON.`, res.status);
  }
}

async function readText(res: Response, signal: AbortSignal | undefined, url: string): Promise<string> {
  try {
    return await res.text();
  } catch (e) {
    if (signal?.aborted) throw new ApiError('aborted', `${url} was cancelled.`, 0);
    throw e;
  }
}

/** `text/markdown` (reports). */
export async function requestText(path: string, opts: RequestOptions = {}): Promise<string> {
  const { res, url } = await send('GET', path, opts, 'text/markdown, text/plain');
  return readText(res, opts.signal, url);
}

export interface Download {
  blob: Blob;
  filename: string;
}

/** A binary or text attachment, with the filename from `Content-Disposition`. */
export async function requestDownload(path: string, fallbackName: string, opts: RequestOptions = {}): Promise<Download> {
  const { res } = await send('GET', path, opts, '*/*');
  const blob = await res.blob();
  return { blob, filename: filenameFrom(res.headers.get('Content-Disposition')) ?? fallbackName };
}

export function filenameFrom(disposition: string | null): string | null {
  if (!disposition) return null;
  const m = /filename\*?=(?:UTF-8'')?"?([^";]+)"?/i.exec(disposition);
  return m ? decodeURIComponent(m[1]) : null;
}

// ----- cursor pagination (§7) ------------------------------------------------------------------

export type PageFetcher<T> = (cursor: string | null, signal?: AbortSignal) => Promise<CursorPage<T>>;

/** Every page in order, following `next_cursor` until it is `null`. */
export async function* pages<T>(fetchPage: PageFetcher<T>, signal?: AbortSignal): AsyncGenerator<CursorPage<T>> {
  let cursor: string | null = null;
  const seen = new Set<string>();
  for (;;) {
    const page = await fetchPage(cursor, signal);
    yield page;
    if (page.next_cursor === null) return;
    if (seen.has(page.next_cursor)) {
      throw new ApiError('bad_response', `The server repeated cursor ${page.next_cursor}; paging stopped.`, 0);
    }
    seen.add(page.next_cursor);
    cursor = page.next_cursor;
  }
}

export interface Collected<T> {
  items: T[];
  /** More items exist beyond `maxItems`; the UI must say the list is incomplete. */
  incomplete: boolean;
  pages: number;
}

/**
 * Items of every page up to `maxItems`. Stops early (`incomplete: true`) rather than loading an
 * unbounded list; a byte-capped page (`truncated_by: "bytes"`) simply continues at its cursor.
 */
export async function collectAll<T>(fetchPage: PageFetcher<T>, opts: { maxItems?: number; signal?: AbortSignal } = {}): Promise<Collected<T>> {
  const max = opts.maxItems ?? Number.POSITIVE_INFINITY;
  const items: T[] = [];
  let n = 0;
  for await (const page of pages(fetchPage, opts.signal)) {
    n += 1;
    for (const item of page.items) {
      if (items.length >= max) return { items, incomplete: true, pages: n };
      items.push(item);
    }
    if (items.length >= max && page.next_cursor !== null) return { items, incomplete: true, pages: n };
  }
  return { items, incomplete: false, pages: n };
}

// ----- UI error states -------------------------------------------------------------------------

export type ErrorState =
  | 'waking'
  | 'network'
  | 'unauthorized'
  | 'forbidden'
  | 'not_found'
  | 'conflict'
  | 'invalid'
  | 'budget_exceeded'
  | 'rate_limited'
  | 'response_too_large'
  | 'quota_exceeded'
  | 'integrity_failed'
  | 'platform_mismatch'
  | 'unavailable'
  | 'internal'
  | 'bad_response'
  | 'unsafe_integer'
  | 'aborted'
  | 'unexpected';

export interface ErrorView {
  state: ErrorState;
  /** `ERROR 422 budget_exceeded`: status and code verbatim. */
  head: string;
  /** The server's message verbatim. */
  message: string;
  /** Facts from `details` and headers, worded but not interpreted. */
  facts: string[];
  /** What the user can do. */
  action: string | null;
}

const STATE_OF: Record<string, ErrorState> = {
  waking: 'waking',
  network: 'network',
  unauthorized: 'unauthorized',
  forbidden: 'forbidden',
  not_found: 'not_found',
  conflict: 'conflict',
  invalid_request: 'invalid',
  invalid_config: 'invalid',
  budget_exceeded: 'budget_exceeded',
  rate_limited: 'rate_limited',
  response_too_large: 'response_too_large',
  quota_exceeded: 'quota_exceeded',
  integrity_failed: 'integrity_failed',
  platform_mismatch: 'platform_mismatch',
  unavailable: 'unavailable',
  internal: 'internal',
  bad_response: 'bad_response',
  unsafe_integer: 'unsafe_integer',
  aborted: 'aborted',
};

const num = (v: unknown) => (typeof v === 'number' ? v.toLocaleString('en-US') : String(v));

/** Maps any failure to what the error box shows (UI_DESIGN_SPEC §6.9, §10). */
export function describeError(error: unknown): ErrorView {
  if (!(error instanceof ApiError)) {
    const message = error instanceof Error ? error.message : String(error);
    return { state: 'unexpected', head: 'ERROR unexpected failure', message, facts: [], action: null };
  }
  const d = error.details ?? {};
  const state = STATE_OF[error.code] ?? (error.status >= 500 ? 'internal' : 'unexpected');
  const facts: string[] = [];
  let action: string | null = null;
  switch (state) {
    case 'waking':
      action = 'Wait; the first response from a sleeping free host can take about a minute.';
      break;
    case 'unauthorized':
      action = 'The key was refused and removed from this tab. Enter a valid API key to continue.';
      break;
    case 'forbidden':
      if (d.required_scope) facts.push(`required scope: ${String(d.required_scope)}`);
      action = 'The key is valid but lacks the scope for this action. It was kept.';
      break;
    case 'not_found':
      if (d.resource) facts.push(`resource: ${String(d.resource)}`);
      break;
    case 'conflict':
      if (d.reason) facts.push(`reason: ${String(d.reason)}`);
      if (d.current_version !== undefined) facts.push(`current version: ${num(d.current_version)}`);
      break;
    case 'invalid':
      if (d.field) facts.push(`field: ${String(d.field)}`);
      if (Array.isArray(d.problems)) for (const p of d.problems) facts.push(String(p));
      break;
    case 'budget_exceeded':
      if (d.what !== undefined) facts.push(`limit: ${String(d.what)} \u2264 ${num(d.limit)}; estimate ${num(d.estimate)} (profile ${String(d.profile)})`);
      action = 'Shorten the dataset span, raise the strategy interval, or run it on a local server.';
      break;
    case 'rate_limited': {
      const s = typeof d.retry_after_s === 'number' ? d.retry_after_s : error.retryAfterS;
      if (s !== null && s !== undefined) facts.push(`retry after ${num(s)} s`);
      action = 'Too many requests; wait before retrying.';
      break;
    }
    case 'quota_exceeded':
      if (d.scope !== undefined) facts.push(`quota ${String(d.scope)}: used ${num(d.used)} of ${num(d.limit)}`);
      if (typeof d.resets_at_ns === 'string') facts.push(`resets at ns ${d.resets_at_ns}`);
      break;
    case 'response_too_large':
      if (typeof d.limit_bytes === 'number') facts.push(`limit: ${num(d.limit_bytes)} bytes for this caller`);
      action = typeof d.hint === 'string' ? d.hint : null;
      break;
    case 'integrity_failed':
      if (d.run_id) facts.push(`run ${String(d.run_id)}, ${String(d.object)} expected sha256 ${String(d.expected_sha256)}`);
      action = 'The stored artefact did not match its recorded hash; the run is quarantined and is not shown.';
      break;
    case 'platform_mismatch':
      action = 'The stored result was computed on another platform class; it is not returned as this platform\u2019s result.';
      break;
    case 'unavailable':
      if (d.reason) facts.push(`reason: ${String(d.reason)}`);
      if (error.retryAfterS !== null) facts.push(`retry after ${num(error.retryAfterS)} s`);
      break;
    default:
      break;
  }
  if (error.requestId) facts.push(`request id ${error.requestId}`);
  const head = `ERROR ${error.status > 0 ? `${error.status} ` : ''}${error.code}`;
  return { state, head, message: error.message, facts, action };
}
