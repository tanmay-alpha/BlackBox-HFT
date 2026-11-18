// Network accounting from the Chrome DevTools Protocol `Network` domain of one page.
// Encoded bytes are `Network.loadingFinished.encodedDataLength` (bytes on the wire including
// response headers); decoded bytes are the sum of `Network.dataReceived.dataLength` (body after
// content decoding); a request's duration runs from `requestWillBeSent` to `loadingFinished` on
// the protocol's monotonic clock, so it includes the emulated latency and throughput limits.

export class NetLog {
  constructor(cdp, apiBase) {
    this.apiBase = apiBase;
    this.requests = [];
    this.byId = new Map();
    this.inflight = 0;
    this.lastEventAt = Date.now();

    cdp.on('Network.requestWillBeSent', (e) => {
      this.lastEventAt = Date.now();
      const prev = this.byId.get(e.requestId);
      if (prev && e.redirectResponse) {
        // A redirect reuses the request id: close the first hop and start the next one.
        prev.status = e.redirectResponse.status;
        prev.encoded = e.redirectResponse.encodedDataLength ?? 0;
        prev.end = e.timestamp;
        prev.redirected = true;
        this.inflight -= 1;
      }
      const r = {
        url: e.request.url,
        method: e.request.method,
        type: e.type ?? 'Other',
        start: e.timestamp,
        responseAt: null,
        end: null,
        status: null,
        encoded: 0,
        decoded: 0,
        fromCache: false,
        failed: null,
      };
      this.byId.set(e.requestId, r);
      this.requests.push(r);
      this.inflight += 1;
    });
    cdp.on('Network.requestServedFromCache', (e) => {
      const r = this.byId.get(e.requestId);
      if (r) r.fromCache = true;
    });
    cdp.on('Network.responseReceived', (e) => {
      this.lastEventAt = Date.now();
      const r = this.byId.get(e.requestId);
      if (!r) return;
      r.status = e.response.status;
      r.type = e.type ?? r.type;
      r.responseAt = e.timestamp;
      if (e.response.fromDiskCache || e.response.fromPrefetchCache) r.fromCache = true;
    });
    cdp.on('Network.dataReceived', (e) => {
      this.lastEventAt = Date.now();
      const r = this.byId.get(e.requestId);
      if (r) r.decoded += e.dataLength;
    });
    const finish = (failed) => (e) => {
      this.lastEventAt = Date.now();
      const r = this.byId.get(e.requestId);
      if (!r || r.end !== null) return;
      r.end = e.timestamp;
      if (failed) r.failed = e.canceled ? 'canceled' : e.errorText || 'failed';
      else r.encoded = e.encodedDataLength;
      this.inflight -= 1;
    };
    cdp.on('Network.loadingFinished', finish(false));
    cdp.on('Network.loadingFailed', finish(true));
  }

  /** Milliseconds since the last network event, or 0 while a request is in flight. */
  quietFor() {
    return this.inflight > 0 ? 0 : Date.now() - this.lastEventAt;
  }

  /** Position for `summary(from)`: requests started after this call. */
  mark() {
    return this.requests.length;
  }

  summary(from = 0) {
    const list = this.requests.slice(from).filter((r) => !r.url.startsWith('data:') && !r.url.startsWith('blob:'));
    const byType = {};
    let total = 0;
    for (const r of list) {
      const t = (byType[r.type] ??= { count: 0, encoded_bytes: 0, decoded_bytes: 0 });
      t.count += 1;
      t.encoded_bytes += r.encoded;
      t.decoded_bytes += r.decoded;
      total += r.encoded;
    }
    const ms = (r) => (r.end === null ? null : Math.round((r.end - r.start) * 1000 * 10) / 10);
    // Offsets from the first request of the page (the document), for a waterfall.
    const origin = this.requests.length ? this.requests[0].start : 0;
    const at = (t) => (t === null ? null : Math.round((t - origin) * 1000));
    const view = (r) => ({
      method: r.method,
      url: r.url.startsWith(this.apiBase) ? r.url.slice(this.apiBase.length) : r.url,
      type: r.type,
      status: r.status,
      duration_ms: ms(r),
      start_ms: at(r.start),
      end_ms: at(r.end),
      ttfb_ms: r.responseAt === null ? null : Math.round((r.responseAt - r.start) * 1000 * 10) / 10,
      encoded_bytes: r.encoded,
      decoded_bytes: r.decoded,
      from_cache: r.fromCache,
      failed: r.failed,
      pending: r.end === null,
    });
    const isApi = (r) => r.url.startsWith(this.apiBase);
    const api = list.filter((r) => isApi(r) && r.type !== 'Preflight');
    const preflights = list.filter((r) => isApi(r) && r.type === 'Preflight');
    return {
      request_count: list.length,
      total_encoded_bytes: total,
      js_encoded_bytes: byType.Script?.encoded_bytes ?? 0,
      from_cache_count: list.filter((r) => r.fromCache).length,
      failed_count: list.filter((r) => r.failed !== null).length,
      pending_count: list.filter((r) => r.end === null).length,
      by_type: byType,
      api: {
        count: api.length,
        // HTTP status >= 400 or a failed load (e.g. 503 when the server's egress budget is spent).
        error_count: api.filter((r) => r.failed !== null || (r.status !== null && r.status >= 400)).length,
        preflight_count: preflights.length,
        encoded_bytes: api.reduce((a, r) => a + r.encoded, 0),
        decoded_bytes: api.reduce((a, r) => a + r.decoded, 0),
        preflight_encoded_bytes: preflights.reduce((a, r) => a + r.encoded, 0),
        durations_ms: api.map(ms),
        requests: [...api, ...preflights].map(view),
      },
      static: list.filter((r) => !isApi(r)).map(view),
    };
  }
}
