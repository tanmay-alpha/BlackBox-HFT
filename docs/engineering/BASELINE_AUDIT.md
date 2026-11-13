# Baseline audit: BlackBox HFT at `5fce27d`

Phase 0 audit of the code as it stands at HEAD `5fce27d` ("Static public viewer, final-review
fixes"; `git log -1` shows commit time 2026-11-14 02:45 +0530, working tree clean). Its purpose
is to give the implementers of the research-laboratory redesign a verified starting point.
Statements cite `file:line` at that commit, a command that was run for this audit together with
its output, or a URL that was read. Anything that was not observed is marked **[INFERENCE]**.

**How the reproductions were run.** Every reproduction used the release binary
`target/release/blackbox.exe`, built 2026-11-14 02:47 after the HEAD commit with no local changes.
It was copied into `target/scratch-BaselineAudit/bin/` and pointed at a scratch `--data-dir`. The
data was synthetic only: `blackbox synth` produced `synthetic-demo` (seed 7, 120 s, 62,929 rows);
`synth --id synth-b --seed 8` produced a second dataset; `synth --id synth-long --duration-s 1800`
produced a 1800 s dataset of 953,910 rows. A scratch server ran with
`serve --addr 127.0.0.1:18791`. HTTP requests and memory sampling were driven from Python:
`urllib` for requests, `GetProcessMemoryInfo` through `ctypes` for memory. The Tardis dataset was
read only for one aggregate check, which found no number above 2^53 in its `dataset.json`. The
scratch directory was deleted at the end of the audit.

---

## 0. Verdicts on the eight mandatory targets

| # | Target | Verdict | One-line evidence |
|---|---|---|---|
| 1 | Compare alignment / eligibility | **CONFIRMED** (misleading alignment; no eligibility gate) | Order ids are paired by `(tick_no, slot)` regardless of interval, dataset or strategy. "Identical" ignores every timestamp. Reproduced: a 100 ms vs 200 ms interval pairs decisions made 0.1 s apart and reports 13 "identical". A queue-model change hides 5 different fill times *before* the "first divergence", whose text says everything before it was identical (§5.1) |
| 2 | `FsRunStore::get()` integrity | **CONFIRMED** (no verification) | `store.rs:213-236` never recomputes `result_sha256`. A tampered `run.json` (fill price +50 ticks, `net_pnl` changed) was served after a restart with its original `result_sha256` (§5.2) |
| 3 | `find_or_execute` single-flight | **CONFIRMED** (no single-flight; no corruption observed) | 4 concurrent identical `POST /api/runs` each simulated (distinct `wall_time_ms` and labels; last writer's label stored). 8 concurrent requests gave 4 simulations, bounded only by `RUN_SLOTS` (§5.3) |
| 4 | Cross-platform determinism policy | **PARTIAL** (documented caveat; no enforced policy) | Platform is recorded but excluded from identity (`pipeline.rs:56-71`, `pipeline.rs:154`). The Windows golden hash for `maker_slow` was reproduced here (`662ad3b6…`), against a Linux pin of `9a809e78…` for the same run id. No Windows CI job exists. The static bundle is Windows-produced (§5.4) |
| 5 | Resource isolation (RUN_SLOTS 4 / GRID_SLOTS 1) | **CONFIRMED** (oversubscription possible; ~4.5× the 512 MB host for one legal run) | One legal run (1.8 M ticks) peaked at 2,281 MB private / 1,578 MB working set and wrote a 918 MB `run.json`. The two semaphores are independent and a grid uses `available_parallelism()` threads, so 4 + N simulations can run at once. Loading that run into the server took the peak working set to 1,630 MB (§5.5) |
| 6 | Static vs live build CI | **CONFIRMED gap** | CI builds and E2E-tests only the live build (`ci.yml:48-49, 101-111`). `npm run build:static` is never run in CI. Vercel cannot build it (bundle in git-ignored `data/`). The live URL returned 404 during the audit (§5.6) |
| 7 | Timestamp precision | **PARTIAL** (relative ns safe in practice; no guard; two non-timestamp u64/i64 hazards reproduced) | The maximum relative ns observed was 1.8 × 10^12, against 2^53 ≈ 9.0 × 10^15. No server-side span guard exists. `generator.origin_ns` (absolute, 1.7 × 10^18) is a JSON number. A `seed` of 2^53+1 is accepted by the API, read as 2^53 by `JSON.parse`, and that seed is a different experiment (`3dd36cf0…` vs `84cc6d23…`) (§5.7) |
| 8 | Documentation accuracy | **CONFIRMED** (13 stale or misleading claims) | Planned-vs-done drift (ARCHITECTURE, SECURITY, TESTING); a hash-definition comment missing the revision; overclaims about compare exactness and admission "capping memory" (§5.8) |

---

## 1. Current architecture

### 1.1 Crates

The dependency rule in `docs/ARCHITECTURE.md:44-46` matches the code: only `bb-sim` and `bb-data`
link `hftbacktest`.

| Crate | Responsibility (verified entry points) |
|---|---|
| `bb-core` | Shared serde contract. `RunConfig` (`config.rs:13-22`), `RunResult`/`RunManifest`/`OrderRecord`/`Metrics` (`result.rs`), `Comparison`/`GridSpec` (`compare.rs`), flight events, hashing, PRNG. Relative-ns time convention (`lib.rs:8-12`). `CONTRACT_VERSION = 1`, `SIMULATION_REVISION = 2` (`lib.rs:37,47`). Order id `(tick_no << 8) \| slot` (`strategy.rs:25-27`) |
| `bb-data` | Tardis CSV ingest (`tardis.rs`), normalisation, quality checks (`quality.rs`), synthetic generator (`synthetic.rs`, cap 20 M events at `:66`), `.npz` store with SHA-256 (`store.rs`), book reconstruction for replay (`book.rs`) |
| `bb-strategies` | Touch maker, Avellaneda-Stoikov, imbalance taker, scripted (`config.rs:163-172`). Pure state machines over `LocalView` |
| `bb-sim` | `simulate()` (`lib.rs:90-182`): fixed-interval `elapse` loop, risk checks, ledger, equity samples, recording taps, engine cross-check |
| `bb-forensics` | Pure post-processing. `build_orders`, `enrich_fills` (mid and markouts from the exchange-clock BBO), `compute_metrics`, `explain_order` (`explain.rs`, 2,975 lines), `compare_runs` (`compare.rs:367-448`), `result_sha256` (`lib.rs:30`) |
| `bb-app` | Pipeline (`pipeline.rs`), `RunStore`/`FsRunStore` (`store.rs`), dataset cache (`datasets.rs`), presets and demos, axum server (`server.rs`), Host/Origin guard (`access.rs`), CLI (`main.rs`, 15 subcommands at `:51-110`), static exporter (`export_static.rs`) |
| `web/` | React 19 + Vite 7 console. Routes are listed at `App.tsx:190-203`. The API client switches to static files when built in `--mode static` (`client.ts:244`, `vite.config.ts:14-18`) |

### 1.2 Data flow of one run

`POST /api/runs` → `create_run` (`server.rs:330-360`) → admitted on `RUN_SLOTS`
(`server.rs:335`) → `check_config` → `DatasetCache::get_or_load` (`datasets.rs:128-134`; hash-checked
load, BBO series built once) → `pipeline::find_or_execute` (`pipeline.rs:196-223`):

1. `run_identity`: sha256 of `(CONTRACT_VERSION, SIMULATION_REVISION, ENGINE_NAME, ENGINE_VERSION,
   events_sha256, config)`. The run id is the first 16 hex characters (`pipeline.rs:56-71`).
2. `store.summary(run_id)`. If it exists with an equal full `config_sha256`, that summary is
   returned (`:205-210`).
3. Otherwise `execute` runs under `catch_unwind` (`:213-221`): `bb_sim::simulate`, then
   `build_orders`, `enrich_fills` and `compute_metrics`, then `result_sha256(tape, fills, equity)`
   (`pipeline.rs:137-147`).
4. `store.put` (`store.rs:201-211`) writes `run.json` and then `summary.json` atomically per file.

All later views (orders, tape, timeline, explanation, compare, report, export) are recomputed
from the stored `RunResult`. None of them re-runs the engine (`docs/ARCHITECTURE.md:80-82`,
consistent with `server.rs`).

### 1.3 Endpoints (from `server.rs:76-111`)

All are under `/api` and wrapped by `access::guard` (`main.rs:1095`). Twenty-one method/path pairs
match the table in `docs/API.md:24-46`.

| Method | Path | Execution | Limits |
|---|---|---|---|
| GET | `/health` | async | – |
| GET | `/datasets`, `/datasets/{id}` | blocking pool | id `[a-z0-9_-]+` |
| GET | `/datasets/{id}/replay` | blocking | default span 60 s; levels ≤ 100; trades ≤ 50,000; frames < 5,000 (`server.rs:42-53`) |
| GET | `/presets`, `/demos` | blocking | – |
| POST | `/runs` | **RUN_SLOTS (4)** + blocking, synchronous | label ≤ 200 chars (`:60`); body 2 MiB (`docs/API.md:15`) |
| GET | `/runs` | blocking | **unpaginated**, reads every `summary.json` (`store.rs:243-259`) |
| GET/DELETE | `/runs/{run_id}` | blocking | 16-hex id (`store.rs:60-78`) |
| GET | `/runs/{id}/orders` | blocking, loads full run | limit ≤ 2,000 (`server.rs:55-56`) |
| GET | `/runs/{id}/orders/{order_id}` | blocking, full run | – |
| GET | `/runs/{id}/fills`, `/equity` | blocking, full run | **unpaginated** |
| GET | `/runs/{id}/tape` | blocking, full run | limit ≤ 5,000 (`:57-58`) |
| GET | `/runs/{id}/timeline` | blocking, full run | max_events ≤ 50,000 (`docs/API.md:133`) |
| GET | `/runs/{id}/engine-check`, `/report`, `/export` | blocking, full run | export = whole run |
| POST | `/compare` | blocking, **no admission**, loads 2 full runs | – |
| POST | `/grids` | **GRID_SLOTS (1)** + blocking, synchronous; `grid_threads` workers | ≤ 100 cells (`bb-core compare.rs:15`) |

---

## 2. Functionality inventory against the target workspaces

Status values: **Existing**, **Partial** (works but misses required capability), **Nonfunctional**
(present but does not work as intended), **Missing**.

| Target workspace | Current implementation | Status | Gaps |
|---|---|---|---|
| Overview | Mission Control (`App.tsx:192`, `MissionControlPage.tsx`) | Existing (local) / **Nonfunctional (hosted)** | The hosted viewer returned 404 during the audit (§5.6) |
| Market Data Explorer | Datasets and quality (`DatasetsPage.tsx`), Market Replay heatmap and ladder (`ReplayPage.tsx`, `heatmap.ts`) | Partial | No ingest or upload through the API (CLI only, `main.rs:65`); no dataset deletion; vendor data cannot be hosted (`DEPLOYMENT.md:12-15`) |
| Order Book Lab | Replay depth ladder and heatmap; Replay Terminal with own orders overlaid (`ReplayTerminalPage.tsx`) | Partial | Read-only book view. No interactive queue-position sandbox; queue position exists only as model output per order |
| Strategy Lab | `LabPage.tsx` with four built-in kinds incl. scripted orders | Existing | Fixed strategy set; no user code (by design) |
| Simulation Workbench (async jobs, cancel) | `POST /api/runs` and `POST /api/grids` are synchronous (`server.rs:330-360, 679-694`) | **Missing** | No job ids, queue, progress, cancellation (`TODO.md:69-70`, `SECURITY.md:179-180`) or single-flight (§5.3). The client has no abort or timeout (`grep AbortController web/src/api/client.ts` → none) |
| Execution Forensics | Order Forensics (`OrderForensicsPage.tsx`), explanations (`bb-forensics explain.rs`), tape and timeline endpoints | Existing | Explanations are rule-based; inputs are not integrity-checked (§5.2) |
| Risk & Performance | Run overview metrics (`result.rs:195-255`), account charts | Partial | No risk-adjusted or per-period statistics (no Sharpe/Sortino/VaR anywhere: `grep -rni sharpe crates web/src` → none). No funding or margin (`LIMITATIONS.md:69-71`). `max_drawdown` depends on `sample_every_ticks`. The equity endpoint is unpaginated (501 MB measured, §5.5) |
| Experiment Comparison | Compare, Latency Lab demos, Sensitivity Grid | Partial, with **correctness defects** | Pairwise only. Alignment is misleading across intervals and timing-blind (§5.1). No multi-seed or statistical summary |
| Research Reports | `GET /runs/{id}/report` Markdown per run (`report.rs`) | Partial | Single run only; no comparison or experiment report; no versioned report artefact |
| System / Dataset Admin | `/health`, run deletion | Missing (mostly) | No users, API keys, quotas, storage usage, job admin, dataset admin or retention |

---

## 3. API capabilities and limits

- **Contract.** serde JSON of `bb-core` types (`docs/API.md:3-4`). Unknown fields are rejected
  (`config.rs:10-12`). Errors use the shape `{"error":{code,message}}` (`docs/API.md:13-18`).
- **Idempotent but not single-flight.** A stored run is returned when its full `config_sha256`
  matches (`pipeline.rs:205-210`). Concurrent identical requests all simulate (§5.3). The label
  sent with a repeat request is silently ignored: a request labelled `again` got back the stored
  label `concurrent-3` in this audit.
- **Synchronous.** Requests hold an HTTP connection for the whole simulation. A disconnect does
  not cancel the work (`SECURITY.md:179-180`).
- **Work bounds.** `interval_ns` must be in 1 ms … 1 h (`config.rs:261-263`). Latencies must be
  ≤ 60 s (`:265`). Ticks are limited to `MAX_TICKS_PER_RUN = 2,000,000` per run against the
  dataset span (`config.rs:268`, `pipeline.rs:100-112`). Grids are limited to 100 cells. There is
  **no output budget** for tape events, equity samples or bytes (`SECURITY.md:158`, "No output-size
  budget").
- **Response size.** `fills`, `equity` and `export` grow with the run (`SECURITY.md:181-182`).
  Measured: the equity of a 1.8 M-tick run is **501 MB in one response** (§5.5).
- **Access.** There is no authentication. Host and Origin are checked against allowlists of
  `host:port` loopback names. Allowed origins are derived as `http://` only (`access.rs:46-57`).
  **No CORS headers** are sent (`access.rs:14-15`). A Vercel-hosted frontend therefore cannot call
  this API cross-origin without code changes. **[INFERENCE]** A Render HTTPS host would also need
  `--allow-host` for a port-less `Host` header.

---

## 4. Architecture-level findings by area

### 4.1 Simulation correctness risks

1. **Comparison semantics** (target 1). These are the most serious scientific risk: the output
   reads like exact causal evidence but is not (§5.1).
2. **Run integrity** (target 2). Forensics, metrics and reports are served from unverified bytes.
3. **Platform-dependent results under one identity** (target 4).
4. **Mixed clocks in reported equity.** `EquitySample.position` and equity come from the
   exchange-side "truth" ledger, which includes fills the strategy has not yet been told about,
   but are marked at the *local* mid (`bb-sim lib.rs:235-239, 568-595`). When the local book is
   one-sided the previous mid is reused (`lib.rs:223-230`). This is documented as two ledgers in
   `PNL_ACCOUNTING.md` ("Two ledgers: true and known") and is not a bug, but it means `net_pnl`
   depends on the sampling grid. Comparing runs with different `interval_ns` or
   `sample_every_ticks` compares different marking times. In particular, the final sample is taken
   at `last_tick + interval` (`lib.rs:165-167`).
5. **No look-ahead found in the decision path.** Strategies see only the engine's local depth and
   the trades received (`lib.rs:205-220`; `strategy.rs:3-6`). Fill mids and markouts use the
   exchange-clock BBO of the whole dataset (`datasets.rs:27-28`), but only as post-hoc
   **derived** fields (`result.rs:78-83`); they never feed the strategy.
6. **Fragile queue probe.** The engine's private queue estimate is read by parsing its `Debug`
   output (`bb-sim queue_probe.rs:1, 42-45`), at 268.7 ns per read (`PERFORMANCE.md:79-81`). An
   engine upgrade can silently break the "modeled queue ahead" evidence shown in forensics.
7. **Golden coverage gaps.** Avellaneda-Stoikov, jittered latency, feed offset and drawdown halt
   are not pinned (`EXPERIMENT_REPRODUCIBILITY.md:143-146`), so a semantics change on those paths
   would not force a `SIMULATION_REVISION` bump.

### 4.2 Data storage limits (`RunStore` / `FsRunStore`)

- **Trait.** `put/get/summary/list/delete` (`store.rs:33-45`) is synchronous and filesystem-shaped
  (`get` returns a fully decoded `Arc<RunResult>`). It has no paging, no partial read (tape page,
  equity range) and no compare-and-set. A Postgres + Blob backend needs a different, async and
  artefact-oriented interface. Implementing the trait as it stands is not enough.
- **Layout.** `<root>/<run_id>/run.json` holds everything; `summary.json` is the visibility marker
  (`store.rs:91-95, 219-222`). The JSON is uncompressed.
- **Size.** About 357–465 B per tape event (`PERFORMANCE.md:398, 429`). Measured here: 31.0 MB for
  67,065 tape events at a 5 ms interval, and **918 MB** for 1,175,531 tape events plus 1.8 M
  equity samples at a 1 ms interval. Nothing caps a run's stored size.
- **Cache.** Up to 16 decoded runs, bounded by count rather than bytes (`store.rs:28-30`,
  `SECURITY.md:183-184`). One 918 MB run stayed resident at 554 MB working set / 757 MB private
  after loading (§5.5).
- **Listing.** `list()` opens every `summary.json` on every `GET /api/runs` (`store.rs:243-259`),
  so the cost is O(N) with no paging.
- **Overwrite semantics.** `put` replaces any run with the same 16-hex id (`store.rs:34-35`).
  `find_or_execute` checks the full hash before *reusing* a run, but a 64-bit prefix collision
  would *overwrite* another experiment's directory. **[INFERENCE]** The birthday bound is about
  2^32 runs, so this is negligible locally but should be designed out.
- **No integrity check, no retention or GC, no quotas, no per-user namespace.**
- **[INFERENCE]** On a free Render web service the local disk is ephemeral, so `FsRunStore` cannot
  be the system of record there. The decided Neon + Blob design addresses this.

### 4.3 Security concerns

The current local-tool model is documented in `SECURITY.md`. For the decided hosted design:

- There is no authentication or authorisation of any kind (`SECURITY.md:176-178`). The decided
  hashed, scoped API keys and the read-only demo workspace do not exist yet.
- `DELETE /api/runs/{id}` is unauthenticated, and the run store is global: any caller can delete
  any run.
- Unbounded CPU and memory per request beyond the tick cap (§5.5) means a single anonymous
  request can OOM a 512 MB host. That is a denial-of-service vector for a public API.
- Stored artefacts are trusted without verification (§5.2). Once artefacts live in Blob storage,
  integrity verification is a security control and not just hygiene.
- The `zip` 6 advisories are accepted with justification (`ci.yml:63-71`, `SECURITY.md:110-125`).
  They remain tied to the pinned engine.
- The CORS/Origin model is loopback-only (§3) and must be redesigned for Vercel → Render calls.

### 4.4 Performance bottlenecks

- **Full-run decode for any per-run read.** The first `GET /runs/{id}/orders?limit=1` on the
  918 MB run took **2.21 s** and raised the server's peak working set from 101 MB to 1,630 MB.
  The equity response took 1.06 s for 501 MB.
- **JSON everywhere.** Tape, fills and equity are stored and served as uncompressed JSON. A full
  `blackbox export-static` bundle is 337.9 MiB of JSON (`DEPLOYMENT.md:99-100`).
- **Grid threads equal the host's CPUs** (`main.rs:285-287, 1077`) and are independent of
  `RUN_SLOTS`.
- **Non-admitted heavy endpoints** (compare, export, timeline, report) run on the tokio blocking
  pool with no concurrency cap. The runtime is built with defaults (`main.rs:1098-1101`).
  **[INFERENCE]** That means up to 512 blocking threads.
- **Simulation cost is small relative to I/O.** The 1.8 M-tick run simulated in 2,243 ms of the
  4.05 s process time. Serialisation and storage dominate.

### 4.5 Deployment architecture (current)

| Shape | Status | Evidence |
|---|---|---|
| Local binary (`blackbox serve` / `demo`) | Works; loopback, no auth | `main.rs:1071-1122`; reproduced in this audit |
| Docker image | Built and smoke-tested in CI; binds `0.0.0.0`, no auth | `Dockerfile`, `ci.yml:73-86` |
| Static viewer on Vercel | Built **manually** from a locally exported, git-ignored bundle and deployed with `vercel deploy --prod` | `DEPLOYMENT.md:76-97`; `copy-static-data.mjs` exits 1 without `data/static-data`. **404 during the audit** (§5.6) |
| Hosted simulator | Does not exist (`DEPLOYMENT.md:110-115`) | – |

### 4.6 Technical debt

- `web/src/api/types.ts` (1,034 lines) is a hand-maintained mirror of `bb-core` (`types.ts:1-12`).
  Nothing checks it against the Rust types, so drift is caught only by E2E.
- `export_static.rs` (897 lines) re-materialises every API response the pages need. A test checks
  it against the server output, but any new endpoint needs a second implementation.
- `demos.rs` duplicates the compare config-diff walk ("Mirrors bb_forensics::compare_runs",
  `demos.rs:16-17`).
- Queue probe via `Debug` parsing (§4.1.6). Five upstream-defect workarounds are tied to
  `hftbacktest =0.9.4` (`EXECUTION_MODEL.md:58-150`).
- `RunSummary` is stored twice (in `run.json` and `summary.json`), and the two copies can diverge
  undetected (§5.2 showed the summary and the run disagreeing on `net_pnl`).
- The `RunStore` trait is sync and whole-run (§4.2).
- `DatasetCache::get_or_load` can load the same dataset twice under a race, by design
  (`datasets.rs:125-127`). That is a transient memory doubling, relevant at 512 MB.
- The property tests use proptest's random seed (`TESTING.md:117-118`), so failures are not
  reproducible from CI logs without the printed seed.

### 4.7 Missing scientific validation

- **Model validity.** No queue model is calibrated or validated against L3 data or real fills
  (`TESTING.md:127-128`, `LIMITATIONS.md:23-33`). The latency models are uncalibrated
  (`LIMITATIONS.md:61-62`).
- **Comparison statistics.** Every comparison is a single deterministic path. There are no
  multi-seed ensembles, no bootstrap or confidence intervals, and no significance statement. The
  grid "is not a prediction" (`LIMITATIONS.md:96-97`), but it offers no dispersion measure either.
- **Comparison validity.** There is no eligibility contract and no timing-aware equivalence
  (§5.1).
- **Cross-platform result equality** for transcendental paths is not guaranteed and not tracked
  per run (§5.4).
- **No independent reference reconciliation per run.** The engine cross-check compares the
  ledger with the engine's own account (`lib.rs:599-636`). Parity with an unwrapped engine run of
  the same request stream exists only as a manual benchmark (`PERFORMANCE.md:68-73`), not as a
  per-run or CI check.
- **Data-quality impact on results is reported, not propagated.** For example, 659 of 2,212 Tardis
  trades print outside the reconstructed BBO (`LIMITATIONS.md:55-60`). Runs on such data are not
  flagged at the run level.

---

## 5. Mandatory investigations: evidence

### 5.1 Target 1: `bb-forensics/src/compare.rs` alignment. **CONFIRMED**

**Code.**

- `align` (`compare.rs:289-343`) merges the two `orders` vectors by `order_id` alone.
- `order_id = (tick_no << 8) | slot` (`bb-core strategy.rs:25-27`). Its doc claim that the same
  id names "the same decision" (`strategy.rs:10-14`) holds only when both runs share dataset
  origin and `interval_ns`. A tick number means a different time under a different interval.
- `pair` (`compare.rs:256-287`) classifies `Identical` from side, price tick, qty and tif, then
  outcome, `filled_qty` and `avg_fill_tick`. It ignores `decision_ts`, `exch_arrival_ts`,
  `first_fill_ts`/`last_fill_ts`, `terminal_ts`, `queue_ahead_at_entry`, `fill_count`,
  `maker_qty`/`taker_qty` and `fees`. A maker fill in one run and a taker fill at the same average
  tick in the other would be "Identical".
- `OrderPair.decision_ts` comes from A when both runs have the order (`compare.rs:257, 282`). The
  first-divergence text prints only that time (`compare.rs:393-406`).
- **Eligibility.** `same_dataset` and `same_strategy` are computed (`compare.rs:375-376`) but only
  add a note (`:380-382`). Alignment, metric deltas and "first divergence" are always produced.
  Neither the server (`server.rs:656-677`) nor the UI (`ComparePage.tsx:239-251`, warnings only)
  refuses. The instrument and tick size are not checked.
- **Exactness claims.** The note says "All aligned decisions and outcomes before it were
  identical, as derived from the tapes" (`compare.rs:397`). The UI says "Every aligned pair before
  it is identical in both runs" (`ComparePage.tsx:665-666`). `LIMITATIONS.md:90-92` says the
  first difference "is identified exactly".

**Reproduction.** Every run below is the `maker_baseline` preset on `synthetic-demo` with exactly
one change. Each pair was compared with `POST /api/compare`, and the hidden differences were
recomputed from `/export` using the same classification rule.

| Case | Change vs A | API alignment (ident / same-dec-diff-out / diff-dec / onlyA / onlyB) | Hidden in "identical" pairs | Verdict |
|---|---|---|---|---|
| Changed decision times (interval) | `interval_ns` 100 → 200 ms | 13 / 9 / 975 / 1063 / 150 | **all 13 "identical" pairs were decided at different times**: e.g. order 0 at 0.1 s vs 0.2 s; order 3584 at 1.5 s vs 3.0 s. First divergence printed as "tick 0, order 1, decision time t=0.100s"; B's order 1 was at 0.200 s | Misleading pairing. Ids are shifted, so decisions at the same time get different ids. Only a "strategy differs" note |
| Changed arrival times | entry latency 1 → 3 ms | 1923 / 36 / 79 / 22 / 10 | `exch_arrival_ts` differs in all 1,923, `queue_ahead_at_entry` in 161, `first_fill_ts` in 15, `terminal_ts` in 773 | Outcome-identical but timing-different pairs are counted as "identical" |
| Identical fills under different causal histories | queue model `power_prob3` → `risk_adverse` | 2044 / 3 / 9 / 4 / 0 | 7 "identical" pairs filled at different times. **5 of them precede the reported first divergence** (tick 671, t = 67.2 s). E.g. order 9472: same price 299991, same queue at entry 0.959, filled at 3.831 s in A vs 3.860 s in B | The "all … before it were identical" statement is false for fill timing |
| Response latency | response 1 → 3 ms | 2039 / 0 / 7 / 14 / 1 | `first_response_ts` differs in all 2,039 | Same pattern |
| Missing orders | (from the rows above) | onlyA/onlyB counted; a first divergence can be `only_a` (response-latency case: order 32256) | – | Handled correctly |
| Different dataset | `dataset_id` → `synth-b` (other seed) | 19 / 17 / 1728 / 296 / 293 | 19 "identical" pairs across unrelated markets; a first divergence and metric deltas were produced | No refusal; note only |
| Different strategy kind | imbalance taker vs touch maker | 0 / 0 / 94 / 1966 / 20 | – | No refusal; 94 "different decision" pairs between unrelated strategies |
| Shifted ids | covered by the interval case | – | Decisions at the same time get different ids; unrelated decisions share ids | Misleading |
| Divergent fills | by code: `fill_count`, maker/taker split and fees are not compared | – | Not observed in these runs (0 such pairs) | Code-level risk |

Never "equivalent": the "same experiment" note requires zero config differences, the same dataset
and the same strategy (`compare.rs:432-434`). No differing pair was ever labelled the same
experiment.

### 5.2 Target 2: `FsRunStore::get()` integrity. **CONFIRMED**

**Code.** `get` (`store.rs:213-236`) checks that `summary.json` exists, decodes `run.json`, and
checks only `manifest.run_id == run_id`. It never recomputes `result_sha256` or `config_sha256`,
and never cross-checks `summary.json`. The round-trip property is asserted only in a test
(`tests/integration.rs:260`). `EXPERIMENT_REPRODUCIBILITY.md:70-71` describes the property but no
runtime check uses it.

**Reproduction.**

1. Run `d9878148374ecdf1` was used. Its `run.json` was edited in place, keeping valid JSON:
   `fills[0].price_tick` 300000 → 300050 and `metrics.net_pnl` −125.712… → 123456.0.
   `manifest.result_sha256` was left as `3a0136306165bed3…`.
2. The running server still served the original values from its in-memory cache
   (`/export` returned `net_pnl` −125.71, price 300000).
3. A fresh-process `blackbox explain d9878148374ecdf1 257` exited 0. It explains from the
   untampered tape ("Filled as maker at 30000.0"), while `fills` now says 30005.0. The
   inconsistency is not detected.
4. After a server restart, `/export` returned `net_pnl` **123456.0**, fill price **300050** and the
   unchanged `result_sha256` `3a0136306165bed3`. `/orders/257` returned the fill at 300050.
5. `POST /api/runs` with the same config returned the stored `summary.json` (`net_pnl`
   −125.71…). Summary and run now disagree, silently.

Even a recompute of `result_sha256` would not cover `orders` and `metrics`, which are outside the
hash (`EXPERIMENT_REPRODUCIBILITY.md:39-41`). They must be re-derived or hashed separately.

### 5.3 Target 3: `find_or_execute()` single-flight. **CONFIRMED** (duplicate work; no corruption observed)

**Code.** The check `store.summary` (`pipeline.rs:205-210`) and the action `execute` + `put`
(`:213-222`) are not atomic. There is no in-flight map and no lock. `put` writes through temp
files unique per pid and sequence (`store.rs:161-165`) and renames over the target. The last
writer wins, including its label.

**Reproduction.**

- New config (`maker_baseline`, `seed` 424242) with 4 concurrent `POST /api/runs` carrying labels
  `concurrent-0..3`. All returned run `1fa4285f84c2f51e` with the same `result_sha256`
  (`57f055c5…`). Each response carried its own label and its own `wall_time_ms` (35/34/32/33), so
  **each request simulated**. The stored run afterwards had label `concurrent-3`.
- New config with a 5 ms interval (67,065 tape events, 31 MB `run.json`) and 8 concurrent
  requests: requests 0–3 simulated (labels `c0..c3`, `wall_time_ms` 102/102/103/102); requests
  4–7, admitted after slots freed, returned the stored run labelled `c3`. So duplication is
  bounded only by `RUN_SLOTS = 4`.
- No HTTP errors or torn files were observed on Windows. **[INFERENCE]** Grid cells
  (`pipeline.rs:238`) and CLI processes sharing a data dir race in the same way, and the `put`
  calls of different processes cannot collide on temp names (pid in the name).

### 5.4 Target 4: cross-platform determinism. **PARTIAL**

**What exists.**

- `RunManifest.platform` and `profile` are recorded (`pipeline.rs:154-155`) and explicitly
  excluded from identity (`EXPERIMENT_REPRODUCIBILITY.md:31`; `run_identity`, `pipeline.rs:56-64`).
  The contract is "equal `config_sha256` *on the same platform* ⇒ equal `result_sha256`"
  (`result.rs:52-54`).
- `golden.rs` pins `maker_slow` per OS (`golden.rs:62-68`: Windows `662ad3b6…`, Linux
  `9a809e78…`). It requires the libm-free `maker_conservative_queue` to be equal everywhere
  (`:69-74`, `:108-117`). Other OSes with non-portable entries are skipped (`:95-106`).

**Reproduction (Windows).** `blackbox synth --id golden --duration-s 30` gave dataset sha
`6960c01e…`, equal to the pin at `golden.rs:23`. `run --preset maker_slow` gave run id
`5540904b09f29397` and result `662ad3b639d5…`, which is the Windows pin. `maker_conservative_queue`
gave `20bc0827…` and `maker_baseline` gave `1f3f67f5…`, both equal to their pins. So one run id
maps to two result hashes depending on OS.

**Policy gaps.**

1. CI runs only `ubuntu-24.04` (`ci.yml:13, 37, 53, 75, 89`). The Windows pins are checked only on
   the developer machine, so the claim "checked on Windows and Linux CI" (`TODO.md:28-29`) is
   ambiguous at best.
2. A run computed on one platform is returned by `find_or_execute` on another, because the
   platform is not in the identity. Its result hash then differs from what that platform would
   compute.
3. The local static bundle records `"platform":"windows-x86_64"` for all 11 runs. This was checked
   by grepping `data/static-data/3571ceb90f93063c/api/runs.json.gz`, which records the platform
   only and contains no vendor data. A Linux Render API serving the same run ids could show
   different results for `power_prob*` and `log_prob*` configs **[INFERENCE: observed difference
   is only for `maker_slow` on the golden dataset]**.
4. There is no designated canonical platform for published results.

### 5.5 Target 5: resource isolation. **CONFIRMED**

**Code.**

- `RUN_SLOTS = 4` and `GRID_SLOTS = 1` are independent semaphores (`server.rs:206-212`).
- A grid runs `grid_threads = available_parallelism()` workers (`main.rs:285-287, 1077`;
  `pipeline.rs:276-296`). That is 24 on this machine (`PERFORMANCE.md:37`).
- Compare, export, timeline and report have no admission (`server.rs:661`). The decoded-run cache
  holds 16 entries regardless of size (`store.rs:30`).
- The comment "this caps the server's CPU and memory" (`server.rs:206-208`) is true for count only.

**Measurements** (release binary; `PeakWorkingSetSize` / `PeakPagefileUsage` = peak private bytes):

| Workload | Ticks | Tape events | Sim wall | Peak WS | Peak private | `run.json` |
|---|---:|---:|---:|---:|---:|---:|
| maker, 120 s synthetic, 1 ms interval (CLI) | 120,001 | 76,696 | 133 ms | 110 MB | 145 MB | – |
| maker, 1800 s synthetic, 1 ms interval (CLI) | 1,800,000 (90 % of cap) | 1,175,531 | 2,243 ms | **1,578 MB** | **2,281 MB** | **918 MB** |
| Server: first `GET /orders?limit=1` of that run | – | – | 2.21 s | 1,630 MB (from 101) | 1,858 MB | resident after: 554 MB WS / 757 MB private |
| Server: `GET /equity` of that run | – | – | 1.06 s | – | – | **501 MB response** |

**Worst case from config limits.**

- Simultaneous simulations = 4 (runs) + `grid_threads` (one grid): 28 here.
- **[INFERENCE]** On Linux, Rust's `available_parallelism` honours cgroup CPU quotas, so a 0.1 CPU
  Render instance would get 1 grid thread, giving 5 concurrent simulations.
- **[INFERENCE, linear extrapolation from the measured 2.28 GB per near-cap run]** About 5 × 2.3 ≈
  11 GB worst case on Render, against **512 MB**. A **single** legal near-cap run exceeds the host
  by ~4.5×. Even the measured 120 s / 1 ms run (145 MB) leaves room for only about 3.
- Each concurrent `compare` adds two full decoded runs on top, and the cache can hold 16.

Conclusion: the admission controls bound concurrency, not memory. The tick budget (2 M) is about
an order of magnitude too generous for the target host.

### 5.6 Target 6: static vs live build coverage. **CONFIRMED gap**

| Build | Command | CI coverage |
|---|---|---|
| Live console | `npm run build` (`package.json:9`) | `web` job (`ci.yml:48-49`); `e2e` job builds it and runs Playwright against `blackbox demo` (`ci.yml:101-111`; `playwright.config.ts` webServer) |
| Static viewer | `npm run build:static` = `tsc -b && vite build --mode static && node scripts/copy-static-data.mjs` (`package.json:10`) | **None.** No CI step runs `--mode static` or `export-static`; `grep static web/e2e/*.ts` → no match. The exporter is covered only by Rust unit tests (`export_static.rs`: `recorded_datasets_are_refused_before_anything_runs_or_is_written`, `synthetic_export_writes_the_documented_bundle`) |
| Vercel | `web/vercel.json` sets `buildCommand: npm run build:static` | Cannot succeed on Vercel builders: `copy-static-data.mjs` exits 1 without `../data/static-data`, which is git-ignored (`.gitignore` `/data/`). The documented path is a manual prebuilt deploy with `buildCommand` stripped (`DEPLOYMENT.md:86-97`) |

`tsc -b` type-checks static-mode code in both builds. What is untested is the static runtime
path (`client.ts:244-400`: marker fetch, bundle-root mapping, "not in the precomputed bundle"
handling) and the bundle-to-client URL contract.

**Observed during the audit:** `https://blackbox-hft.vercel.app/` and
`/static-data/blackbox-static.json` both returned **HTTP 404 "NOT_FOUND"** (curl,
2026-11-13 22:51 UTC). README (`README.md:19`) and `DEPLOYMENT.md:70` state it is live. Cause
unknown; no cloud account was touched.

### 5.7 Target 7: timestamp precision. **PARTIAL**

- **Convention.** Serialized timestamps are `RelNs: i64` relative to `origin_ns`, and `origin_ns`
  is a decimal string (`bb-core lib.rs:8-12`; `result.rs:27`; `dataset.rs:199`). TypeScript maps
  `u64/i64/f64 → number` (`types.ts:9`, `RelNs = number` at `types.ts:22-23`).
- **Range.** 2^53 ns ≈ 104.2 days. Maximum relative values observed:
  - 1.8 × 10^12 ns (1800 s synthetic, this audit)
  - about 6.0 × 10^11 ns (601.1 s Tardis window, `PERFORMANCE.md:122-123`)
  - 3.0 × 10^10 ns (largest markout horizon, `result.rs:11-12`)

  That is about 3,700× headroom. There is **no server-side guard** that a dataset span or a query
  bound is below 2^53. **[INFERENCE]** It is unreachable through the current CLI: synthetic data
  is capped at 20 M events at a 2.5 ms default gap, and Tardis ingest takes one file pair.
- **Absolute ns.** The web converts with BigInt only (`fmt.ts:174-186`). One absolute value is
  serialized as a JSON number: `DatasetManifest.generator.origin_ns = 1700000000000000000`, found
  with a jq scan of the synthetic `dataset.json`. The Tardis manifest has 0 numbers above 2^53.
  The web types it as `generator: JsonValue` (`types.ts:294`) and renders it
  (`DatasetsPage.tsx:362-365`). This particular value is exactly representable (a multiple of 256),
  so nothing is lost today. Any other origin would round.
- **Non-timestamp u64.** `RunConfig.seed: u64` → `seed: number` (`types.ts:86`). The Lab caps
  input at `Number.MAX_SAFE_INTEGER` (`fields.ts:134, 859-861`), but the API accepts any u64.
  Reproduced:
  1. A config with `seed` 9007199254740993 was stored as `…993` (run `3dd36cf0da5c5d77`).
  2. Node's `JSON.parse` yields `…992`.
  3. Resubmitting `…992` gives a different run, `84cc6d23a528829f`.

  So the UI can display a seed that does not reproduce the run.
- **Other u64/i64 fields.** `order_id` (≤ 2 M × 256 ≈ 5.1 × 10^8), `seq`, `tick_no`, counts,
  `created_unix_ms` and `local_ts_shift_ns` are all far below 2^53 in practice.
- **Affected types and files.**
  - Every `RelNs`/`i64` field: `FlightEvent.ts`, `FillRecord.ts`, `EquitySample.ts`, all
    `OrderRecord` `*_ts`, `ReplayWindow`/`market.rs` `ts`/`step_ns`, `DatasetManifest.start`/`end`,
    and the `Explanation` latency fields (`explain.rs:72-100`), mirrored in `web/src/api/types.ts`.
  - Consumers: `features/forensics/timeline.ts:98-115`, `QueueChart.tsx:51-132`,
    `TimeSeriesChart.tsx:37-54`, `ReplayTerminalPage.tsx`, `heatmap.ts`, `gridModel.ts:173`
    (already uses `isSafeInteger`) and `OrderForensicsPage.tsx:40`.
  - Hazards: `generator.origin_ns` (absolute) and `seed` (u64).

### 5.8 Target 8: documentation accuracy. **CONFIRMED**

| # | Claim | Location | Reality |
|---|---|---|---|
| 1 | Hosted viewer and Docker image are "planned work" | `ARCHITECTURE.md:158-159` | Both exist (`Dockerfile`, `ci.yml:73-86`, `export_static.rs`; `TODO.md:56-61` says M7 done) |
| 2 | Static viewer "was not present in the checkout … design requirement, not a verified property" | `SECURITY.md:127-131` | It is present (`export_static.rs`, `client.ts:244`); its static-only properties are still not verified by any test |
| 3 | `golden.rs` has 1 test | `TESTING.md:34` | 2 tests (`golden.rs:108, 119`) |
| 4 | CI runs tests, a determinism check and the web build | `TESTING.md:36-38` | Also `audit`, `docker` and Playwright `e2e` jobs (`ci.yml:51-118`) |
| 5 | "no component or browser end-to-end tests yet (Playwright is planned)" | `TESTING.md:129-130` | Playwright E2E exists and runs in CI (`web/e2e/research-story.spec.ts`, `ci.yml:88-111`) |
| 6 | `config_sha256` covers "(contract version, engine name+version, dataset sha256, config)" | `bb-core result.rs:49-50` | It also covers `SIMULATION_REVISION` (`pipeline.rs:57-64`; correctly stated in `EXPERIMENT_REPRODUCIBILITY.md:14-15`) |
| 7 | "golden hashes pinned and checked on Windows and Linux CI" | `TODO.md:28-29` | CI is Linux-only. Windows is checked only locally (reproduced in §5.4) |
| 8 | `POST /api/runs` "is synchronous and idempotent" | `docs/API.md:48-49` | Idempotent in result but not single-flight. A repeat request's label is ignored (§5.3) |
| 9 | Alignment by decision point; "first point at which they differ is identified exactly" | `LIMITATIONS.md:90-92` | Timing is excluded from "identical", and alignment is meaningless across intervals (§5.1) |
| 10 | "All aligned decisions and outcomes before it were identical" / "Every aligned pair before it is identical" | `compare.rs:397`; `ComparePage.tsx:665-666` | False for fill timing: 5 counter-examples in the queue-model case (§5.1) |
| 11 | Same `order_id` names "the same decision" because the tick grid does not depend on execution assumptions | `bb-core strategy.rs:10-14` | True only for an equal interval and origin. The interval is a strategy parameter, not an execution assumption, and compare does not enforce it |
| 12 | Admission "caps the server's CPU and memory" | `server.rs:206-208`; `SECURITY.md:77-78` | It caps the count only. One legal run peaks at 2.28 GB (§5.5) |
| 13 | Live viewer at `https://blackbox-hft.vercel.app` | `README.md:19`; `DEPLOYMENT.md:70`; `TODO.md:59` | HTTP 404 at audit time (§5.6) |

Not stale, but implies more than is enforced: "a reloaded run still hashes to its own
`result_sha256`" (`EXPERIMENT_REPRODUCIBILITY.md:70-71`) is true of the serializer, but nothing
checks it at load (§5.2). "Deleting `data/runs` only removes cached results: every run is
reproducible" (`DEPLOYMENT.md:39-40`) holds only on the same platform (§5.4).

---

## 6. Proposed target architecture (high level)

This is consistent with the decided zero-cost deployment. Detailed design belongs to later phases.

```
Vercel (static SPA, live + demo modes)
   │  HTTPS, CORS allowlist = Vercel origins, API key header (hashed, scoped)
   ▼
Render free web service (one process, 0.1 CPU / 512 MB, spins down after 15 min idle)
   ├─ axum API: auth → workspace scoping → read paths stream/paginate from Blob
   └─ in-process worker pool (size 1 on this host), same code as `blackbox worker`
        ├─ claims jobs from Postgres (lease + heartbeat; reclaim expired leases on wake)
        ├─ runs simulate() with cooperative cancel + progress + hard output budgets
        └─ writes artefacts to Blob, then commits run row (hashes, sizes, platform) in one txn
Neon Postgres: workspaces, api_keys (hash, scopes), jobs (state machine, unique key),
               runs (identity, result hashes, platform, summary metrics), datasets (manifest)
Vercel Blob (private): content-addressed, compressed, chunked artefacts
               (tape / fills / equity / orders), verified by sha256 on read
```

Principles that answer the findings:

1. **Single-flight.** A unique `(config_sha256, platform)` key on jobs and runs, enforced with
   `INSERT … ON CONFLICT`. A duplicate request returns the existing job.
2. **Integrity.** Verify artefact hashes on every read. Hash the derived orders and metrics as
   well, or recompute them.
3. **Budgets.** Reject at admission from predicted ticks, and abort at runtime on tape events,
   equity samples, bytes and RSS. The defaults must fit 512 MB with headroom, with a single worker
   at a time on this host.
4. **Platform.** Make platform part of the stored result key. Designate `linux-x86_64` (the Render
   build) as canonical for published results. Mark any result from another platform as such.
5. **Comparison contract.** An explicit eligibility check (same dataset hash, instrument,
   interval, origin and strategy kind; otherwise refuse or downgrade to "unaligned metrics only")
   and timing-aware classes.
6. **Paged, streamed reads.** Paginate equity, fills and timeline. Serve summary-only listings
   from Postgres.
7. **Precision.** Guard spans below 2^53 ns. Send `seed` and absolute ns as strings or cap them.
8. **Demo workspace.** Public, read-only, synthetic only, precomputed on the canonical platform.
   This replaces the hand-deployed static bundle, or that bundle is generated in CI from the same
   artefacts.

---

## 7. Ranked backlog

Ranking is by research-correctness impact first, then deployability on the decided host.

| Rank | Item | Why | Acceptance test(s) |
|---|---|---|---|
| 1 | **Comparison eligibility and timing-aware alignment** | Wrong scientific conclusions today (§5.1) | AT-1, AT-2 |
| 2 | **Hard work/output budgets sized for 512 MB** (predicted at admission, enforced at runtime) | One legal request OOMs the host (§5.5) | AT-5 |
| 3 | **Durable job queue with single-flight, progress and cancel; `blackbox worker`** | Workbench is missing; duplicate simulations (§5.3); Render spin-down | AT-4, AT-6 |
| 4 | **Artefact integrity verification** (result hash + derived data) and a Postgres/Blob store behind an async, paged store interface | Tampered or corrupt runs are served (§5.2); FS store is not viable on Render (§4.2) | AT-3, AT-10 |
| 5 | **AuthN/Z: hashed scoped API keys, public read-only demo workspace, CORS for Vercel origins** | No auth; loopback-only access model (§3, §4.3) | AT-11 |
| 6 | **Platform policy** (platform in the result key, canonical Linux, Windows CI job or demoted Windows pins) | Same run id, different results (§5.4) | AT-7 |
| 7 | **Paginate equity, fills and timeline; byte-bounded cache; summary listing from DB** | 501 MB responses; 16-run count cache (§5.5, §4.2) | AT-10 |
| 8 | **Static/demo build in CI + E2E against the demo build; restore or retire the Vercel URL** | Static path untested; live URL 404 (§5.6) | AT-8 |
| 9 | **Precision guards** (span < 2^53, seed ≤ 2^53−1 or string, `generator.origin_ns` as string) | Non-reproducible seeds via the UI (§5.7) | AT-9 |
| 10 | **Scientific validation**: multi-seed ensembles with dispersion; run-level data-quality flags; per-run reference reconciliation against the unwrapped engine in CI on synthetic data; golden coverage for AS, jitter, feed offset and halt | §4.1.7, §4.7 | AT-12, AT-13 |
| 11 | **Contract drift**: generate TS types from `bb-core` (or a schema check in CI); remove the `demos.rs` diff duplication | §4.6 | AT-14 |
| 12 | **Documentation corrections** (the 13 items in §5.8) | Stale or overclaiming docs | AT-15 |

---

## 8. Concrete acceptance tests

Each test is written to be automatable. "Run X" means the `maker_baseline` preset on the
synthetic dataset with the stated change.

- **AT-1 Compare eligibility.**
  - Comparing A with A′ (`interval_ns` 100 → 200 ms) must not produce per-order alignment.
    The response is either refused (422) or flagged `alignment: null` with a reason naming
    `strategy.interval_ns`.
  - The same applies to different `events_sha256`, a different `instrument.tick_size` and a
    different strategy kind.
  - Metric deltas may still be returned, labelled "not like-for-like".
  - A unit test in `bb-forensics` covers each refusal reason.
- **AT-2 Timing-aware classes.**
  - Using the queue-model case of §5.1 (or a `testkit` tape with equal outcome and price but
    `first_fill_ts` 3,831,084,401 vs 3,859,654,055), the pair must not be classed `identical`.
    A class such as `same_outcome_different_timing` or a timing-delta field is required.
  - The first-divergence statement must hold when every pair before it is checked on timing.
  - `exch_arrival_ts`/`first_fill_ts` deltas are reported per pair and summarised (count, min,
    median, max).
- **AT-3 Integrity on read.**
  - Store a run, then flip one byte of a fill price in the stored artefact. Every read endpoint
    for that run returns an `integrity_failed` error (5xx with a stable code). The run is not
    used for idempotent reuse, and a new identical request re-simulates or fails loudly.
  - The same applies to a tampered `metrics.net_pnl` (derived data covered).
- **AT-4 Single-flight.**
  - 8 concurrent identical submissions create exactly 1 job row and exactly 1 simulation. Measure
    it with a counter or a `simulations_started` metric.
  - All 8 responses carry the same job id. The label policy is explicit and tested: either the
    first label wins and is returned, or labels are stored per request.
- **AT-5 Budgets.**
  - A config whose predicted ticks or output exceed the host budget is rejected before
    simulation. The 1800 s / 1 ms case of §5.5 must be rejected under the hosted profile.
  - A run that exceeds the tape-event or byte budget mid-way aborts with `budget_exceeded` and
    writes no artefact.
  - A Linux CI test asserts that the worker's peak RSS (`VmHWM`) for the largest admitted
    synthetic job stays under the configured limit (e.g. 300 MB).
- **AT-6 Cancel and progress.**
  - A submitted job reports monotonically increasing progress (ticks done / predicted).
  - `POST /jobs/{id}/cancel` moves it to `canceled` within one progress interval, the worker slot
    is freed, and no run row or Blob artefact remains.
  - A job whose lease expires (simulated worker death or spin-down) is re-queued exactly once.
- **AT-7 Platform policy.**
  - Every stored result records its platform. A result from a non-canonical platform is never
    returned as the canonical result for the same identity.
  - CI runs `golden.rs` on `windows-latest` and `ubuntu-24.04`, or Windows pins are removed and
    documented as non-canonical.
- **AT-8 Static/demo build in CI.**
  - A CI job runs `blackbox export-static` (synthetic) and `npm run build:static`, serves `dist`,
    and runs a Playwright subset: Mission Control, run overview, order forensics, compare. No
    console errors and no failed requests.
  - A deploy smoke test checks `GET /` returns 200 on the production URL.
- **AT-9 Precision.**
  - `POST /runs` with `seed` 9007199254740993 is rejected (422), or the seed round-trips
    losslessly through the web client (string encoding).
  - The dataset manifest contains no JSON number above 2^53−1. A jq check runs in CI over the
    synthetic manifest.
  - Ingest or generation of a dataset spanning ≥ 2^53 ns is refused.
- **AT-10 Paged reads.**
  - `GET /runs/{id}/equity?offset&limit` and `/fills?offset&limit` are bounded (e.g. ≤ 5 MB per
    response) for the §5.5 run.
  - Listing 10,000 runs reads only database rows (no artefact reads), in under a set time on the
    hosted profile.
  - The server's resident memory after reading pages of the largest run stays under budget.
- **AT-11 Auth.**
  - Without a key: `POST /runs` → 401.
  - With a read-only demo key: `POST /runs` → 403 and `DELETE /runs/{id}` → 403. Demo reads
    succeed.
  - With a research key, its runs are invisible to other workspaces.
  - Keys are stored hashed: a database dump contains no plaintext key.
  - A CORS preflight from the Vercel origin succeeds; one from any other origin fails.
- **AT-12 Ensemble statistics.** The experiment API accepts N seeds (jittered latency) and returns
  per-metric mean, dispersion and per-seed run ids. With constant latency all seeds collapse to
  one result, and this is stated.
- **AT-13 Golden coverage.**
  - Golden pins exist for Avellaneda-Stoikov, jitter, feed offset and drawdown halt.
  - A CI check runs the unwrapped engine on the same request stream as one adapter run and asserts
    equal trade counts, as the benchmark does today (`PERFORMANCE.md:68-73`).
- **AT-14 Contract drift.** CI fails when a `bb-core` serde type changes without the TS types being
  regenerated or updated (schema snapshot diff).
- **AT-15 Docs.** Each row of §5.8 is corrected. The doc claims of counts and CI jobs are checked
  by a script, or deleted in favour of links to `ci.yml`.
