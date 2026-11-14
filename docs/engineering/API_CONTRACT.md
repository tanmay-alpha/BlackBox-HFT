# API contract: `/api/v1`

Status: Phase 0 design, 2026-11-14. This is the contract the Phase 2 server and the new web client build against. It
replaces `docs/API.md` at the cutover described in [TARGET_ARCHITECTURE.md](TARGET_ARCHITECTURE.md) §11. Storage
semantics are in [DATABASE_DESIGN.md](DATABASE_DESIGN.md). UI requirements come from
[UI_DESIGN_SPEC.md](UI_DESIGN_SPEC.md) §10 and §16 (traceability in §12 below).

The OpenAPI 3.1 document generated from the code (§11) is normative once it exists. Where this document and the
generated spec disagree, the spec wins, and this document is corrected in the same pull request.

---

## 1. Conventions

| Topic | Rule |
|---|---|
| Base URL | `https://<render-service>.onrender.com/api/v1` (hosted); `http://127.0.0.1:8787/api/v1` (local). Health endpoints are unversioned: `/healthz`, `/readyz`, `/metrics` |
| Versioning | The major version is in the path. Within v1, changes are additive only: new endpoints, new optional request fields, new response fields. Clients must ignore unknown response fields. Requests reject unknown fields (`invalid_request`), as `bb-core` does today (`#[serde(deny_unknown_fields)]`) |
| Media types | Requests `application/json` (UTF-8, ≤ 1 MiB). Responses `application/json`, except `text/markdown` (reports) and `application/gzip` (bundle export). Responses are gzip-compressed when the client sends `Accept-Encoding: gzip` |
| **Nanoseconds** | Every ns quantity is a **JSON string** holding a decimal i64 (`"1700000000000000000"`, `"-5000"`). This covers relative timestamps, absolute epoch ns, durations, latencies, every `*_ns` field and the wall-clock instants `*_at_ns`. Query parameters take the same decimal form. Field names end in `_ns` |
| Other big integers | `seed` (u64) is a decimal string. All other integers (counts, `order_id`, `seq`, `tick_no`, sizes in bytes, versions) are JSON numbers and guaranteed ≤ 2^53 − 1 |
| Floats | JSON numbers (prices are integer ticks; quantities, PnL and fees are f64) |
| Hashes | lowercase hex: sha256 = 64 chars. `run_id` = first 16 hex chars of the experiment fingerprint (today's run id). Wherever a `run_id` is accepted, the full 64-hex fingerprint is accepted too |
| Ids | Jobs, experiments, sweeps, comparisons, reports: UUIDv7 strings. Datasets: `[a-z0-9_-]{1,64}`. Workspaces: slug |
| Enums | `snake_case` strings; tagged unions carry `"type"` (unchanged from bb-core) |
| Nulls | an absent optional value is `null`, never omitted (stable shapes for generated types) |
| Request ids | Clients may send `X-Request-Id` (`^[A-Za-z0-9-]{8,64}$`). Every response echoes the accepted or generated id in `X-Request-Id` |

## 2. Errors

Every non-2xx response has this body (and `Cache-Control: no-store`):

```json
{
  "error": {
    "code": "budget_exceeded",
    "message": "this dataset spans 1800.000 s, so an interval of 1000000 ns means 1800000 strategy evaluations; the hosted limit is 300000",
    "request_id": "01a1d737-5a1c-7b0e-9e7a-3f0c2d1e4b5a",
    "details": { "what": "max_ticks", "limit": 300000, "estimate": 1800000, "profile": "hosted" }
  }
}
```

`code` is stable and machine-readable. `message` is for humans and may change. The UI shows both verbatim.

| HTTP | `code` | When | `details` |
|---|---|---|---|
| 400 | `invalid_request` | malformed JSON, unknown field, bad path/query parameter, missing `If-Match` where required, body > 1 MiB (413 status, same code) | `{ "field"?: string }` |
| 401 | `unauthorized` | `Authorization` present but malformed, unknown, revoked or expired; or a mutation without any key. Includes `WWW-Authenticate: Bearer` (`error="invalid_token"` when a key was sent) | – |
| 403 | `forbidden` | valid key lacks the scope; write to the demo workspace without `admin`; request from a disallowed `Origin`/`Host` | `{ "required_scope"?: "run" \| "admin" }` |
| 404 | `not_found` | unknown or invisible resource (another workspace's experiment is 404, not 403, so its existence is not disclosed) | `{ "resource": string }` |
| 409 | `conflict` | `If-Match` version mismatch; cancel of a terminal job; 16-hex id ambiguity/collision; alignment requested on a `metrics_only` comparison | `{ "reason": "version_mismatch" \| "job_terminal" \| "identity_collision" \| "alignment_unavailable", "current_version"?: number }` |
| 422 | `invalid_config` | `RunConfig`/`GridSpec` validation failed (all problems listed) | `{ "problems": string[] }` |
| 422 | `budget_exceeded` | admission: the work estimate exceeds `ExecutionLimits` of this deployment | `{ "what": string, "limit": number, "estimate": number, "profile": "hosted" \| "local" }` |
| 429 | `rate_limited` | request-rate limit (§5) | `{ "retry_after_s": number }` + `Retry-After` header |
| 429 | `quota_exceeded` | daily submission quota, active-job quota, or the monthly storage budget | `{ "scope": "experiments_per_day" \| "sweeps_per_day" \| "active_jobs" \| "storage", "limit": number, "used": number, "resets_at_ns": string \| null }` |
| 500 | `integrity_failed` | a stored artefact or summary does not match its recorded hash; the run is quarantined (TARGET_ARCHITECTURE §7) | `{ "run_id": string, "object": "summary" \| "bundle" \| "page", "expected_sha256": string }` |
| 500 | `internal` | anything else; generic message, details only in logs under `request_id` | – |
| 503 | `unavailable` | draining (SIGTERM), database unreachable, Blob blocked or unreachable, heavy-read concurrency saturated | `{ "reason": "draining" \| "database" \| "storage" \| "busy" }` + `Retry-After` |

Job failures are not HTTP errors. They appear in the job resource as `error {category, code, message}`, where
`category ∈ {invalid_config, budget_exceeded, simulation_failed, lease_expired, integrity_failed, storage_failed,
internal}`.

## 3. Authentication and scopes

- Header: **`Authorization: Bearer bbk_<43 base64url chars>`** (32 random bytes; regex `^bbk_[A-Za-z0-9_-]{43}$`).
  Keys are never accepted in URLs or cookies. The server stores only `sha256(key)` and looks keys up by digest. Logs
  show `display_prefix` (`bbk_` + 4 characters) only.
- Keys are minted by the CLI (`blackbox keys mint --workspace <slug> --scopes read,run --label …`), which prints the
  key once. They are revoked by the CLI or `POST /api/v1/system/api-keys/{id}/revoke`.
- Scopes, cumulative: **`read`** (read own workspace + demo) ⊂ **`run`** (create experiments, sweeps, comparisons,
  saved reports; cancel; rename; delete own) ⊂ **`admin`** (`/system/*`, `/metrics`, act on the demo workspace via
  `?workspace=demo`).
- **No header** = anonymous = the **public demo workspace, read-only**. Anonymous `GET`s on demo resources succeed.
  Anonymous mutations get 401 `unauthorized`, and the UI opens the key dialog.
- **Header present but invalid** → 401 on every endpoint, reads included. There is never a silent fallback to demo,
  so the UI can tell "wrong key" from "no key" (UI_DESIGN_SPEC §10.2 clears the key on 401).
- **Valid key, missing scope** → 403 `forbidden` (the UI keeps the key).
- Workspace resolution: listings default to the key's workspace, or demo for anonymous callers. `?workspace=demo`
  lets any caller list demo content. Single-resource reads succeed if the resource is visible in the caller's
  workspace or in demo; otherwise 404.

**Demo workspace semantics:** slug `demo`, kind `demo`, public read. Its content (synthetic datasets, curated
experiments, comparisons, a sweep, saved reports) is created by `blackbox demo seed`, which runs against the hosted
API with an `admin` key. All its runs are therefore computed by the hosted Linux worker and are canonical. Demo
experiments are `pinned` (exempt from retention). Demo quotas are 0, so nothing can be submitted into it without
`admin`.

## 4. CORS and Host checks

- `Access-Control-Allow-Origin`: exact match against **`BB_CORS_ALLOWED_ORIGINS`** (comma-separated, e.g.
  `https://blackbox-hft.vercel.app`). No wildcard and no suffix patterns. Vercel preview URLs are not allowed unless
  listed (DEPLOYMENT.md §6).
- `Access-Control-Allow-Methods: GET, POST, PATCH, DELETE`. `Access-Control-Allow-Headers: Authorization,
  Content-Type, If-Match, If-None-Match, X-Request-Id`. `Access-Control-Expose-Headers: ETag, X-Request-Id,
  Retry-After, X-RateLimit-Limit, X-RateLimit-Remaining`. `Access-Control-Max-Age: 600`. No `Allow-Credentials`: the
  key is a header, not a cookie.
- A preflight from an unlisted origin gets no CORS headers (the browser blocks it). A non-preflight request carrying an
  unlisted `Origin` is refused with 403 `forbidden` (AT-11). Requests without `Origin` (curl, CLI) are allowed.
- `Host` must be in `RENDER_EXTERNAL_HOSTNAME` ∪ `BB_ALLOWED_HOSTS` (custom domains). Render's health checks use those
  hosts (Render docs, health checks).

## 5. Rate limits and quotas

| Limit | Default (hosted) | Mechanism | Response |
|---|---|---|---|
| Anonymous request rate | 120/min per client IP, burst 30 | `tower_governor`, IP from `X-Forwarded-For` (best effort; OSS evaluation §3.5) | 429 `rate_limited` |
| Keyed request rate | 600/min per key, burst 100 | in-memory GCRA keyed by key id | 429 `rate_limited` |
| Heavy read concurrency (artefact routes, replay, compare, report, export) | 4 in flight, 2 s queue | `ConcurrencyLimitLayer` + `LoadShedLayer` | 503 `unavailable` (`busy`) |
| Request timeout | 30 s (reads), 10 s (submissions), `BB_COMPARISON_TIMEOUT_S` = 60 s (`POST /comparisons`, §8.6) | `TimeoutLayer` | 503 `unavailable` |
| Experiments per key per day | 100 (workspace setting) | Postgres, in the submit transaction | 429 `quota_exceeded` |
| Sweeps per key per day | 5 | Postgres | 429 `quota_exceeded` |
| Live admission units per workspace | 5 (`quota_active_jobs`): each live single experiment is one unit; a sweep is **one** unit however many of its cells are live | Postgres, in the submit transaction (`count(DISTINCT coalesce(sweep_id, job_id))`) | 429 `quota_exceeded` (`scope: "active_jobs"`) |
| Storage (Vercel Blob Hobby) | upload attempts ≤ 1,800 of 2,000/month globally and ≤ 300/month per workspace (`quota_uploads_per_month`); bytes ≤ 900 MB of 1 GB | admission checks `bb.storage_usage_monthly`, `bb.workspace_uploads_monthly` and `run_artifacts` sums; the worker reserves **every** upload attempt (retries included) against both counters before the PUT and refuses the upload when either is spent or the reservation fails (job `failed{storage_failed/storage_budget}`) | 429 `quota_exceeded` (`scope: "storage"`) |
| Work per experiment | `ExecutionLimits::hosted()` (`max_ticks`, `max_tape_events`, `max_output_bytes`) | admission (`estimate_work`) + runtime `RunBudget` | 422 `budget_exceeded` / job `failed{budget_exceeded}` |
| Sweep size | ≤ 25 cells hosted (≤ 100 local = `MAX_GRID_CELLS`) | admission | 422 `budget_exceeded` |

Every 429/503 carries `Retry-After` (seconds). Rate-limited routes also send `X-RateLimit-Limit` and
`X-RateLimit-Remaining`. `GET /whoami` reports quota usage.

Every v1 mutation (`POST /experiments`, `PATCH`/`DELETE /experiments/{id}`, `POST /jobs/{id}/cancel`,
`POST`/`PATCH`/`DELETE /comparisons…`, `POST`/`DELETE /reports…`, `POST /system/api-keys/{id}/revoke`) writes one
`bb.audit_events` row (`action`, `outcome = ok`, target, workspace, `api_key_id`, `request_id`) inside the same
transaction as the change, so a rolled-back mutation leaves no row (SECURITY_REVIEW §15, SEC-25). Refused requests
(validation, quota, auth) and anonymous traffic write none.

## 6. Caching

All authenticated responses are `private` and carry `Vary: Authorization, Origin`. Render's free plan has no edge cache
(Render docs, Deploy for Free: edge caching unsupported), so these headers address browsers only.

| Responses | `Cache-Control` | Validator |
|---|---|---|
| Completed-run data: `/runs/{id}` summary, `orders`, `orders/{oid}`, `fills`, `equity`, `tape`, `timeline`, `engine-check`, `report`, `export`; dataset versions, their quality reports and replay windows; saved report content | `private, max-age=31536000, immutable` | `ETag` = `"<bundle or version sha256 prefix>-<query hash>"` |
| Mutable workspace objects: experiments, comparisons, sweeps, report lists, dataset list, presets, demos, `openapi.json` | `private, no-cache` | `ETag` = `"v<version>"` or content hash; `If-None-Match` → 304 |
| Job status (`/jobs`, `/jobs/{id}`), `/status`, `/whoami`, `/healthz`, `/readyz`, `/system/*`, `/metrics`, all errors | `no-store` | – |

A run's data is immutable because the identity is content-addressed. A run that later fails an integrity check is
quarantined, and a re-simulated run for the same identity has identical content (determinism). A mismatch there is a
`nondeterminism_detected` system event, not a silent change.

## 7. Pagination and byte bounds

Two styles, both with a hard response-size cap of **4 MiB of uncompressed JSON** for keyed callers
(`BB_MAX_RESPONSE_BYTES`) and **1 MiB** for anonymous callers:

- **Cursor** (all lists of DB resources and append-only run sections): `?limit=&cursor=`. The response is
  `{ "items": [...], "next_cursor": string | null, "truncated_by": "limit" | "bytes" | null }`. Cursors are opaque
  base64url strings encoding the keyset (`created_at`, `id`, or the section row index) and a hash of the filters. A
  cursor reused with different filters → 400.
- **Offset** (only where the UI needs random access and totals: `orders`): `?offset=&limit=` →
  `{ "items": [...], "offset": n, "total": n, "next_offset": n | null, "truncated_by": … }`.

If the byte cap is reached before `limit` items, the page ends early with `truncated_by: "bytes"`, and
`next_cursor`/`next_offset` resumes exactly there. A single item larger than the cap (not possible under hosted
limits) → 500 `internal`.

**One enforcement point.** Every response passes the egress middleware, which buffers the serialized body (all
bodies are built in memory), measures its exact length and refuses a body above the caller's cap (anonymous 1 MiB,
keyed `BB_MAX_RESPONSE_BYTES`) with **500 `internal`, `details: {"reason": "response_too_large", "limit_bytes": n}`**.
Handlers bound their output first (pages, downsampled series, replay windows answer 400 with advice), so this refusal
signals a server defect, never silently truncated JSON. The only exempt body is the keyed bundle export
(`application/gzip`, bounded by the output limit). The same measured length, not a `Content-Length` header, is added
to the daily egress counter of the class (cached anonymous bodies included; `HEAD` counts 0).

**Analytics series.** `GET /runs/{id}/analytics` and `GET /datasets/{id}[/versions/{v}]/analytics` return every
aggregate in full; each series field (`curve`, `points`, `ofi_intervals`, `trade_intervals`) longer than 100 points
is replaced by 100 points evenly spaced by index (first and last kept) and listed in
`downsampled: [{ "path": "<JSON Pointer>", "total": n, "returned": 100 }]` (absent when nothing was shortened).
The complete points are paged by `GET /runs/{id}/analytics/series?path=&offset=&limit≤5000` and
`GET /datasets/{id}/versions/{v}/analytics/series?path=&offset=&limit≤5000` →
`{ "path", "total", "offset", "items": [...], "next_offset": n | null, "truncated_by": … }` (offset style above). The
stored report and its sha256 are unchanged; downsampling happens when serving.

## 8. Endpoints

Scope column: `–` = public (anonymous OK), `read`/`run`/`admin` = minimum key scope. Every `GET` that is available
anonymously works on demo content.

### 8.1 Health, status, identity, schema

| Method & path | Scope | Response |
|---|---|---|
| `GET /healthz` | – | `200 {"status":"ok"}`. Never touches the database (Render health check) |
| `GET /readyz` | – | `200 {"status":"ready","db":"ok","migrations":"0001","blob":"configured"}` or `503` `unavailable` (`draining`/`database`/`storage`). Wakes Neon; not for polling |
| `GET /api/v1/status` | – | `Status` (§9.1). Served from memory, no DB access; the UI polls it every 30 s |
| `GET /api/v1/whoami` | – | `Whoami` (§9.2). The UI's cheap authenticated read to validate a key |
| `GET /api/v1/openapi.json` | – | OpenAPI 3.1 document |
| `GET /metrics` | admin | Prometheus text format (TARGET_ARCHITECTURE §15) |

### 8.2 Datasets

| Method & path | Scope | Query | Response |
|---|---|---|---|
| `GET /datasets` | – | `cursor`, `limit ≤ 100` | page of `DatasetSummary` |
| `GET /datasets/{dataset_id}` | – | – | `Dataset` (summary + current version) |
| `GET /datasets/{dataset_id}/versions` | – | `cursor`, `limit` | page of `DatasetVersion` (without `manifest`) |
| `GET /datasets/{dataset_id}/versions/{version}` | – | – | `DatasetVersion` incl. full `manifest` (bb-core `DatasetManifest`, ns as strings) and `timestamp_unit` |
| `GET /datasets/{dataset_id}/versions/{version}/quality` | – | `checker_version` (default latest) | `{checker_version, passed, error_count, warning_count, book_valid_fraction, report: QualityReport, report_sha256}` |
| `GET /datasets/{dataset_id}/versions/{version}/replay` | – | `t0_ns`, `t1_ns`, `step_ns`, `levels ≤ 100`, `max_trades ≤ 50000` | `ReplayWindow` (frames < 5,000 and ≤ 4 MiB; otherwise `truncated_by: "bytes"`) |
| `GET /datasets/{dataset_id}/replay` | – | same | replay of the current version. Answers `Cache-Control: no-cache` plus `Content-Location` naming the versioned URL |
| `GET /presets`, `GET /demos` | – | `dataset_id` | as today (`Preset[]`, `Demo[]`), ns as strings |

### 8.3 Experiments (submit = enqueue job)

`POST /api/v1/experiments` — scope `run`

```json
{ "config": { "dataset_id": "synthetic-demo", "seed": "7", "strategy": { "interval_ns": "100000000", "kind": { "type": "touch_maker", "order_qty": 0.01, "max_position": 0.05, "offset_ticks": 0, "skew_ticks_per_unit": 20 } }, "execution": { … }, "risk": { … }, "record": { … } },
  "label": "maker baseline" }
```

| Outcome | Status | Body |
|---|---|---|
| New job queued | `202 Accepted`, `Location: /api/v1/experiments/{id}` | `{ "created": true, "experiment": Experiment, "job": Job, "run": null }` |
| Same experiment already live in this workspace (job running or queued) | `200` | `{ "created": false, "experiment": …, "job": Job, "run": null }` (stored label; AT-4 "first label wins") |
| Already simulated (canonical run exists) | `200` | `{ "created": <true if new to this workspace>, "experiment": …, "job": null, "run": RunSummary }` |
| Invalid | 422 `invalid_config` / `budget_exceeded`; 429 `quota_exceeded`; 401/403 | error envelope |

Idempotency key = experiment fingerprint (content address). No `Idempotency-Key` header is needed: N identical
concurrent POSTs create exactly one job (DATABASE_DESIGN §13.3).

| Method & path | Scope | Query / body | Response |
|---|---|---|---|
| `GET /experiments` | read | `workspace`, `dataset_id`, `strategy_kind`, `status` (repeatable: `created`…`expired`, `completed`), `sweep_id`, `label_prefix`, `created_after_ns`, `created_before_ns`, `mine=true` (created by this key), `cursor`, `limit ≤ 100` (default 50); newest first | page of `Experiment` (without `config`) |
| `GET /experiments/{id}` | read | – | `Experiment` with `config` |
| `PATCH /experiments/{id}` | run | `If-Match: "v<version>"` (required); `{ "label": string }` | `Experiment`; 409 `conflict` on version mismatch |
| `DELETE /experiments/{id}` | run | `If-Match` optional | `204`. Detaches the experiment and cancels its job if nobody else wants it (TARGET_ARCHITECTURE §5.4). The run is garbage-collected later if unreferenced |

### 8.4 Runs (completed results) and run data

`{run_id}` = 16 or 64 hex. Every endpoint takes `platform` (default canonical `linux-x86_64`). Visibility requires an
experiment in the caller's workspace or in demo. A run that fails verification answers 500 `integrity_failed`.

| Method & path | Scope | Query | Response |
|---|---|---|---|
| `GET /runs` | read | as `/experiments` plus `sort = created_desc \| net_pnl_desc \| net_pnl_asc \| max_drawdown_asc` | page of `RunSummary` (completed only; from Postgres, no artefact reads; AT-10) |
| `GET /runs/{run_id}` | read | `platform` | `RunSummary` (metrics checked against `summary_sha256`) |
| `GET /runs/{run_id}/engine-check` | read | | `EngineStateCheck` |
| `GET /runs/{run_id}/orders` | read | `outcome`, `side`, `offset`, `limit ≤ 2000` (default 200) | offset page of `OrderRecord` with filtered `total` |
| `GET /runs/{run_id}/orders/{order_id}` | read | | `{order, events: FlightEvent[], fills: FillRecord[], explanation: Explanation, instrument: InstrumentMeta}` |
| `GET /runs/{run_id}/fills` | read | `t0_ns`, `t1_ns`, `cursor`, `limit ≤ 5000` | cursor page of `FillRecord` |
| `GET /runs/{run_id}/equity` | read | `t0_ns`, `t1_ns`, `max_points ≤ 5000` (default 2000), `cursor` | `{items: EquitySample[] \| EquityBucket[], downsampled: bool, bucket_ns: string \| null, next_cursor, truncated_by}`. When the window has more than `max_points` samples, buckets carry `{t0_ns, t1_ns, first, last, min_equity, max_equity, n}` and the UI shows `DOWNSAMPLED` |
| `GET /runs/{run_id}/tape` | read | `from_seq`, `order_id`, `limit ≤ 5000` (default 500) | cursor page of `FlightEvent` (cursor = next seq) |
| `GET /runs/{run_id}/timeline` | read | `t0_ns`, `t1_ns`, `max_events ≤ 50000` | `RunTimeline` (existing semantics, ns as strings, plus `truncated_by`) |
| `GET /runs/{run_id}/report` | read | | `text/markdown` research report, deterministic |
| `GET /runs/{run_id}/export` | read | | `application/gzip`, `Content-Disposition: attachment; filename="run-<run_id>-<platform>.bbz"`: the verified bundle (gzip-member NDJSON, format v1; `blackbox verify-bundle` checks it offline). Bounded by `max_output_bytes` |

### 8.5 Jobs

| Method & path | Scope | Query / body | Response |
|---|---|---|---|
| `GET /jobs` | read | `workspace`, `state` (repeatable), `mine=true`, `cursor`, `limit ≤ 100` | page of `Job` (jobs linked to the workspace's experiments), newest first |
| `GET /jobs/{id}` | read | – | `Job`, including live `progress` from the in-process worker's memory when it holds the lease |
| `POST /jobs/{id}/cancel` | run | `{}` | `202 Job`. `queued`/`created` → `cancelled` immediately; `running` → `cancelling` (the worker stops at the next checkpoint); already `cancelling`/`cancelled` → `202`, idempotent; `completed`/`failed`/`expired` → 409 `conflict` (`job_terminal`). If another workspace's experiment still wants the job, only the caller's experiment is detached and the job continues; the response then has `state` unchanged and `detached: true` |

Polling contract (UI §10.4): poll `GET /jobs?state=created&state=queued&state=running&state=cancelling` every 2 s
(5 s when waking or hidden). No server event stream in v1: it would hold connections open across SIGTERM drains and
keep the free instance busy. Polling payloads are small and `no-store`.

### 8.6 Comparisons

`POST /api/v1/comparisons` — scope `run`; body `{ "run_a": "<run_id>", "run_b": "<run_id>", "label"?: string }`.
Returns `201` (new) or `200` (existing for `(workspace, a, b, comparator_version)`, returned without recomputing)
with a `Comparison`. Both runs must be visible; otherwise 404.

Work: eligibility is decided from the verified bundle trailers (manifest + metrics) first. An ineligible pair gets
its `metrics_only` result without decoding any order or tape; only an eligible pair loads both runs and aligns them,
behind the heavy-read limit. This request has its own timeout, `BB_COMPARISON_TIMEOUT_S` (default 60 s, 1–600)
instead of the 10 s of other mutations; on timeout it answers 503 `unavailable` (`busy`), the abandoned alignment
stops at its next checkpoint (every 4,096 orders) and keeps its heavy-read permit until it has stopped.

The eligibility contract (AT-1/AT-2; implemented in `bb-forensics` by the Phase 1 compare work) is part of the
resource:

```json
{
  "id": "01a1…", "run_a": "136d41410c6b23f1", "run_b": "9f2c…", "comparator_version": 2, "label": "", "version": 1,
  "eligibility": {
    "status": "metrics_only",
    "reasons": [ { "field": "strategy.interval_ns", "a": "100000000", "b": "200000000" } ]
  },
  "config_diffs": [ { "path": "strategy.interval_ns", "a": "100000000", "b": "200000000" } ],
  "metrics": [ { "key": "net_pnl", "label": "Net PnL", "unit": "quote", "a": -1.23, "b": 0.41, "delta": 1.64, "like_for_like": false } ],
  "alignment": null,
  "interpretation": [ { "basis": "modeled", "statement": "…" } ],
  "created_at_ns": "1763095313707237000"
}
```

`status = "aligned"` ⇔ `reasons = []`. Reasons name each differing eligibility field: dataset `events_sha256`, dataset
`origin_ns`, `instrument.tick_size`, `strategy.kind.type`, `strategy.interval_ns`. When aligned, `alignment` holds
counters per class (timing-aware classes, e.g. `same_outcome_different_timing`), `first_divergence`, and timing-delta
summaries `{count, min_ns, median_ns, max_ns}` for `exch_arrival_ts` and `first_fill_ts`.

| Method & path | Scope | Response |
|---|---|---|
| `GET /comparisons` | read | page of `Comparison` (without pairs) |
| `GET /comparisons/{id}` | read | `Comparison` |
| `GET /comparisons/{id}/pairs` | read | `class`, `cursor`, `limit ≤ 1000` → page of `OrderPair`; 409 `conflict` (`alignment_unavailable`) when `metrics_only` |
| `PATCH /comparisons/{id}` | run | `If-Match`; `{label}` |
| `DELETE /comparisons/{id}` | run | `204` |

### 8.7 Sweeps

`POST /api/v1/sweeps` — scope `run`; body `{ "spec": GridSpec, "label"?: string }` (GridSpec = bb-core, ns as
strings). Admission covers every cell (§5). The response is `202 { "created": bool, "sweep": Sweep }`. Idempotent per
`(workspace, spec fingerprint)`.

```json
{
  "id": "01a1…", "label": "latency × queue", "version": 1,
  "x": { "axis": "order_latency_ns", "labels": ["1 ms", "5 ms", "20 ms"] },
  "y": { "axis": "queue_model", "labels": ["risk_adverse", "log_prob"] },
  "state": "running",
  "progress": { "cells_total": 6, "completed": 3, "running": 1, "queued": 2, "failed": 0, "cancelled": 0, "expired": 0 },
  "cells": [ { "x": 0, "y": 0, "experiment_id": "01a1…", "job_id": "01a1…", "state": "completed", "run_id": "136d41410c6b23f1",
              "metrics": { "net_pnl": -1.23, "fill_count": 61, "fees": 0.06, "max_drawdown": 2.5 } } ],
  "created_at_ns": "1763095313707237000"
}
```

`state` is derived: `queued` (nothing started), `running`, `completed` (all cells completed), `partial` (terminal
with ≥ 1 failed/expired cell), `cancelled`.

| Method & path | Scope | Response |
|---|---|---|
| `GET /sweeps` | read | page of `Sweep` (without `cells`) |
| `GET /sweeps/{id}` | read | `Sweep` |
| `POST /sweeps/{id}/cancel` | run | `202 Sweep` (cancels each live cell under §8.5 rules) |
| `PATCH /sweeps/{id}`, `DELETE /sweeps/{id}` | run | label / `204` (cells detach) |

### 8.8 Reports

| Method & path | Scope | Response |
|---|---|---|
| `GET /runs/{run_id}/report`, `GET /comparisons/{id}/report`, `GET /sweeps/{id}/report` | read | `text/markdown`, generated on demand (deterministic for runs) |
| `POST /reports` | run | `{ "kind": "run" \| "comparison" \| "sweep", "subject_id": string, "title": string }` → `201 Report` (immutable snapshot, body sha256) |
| `GET /reports` | read | page of `Report` (metadata) |
| `GET /reports/{id}` | read | `Report` metadata |
| `GET /reports/{id}/content` | read | `text/markdown`, immutable, `ETag` = body sha256 |
| `DELETE /reports/{id}` | run | `204` |

### 8.9 System (admin)

| Method & path | Response |
|---|---|
| `GET /system/workers` | `worker_heartbeats` rows (worker id, mode, platform, version, state, current job, last heartbeat age) |
| `GET /system/queue` | `{depth_by_platform, running, oldest_queued_age_ms, lease_reclaims_24h}` |
| `GET /system/events` | `kind`, `level`, `cursor` → page of `system_events` + `audit_events` (`source` field) |
| `GET /system/usage` | `{blob: {month, uploads, uploads_limit: 2000, reads, reads_limit: 10000, bytes_stored, bytes_limit}, db: {size_bytes, limit_bytes}}` |
| `GET /system/api-keys` | keys (no digests): id, workspace, label, display prefix, scopes, created/expires/revoked/last used |
| `POST /system/api-keys/{id}/revoke` | `200` key |

## 9. Resource shapes (non-bb-core)

bb-core types (`RunConfig`, `Metrics`, `OrderRecord`, `FillRecord`, `EquitySample`, `FlightEvent`, `Explanation`,
`DatasetManifest`, `QualityReport`, `GridSpec`, `ReplayWindow`, `RunTimeline`, `OrderPair`, …) are reused as they are,
with ns fields as strings (TARGET_ARCHITECTURE §10).

### 9.1 `Status`

```json
{
  "status": "ok",
  "api_version": "v1", "contract_version": 2,
  "blackbox_version": "0.2.0", "git_sha": "4ac54f3…",
  "engine": { "name": "hftbacktest", "version": "0.9.4", "features": ["backtest"] },
  "server_time_ns": "1763095313707237000",
  "worker": { "mode": "in_process", "state": "idle", "slots": 1, "busy": 0,
              "last_heartbeat_age_ms": 1200, "heartbeat_interval_ms": 15000 },
  "queue": { "depth": 0, "running": 0, "as_of_ns": "1763095300000000000" },
  "limits": { "profile": "hosted", "max_ticks": 300000, "max_tape_events": 250000, "max_output_bytes": 8388608,
              "max_sweep_cells": 25, "deadline_ms": 600000 },
  "disclaimer": "Research tool. Every result is simulated; nothing here is an exchange execution."
}
```

`status ∈ {ok, draining}`. `worker.state ∈ {starting, idle, busy, draining, stopped}`. `queue` comes from the worker's
most recent poll (no extra DB query), and `as_of_ns` tells the UI how old it is. `limits` values are SimCore's; the
numbers shown are placeholders.

`last_heartbeat_age_ms` is a process-liveness signal, kept in memory. While `busy` it is the job heartbeat (each lease
extension, so a stuck job shows a growing age). While `idle` the worker parks without polling Postgres (Neon scales to
zero) and refreshes the heartbeat in memory every `heartbeat_interval_ms`, with no database access; an idle, live worker
therefore never looks stale. The UI calls a worker stale only when the age exceeds 3 × `heartbeat_interval_ms`, which
then means the process or a running job stopped reporting. The `worker_heartbeats` row (`GET /system/workers`) is
written only on state transitions and job heartbeats, so its age during a long idle period is expected to grow.

### 9.2 `Whoami`

```json
{ "mode": "research",
  "workspace": { "slug": "lab", "name": "Research lab", "kind": "research" },
  "key": { "id": "01a1…", "label": "lead laptop", "display_prefix": "bbk_TEST", "scopes": ["read", "run"], "expires_at_ns": null },
  "quotas": { "experiments_per_day": { "limit": 100, "used": 3 }, "sweeps_per_day": { "limit": 5, "used": 0 },
              "active_jobs": { "limit": 5, "used": 1 } } }
```

Anonymous: `{"mode":"demo","workspace":{"slug":"demo",…,"kind":"demo"},"key":null,"quotas":null}`.

### 9.3 `Job`

```json
{
  "id": "01a1d737-598f-779c-9006-939fc0b40bdf", "kind": "run", "state": "running", "version": 3,
  "experiment_fingerprint": "136d41410c6b23f1ac7dbb2ed227345110d5a83304b9f959b44eb6555c79c148",
  "platform": "linux-x86_64", "priority": 100,
  "attempt": 1, "max_attempts": 3, "lease_losses": 0,
  "created_at_ns": "1763095313707237000", "queued_at_ns": "1763095313707237000",
  "started_at_ns": "1763095313724344000", "finished_at_ns": null,
  "queue_position": null, "queue_depth": 0,
  "progress": { "done": 600, "total": 1200, "unit": "ticks", "tape_events": 812 },
  "last_heartbeat_at_ns": "1763095318724344000", "heartbeat_interval_ms": 15000,
  "cancel_requested_at_ns": null,
  "error": null, "expired_reason": null,
  "run_id": null, "run_ids": [],
  "estimate": { "ticks": 1200, "tape_events_upper": 2400, "output_bytes_upper": 409600 }
}
```

- `state` ∈ the 8 states. The edges are in TARGET_ARCHITECTURE §4.1. **`running → queued` is a legal edge**
  (re-queued after a drain or a lost lease). The UI shows `attempt`/`lease_losses` instead of discarding the update.
- `queue_position` is 1-based among queued jobs of the same platform in claim order. It is `null` unless the job is
  `queued`.
- `progress` is `null` before the first report. `done` never decreases within an attempt; a new attempt starts again
  at 0. `total` is exact (ticks are fixed by dataset span and interval).
- `run_ids` (UI §16) equals `[run_id]` once completed. `error` = `{category, code, message}` when `failed`.

### 9.4 `Experiment`, `RunSummary`, `Dataset*`, `Report`

```json
// Experiment
{ "id": "01a1…", "workspace": "lab", "label": "maker baseline", "version": 1, "pinned": false,
  "run_id": "136d41410c6b23f1", "fingerprint": "136d4141…c148",
  "dataset": { "id": "synthetic-demo", "version": 1 }, "strategy_kind": "touch_maker", "interval_ns": "100000000",
  "platform": "linux-x86_64", "sweep": null,
  "status": "running", "job": Job | null, "run": RunSummary | null,
  "estimate": { "ticks": 1200, "tape_events_upper": 2400, "output_bytes_upper": 409600 },
  "created_by": { "display_prefix": "bbk_TEST", "label": "lead laptop" } | null,
  "created_at_ns": "…", "config": RunConfig /* detail only */ }

// RunSummary
{ "run_id": "136d41410c6b23f1", "fingerprint": "…", "platform": "linux-x86_64", "canonical": true,
  "dataset": { "id": "synthetic-demo", "version": 1, "events_sha256": "…", "origin_ns": "1700000000000000000" },
  "strategy_kind": "touch_maker", "interval_ns": "100000000",
  "contract_version": 2, "simulation_revision": 2, "engine": { … }, "blackbox_version": "0.2.0", "build_profile": "release",
  "result_sha256": "…", "summary_sha256": "…",
  "bundle": { "format_version": 1, "sha256": "…", "size_bytes": 61234 },
  "counts": { "ticks": 1200, "orders": 240, "fills": 61, "tape_events": 3020, "equity_samples": 1201 },
  "metrics": Metrics, "engine_check": EngineStateCheck,
  "wall_time_ms": 412, "peak_rss_bytes": 98000000, "created_at_ns": "…" }

// DatasetVersion
{ "dataset_id": "synthetic-demo", "version": 1, "events_sha256": "…", "normalization_version": 1, "contract_version": 2,
  "instrument": { "exchange": "synthetic", "symbol": "SYNTH-PERP", "tick_size": 0.1, "lot_size": 0.001, "contract_size": 1 },
  "origin_ns": "1700000000000000000", "start_ns": "0", "end_ns": "120000000000",
  "timestamp_unit": "ns", "rows": 62929, "events_bytes": 4027456,
  "source": { "id": "blackbox-synthetic", "vendor": "…", "licence": "MIT", "hostable": true },
  "quality": { "checker_version": 1, "passed": true, "error_count": 0, "warning_count": 2 },
  "retired_at_ns": null, "manifest": DatasetManifest /* detail only */ }

// Report
{ "id": "01a1…", "kind": "run", "subject_id": "136d41410c6b23f1", "title": "…", "generator_version": 1,
  "format": "markdown", "body_sha256": "…", "size_bytes": 18234, "created_at_ns": "…" }
```

`Experiment.status` is `completed` when a live run exists for its identity. Otherwise it is the job's state, or
`detached` when the job row was retired by retention without a run (the UI offers "Re-submit").

## 10. Examples

```http
POST /api/v1/experiments HTTP/1.1
Host: blackbox-hft-api.onrender.com
Origin: https://blackbox-hft.vercel.app
Authorization: Bearer bbk_…
Content-Type: application/json

{"config":{…},"label":"maker baseline"}

HTTP/1.1 202 Accepted
Location: /api/v1/experiments/01a1d737-5a1c-7b0e-9e7a-3f0c2d1e4b5a
Cache-Control: no-store
X-Request-Id: 01a1d737-5a1c-7b0e-9e7a-3f0c2d1e4b5a
Access-Control-Allow-Origin: https://blackbox-hft.vercel.app
Vary: Authorization, Origin

{"created":true,"experiment":{…},"job":{"id":"01a1…","state":"queued","queue_position":1,…},"run":null}
```

```http
GET /api/v1/runs/136d41410c6b23f1/tape?from_seq=0&limit=5000 HTTP/1.1

HTTP/1.1 200 OK
Cache-Control: private, max-age=31536000, immutable
ETag: "9c41e2…-a1f0"

{"items":[{"seq":0,"ts":"1000000","clock":"local",…}],"next_cursor":"eyJzIjo1MDAwfQ","truncated_by":"limit"}
```

## 11. OpenAPI and TypeScript generation

- **Server:** `utoipa` 6 + `utoipa-axum` 0.3 (OSS evaluation §3.5). Routers are built with `OpenApiRouter`, and
  handlers carry `#[utoipa::path]` with response codes and error schemas. bb-core types derive `ToSchema` behind a
  bb-core `openapi` feature. The ns newtype (`NsString(i64)`) is declared with `#[schema(value_type = String,
  pattern = "^-?[0-9]{1,19}$")]`, so generated clients see strings.
- **Spec output:** served at `GET /api/v1/openapi.json`, and printed offline by `blackbox openapi` (no DB or network
  needed). The committed snapshot is `web/src/api/openapi.json`.
- **TypeScript:** `openapi-typescript` (npm `openapi-typescript` 7.13.0, MIT, read from the npm registry 2026-11-14)
  generates `web/src/api/schema.d.ts`. `web/src/api/client.ts` is a thin typed `fetch` wrapper over its `paths` type,
  adding base URL, auth header, error-envelope decoding and abort/timeout. The hand-maintained
  `web/src/api/types.ts` (1,034 lines) is deleted at cutover.
- **CI (AT-14):** `blackbox openapi | diff - web/src/api/openapi.json` and
  `npx openapi-typescript web/src/api/openapi.json -o "$RUNNER_TEMP/schema.d.ts" && diff "$RUNNER_TEMP/schema.d.ts" web/src/api/schema.d.ts`.
  Either diff fails the build.
- **Docs UI:** none in the API process (no Swagger bundle on the 512 MB host). The web app may render the spec.

## 12. UI_DESIGN_SPEC §16 traceability

| UI need (§16) | Provided by |
|---|---|
| All ns values as decimal strings | §1 (wire rule), TARGET_ARCHITECTURE §10 (bb-core serde, `CONTRACT_VERSION` 2) |
| Health: `worker.state`, `worker.last_heartbeat_age_ms`, `worker.heartbeat_interval_ms`, `queue.depth`, `server_time_ns` | `GET /api/v1/status` §9.1 (memory-only, safe to poll every 30 s) |
| Job resource: `id`, `state` (8), `created/queued/started/finished_at`, `queue_position`, `progress {done,total}`, `last_heartbeat_at`, `error {code,message}`, `expired_reason`, `run_ids[]` | `Job` §9.3 (`*_at_ns` names) |
| Job list filter by state and "mine" | `GET /jobs?state=…&mine=true` §8.5 |
| Optional server event stream | Not offered in v1; polling contract §8.5 |
| Auth header, cheap whoami, 401 vs 403 | §3, `GET /api/v1/whoami` |
| CORS + CSP `connect-src` | §4; CSP change in DEPLOYMENT.md §6 |
| Dataset manifest timestamp resolution | `DatasetVersion.timestamp_unit` §9.4 |

Changes the UI spec must absorb (reported to Main): (1) the 401 code is `unauthorized`, not `invalid_key` (§10.2
mock-up); (2) `running → queued` is a legal transition (§10.4); (3) the `PRECOMPUTED BUNDLE` access mode (§10.2) is
retired along with the static viewer, and the public demo is served live by this API; (4) the UI's health poll moves
from `/api/health` to `/api/v1/status`. A non-JSON body there still means `WAKING`, because Render serves its own
loading page while the instance spins up (Render docs, Deploy for Free).

## 13. Implementation notes and deviations (p2-api, 2026-11-14)

The generated `GET /api/v1/openapi.json` is normative. Where the implementation differs from the sections
above, and why:

| Topic | Implemented | Reason |
|---|---|---|
| ns encoding (§1) | bb-core/bb-analytics still serialize ns as numbers; bb-app converts at the HTTP boundary (`crates/bb-app/src/wire.rs`, explicit key list) and accepts strings or numbers on input. `CONTRACT_VERSION` stays 1 | Changing bb-core serde would change `config_sha256`/`result_sha256` and the pinned golden hashes (Main's decision) |
| Added resources | `GET /runs/{id}/analytics` (RunAnalytics), `GET /datasets/{id}/analytics` and `/versions/{v}/analytics` (DatasetAnalytics); `RunSummary.label`, `RunSummary.experiment_id`, `RunSummary.config` (detail); `Demo.runs[].run_id/experiment_id`; `Comparison.status` + bb-core comparison fields, `alignment.pairs` always empty (paged by `/pairs`); `GET /experiments?run_id=`; `GET /comparisons/{id}/report`, `GET /sweeps/{id}/report` | UI needs (UIv1) and the analytics scope addition |
| `/readyz` | Answers from the state of the start-up DB check/migration; `?probe=1` (admin) runs a live probe | SECURITY_REVIEW SEC-10 (no DB traffic from public paths) |
| Anonymous `GET /runs/{id}/export` | 401 | SEC-12 |
| Read keys on any mutation | 403 in the auth layer, before the body is read | SEC-1 |
| Status limits | `max_ticks`/`max_tape_events`/`max_output_bytes` are `null` when unlimited (local profile) | values above 2^53 − 1 are not representable |
| Errors | 413/415/431 use `invalid_request`; `platform_mismatch` is 409 | SEC-5 |
| Host/CORS | Same-origin requests (Origin authority = Host) are always allowed, besides `BB_CORS_ALLOWED_ORIGINS` | the local UI served by `serve` sends `Origin` on POST |
| API keys | `bbk_` + base64url(24 random bytes ‖ 8-byte HMAC-SHA256 tag with `BB_API_KEY_PEPPER`); forged keys fail before any DB access | SECURITY_REVIEW §9.1, SEC-31 |
| Rate limits | `governor` keyed limiters in the auth layer (per IP for anonymous, per key, plus a global anonymous cap; client IP = last `X-Forwarded-For` entry) instead of a `tower_governor` layer | one layer must apply different quotas to anonymous and keyed callers |
| Quotas | hosted `quota_active_jobs` = 5 admission units, a sweep counting as one unit (migration 0009) | a 25-cell sweep must still be admissible next to single experiments |
| Sweeps | v1 sweeps still take `GridSpec`; switch to `bb_core::sweep::SweepSpec` (p5-sweeps) is pending | merged after this branch's scope was fixed |
| `demo seed` | Runs against the database directly (`DATABASE_URL`), not against the hosted API; `--platform linux-x86_64` seeds canonical jobs for the hosted worker | no admin-API seeding path is needed for a queue-based seed |
