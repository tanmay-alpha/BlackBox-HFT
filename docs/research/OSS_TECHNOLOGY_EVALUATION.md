# Open-source technology evaluation (Phase 0)

Scope: frameworks, databases, libraries and services that could shape BlackBox's next phase:
research lab with a hosted API, a job queue and durable run artefacts. Engine and data-source
sources (hftbacktest, Tardis, Binance, Databento DBN, HFTENGINE, market-maker-rs) are covered in
[../OPEN_SOURCE_RESEARCH.md](../OPEN_SOURCE_RESEARCH.md) and are not repeated here. Where that
document already covers a project (NautilusTrader's licence obligations, ABIDES' licence, Arrow,
DuckDB), this one only adds what is new: maintenance status today, deployment cost and a decision.

All observations are from **2026-11-14**. Evidence types:

This is the pre-implementation technology survey, not a current feature or limits inventory.
For the delivered stack see [../engineering/API_CONTRACT.md](../engineering/API_CONTRACT.md),
[../engineering/PROJECT_STATE.md](../engineering/PROJECT_STATE.md) and
[../engineering/FINAL_AUDIT.md §7](../engineering/FINAL_AUDIT.md#7-fixes-after-the-audit).
The application now uses `/api/v1`, `SweepSpec` seed ensembles, a 5-unit active quota (each sweep
one unit) and a 300 Blob-upload-attempt/workspace/month default; old local-cache observations
and proposed paths below describe the survey baseline, not today's server.

- **gh**: `gh api repos/<owner>/<repo>` (stars, `pushed_at`, `license.spdx_id`, `archived`),
  `gh api repos/<o>/<r>/commits?per_page=1` (last commit on the default branch),
  `gh api repos/<o>/<r>/releases/latest`, `gh api repos/<o>/<r>/license` (licence file text).
- **crates.io**: `https://crates.io/api/v1/crates/<name>` (max stable version, date, licence field
  of the newest version) and `/<name>/<version>/dependencies`.
- **npm**: `https://registry.npmjs.org/<pkg>/latest`.
- **docs**: official documentation pages, URL given at the point of use.
- **measured**: a command run for this document; the command is described next to the number.
- **[INFERENCE]**: a conclusion not directly observed.

## 1. Deployment constraints these decisions are made against

The user has fixed the hosting target to zero cost (decision relayed by Main on 2026-11-14): **one**
free Render web service running the axum API plus an in-process worker pool consuming a
Postgres-backed job queue (also runnable later as a separate `blackbox worker` process), Neon free
Postgres provisioned through the Vercel Marketplace, a private Vercel Blob store for run artefacts,
a Vercel-hosted frontend, auth = public read-only demo workspace + hashed scoped API keys, **no
Redis**.

| Constraint | Value | Source |
|---|---|---|
| Render free web service | 512 MB RAM, "Less than 1 CPU", $0 | <https://render.com/pricing> (free row) |
| Render free spin-down | "Render spins down a Free web service that goes 15 minutes without receiving any inbound traffic"; spin-up "takes about one minute" | <https://render.com/docs/free> |
| Render free filesystem | ephemeral; changes "lost every time the service redeploys, restarts, or spins down"; free services cannot attach a persistent disk | <https://render.com/docs/free> |
| Render free hours | 750 free instance hours per month; when exhausted, free services are suspended until next month | <https://render.com/docs/free> |
| Render client IP | traffic passes through Cloudflare and Render's load balancers; "read the x-forwarded-for header" | <https://render.com/articles/how-render-handles-ddos-attacks> |
| Neon free compute | 100 CU-hours/project/month ("enough to run a 0.25 CU compute … for 400 hours/month"); scale to zero after 5 min inactivity, cannot be disabled on Free | <https://neon.com/docs/introduction/plans>, <https://neon.com/docs/introduction/scale-to-zero> |
| Neon free storage / egress | 1 GB Postgres storage per project; 5 GB egress per project per month | <https://neon.com/docs/introduction/plans> |
| Neon idle connections | "The Activity Monitor closes idle connections after 5 minutes of inactivity" | <https://neon.com/docs/reference/glossary> |
| Neon pooled connections | PgBouncer **transaction mode**; not supported on pooled connections: `SET`, `LISTEN`/`NOTIFY`, SQL-level `PREPARE`, temp tables, **session-level advisory locks**; protocol-level prepared statements are supported (`max_prepared_statements=1000`) | <https://neon.com/docs/connect/connection-pooling> |
| Neon via Vercel Marketplace | injects `DATABASE_URL` (pooled, PgBouncer) and `DATABASE_URL_UNPOOLED` (direct) into the **Vercel** project; a Render service needs them copied manually [INFERENCE: the integration only targets Vercel projects] | <https://neon.com/docs/guides/vercel-managed-integration> |
| Vercel Blob Hobby | 1 GB storage/month, first 10,000 simple operations, first 2,000 advanced operations, first 10 GB Blob Data Transfer; rate limits 1,200 simple / 900 advanced ops per minute | <https://vercel.com/docs/vercel-blob/usage-and-pricing> |
| Vercel Blob op accounting | uploads, copies and **list** are advanced operations; a URL read is a simple operation **only on a CDN cache MISS**; deletes are free | <https://vercel.com/docs/vercel-blob> |

Current code baseline (for integration cost): axum 0.8.9, tower-http 0.6.11, tokio 1.54.1,
flate2 1.1.11 (`Cargo.lock`); workspace `rust-version = "1.91.1"` (`Cargo.toml:9`); local toolchain
rustc 1.99.1 (`docs/PERFORMANCE.md:40`). Web: `lightweight-charts ^5.3.0`, React 19, no table or
other chart library (`web/package.json`); the depth heatmap is a hand-written canvas renderer
(`web/src/features/replay/heatmap.ts:6`, `:581-597`) and tables are hand-rolled with server paging
(`web/src/features/runs/RunOverviewPage.tsx:23`, `PAGE_SIZE = 100`).

## 2. Maintenance and licence snapshot

Commit dates are the newest commit on the default branch; "release" is GitHub's `releases/latest`
(some projects publish to package registries instead, noted where relevant). All rows: **gh**,
2026-11-14.

| Repository | Stars | Last commit | Latest GitHub release | SPDX (GitHub API) | Licence file says | Archived |
|---|---:|---|---|---|---|---|
| nautechsystems/nautilus_trader | 31,509 | 2026-11-13 (`develop`) | v1.231.0, 2026-08-02 | `LGPL-3.0` | LGPL-3.0 (see OPEN_SOURCE_RESEARCH §2) | no |
| abides-sim/abides | 596 | 2020-11-19 (last push 2023-07-06) | v1.1, 2020-03-15 | `NOASSERTION` | BSD 3-Clause, © 2019 Georgia Tech Research Corp. | no |
| jpmorganchase/abides-jpmc-public | 175 | 2023-12-13 | none | `NOASSERTION` | BSD 3-Clause, © 2021 J.P. Morgan Chase | **yes** |
| quantstart/qstrader (as named in the brief) | 154 | 2018-12-21 | none | `MIT` | — | no |
| mhallsmoore/qstrader (the maintained one) | 3,488 | 2024-06-24 | v0.3.0, 2024-06-24 | `MIT` | MIT, © 2015-2024 QuantStart.com, QuarkGluon Ltd | no |
| QuantConnect/Lean | 22,517 | 2026-11-13 | v2.4.0.1, 2017-08-08 (GitHub releases not used for current versions) | `Apache-2.0` | Apache-2.0 | no |
| ranaroussi/quantstats | 7,864 | 2026-11-02 | v0.0.86, 2026-09-27 | `Apache-2.0` | Apache-2.0 (`LICENSE.txt`) | no |
| polakowo/vectorbt | 9,842 | 2026-11-11 | v1.2.1, 2026-11-11 | `NOASSERTION` | **"Apache 2.0 with Commons Clause"** (`LICENSE.md`) | no |
| quantopian/pyfolio | 6,442 | 2020-07-15 (last push 2023-12-23) | 0.9.2, 2019-04-15 | `Apache-2.0` | Apache-2.0 | no |
| stefan-jansen/pyfolio-reloaded (maintained fork) | 624 | 2026-11-02 | 0.10.2, 2026-11-02 | `Apache-2.0` | Apache-2.0 | no |
| duckdb/duckdb | 43,048 | 2026-11-13 | v2.0.0, 2026-10-21 | `MIT` | MIT | no |
| pola-rs/polars | 40,335 | 2026-11-13 | py-2.1.0, 2026-10-25 | `MIT` | MIT | no |
| questdb/questdb | 17,528 | 2026-11-13 | 10.2.0, 2026-11-10 | `Apache-2.0` | Apache-2.0 (`LICENSE.txt`) | no |
| timescale/timescaledb | 23,854 | 2026-11-13 | 2.32.0, 2026-11-12 | `NOASSERTION` | **Apache-2.0 outside `tsl/`, Timescale License (TSL) inside `tsl/`** | no |
| tradingview/lightweight-charts | 17,850 | 2026-11-13 | v5.3.0, 2026-11-10 | `Apache-2.0` | Apache-2.0 | no |
| grafana/grafana | 77,889 | 2026-11-13 | v13.3.1, 2026-11-10 | `AGPL-3.0` | GNU AGPL v3 | no |
| open-telemetry/opentelemetry-rust | 2,771 | 2026-11-10 | opentelemetry-0.33.1, 2026-10-09 | `Apache-2.0` | Apache-2.0 | no |
| launchbadge/sqlx → **transact-rs/sqlx** (API redirect) | 17,665 | 2026-11-10 | none on GitHub; crate 0.9.0, 2026-05-21 | `Apache-2.0` | crate: `MIT OR Apache-2.0` | no |
| apalis-dev/apalis | 1,504 | 2026-11-13 | v1.1.1, 2026-11-10 | `MIT` | crate: `MIT OR Apache-2.0` | no |
| apalis-dev/apalis-postgres | 8 | push 2026-11-12 | crate 1.1.0, 2026-11-12 | `MIT` | MIT file; **crates.io licence field: `non-standard`** | no |
| apache/arrow-rs | 3,670 | 2026-11-13 | 60.1.0, 2026-10-28 | `Apache-2.0` | crates `arrow`/`parquet` 60.1.0: `Apache-2.0` | no |
| apache/arrow-rs-object-store | 337 | 2026-11-01 | v0.14.2, 2026-09-15 | `Apache-2.0` | crate `object_store` 0.14.2: `MIT/Apache-2.0` | no |
| juhaku/utoipa | 4,154 | 2026-10-20 | utoipa-swagger-ui-10.0.2, 2026-10-20; crate `utoipa` 6.0.0, 2026-09-22 | `Apache-2.0` | crates: `MIT OR Apache-2.0` | no |
| tamasfe/aide | 682 | 2026-09-21 | release-aide-0.15.1, 2025-08-19; crate 0.15.1 updated 2026-04-14 | `Apache-2.0` | crate: `MIT OR Apache-2.0` | no |
| benwis/tower-governor | 359 | 2025-08-14 | none; crate `tower_governor` 0.8.0, 2025-08-14 | `MIT` | crate: `MIT OR Apache-2.0` | no |
| tower-rs/tower-http | 934 | 2026-10-25 | tower-http-0.7.1, 2026-08-31 | none detected (licence API 404) | crate: `MIT` | no |
| tokio-rs/tracing | 6,978 | 2026-11-12 | tracing-appender-0.2.5, 2026-04-17; crate `tracing` 0.1.44, 2025-12-18 | `MIT` | MIT | no |
| tokio-rs/tracing-opentelemetry | 416 | 2026-11-10 | v0.33.0, 2026-05-18; crate 0.34.0, 2026-09-23 | `MIT` | MIT | no |
| leeoniya/uPlot | 10,637 | 2026-11-13 | 1.6.32, 2025-03-14 | `MIT` | MIT | no |
| apache/echarts | 67,719 | 2026-10-04 | 6.1.0, 2026-05-19 | `Apache-2.0` | Apache-2.0 | no |
| airbnb/visx | 21,107 | 2026-06-22 | v4.0.0, 2026-06-11 | `MIT` | MIT | no |
| d3/d3 | 113,915 | 2026-11-03 (push) | npm `d3` 7.9.2, 2026-11-03 | `ISC` | ISC | no |
| TanStack/table | 28,573 | 2026-10-21 | npm `@tanstack/react-table` 9.3.1, 2026-10-12 | `MIT` | MIT | no |
| TanStack/virtual | 7,199 | 2026-11-06 | npm `@tanstack/react-virtual` 3.14.26, 2026-11-03 | `MIT` | MIT | no |
| vercel/storage (`@vercel/blob`) | — | `packages/blob/src` last changed 2026-11-09 (`3b0e3a9fe532`) | npm `@vercel/blob` 2.9.0 | `Apache-2.0` | npm: `Apache-2.0` | no |

Licence corrections to the brief's list (all verified above):

- **vectorbt**: correct that it is not plain Apache-2.0. The file is Apache-2.0 plus the Commons
  Clause condition: the grant "will not include … the right to Sell the Software", where Sell
  includes providing "for a fee … (including … fees for hosting or consulting/support services …)
  a product or service whose value derives, entirely or substantially, from the functionality of
  the Software" (`polakowo/vectorbt/LICENSE.md`). It is source-available, not OSI open source.
- **TimescaleDB**: correct that it is split. `tsl/` code is under the Timescale License, whose
  §2.2 prohibits "using any TSL Licensed Software to provide time-sharing services or
  database-as-a-service services, or … any form of software-as-a-service … in which the TSL
  Licensed Software is offered or made available to third parties to provide time-series database
  functions", except as part of "Your Value Added Products or Services"
  (`timescale/timescaledb/tsl/LICENSE-TIMESCALE`).
- **Grafana**: AGPL-3.0 confirmed (licence file heading "GNU AFFERO GENERAL PUBLIC LICENSE Version 3").
- **NautilusTrader**: `LGPL-3.0` confirmed. **QuestDB**: `Apache-2.0` confirmed.
- **ABIDES** has two lines: the original (`abides-sim/abides`, last commit 2020) and J.P. Morgan's
  rewrite `abides-jpmc-public`, which is **archived** (read-only). Both BSD 3-Clause by file.
- **qstrader**: `quantstart/qstrader` is a 2018 snapshot; the maintained repository is
  `mhallsmoore/qstrader`.
- **pyfolio**: `quantopian/pyfolio` is not archived but has had no commit since 2020-07-15; the
  maintained fork is `stefan-jansen/pyfolio-reloaded`.
- **sqlx**: `gh api repos/launchbadge/sqlx` resolves to `transact-rs/sqlx`; the crate's
  `repository` field and README still say launchbadge.

## 3. Evaluations

Format per item: **Maintenance · Licence · Integration · Usefulness · Runtime cost · Risk ·
Decision.** Decisions: *embed* (linked dependency), *integrate* (separate process/service or
offline tool talking to ours), *adapt idea* (re-implement a concept), *learn only*, *reject*.

### 3.1 Simulation frameworks

**NautilusTrader.** Maintenance: very active (commit 2026-11-13, release v1.231.0 2026-08-02).
Licence: LGPL-3.0; obligations if linked are already set out in OPEN_SOURCE_RESEARCH §2 (relink
option is the practical burden with static Rust linking). Integration: Rust core with a Python
API; replacing hftbacktest would be a rewrite of `bb-sim` and the recorder. Usefulness: its
backtest documentation is the best public statement of event-ordering rules we found (section 5).
Runtime: [INFERENCE] a full Nautilus node is far larger than our 512 MB service needs.
Risk: copyleft on copied code. **Decision: learn only** (concepts in §5; no code copied).

**ABIDES.** Maintenance: original last commit 2020-11-19; J.P. Morgan rewrite archived. Licence:
BSD-3-Clause (file). Integration: Python, agent-based; a different paradigm from replay (the
market reacts through other agents). Usefulness: its kernel's computation-delay and pairwise
latency model are directly relevant ideas (section 5). **Decision: learn only.**

**qstrader.** Maintenance: last commit 2024-06-24. Licence: MIT. Integration: Python,
portfolio-level (daily-bar style) backtester [INFERENCE from its positioning as a schedule-driven
portfolio engine; README not read in depth]; no order book, no queue, no latency. Usefulness for
microstructure research: none. **Decision: reject.**

**QuantConnect Lean.** Maintenance: very active (commit 2026-11-13). Licence: Apache-2.0
(permissive; NOTICE retention if code copied). Integration: C#/.NET engine with Python strategies;
no sensible embedding into a Rust workspace. Usefulness: reference for pluggable fill/fee/slippage
model interfaces [INFERENCE: not read for this document]. Runtime: a .NET runtime on 512 MB is a
poor fit [INFERENCE]. **Decision: learn only.**

### 3.2 Performance analytics libraries

**quantstats.** Active (v0.0.86, 2026-09-27); Apache-2.0; Python + pandas. Usefulness: a
catalogue of return/risk metrics and tear-sheet layout. Our metrics are computed in Rust from our
own ledger (`docs/PNL_ACCOUNTING.md`); embedding Python would add a second runtime to a 512 MB
box. **Decision: adapt idea** (metric naming and definitions only, re-derived and cited; users may
run quantstats offline on an exported equity series).

**vectorbt.** Active (v1.2.1, 2026-11-11). Licence: Apache-2.0 **+ Commons Clause**. Hosting a
paid service whose value derives substantially from it would be prohibited; even for a free demo
the clause makes the licence non-OSI and a liability if the project ever charges. Integration:
Python/NumPy vectorised bar backtests, the opposite of event-level queue simulation.
**Decision: reject** (and do not copy code).

**pyfolio / pyfolio-reloaded.** Original unmaintained since 2020; fork active (0.10.2,
2026-11-02); both Apache-2.0; Python. Same reasoning as quantstats. **Decision: learn only**
(original: reject as a dependency because unmaintained).

### 3.3 Data engines and databases

**DuckDB.** Active (v2.0.0, 2026-10-21); MIT. Rust crate `duckdb` 1.20000.2 (MIT) depends on
`libduckdb-sys =1.20000.2` (crates.io dependencies API), i.e. a vendored C++ build [INFERENCE:
`bundled` builds compile the whole C++ engine, slow on this Windows GNU toolchain]. Usefulness: an
excellent local tool for querying exported runs (Parquet/NDJSON) across many runs. Runtime: an
embedded engine inside the 512 MB API competes with the simulator for memory. **Decision:
integrate (offline only)** — document `duckdb` CLI recipes over exported artefacts; do not link.

**Polars.** Active; MIT; Rust crate `polars` 0.55.2 (2026-08-06). Usefulness: same niche as DuckDB
for users; nothing in the server needs a DataFrame engine. **Decision: learn only** (users may
use it on exports).

**QuestDB.** Active (10.2.0, 2026-11-10); Apache-2.0 (permissive; no network copyleft). Runtime:
its capacity-planning docs recommend "at least 8GB of RAM for basic workloads"
(<https://questdb.com/docs/getting-started/capacity-planning/>), and it would be a second stateful
service needing a persistent disk, which Render free services cannot attach. Our tick data is
licensed vendor data we must not host anyway (contract), so a hosted tick database has no input.
**Decision: reject.**

**TimescaleDB.** Active (2.32.0, 2026-11-12). Licence split as above. On Neon: "Only Apache-2
licensed features are supported. Compression is not supported." (extension table, versions up to
2.24.0; <https://neon.com/docs/extensions/pg-extensions>). We would not be offering a time-series
DBaaS, so TSL §2.2 would not bite us as a *user*, but Neon does not offer the TSL features anyway.
Usefulness: low — run traces live in object storage, Postgres only holds metadata and job state,
well inside 1 GB. **Decision: reject** (revisit only if per-tick metrics move into Postgres).

### 3.4 Visualisation and observability services

**Lightweight Charts.** Active (v5.3.0, 2026-11-10, matches our `^5.3.0`). Apache-2.0; NOTICE
attribution already present in `NOTICE`. Measured bundle cost (esbuild 0.28.2,
`createChart`+`LineSeries`, minified, `gzip -9`): **54,033 B**. Good at time series (equity,
inventory, mid/price lines); it has no heatmap or ladder primitive. **Decision: embed (keep).**

**Grafana.** Active (v13.3.1). AGPL-3.0: running an *unmodified* Grafana as a separate service
creates no obligation for our code; modifying and exposing it over a network would require
offering the modified source (AGPL §13) [INFERENCE: standard reading of AGPL-3.0, not legal
advice]. Runtime: Grafana's own minimum is "512 MB" memory and "1 core"
(<https://grafana.com/docs/grafana/latest/setup-grafana/installation/>), i.e. an entire extra free
service at its floor, plus a metrics store. **Decision: reject self-hosting**; the same page
advertises a Grafana Cloud free tier ("10k metrics, 50GB logs, 50GB traces") that could receive
OTLP later — *integrate optionally*, no code dependency.

**opentelemetry-rust.** Active (0.33.1, 2026-10-09); Apache-2.0. README status table: Logs and
Metrics API/SDK/OTLP exporter **Stable**; **Traces API/SDK/OTLP exporter Beta**; Context and
Propagators Beta. Integration: via `tracing-opentelemetry` 0.34.0 (MIT), which depends on
`opentelemetry ^0.33.0` and `tracing-subscriber ^0.3.22` (crates.io dependencies API). Runtime:
an OTLP exporter adds a background batch task and an HTTP/gRPC client [INFERENCE: modest, not
measured]. Risk: 0.x churn — the OTel crates move in lockstep and `tracing-opentelemetry` must
match. **Decision: integrate later, optional** (cargo feature + env var, off by default).

### 3.5 Rust backend crates

**sqlx (Postgres, migrations, offline mode).** Crate 0.9.0 (2026-05-21), `MIT OR Apache-2.0`,
**`rust_version` 1.94.0** (crates.io). Our workspace declares `rust-version = "1.91.1"`
(`Cargo.toml:9`), so adopting 0.9 requires raising it to ≥ 1.94 (the local toolchain is 1.99.1;
hftbacktest's own MSRV is 1.91.1, so the raise is safe for it). Features used: `macros` for
compile-time-checked `query!`, `migrate` for "compile-time embedded migrations" (README).
Offline mode: README says "Offline mode is now always enabled"; `cargo sqlx prepare --workspace`
writes `.sqlx/` query metadata, `cargo sqlx prepare --check` fails CI when stale, and
`SQLX_OFFLINE=true` stops a stray `DATABASE_URL` from making `cargo build` contact a database
(`sqlx-cli/README.md`). Neon specifics found in source and docs:

- sqlx prepares and caches statements per connection at the protocol level (README: "prepared
  and cached per connection") — supported by Neon's PgBouncer (`max_prepared_statements=1000`).
- **The migrator takes a session-level advisory lock** (`SELECT pg_advisory_lock($1)` in
  `sqlx-postgres/src/migrate.rs:198`, default `locking: true` in `sqlx-core/src/migrate/migrator.rs:39`).
  Neon's pooler does not support session-level advisory locks, so **migrations must run over
  `DATABASE_URL_UNPOOLED`**; do not turn locking off (`set_locking(false)` is documented as
  risking "errors or data loss" with concurrent migrators).
- Neon closes idle connections after 5 minutes; the pool must tolerate dead connections
  (sqlx's `test_before_acquire`, on by default [INFERENCE: from sqlx `PoolOptions` defaults, not
  re-read today]) and keep `min_connections = 0` so the API does not hold Neon awake.

**Decision: embed** (Postgres + rustls TLS, `macros`, `migrate`; pooled URL for the app, unpooled
URL for `sqlx::migrate!`).

**Job queue: apalis vs a hand-written `FOR UPDATE SKIP LOCKED` queue.** Candidates observed:

| Option | Licence | Observed state | Fit with Neon pooler / Render free |
|---|---|---|---|
| `apalis` 1.1.1 + `apalis-postgres` 1.1.0 | MIT OR Apache-2.0 / MIT file, crates.io field `non-standard` | active; apalis-postgres repo has 8 stars; 23 migration files including `CREATE EXTENSION IF NOT EXISTS hstore` (`migrations/20260826101335_metadata_hstore.sql`); 1.0 required a manual SQL step moving `public._sqlx_migrations` into schema `apalis` (README) | `PostgresStorage` polls; `PostgresStorageFactory` "uses Postgres `NOTIFY`" and the pubsub example uses `LISTEN/NOTIFY` (README) — LISTEN needs a direct connection; `hstore` is available on Neon (extension table) |
| `graphile_worker` 0.14.2 | MIT | 88 stars, push 2026-11-12 | Rust port of a LISTEN/NOTIFY-based design [INFERENCE] |
| `underway` 0.2.0 | Apache-2.0 | 175 stars, crate updated 2025-07-16 | pre-1.0 |
| `fang` 0.11.0 | MIT | 724 stars, push 2026-07-02 | — |
| `sqlxmq` 0.6.0 | Apache-2.0 | 167 stars, last push 2025-05-25 | stale |
| `pgmq` (extension + client) | PostgreSQL | 5,396 stars | Postgres extension; not in the Neon extension rows searched [INFERENCE: not confirmed absent] |
| Hand-written | ours | — | one table + three statements; works through the pooler |

PostgreSQL documents `SKIP LOCKED` as able to "avoid lock contention with multiple consumers
accessing a queue-like table" (<https://www.postgresql.org/docs/current/sql-select.html>). What we
need is small: one job kind (simulate a run, later sweeps), single-digit concurrency, at-least-once
delivery with idempotent completion, leases that survive Render killing the process on spin-down,
and no LISTEN dependency. A sketch (our design, not copied):

```sql
-- claim one job; a single autocommit statement, so it works in PgBouncer transaction mode
UPDATE jobs SET status = 'running', attempts = attempts + 1,
       lease_owner = $1, lease_expires_at = now() + interval '60 seconds'
WHERE id = (
  SELECT id FROM jobs
  WHERE (status = 'queued' AND run_after <= now())
     OR (status = 'running' AND lease_expires_at < now())   -- reclaim after crash/spin-down
  ORDER BY priority, id
  FOR UPDATE SKIP LOCKED
  LIMIT 1)
RETURNING *;
-- heartbeat: UPDATE jobs SET lease_expires_at = now() + interval '60 seconds' WHERE id = $1 AND lease_owner = $2;
-- finish:    UPDATE jobs SET status = 'succeeded', result_ref = $3 WHERE id = $1 AND lease_owner = $2;
```

Wake-up without LISTEN: the API and worker share a process, so an insert signals a
`tokio::sync::Notify`; a standalone `blackbox worker` polls with backoff. Poll only while jobs are
queued/running, otherwise the poller keeps Neon's compute active and burns the 100 CU-hour budget
(400 h at 0.25 CU ≈ 13 h/day [arithmetic]). Attempts cap → `failed` with `last_error`; an
`idempotency_key UNIQUE` column makes client retries safe. **Decision: adapt idea — hand-written
queue on sqlx; reject apalis for now** (extension + private schema + migration churn + LISTEN
orientation buy nothing at our scale; revisit if we need cron, DAGs or many job kinds).

**arrow-rs / parquet.** Very active (60.1.0, 2026-10-28), Apache-2.0, `rust_version` 1.88.
Usefulness was measured (section 4): Parquet does not beat gzip JSON on size for our traces, and
our identity hash is defined over canonical JSON. **Decision: learn only for the server; integrate
later as an offline `blackbox export --format parquet` if users ask for DuckDB/Polars analytics.**

**object_store.** Active (0.14.2, 2026-09-15), `MIT/Apache-2.0`. Backends: local filesystem,
Amazon S3, GCS, Azure, and "HTTP/WebDAV Storage" (`src/lib.rs`). **Vercel Blob is not
S3-compatible** (no S3 endpoint is documented; search of vercel.com docs found none), and the
WebDAV `HttpStore` writes with `PUT` to the object URL, creates directories with `MKCOL` and lists
with `PROPFIND` (`src/http/client.rs:128,175,249`), none of which Vercel Blob's API uses (section
3.7). Reads would likely work through `HttpStore` with `ClientOptions::with_default_headers`
(`src/client/mod.rs:547`) carrying the bearer token [INFERENCE, untested], but writes and listing
would not. **Decision: reject for Vercel Blob**; keep as the implementation behind the same trait
if artefacts ever move to an S3-compatible store (see the Neon Object Storage note in 3.7).

**OpenAPI: utoipa vs aide.** `utoipa` 6.0.0 + `utoipa-axum` 0.3.0 (requires `axum ^0.8.4`;
we have 0.8.9), both `MIT OR Apache-2.0`; README lists "OpenAPI 3.1" and emits
`"openapi": "3.1.0"`; repository commit 2026-10-20; companion `utoipa-swagger-ui` 10.0.2 and
`utoipa-scalar` 0.4.0. `aide` 0.15.1 (`axum ^0.8.1`, optional; `schemars ^0.9.0`), last crate
update 2026-04-14, last commit 2026-09-21. Both are code-first; utoipa uses derive/attribute macros
on handlers and types, aide derives from `schemars` JSON Schemas and wraps the router.
**Decision: embed utoipa (+ utoipa-axum)**: larger user base (63.4M vs 4.0M downloads,
crates.io), more recent releases, OpenAPI 3.1 out of the box. Serve the JSON spec only; skip the
Swagger UI bundle on the 512 MB box (the Vercel frontend or a static page can render it).

**Rate limiting and request limits.** `tower_governor` 0.8.0 (2025-08-14, 15 months without a
release; `governor ^0.10`, `tower ^0.5.1`, optional `axum ^0.8`) — in-memory GCRA keyed per peer;
fine for one instance. Behind Render + Cloudflare the TCP peer is a proxy, so the key must come
from `X-Forwarded-For` (Render's guidance); the left-most entry is client-supplied [INFERENCE:
standard XFF semantics], so per-IP limits are best-effort only. `tower-http` 0.7.1 (MIT; we are on
0.6.11) provides request-body limits, timeouts, compression, CORS, request IDs and `TraceLayer`;
`tower` 0.5.3 provides `ConcurrencyLimitLayer`/`LoadShedLayer`. **Decision: embed tower-http
(upgrade to 0.7) + tower concurrency limits; integrate tower_governor only for anonymous
read endpoints; enforce job-creation quotas per API key in Postgres** (a counter/time-window
check in the same transaction as the job insert), which is authoritative across restarts and a
future second process, unlike in-memory limiters.

**tracing / tracing-opentelemetry.** `tracing` 0.1.44 and `tracing-subscriber` 0.3.23, MIT, the
de-facto standard; JSON logs to stdout land in Render's log stream with no extra service.
**Decision: embed tracing + tracing-subscriber (JSON formatter, `EnvFilter`); integrate
tracing-opentelemetry + opentelemetry-otlp later behind a feature flag.**

### 3.6 Frontend: charts and tables

Bundle sizes measured with esbuild 0.28.2 (`--bundle --minify --format=esm`, React external),
then `gzip -9 | wc -c`, on packages installed into a scratch directory:

| Library (npm, licence) | Entry measured | min bytes | gzip bytes | Rendering |
|---|---|---:|---:|---|
| lightweight-charts 5.3.0 (Apache-2.0) | `createChart`, `LineSeries` | 165,655 | 54,033 | canvas |
| uplot 1.6.32 (MIT; last release 2025-03-14, commits to 2026-11-13) | default export | 51,994 | 22,988 | canvas |
| echarts 6.1.0 (Apache-2.0) | core + `HeatmapChart`, grid, visualMap, tooltip, dataZoom, canvas renderer | 565,639 | 190,185 | canvas/SVG |
| echarts 6.1.0 | full `import * as echarts` | 1,141,530 | 382,569 | canvas/SVG |
| @visx/heatmap 4.0.0 + @visx/scale 4.0.0 (MIT) | `HeatmapRect`, `scaleLinear` | 33,456 | 12,683 | **SVG** — one `<rect>` per bin (`esm/heatmaps/HeatmapRect.js:54`) |
| d3-scale 4.0.3 (ISC) | `scaleLinear` | 19,263 | 7,822 | none (math only) |
| @tanstack/react-table 9.3.1 + @tanstack/react-virtual 3.14.26 (MIT) | whole namespace + `useVirtualizer` (upper bound) | 158,255 | 43,656 | headless |

Our replay heatmap can be 5,000 frames × 100 levels = 500,000 cells (`docs/PERFORMANCE.md:443`).
An SVG `<rect>` per cell (visx, or ECharts' SVG renderer) is not viable at that size [INFERENCE:
DOM node count]; the existing `ImageData` blit (`heatmap.ts:581-597`) is the right technique and
costs no dependency.

- **Lightweight Charts — embed (keep)** for price/equity/inventory time series.
- **Custom canvas heatmap and ladder — keep**; no library improves on an `ImageData` blit.
- **uPlot — adapt later if needed**: 23 KB gzip for dense multi-series, non-financial plots
  (latency histograms, queue-position traces, many-run overlays) where lightweight-charts' time
  axis is awkward. Not adopted now: one chart library is enough until such a view exists.
- **ECharts — reject**: 190 KB gzip for the heatmap subset, and our hardest view is already solved.
- **visx — reject** for depth/heatmaps (SVG per cell); **d3-scale — optional embed** if a view needs
  log/quantile colour scales (7.8 KB gzip).
- **TanStack Table + TanStack Virtual — integrate** for orders/fills/tape tables: headless (keeps
  our CSS), sorting/column state from Table, windowed rendering from Virtual so a 5,000-row tape
  page (`docs/PERFORMANCE.md:444`) renders only visible rows. Note `@tanstack/react-table` is now
  major version **9** (9.3.1); check its migration notes before copying v8 examples.

### 3.7 Vercel Blob from a Rust client

Question from Main: is the Vercel Blob HTTP API documented and usable from non-JS code, and can
`object_store` talk to it?

**Documented:**

- Official SDKs are JS (`@vercel/blob` 2.9.0, Apache-2.0) and Python (`vercel` on PyPI, 0.13.0;
  reference at <https://vercel.com/docs/vercel-blob/python-sdk-reference>).
- **Reads without the SDK are documented**: "Accessing without the SDK" shows
  `curl https://my-store-id.private.blob.vercel-storage.com/my-file.png -H "Authorization: Bearer $BLOB_READ_WRITE_TOKEN"`,
  with `BLOB_READ_WRITE_TOKEN` for "code runs outside Vercel"
  (<https://vercel.com/docs/vercel-blob/private-storage>). Range requests are supported
  (<https://vercel.com/docs/vercel-blob/examples>, shown on a public URL; [INFERENCE] the same
  holds for authenticated private reads). Consistent reads bypass the CDN with `useCache: false`.
- **OIDC is unavailable to us**: it is for code running on Vercel; outside Vercel the long-lived
  read-write token is the documented credential (<https://vercel.com/docs/vercel-blob>,
  "When to use a read-write token").
- The Vercel REST API documents store management (e.g. `POST /storage/stores/blob`, linked from
  <https://vercel.com/docs/vercel-blob/security>) but **not** object upload/list/delete.

**Undocumented but stable-looking wire protocol**, read from `vercel/storage` source at
`packages/blob/src` (commit `3b0e3a9fe532`, 2026-11-09):

| Operation | Request |
|---|---|
| Put | `PUT https://vercel.com/api/blob/?pathname=<path>` (`put.ts:124-129`, base URL `helpers.ts:13`), body = bytes |
| Common headers | `authorization: Bearer <token>`, `x-api-version: 13` (`api.ts:148`), `x-vercel-blob-store-id: <id>` (`api.ts:343`; store id is the 4th `_`-separated field of the read-write token, `helpers.ts:184`), optional `x-content-length` |
| Put options | `x-vercel-blob-access: private`, `x-content-type`, `x-add-random-suffix`, `x-allow-overwrite`, `x-cache-control-max-age`, `x-if-match`, `x-ttl-days` (`put-helpers.ts:105-113`) |
| Put response | JSON with `url`, `downloadUrl`, `pathname`, `contentType`, `contentDisposition`, `etag` (`put-helpers.ts`, `PutBlobResult`) |
| Get | `GET https://<storeId>.private.blob.vercel-storage.com/<path>` with bearer token (`get.ts:183`, `helpers.ts:576`); `?cache=0` for a consistent read (`get.ts`) |
| List | `GET https://vercel.com/api/blob/?limit=&prefix=&cursor=&mode=` (`list.ts:146-164`) |
| Delete | `POST https://vercel.com/api/blob/delete`, JSON `{"urls": [...]}` (`del.ts:30-48`) |

Feasibility: a minimal client is ~150 lines on `reqwest` 0.13 (rustls): `put(path, bytes,
content_type) -> etag`, `get(path, range) -> bytes stream`, `delete(paths)`; no `list` needed if
Postgres is the index of artefacts. Requirements: retries with backoff on 5xx/429 (rate limits
above), pinning `x-api-version` and an integration test against a dedicated dev store (run by a
human, not CI), and treating the protocol as **unofficial** — a breaking change would surface as
a failed upload, so failures must leave the job retryable. Body sizes are small (≤ ~1.5 MB per
run, section 4), so multipart upload is unnecessary.

Budget fit on Hobby: 2,000 advanced ops/month ⇒ **≤ ~2,000 run uploads/month if each run is one
blob**; never call `list` from the app (each list is an advanced op). Reads cost a simple op only
on CDN miss; keep a small in-process LRU **bounded by bytes, not entry count** (the current run
cache is 16 entries, `docs/SECURITY.md:171`; 16 real-size runs decoded would not fit in 512 MB
[INFERENCE from the 26 MB in-memory Arrow figure in section 4]).

Security: the read-write token grants read **and write** to the whole store and never expires
(<https://vercel.com/docs/vercel-blob/security>, "A read-write token never expires"). Use a
dedicated store for artefacts only, keep the token only in Render's environment, never send it to
the browser (the frontend fetches artefacts through our API, which enforces workspace/key scope),
and rotate on suspicion.

**Alternative worth knowing (not a recommendation against the user's decision):** Neon now ships
S3-compatible Object Storage, "available on all plans, including Free", 5 GB per project on Free,
with `ListObjectsV2`, ranged `GetObject`, `PutObject`, multipart and SigV4 presigned URLs, in four
AWS regions (<https://neon.com/docs/introduction/plans>,
<https://neon.com/docs/storage/object-storage>, <https://neon.com/docs/storage/s3-compatibility>).
That would let `object_store` (feature `aws`) be used unchanged. Unverified: whether it is
offered on Vercel-managed Neon projects, and whether its requests count against Neon compute.

## 4. Columnar artefacts: Parquet/Arrow vs gzip JSON (measured)

`docs/PERFORMANCE.md:398` records ~465 B per tape event for a stored real-data run. Measured on
2026-11-14 with Python 3.11 `json`/`gzip` (level 6, [INFERENCE] equal to flate2's default level)
and pyarrow 24.0.0 as a stand-in for arrow-rs's Parquet writer (same file format; [INFERENCE]
sizes within a few percent). The tape was flattened to 6 scalar columns + one `body` struct
(union of all event-type fields, nulls elsewhere). Only aggregate sizes were taken from the
vendor-derived run; no rows were read out or copied.

| Quantity | Synthetic run `130d67fc371bd27f` (25,276 events) | Real-data run `835ba545c3da0595` (64,540 events) |
|---|---:|---:|
| Stored `run.json` | 9,017,604 B (356.8 B/event) | 30,167,590 B (**467.4 B/event**) |
| Tape only, compact JSON | 258.5 B/event | 312.5 B/event |
| Whole `run.json`, gzip-6 | 696,777 B | 1,455,768 B (22.6 B/event) |
| Tape, gzip-6 | 480,222 B (19.0 B/event) | 979,570 B (**15.2 B/event**) |
| Tape, Parquet zstd | 515,391 B (20.4 B/event) | 1,000,836 B (**15.5 B/event**) |
| Tape, Parquet snappy | 714,087 B | 1,544,258 B |
| Tape, Parquet gzip | 496,617 B | — |
| Tape as in-memory Arrow table | 9,027,387 B | 26,291,671 B |

Findings:

- **Size is not a reason to switch**: gzip-6 JSON and Parquet+zstd are within ~3% of each other;
  both shrink the trace ~20×. The free-text `reason` field is only 7.4% of the synthetic tape's
  JSON bytes.
- **Identity favours JSON**: `result_sha256` is SHA-256 of the canonical JSON of (tape, fills,
  equity) (`docs/EXPERIMENT_REPRODUCIBILITY.md:36`). A gzip-JSON artefact is verified by
  decompress-and-hash; a Parquet artefact would need re-serialisation to canonical JSON first.
- **Memory is the real constraint**: a whole real-size run materialised in memory is tens of MB
  (26 MB as Arrow; serde structs not measured), so paging endpoints must not decode the whole run
  per request on a 512 MB/0.1 CPU box.
- **Random access without extra uploads**: Parquet gives row-group access but needs `arrow`/
  `parquet` linked into the server. The same property is available from **gzip members**: a gzip
  file may be a concatenation of independently compressed members (RFC 1952 §2.2), so a single
  blob made of one member per 5,000-event page (the API's maximum tape page,
  `docs/PERFORMANCE.md:444`) still decompresses as one NDJSON stream, while a page is one ranged
  GET using a byte-offset index stored in Postgres. One upload = one advanced op per run
  [INFERENCE: ranged reads of a cached blob are CDN hits; not verified for private stores].

**Recommendation:** artefact = gzip NDJSON in page-sized gzip members + offset index row in
Postgres + summary/metrics in Postgres. Parquet only as an offline export.

At the measured sizes, Vercel Blob's 1 GB Hobby allowance holds roughly 700 real-size or 1,400
synthetic-size runs [arithmetic: 1 GB / 1.46 MB, 1 GB / 0.70 MB]; the public demo workspace only
ever stores synthetic runs (vendor terms, `docs/OPEN_SOURCE_RESEARCH.md` §8).

## 5. What NautilusTrader and ABIDES teach about ordering and latency

Concepts only; no code copied. BlackBox's current model is in `docs/LATENCY_MODEL.md` and
`docs/EXECUTION_MODEL.md`.

**NautilusTrader** (docs under `docs/concepts/` on `develop`, read 2026-11-14):

1. **Phase order within a timestamp** (`backtesting/execution-flow.md`): (1) the simulated exchange
   applies the data point and matches *existing* orders, (2) strategies receive the data and may
   submit, (3) venue commands are drained and matched, repeating until no command is eligible, so
   cascades (a hedge sent from a fill callback) settle at the same timestamp. "The three phases
   ensure resting orders see the incoming market before newly submitted orders do." Relevance: we
   should state our equivalent rule explicitly per tick (what the strategy sees vs what resting
   orders see), since hftbacktest fixes it for us and a reader cannot otherwise know it.
2. **Latency-delayed commands are released only at settlement points**, and "Market data for
   another instrument does not activate an older command against stale market state"
   (`execution-flow.md`, "Latency-delayed commands"). Relevance: if we ever go multi-instrument,
   arrival of a delayed order must be tied to its own instrument's book state.
3. **Separate insert / update / delete latencies** (`get_insert_latency`, `get_update_latency`,
   `get_delete_latency` in `behavioral_models.md`). We use one entry latency for new orders and
   cancels (`LATENCY_MODEL.md:40`). Cancel latency differing from insert latency is a realistic,
   cheap extension of our `jitter` model.
4. **Stable ordering by a single receipt-side timestamp**: "Data is ordered by `ts_init` using a
   stable sort", with an explicit secondary key for ties in one data family; and
   "`ts_init - ts_event` measures observed delay only when the clocks … are synchronized"
   (`concepts/data/index.md`). This matches our two-clock design and our caution that feed latency
   is the vendor's capture latency (`LATENCY_MODEL.md:54-56`).
5. **Queue-position rules spelled out** (`backtesting/trade-execution.md`): snapshot same-side
   displayed size on acceptance; trades at the price reduce quantity ahead; "Newly added liquidity
   does not move an existing simulated order further back"; on a book snapshot, quantity ahead is
   "capped at the snapshot size"; a price change resets queue position while a quantity-only
   change keeps progress by default (policy configurable). Useful as a checklist against
   hftbacktest's queue models (`docs/QUEUE_MODELS.md`) and for a future "queue reset policy"
   experiment knob.
6. **Deterministic identifiers** from `(venue, raw_id, ts_init)` plus a counter for same-timestamp
   fills (`execution-flow.md`) — the same goal as our pure-function jitter keyed on what is being
   delayed (`LATENCY_MODEL.md:90-98`).

**ABIDES** (`abides-jpmc-public/abides-core`, archived; concepts):

1. **Computation delay**: after each wake-up the kernel advances the agent's own clock by its
   computation delay, and messages it sends leave at
   `current_time + computation_delay + accumulated_delay + per-message delay`, *then* network
   latency is added (`kernel.py:364-369`, `:562-566`). BlackBox decides on a tick grid and sends at
   `t_k` with zero think-time. A configurable **decision latency** between "view as of t_k" and
   "request sent" is a missing, cheap and explanatory knob.
2. **Pairwise, directional latency with heavy-tailed jitter**: `min_latency + a / x³` with `x`
   uniform on `(jitter_clip, 1]`, parameters per sender→recipient pair
   (`latency_model.py`, "Cubic Model"). Our jitter is uniform on `[0, jitter_ns]`. A clipped
   heavy-tailed option would let users study tail-latency effects (rare slow cancels) that uniform
   jitter cannot produce.
3. **Tie-breaking is a modelling decision**: the priority queue holds
   `(deliver_at, (sender_id, recipient_id, message))` (`kernel.py:607`), so equal delivery times
   are ordered by sender id, then recipient id, then a global creation counter
   (`message.py:17-31`); the code's own comment calls same-time delivery "arbitrary" rather than
   random. [INFERENCE from Python tuple comparison.] Lesson: document our tie rule explicitly (we
   already avoid emission-order dependence via the FIFO order bus, `LATENCY_MODEL.md:108-114`).
4. **What replay cannot do**: ABIDES' market reacts because other agents respond; ours cannot
   (OPEN_SOURCE_RESEARCH §5). This belongs in user-facing limitations next to every P&L figure.

## 6. Decision table

| Project | Capability | License | Integration cost | Decision | Justification |
|---|---|---|---|---|---|
| NautilusTrader | Rust/Python event-driven trading engine | LGPL-3.0 (copyleft, weak) | Very high (replace engine) | Learn only | Best public ordering/queue/latency rules (§5); LGPL relink burden; no reason to replace hftbacktest |
| ABIDES (abides-sim; jpmc fork archived) | Agent-based market simulation kernel | BSD-3-Clause (file) | High (Python, different paradigm) | Learn only | Unmaintained since 2020/2023; computation-delay and heavy-tail latency ideas worth adapting |
| qstrader (mhallsmoore) | Python portfolio backtester | MIT | n/a | Reject | No order book, queue or latency; brief's `quantstart/qstrader` is a 2018 snapshot |
| QuantConnect Lean | C# algorithmic trading engine | Apache-2.0 | Very high (.NET) | Learn only | Active and permissive but wrong runtime and granularity for a 512 MB Rust service |
| quantstats | Python performance metrics/tear sheets | Apache-2.0 | Medium (Python runtime) | Adapt idea | Re-derive metric definitions in Rust; users can run it offline on exports |
| vectorbt | Vectorised Python backtesting | Apache-2.0 + Commons Clause (source-available) | Medium | Reject | Non-OSI "no Sell" condition; bar-level vectorisation unsuited to queue simulation |
| pyfolio / pyfolio-reloaded | Python risk analytics | Apache-2.0 | Medium | Learn only (original: reject) | Original unmaintained since 2020; fork active but Python-only |
| DuckDB | Embedded analytical SQL | MIT | Low offline / high embedded (C++ build) | Integrate (offline) | Great for querying exports locally; not in the 512 MB server |
| Polars | DataFrame engine | MIT | Medium | Learn only | No server need; users may use it on exports |
| QuestDB | Time-series DB (JVM) | Apache-2.0 | High (extra stateful service) | Reject | ≥ 8 GB RAM recommended, needs disk (none on Render free), no hostable tick data |
| TimescaleDB | Postgres time-series extension | Apache-2.0 core + TSL (`tsl/`, no-DBaaS clause) | Low on Neon (Apache features only) | Reject | Neon: Apache features only, no compression; Postgres only holds metadata |
| Lightweight Charts | Financial time-series charts | Apache-2.0 (NOTICE) | Already integrated | Embed (keep) | Active, 54 KB gzip measured, covers equity/price/inventory |
| Grafana | Dashboards | AGPL-3.0 (network copyleft) | High (own service ≥ 512 MB/1 core) | Reject self-host; optional Grafana Cloud via OTLP | Exceeds free compute; AGPL fine only if unmodified |
| opentelemetry-rust | Traces/metrics/logs SDK + OTLP | Apache-2.0 | Low–medium | Integrate later (optional feature) | Logs/metrics Stable, traces Beta; no collector on free tier |
| sqlx | Async Postgres, migrations, compile-time checked queries | MIT OR Apache-2.0 | Medium (MSRV → 1.94, `.sqlx` in CI) | Embed | Offline mode, embedded migrations; pooled URL for app, unpooled for migrations |
| apalis (+ apalis-postgres) | Background job framework | MIT OR Apache-2.0 / MIT (crates.io field `non-standard`) | Medium | Reject (for now) | hstore extension, own schema, migration churn, LISTEN-oriented; one job kind does not need it |
| Hand-written SKIP LOCKED queue | Postgres job queue | ours | Low (~1 table, 3 statements) | Adapt idea (implement) | Works through PgBouncer transaction mode; leases survive Render spin-down |
| arrow-rs / parquet | Columnar format | Apache-2.0 | Medium (large dependency) | Learn only (offline export later) | Measured: no size gain over gzip JSON; hash is over canonical JSON |
| object_store | Object storage abstraction | MIT/Apache-2.0 | Low for S3; none for Vercel Blob | Reject (for Vercel Blob) | No Vercel Blob backend; WebDAV store incompatible; reuse if moving to S3-compatible storage |
| Vercel Blob (`@vercel/blob` protocol) | Private object storage | Apache-2.0 SDK; service ToS | Low (~150-line reqwest client) | Integrate | User decision; reads documented, writes via SDK wire protocol (unofficial) |
| utoipa (+ utoipa-axum) | OpenAPI 3.1 generation for axum 0.8 | MIT OR Apache-2.0 | Low–medium (annotations) | Embed | More active and widely used than aide; axum ^0.8.4 compatible |
| aide | OpenAPI generation via schemars | MIT OR Apache-2.0 | Medium | Reject | Slower releases; router-wrapping API; no advantage over utoipa here |
| tower_governor | Keyed in-memory rate limiting | MIT OR Apache-2.0 | Low | Integrate (anonymous reads only) | Stale since 2025-08; per-IP via XFF is best-effort; DB quotas for jobs instead |
| tower-http / tower | Body limits, timeouts, CORS, compression, tracing, concurrency limits | MIT | Low (0.6 → 0.7 upgrade) | Embed | Already used; standard |
| tracing + tracing-subscriber | Structured logging/spans | MIT | Low | Embed | JSON to stdout → Render logs, zero extra services |
| tracing-opentelemetry | tracing → OTel bridge | MIT | Low | Integrate later (optional) | Must match opentelemetry 0.33 |
| uPlot | Fast canvas plots | MIT | Low | Adapt later if needed | 23 KB gzip; for dense non-financial plots only when such a view exists |
| ECharts | General charting incl. heatmaps | Apache-2.0 | Medium | Reject | 190 KB gzip for heatmap subset; our canvas heatmap already solves it |
| visx / d3 | React SVG primitives / scales | MIT / ISC | Low | Reject visx; d3-scale optional | SVG rect per cell infeasible at 500k cells; d3-scale 7.8 KB if colour scales needed |
| TanStack Table + Virtual | Headless tables + row virtualisation | MIT | Low–medium (v9 API) | Integrate | Replaces hand-rolled tables; windowed rendering of 5,000-row pages |

## 7. Recommended stack (zero-cost hosting)

| Concern | Recommendation | Notes |
|---|---|---|
| **Job queue** | Hand-written Postgres queue on sqlx: `jobs` table with `status`, `priority`, `run_after`, `attempts`/`max_attempts`, `lease_owner`, `lease_expires_at`, `idempotency_key UNIQUE`, `workspace_id`, `api_key_id`, `result_ref`, `last_error`; claim with one `UPDATE … WHERE id = (SELECT … FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING *`; heartbeat; reclaim expired leases | In-process worker pool of **1** on 0.1 CPU [INFERENCE: more workers only contend]; wake via `tokio::sync::Notify`, poll only while work is pending so Neon can scale to zero; same code runs as `blackbox worker` later. No LISTEN/NOTIFY (pooler), no Redis |
| **DB access** | sqlx 0.9 (`postgres`, rustls TLS, `macros`, `migrate`), `PgPool` with small `max_connections` (≤ 5) and `min_connections = 0`, pooled `DATABASE_URL` | Raise workspace `rust-version` to ≥ 1.94; commit `.sqlx/`; CI runs `cargo sqlx prepare --check --workspace` against a throwaway Postgres; builds use `SQLX_OFFLINE=true` |
| **Migrations** | `sqlx::migrate!()` embedded, run at startup over **`DATABASE_URL_UNPOOLED`** (advisory lock needs a session), then drop that connection | Keep migration locking on; copy both Neon URLs from the Vercel project into Render env |
| **OpenAPI** | utoipa 6 + utoipa-axum 0.3; serve `/api/openapi.json`; render docs in the Vercel frontend, not from the API process | Spec becomes the contract for the TS client types in `web/src/api/types.ts` [INFERENCE: generation tool not chosen here] |
| **Columnar artefacts** | **gzip NDJSON**, one gzip member per ≤ 5,000-event page, single blob per run, page byte offsets + summary metrics in Postgres; no arrow/parquet in the server | Measured ~15 B/event (real) vs Parquet zstd ~15.5; keeps `result_sha256` verification trivial; offline `export --format parquet` for DuckDB/Polars users later |
| **Object storage client** | Own `ArtifactStore` trait with two impls: local directory (dev/tests) and Vercel Blob over `reqwest` 0.13 (PUT `vercel.com/api/blob/?pathname=`, GET `<store>.private.blob.vercel-storage.com/<path>` with `Range`, POST `/delete`), `x-api-version` pinned, retries on 5xx/429 | No `list` calls (Postgres is the index); byte-bounded LRU for decoded pages; token only in Render env, dedicated store; `object_store` becomes a third impl only if moving to S3-compatible storage |
| **Observability** | `tracing` + `tracing-subscriber` JSON to stdout (Render logs), request IDs and `TraceLayer` from tower-http, job lifecycle spans with `job_id`/`run_id`; optional cargo feature for `tracing-opentelemetry` + `opentelemetry-otlp` 0.33 to a hosted free backend | No self-hosted Grafana/collector on the free tier |
| **Request protection** | tower-http 0.7 body-size limit, timeouts, CORS (Vercel origin only), compression; tower `ConcurrencyLimitLayer`; tower_governor per-IP (XFF) for anonymous reads; per-API-key job quotas checked in Postgres in the job-insert transaction | API keys: random ≥ 32 bytes, store only SHA-256 digest + short lookup prefix + scopes, constant-time comparison (`subtle` 2.6.1, BSD-3-Clause) [INFERENCE: fast hash is adequate for high-entropy keys, unlike passwords] |
| **Charts** | Keep lightweight-charts 5.3 for time series; keep the custom canvas heatmap and ladder; add uPlot only for a future dense non-financial plot; d3-scale only if colour scales are needed | No ECharts, no visx |
| **Tables** | `@tanstack/react-table` 9 + `@tanstack/react-virtual` 3 for orders, fills and tape | Keep server-side paging; virtualise within a page |

Copyleft exposure of this stack: **none** — every recommended dependency is MIT, Apache-2.0,
ISC or BSD-3-Clause. The only copyleft projects evaluated (NautilusTrader LGPL-3.0, Grafana
AGPL-3.0) and the source-available ones (vectorbt Commons Clause, TimescaleDB TSL) are excluded or
used only as unmodified external services.

## 8. Unverified or open items

- Whether Neon Object Storage is available on Vercel-managed Neon projects and how it is metered.
- Whether ranged reads of a private Vercel Blob are served from CDN cache (simple-op cost) — the
  docs show Range on a public URL only.
- What Vercel Blob does on Hobby when an allowance is exhausted (block vs overage) — not read.
- Whether Render sends SIGTERM with a grace period on free-tier spin-down (the lease design does
  not depend on it).
- Rust-side memory for decoding a real-size run with serde (only the Arrow figure was measured).
- Lean's fill/slippage interfaces and qstrader's README were not read in depth; their decisions do
  not depend on them.

## Reproduce

```bash
# repository metadata (repeat for each repo in section 2)
gh api repos/<owner>/<repo> --jq '[.full_name,.stargazers_count,.pushed_at,.license.spdx_id,.archived]|@tsv'
gh api "repos/<owner>/<repo>/commits?per_page=1" --jq '.[0].commit.committer.date'
gh api repos/<owner>/<repo>/releases/latest --jq '[.tag_name,.published_at]|@tsv'
gh api repos/<owner>/<repo>/license --jq .content | base64 -d | head
# crates and npm
curl -s -A "<ua>" https://crates.io/api/v1/crates/<name> | jq '.crate.max_stable_version, .versions[0].license'
curl -s https://registry.npmjs.org/<pkg>/latest | jq '.version, .license'
# bundle size (scratch dir with the packages installed)
web/node_modules/.bin/esbuild entry.js --bundle --minify --format=esm --external:react --external:react-dom --outfile=out.js
gzip -9 -c out.js | wc -c
```

Artefact-size figures in section 4 came from loading the two named `data/runs/<id>/run.json` files
in Python, re-serialising the tape compactly, and writing it with `gzip.compress(level=6)` and
`pyarrow.parquet.write_table(compression=...)`; only byte counts were recorded.
