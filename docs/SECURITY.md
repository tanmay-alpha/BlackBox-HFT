# Security

BlackBox is a hosted and local research tool: it replays market data through a simulator and
explains simulated orders. It has no exchange connectivity or trading credentials.

## Hosted v1 security

The current console at https://blackbox-hft.vercel.app uses the live
https://blackbox-hft-api.onrender.com/api/v1 API. Anonymous access is read-only in the synthetic
demo workspace; mutations require hashed, scoped, pepper-tagged API keys. Workspaces scope reads
and mutations; experiments enter a leased Postgres queue with runtime budgets and cancellation.
Run bundles use filesystem storage locally and private Vercel Blob storage when hosted, with
integrity verification, quarantine and byte-bounded caches.

`v1/auth.rs`, `v1/edge.rs`, `admission.rs`, `worker.rs` and `bb-db` implement these controls.
The active-job default is 5 admission units per workspace (a sweep counts as one unit);
each workspace has 300 Blob upload attempts/month, including retries. Response bodies are
measured centrally: anonymous responses are capped at 1 MiB and keyed non-export responses at
`BB_MAX_RESPONSE_BYTES` (default 4 MiB). Paged reads and downsampled analytics keep normal
responses within those caps; oversize bodies are refused, not silently truncated.

These are source properties, not a claim that every security acceptance exercise passed.
See [SECURITY_REVIEW](engineering/SECURITY_REVIEW.md) for the dated threat model and requirements,
[FINAL_AUDIT §3–§7](engineering/FINAL_AUDIT.md#3-acceptance-verdicts) for observed gaps and subsequent
source fixes, and [PROJECT_STATE](engineering/PROJECT_STATE.md) for release status and remaining
verification boundaries. Production deployment must be checked separately from source fixes.

## Deprecated local `/api` tooling

The remaining sections document the earlier file-store server and its 2026-11-14 review.
They describe the removed `blackbox demo` server and its Host/Origin guard (`access.rs`, also removed), **not** `/api/v1`.

## Threat model

**What there is to protect.** Stored datasets and runs under `--data-dir`, the CPU and memory of the
machine running the server, and the files outside the data directory. There are no accounts, no
passwords, no exchange API keys, no wallets and no order routing: nothing in the binary can trade.
The engine dependency is compiled with the `backtest` feature only. `cargo tree -p bb-app -e
features | grep -i 'hftbacktest feature'` lists `backtest`, `hftbacktest-derive`, `nom`, `uuid`
and `zip`, and no `live` (checked 2026-11-14).

**Who can reach it.** By default the server binds `127.0.0.1:8787` (`blackbox serve --addr`). The
realistic attacker is therefore not a remote host but **a web page open in the user's own browser**,
which can send requests to loopback (cross-site requests, or DNS rebinding of an attacker's hostname
to 127.0.0.1), and **malformed or hostile input**: a run configuration, query parameters, a path
segment, or a CSV file being ingested.

**Out of scope.** A malicious local user or process (it can read `data/` directly), and a server
deliberately bound to a non-loopback address on an untrusted network (see residual risks).

## Controls

| Control | Code | Test |
|---|---|---|
| Host allowlist (DNS rebinding) | `crates/bb-app/src/access.rs` | `access.rs::dns_rebinding_host_is_refused`, `own_names_are_admitted` |
| Origin allowlist (cross-site requests) | `crates/bb-app/src/access.rs` | `access.rs::foreign_origin_is_refused_even_with_a_valid_host`, `explicit_additions_are_honoured`; `crates/bb-app/tests/integration.rs` (asserts code `forbidden`) |
| No CORS headers | `crates/bb-app/src/access.rs` (none emitted) | curl probe below |
| Config bounds | `crates/bb-core/src/config.rs` (`RunConfig::validate`) | `integration.rs::degenerate_intervals_are_refused_before_simulation` |
| Dataset-relative tick budget | `crates/bb-app/src/pipeline.rs` | curl probe below |
| Fail-fast grid validation | `crates/bb-core/src/compare.rs` | `integration.rs::oversized_grid_is_rejected_without_visiting_cells` |
| Simulation admission limits | `crates/bb-app/src/server.rs` (`RUN_SLOTS`, `GRID_SLOTS`) | none (code reading) |
| Replay frame bound | `crates/bb-app/src/server.rs`, `crates/bb-data/src/book.rs` | `server.rs::replay_rejects_empty_windows_bad_steps_and_oversized_responses` |
| Generic internal errors | `crates/bb-app/src/error.rs` | `error.rs::codes_and_statuses_follow_the_api_contract` |
| Dataset and run id validation | `crates/bb-data/src/store.rs`, `crates/bb-app/src/store.rs` | `bb-data store.rs::unknown_and_malformed_ids`; `bb-app store.rs::run_id_shape_is_enforced`, `malformed_ids_never_reach_the_filesystem` |
| CSV line cap | `crates/bb-data/src/tardis.rs` | `tardis.rs::hostile_long_line_is_refused` |
| Fetch script argument validation | `scripts/fetch-tardis-sample.sh` | none |
| Pinned CI actions, RustSec audit | `.github/workflows/ci.yml` | CI `audit` job |

### Host and Origin allowlists

`access.rs` wraps every route. A request whose `Host` header is not one of the server's own
authorities — `127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>`, plus the bind address itself
when it is a specific non-loopback IP — gets `403 forbidden`. A request carrying an `Origin` that is
not `http://` + one of those authorities, or an origin added explicitly, also gets `403 forbidden`.
Browsers send `Origin` on every cross-origin request and on same-origin non-GET requests, so another
site can neither read results nor start simulations or delete runs; a rebinding page fails the Host
check. Requests without `Origin` (curl, same-origin GET) pass the Origin check.

Adding names:

- **Vite dev server** (`npm run dev`, port 5173): the proxy in `web/vite.config.ts` rewrites `Host`
  to the target (`changeOrigin: true`) but forwards the browser's `Origin`, so start the API with
  `blackbox demo --allow-origin http://localhost:5173` for legacy API requests only; the current console needs v1.
- **Legacy Docker commands:** the old `demo --addr 0.0.0.0:8787` command exposes only deprecated
  `/api`. The current compose image starts the authenticated v1 stack instead; see
  [DEPLOYMENT.md](DEPLOYMENT.md#local). Legacy Host/Origin flags below do not configure v1.
- **Another hostname** (only on a trusted network): `--allow-host myhost:8787`. Each flag is
  repeatable.

**No CORS.** No `Access-Control-*` header is ever sent: the bundled UI is same-origin and the dev
server proxies, so there is no reason to let a browser share responses with another origin.

### Work bounds

- **Config bounds** (`config.rs`): `strategy.interval_ns` in 1 ms … 1 h; order latencies and jitter
  0 … 60 s; feed offset at most 60 s. Violations return `422 invalid_config` listing every problem.
- **Tick budget** (`pipeline.rs`): ticks = dataset span / interval must not exceed
  `MAX_TICKS_PER_RUN` = 2,000,000, checked against the actual dataset before anything runs, so a
  legal interval on a long dataset is still refused.
- **Grids** (`compare.rs`): more than `MAX_GRID_CELLS` = 100 cells is refused before any cell is
  expanded or simulated (the test asserts rejection in under 200 ms).
- **Admission** (`server.rs`): at most 4 single-run simulations execute at once and one sensitivity
  grid at a time; further requests wait for a slot.
- **Replay window** (`server.rs`): a window that would hold `MAX_REPLAY_FRAMES` = 5,000 or more
  depth frames is refused with `400`; the frame count is computed without overflow. Levels are
  capped at 100, trades at 50,000, order pages at 2,000, tape pages at 5,000.
- **CSV line cap** (`tardis.rs`): a line longer than 4,096 bytes is an ingestion error; the reader
  never buffers more than that plus the line ending.

### Errors and paths

- **Generic internal errors** (`error.rs`): an `internal` (500) response carries only "internal
  server error; the details are in the server log"; the detail goes to stderr. Client errors keep
  their specific messages.
- **Ids never become arbitrary paths.** Dataset ids must match `[a-z0-9_-]+` before
  `root.join(id)` (`bb-data store.rs::dataset_dir`). Run ids must be exactly 16 lowercase hex
  characters, checked by every run-store operation before a path is built (`bb-app
  store.rs::validate_run_id`).
- **Fetch script**: date must be `YYYY-MM-DD`, exchange `[a-z0-9][a-z0-9-]*`, symbol
  `[A-Za-z0-9][A-Za-z0-9_-]*`, before any of them reaches a URL or path; `set -euo pipefail`,
  `curl --fail`, download to `.part` then rename.

### Supply chain

- Every GitHub Action in `.github/workflows/ci.yml` is pinned to a full commit SHA with the version
  in a comment.
- The `audit` job runs `rustsec/audit-check` against `Cargo.lock`. No local `cargo audit` was run.
- `npm audit --omit=dev` in `web/` on 2026-11-14 reported **found 0 vulnerabilities**.

#### Dependency advisories

The first CI run of the audit job (run 43503383714, 2026-11-13) reported five advisories, all in
dependencies that the pinned engine fixes:

| Advisory | Crate | Issue | Status |
|---|---|---|---|
| RUSTSEC-2026-0367 | zip 6.0.0 | `extract()` preserves SUID/SGID bits | Not reachable |
| RUSTSEC-2026-0368 | zip 6.0.0 | `ZipArchive::extract` follows symlink targets outside the destination | Not reachable |
| RUSTSEC-2026-0369 | zip 6.0.0 | `extract()` applies permissions through symlinks | Not reachable |
| RUSTSEC-2026-0429 | zip 6.0.0 | `ZipArchive::extract` aborts on a symlink entry with a large declared size | Not reachable |
| RUSTSEC-2025-0141 | bincode | unmaintained (informational) | Accepted |

The `zip` advisories are fixed only in zip 9.x, while hftbacktest 0.9.4 depends on `zip = "6.0.0"`
(caret range), so upgrading would require forking the engine. All four concern the extraction
APIs. A search of both code bases (`grep -rn "\.extract(\|ZipStreamReader"` over
`hftbacktest-0.9.4/src` and `crates/`) finds no call: the engine opens archives with
`ZipArchive::new` + `by_name` (`backtest/data/npy/mod.rs`), BlackBox writes them with `ZipWriter`
(`crates/bb-data/src/store.rs`), and BlackBox reads only archives it wrote, after their SHA-256 has
been checked against the dataset manifest. The CI job lists these IDs in `ignore` with this
justification; revisit when the engine moves to zip 9.

Four advisories issued on 2026-11-17 failed the Phase 6/7 release CI (run 44287056318) and were
accepted with this evidence:

| Advisory | Crate | Issue | Status |
|---|---|---|---|
| RUSTSEC-2026-0545 | zip 6.0.0 | AES entries accepted without their authentication code when truncated or empty (`aes-crypto`, password reads such as `by_name_decrypt`) | Not reachable: no password-decrypting read in either code base |
| RUSTSEC-2026-0547 | zip 6.0.0 | Shrink/Reduce/Implode decoders preallocate the declared size | Not reachable: needs the `legacy-zip` feature, which `cargo tree -e features -i zip` does not list |
| RUSTSEC-2026-0548 | zip 6.0.0 | `ZipStreamReader` uses local-header names | Not reachable: `ZipStreamReader` is never called |
| RUSTSEC-2026-0543 | chrono 0.4.45 | unmaintained (informational) | Accepted: no known vulnerability; used by sqlx and for timestamps; replacing it means moving sqlx to `time` |

All three zip fixes are in zip 9.x only, so the same engine constraint applies.

### Hosted console

The removed static-file viewer is not a deployment mode. The Vercel SPA reads the live v1 API;
anonymous visitors cannot submit jobs, while authenticated research users can. See the hosted
security summary above and [DEPLOYMENT.md](DEPLOYMENT.md).

## Probes against the running server

Deprecated file-store `/api` release server on `127.0.0.1:8787`, 2026-11-14.

| Probe | Observed |
|---|---|
| `curl -H "Host: evil.example:8787" /api/health` | `403 forbidden` ("requests addressed to host 'evil.example:8787' are refused …") |
| `curl -X DELETE -H "Origin: https://evil.example" /api/runs/0000000000000000` | `403 forbidden` ("requests from origin 'https://evil.example' are refused …") |
| `curl -i -H "Origin: http://127.0.0.1:8787" /api/health` | `200`, no `Access-Control-*` header |
| `curl /api/runs/..%2F..%2Fetc` | `invalid_request`: malformed run id `"../../etc"` |
| `curl /api/runs/ZZZ` | `invalid_request`: malformed run id |
| run with a 1 ns interval (lead) | `422 invalid_config` |
| replay with a huge window (lead) | `400` |

## Independent review, 2026-11-14

An independent agent (openai-codex/gpt-6.1-sol) reviewed the server, ingestion, CI and the fetch
script by static reading only: it executed no requests and no audits, and its exploit outcomes are
inferences from source. Its eight findings, with locations as they were in the reviewed code, and
what was done (fixed in commit `3723bb5`; CI pins in `2a4c9d6`):

| # | Severity | Location (reviewed code) | Scenario | Recommended fix | How fixed | Verified by |
|---|---|---|---|---|---|---|
| 1 | High | `server.rs` CORS layer (`allow_origin(Any)`, GET/POST/DELETE) | Any web page open in the user's browser could list runs, read exports, start expensive runs or grids and `DELETE` runs; no credential needed | Same-origin by default, explicit dev origins, reject untrusted `Origin`, per-session token for mutations | CORS layer removed; Origin allowlist in `access.rs`. **No token was added** (see residual risks) | `access.rs` tests; curl: foreign-Origin `DELETE` → 403 |
| 2 | High | `server.rs` router, `main.rs` listener: no `Host` check | DNS rebinding: `attacker.example:8787` re-pointed at 127.0.0.1 makes API calls same-origin | Host/authority allowlist plus origin checks and authentication | Host allowlist in `access.rs` (`--allow-host` to extend) | `access.rs::dns_rebinding_host_is_refused`; curl: foreign Host → 403 |
| 3 | High | `pipeline.rs`, `config.rs`: only `interval_ns > 0` required | `interval_ns: 1` asks for ~10⁹ strategy evaluations and equity samples per simulated second; no tick budget, no admission control; disconnecting does not cancel | Dataset-relative work budget, output budgets, global concurrency limit, cancellation | Interval 1 ms … 1 h (`config.rs`); 2,000,000-tick budget against the dataset span (`pipeline.rs`); semaphores of 4 runs / 1 grid (`server.rs`). **No cancellation, no output-size budget** (residual) | `integration.rs::degenerate_intervals_are_refused_before_simulation`; lead's probe: 1 ns interval → 422 `invalid_config`. Semaphores: not covered by a test |
| 4 | High | `pipeline.rs` with the engine's `elapse`: no upper bound on interval | `interval_ns = i64::MAX` wraps the engine clock negative in release builds, so the run never reaches the next event and samples forever | Checked, dataset-aware bounds on interval and latency arithmetic, plus the work budget | Interval ≤ 1 h, latencies and jitter ≤ 60 s, feed offset ≤ 60 s (`config.rs`), and the tick budget | `degenerate_intervals_are_refused_before_simulation` (rejects `i64::MAX` interval and `i64::MAX` entry latency) |
| 5 | High | `compare.rs` grid validation | A 100,000 × 100,000 grid (body under the 2 MiB limit) triggers 10¹⁰ cell validations before the size error is returned | Reject oversized/empty axes before visiting any cell, checked multiplication | `compare.rs` returns immediately when cells exceed 100 | `integration.rs::oversized_grid_is_rejected_without_visiting_cells` (asserts < 200 ms and one problem) |
| 6 | Medium | `tardis.rs` gzip + `read_until` | A gzip stream with a 2³²-byte line grows one buffer without limit; many short rows grow event vectors and maps unbounded (CLI ingestion, not HTTP) | Line-length cap, plus decompressed-byte, row and retained-event budgets | 4,096-byte line cap enforced while reading (`tardis.rs`). **No total byte/row budget** (residual) | `tardis.rs::hostile_long_line_is_refused` |
| 7 | Low | `server.rs` replay: `span / step_ns + 1` | `t1 = i64::MAX, step_ns = 1` overflows: panic in debug, wrap past the check in release (the reviewer noted `book.rs` still widened the step, so no unbounded allocation) | Compare `span / step_ns` before adding one; return `invalid_request` | Check rewritten as `frames >= MAX_REPLAY_FRAMES` on `span / step_ns` | `server.rs::replay_rejects_empty_windows_bad_steps_and_oversized_responses`; lead's probe: huge window → 400 |
| 8 | Low | `ci.yml`: actions referenced by mutable tags | A retargeted tag changes code CI runs without a repository change | Pin to full commit SHAs, restrict token permissions | Every action pinned to a commit SHA; workflow-level `permissions: contents: read`, widened only for the `audit` job | Reading `.github/workflows/ci.yml` |

The reviewer's further observations, also addressed: internal errors returned `to_string()`
verbatim, including store file paths and worker/panic detail (now the generic message in
`error.rs`); the fetch script did not constrain exchange and symbol (an exchange containing `../`
could create directories outside `data/raw`) and promoted downloads before checking them (now both
validated by regex, and `gzip -t` runs before `.part` is renamed). It found sound: path ids, the
static file fallback (`ServeDir`), report/export filenames, replay and paging caps, the 2 MiB JSON
body limit, rejection of non-finite numbers, the 16-entry run cache, and the absence of the engine's
`live` feature. It could not run `cargo audit` or `npm audit`.

## Historical residual risks of deprecated `/api` tooling

These were the local review's residual findings, not a current v1 mitigation verdict; budgets,
cancellation, paging and byte-bounded caches in the hosted stack are described above.

- **No authentication.** Anyone who can reach a non-loopback bind (`--addr 0.0.0.0:…`, Docker port
  published on all interfaces) and send an allowed Host can read, run and delete. Bind to loopback
  or put it behind something that authenticates.
- **No cancellation.** An in-flight simulation cannot be cancelled; a disconnected client does not
  stop it. Its cost is bounded by the work budgets above, not by cancellation.
- **Response size proportional to run size.** Timeline, fills and equity responses are not
  paginated; they grow with the run's fills and ticks (ticks are capped at 2,000,000).
- **Run cache bounded by count, not bytes.** The in-memory cache holds up to 16 runs
  (`CACHE_CAPACITY`, `bb-app store.rs`) whatever their size.
- **Ingestion bounded per line, not in total.** A file's total row count is not capped, by design:
  a full Tardis day of L2 updates is large (the 2020-02-01 BTCUSDT book file has 27,199,023 rows).
  Ingestion is an explicit CLI action on files the user chose.
- **Dependency advisories** rely on CI's RustSec job; no local `cargo audit` was run.
