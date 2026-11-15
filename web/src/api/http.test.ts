import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';

import { clearApiKey, setApiKey } from './auth';
import { ApiError, collectAll, describeError, errorFromResponse, filenameFrom, pages, queryString, request, resolveBase, setUnauthorizedHandler } from './http';
import type { CursorPage } from './v1';

class MemStorage implements Storage {
  private m = new Map<string, string>();
  get length() {
    return this.m.size;
  }
  clear() {
    this.m.clear();
  }
  getItem(k: string) {
    return this.m.get(k) ?? null;
  }
  key(i: number) {
    return [...this.m.keys()][i] ?? null;
  }
  removeItem(k: string) {
    this.m.delete(k);
  }
  setItem(k: string, v: string) {
    this.m.set(k, String(v));
  }
}

const KEY = `bbk_${'A'.repeat(43)}`;

const json = (status: number, body: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json', ...headers } });

const envelope = (code: string, message: string, details: Record<string, unknown> | null = null) => ({
  error: { code, message, request_id: '01a1d737-5a1c-7b0e-9e7a-3f0c2d1e4b5a', details },
});

let fetchMock: Mock;

beforeEach(() => {
  vi.stubGlobal('sessionStorage', new MemStorage());
  vi.stubGlobal('localStorage', new MemStorage());
  fetchMock = vi.fn();
  vi.stubGlobal('fetch', fetchMock);
});

afterEach(() => {
  setUnauthorizedHandler(null);
  vi.unstubAllGlobals();
});

describe('base URL', () => {
  it('defaults to same-origin /api/v1 and trims a trailing slash', () => {
    expect(resolveBase(undefined)).toBe('/api/v1');
    expect(resolveBase('  ')).toBe('/api/v1');
    expect(resolveBase('https://blackbox-hft-api.onrender.com/api/v1/')).toBe('https://blackbox-hft-api.onrender.com/api/v1');
  });

  it('repeats array query keys and skips absent values', () => {
    expect(queryString({ state: ['queued', 'running'], mine: true, cursor: null, limit: undefined })).toBe('?state=queued&state=running&mine=true');
  });
});

describe('error envelope mapping', () => {
  it('keeps code, message, request id and details verbatim', () => {
    const e = errorFromResponse(
      json(422, envelope('budget_exceeded', 'this dataset spans 1800.000 s', { what: 'max_ticks', limit: 300000, estimate: 1800000, profile: 'hosted' })),
      JSON.stringify(envelope('budget_exceeded', 'this dataset spans 1800.000 s', { what: 'max_ticks', limit: 300000, estimate: 1800000, profile: 'hosted' })),
      'POST /api/v1/experiments',
    );
    expect(e.code).toBe('budget_exceeded');
    expect(e.status).toBe(422);
    expect(e.message).toBe('this dataset spans 1800.000 s');
    expect(e.requestId).toBe('01a1d737-5a1c-7b0e-9e7a-3f0c2d1e4b5a');
    const view = describeError(e);
    expect(view.state).toBe('budget_exceeded');
    expect(view.head).toBe('ERROR 422 budget_exceeded');
    expect(view.facts.join('\n')).toMatch(/max_ticks \u2264 300,000; estimate 1,800,000 \(profile hosted\)/);
  });

  it.each([
    [401, 'unauthorized', 'unauthorized'],
    [403, 'forbidden', 'forbidden'],
    [429, 'rate_limited', 'rate_limited'],
    [429, 'quota_exceeded', 'quota_exceeded'],
    [500, 'integrity_failed', 'integrity_failed'],
    [409, 'platform_mismatch', 'platform_mismatch'],
    [422, 'invalid_config', 'invalid'],
    [503, 'unavailable', 'unavailable'],
    [500, 'something_new', 'internal'],
  ])('HTTP %i %s maps to the %s state', (status, code, state) => {
    const body = JSON.stringify(envelope(code, 'm'));
    expect(describeError(errorFromResponse(json(status, JSON.parse(body)), body, 'GET x')).state).toBe(state);
  });

  it('reads Retry-After for rate limits', () => {
    const body = JSON.stringify(envelope('rate_limited', 'slow down', { retry_after_s: 7 }));
    const e = errorFromResponse(json(429, JSON.parse(body), { 'Retry-After': '7' }), body, 'GET x');
    expect(e.retryAfterS).toBe(7);
    expect(describeError(e).facts).toContain('retry after 7 s');
  });

  it('shows the byte limit and the hint of response_too_large', () => {
    const body = JSON.stringify(envelope('response_too_large', 'too big', { limit_bytes: 1048576, hint: 'request less' }));
    const view = describeError(errorFromResponse(json(422, JSON.parse(body)), body, 'GET x'));
    expect(view.state).toBe('response_too_large');
    expect(view.facts).toContain('limit: 1,048,576 bytes for this caller');
    expect(view.action).toBe('request less');
  });

  it('lists every invalid_config problem', () => {
    const body = JSON.stringify(envelope('invalid_config', 'invalid', { problems: ['a must be > 0', 'b too large'] }));
    expect(describeError(errorFromResponse(json(422, JSON.parse(body)), body, 'POST x')).facts).toEqual(
      expect.arrayContaining(['a must be > 0', 'b too large']),
    );
  });

  it('tells the user what a 404 means instead of offering a pointless retry', () => {
    const body = JSON.stringify(envelope('not_found', 'run not found', { resource: 'run' }));
    const view = describeError(errorFromResponse(json(404, JSON.parse(body)), body, 'GET /api/v1/runs/x'));
    expect(view.state).toBe('not_found');
    expect(view.facts).toContain('resource: run');
    expect(view.action).toMatch(/Check the id in the address/);
    expect(view.action).toMatch(/retrying will not help/);
  });
});

describe('cold start (WAKING) detection', () => {
  const html = '<!doctype html><html><body>Service waking up</body></html>';

  it('treats an HTML body on 502/503/504 as waking', () => {
    for (const status of [502, 503, 504]) {
      const e = errorFromResponse(new Response(html, { status, headers: { 'Content-Type': 'text/html' } }), html, 'GET /api/v1/status');
      expect(e.code).toBe('waking');
      expect(describeError(e).state).toBe('waking');
    }
  });

  it('treats a 200 loading page (not JSON) as waking', async () => {
    fetchMock.mockResolvedValue(new Response(html, { status: 200, headers: { 'Content-Type': 'text/html' } }));
    await expect(request('GET', '/status')).rejects.toMatchObject({ code: 'waking', status: 200 });
  });

  it('keeps the API\u2019s own JSON 503 as unavailable, not waking', () => {
    const body = JSON.stringify(envelope('unavailable', 'draining', { reason: 'draining' }));
    expect(errorFromResponse(json(503, JSON.parse(body)), body, 'GET x').code).toBe('unavailable');
  });

  it('reports a non-JSON 4xx as bad_response', () => {
    const e = errorFromResponse(new Response('nope', { status: 404, statusText: 'Not Found' }), 'nope', 'GET x');
    expect(e.code).toBe('bad_response');
  });
});

describe('request', () => {
  it('sends the session key as a Bearer header, never in the URL', async () => {
    setApiKey(KEY);
    fetchMock.mockResolvedValue(json(200, { ok: true }));
    await request('GET', '/whoami', { query: { a: 1 } });
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/whoami?a=1');
    expect(url).not.toContain(KEY);
    expect((init.headers as Record<string, string>).Authorization).toBe(`Bearer ${KEY}`);
    expect(init.credentials).toBe('omit');
  });

  it('cancels through AbortController and reports `aborted`', async () => {
    fetchMock.mockImplementation((_url: string, init: RequestInit) =>
      new Promise((_resolve, reject) => {
        init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
      }),
    );
    const ctl = new AbortController();
    const p = request('GET', '/runs', { signal: ctl.signal });
    ctl.abort();
    await expect(p).rejects.toMatchObject({ code: 'aborted' });
  });

  it('clears the stored key and notifies on a 401, but keeps it on a 403', async () => {
    const onUnauthorized = vi.fn();
    setUnauthorizedHandler(onUnauthorized);
    setApiKey(KEY);
    fetchMock.mockResolvedValueOnce(json(403, envelope('forbidden', 'needs run', { required_scope: 'run' })));
    await expect(request('POST', '/experiments', { body: {} })).rejects.toMatchObject({ code: 'forbidden', status: 403 });
    expect(sessionStorage.getItem('bb.apiKey')).toBe(KEY);
    fetchMock.mockResolvedValueOnce(json(401, envelope('unauthorized', 'unknown key')));
    await expect(request('GET', '/runs')).rejects.toMatchObject({ code: 'unauthorized' });
    expect(sessionStorage.getItem('bb.apiKey')).toBeNull();
    expect(onUnauthorized).toHaveBeenCalledOnce();
  });

  it('a candidate key is sent without being stored, and a 401 for it clears nothing', async () => {
    fetchMock.mockResolvedValue(json(401, envelope('unauthorized', 'unknown key')));
    await expect(request('GET', '/whoami', { key: KEY })).rejects.toBeInstanceOf(ApiError);
    const init = fetchMock.mock.calls[0][1] as RequestInit;
    expect((init.headers as Record<string, string>).Authorization).toBe(`Bearer ${KEY}`);
    expect(sessionStorage.getItem('bb.apiKey')).toBeNull();
  });

  it('reads the attachment filename', () => {
    expect(filenameFrom('attachment; filename="run-136d41410c6b23f1-linux-x86_64.bbz"')).toBe('run-136d41410c6b23f1-linux-x86_64.bbz');
    expect(filenameFrom(null)).toBeNull();
  });
});

describe('API key storage', () => {
  it('is kept in sessionStorage only, never localStorage', () => {
    const setLocal = vi.spyOn(localStorage, 'setItem');
    setApiKey(KEY);
    expect(sessionStorage.getItem('bb.apiKey')).toBe(KEY);
    expect(setLocal).not.toHaveBeenCalled();
    expect(localStorage.length).toBe(0);
    clearApiKey();
    expect(sessionStorage.getItem('bb.apiKey')).toBeNull();
  });

  it('refuses a malformed key', () => {
    expect(() => setApiKey('bbk_short')).toThrow();
    expect(sessionStorage.length).toBe(0);
  });
});

describe('cursor pagination', () => {
  const source = (n: number, per: number) => async (cursor: string | null): Promise<CursorPage<number>> => {
    const start = cursor === null ? 0 : Number(cursor);
    const items = Array.from({ length: Math.min(per, n - start) }, (_, i) => start + i);
    const next = start + per < n ? String(start + per) : null;
    return { items, next_cursor: next, truncated_by: next === null ? null : 'limit' };
  };

  it('follows next_cursor until null', async () => {
    const all = await collectAll(source(7, 3));
    expect(all).toEqual({ items: [0, 1, 2, 3, 4, 5, 6], incomplete: false, pages: 3 });
  });

  it('stops at maxItems and says the list is incomplete', async () => {
    const some = await collectAll(source(10, 3), { maxItems: 5 });
    expect(some.items).toEqual([0, 1, 2, 3, 4]);
    expect(some.incomplete).toBe(true);
  });

  it('is complete when maxItems lands exactly on the last page', async () => {
    expect((await collectAll(source(6, 3), { maxItems: 6 })).incomplete).toBe(false);
  });

  it('continues after a byte-capped page', async () => {
    let calls = 0;
    const fetchPage = async (cursor: string | null): Promise<CursorPage<string>> => {
      calls += 1;
      return cursor === null ? { items: ['a'], next_cursor: 'c1', truncated_by: 'bytes' } : { items: ['b'], next_cursor: null, truncated_by: null };
    };
    expect((await collectAll(fetchPage)).items).toEqual(['a', 'b']);
    expect(calls).toBe(2);
  });

  it('refuses a repeated cursor instead of looping forever', async () => {
    const loop = async (): Promise<CursorPage<number>> => ({ items: [1], next_cursor: 'same', truncated_by: 'limit' });
    const it = pages(loop);
    await it.next();
    await it.next();
    await expect(it.next()).rejects.toMatchObject({ code: 'bad_response' });
  });
});
