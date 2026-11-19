# BlackBox HFT

A market-microstructure and execution research laboratory. It replays Level-2 market data through the
[HftBacktest](https://github.com/nkaz001/hftbacktest) engine, records the complete lifecycle of every simulated
order, and answers: **why did the strategy behave differently when latency, queue position, fees or risk changed?**

**Start at `/`: the public latency experiment** (no sign-in). One strategy at 1 ms and 50 ms order latency: the
answer first (both runs lose; the 50 ms run loses less because it fills less and so pays less in fees, which is not
evidence that slower is better), then the equity curves, the first divergence, the replayed book with both runs'
orders, and each order's lifecycle in both scenarios. The console has five sections:
**Explore** (Research home at `/research`, latency experiment at `/`, all experiments at `/experiments`) ·
**Research Studio** (Strategy Lab, Jobs & Sweeps, Execution studies at `/studies`, Queue model lab at `/queue-lab`,
Reports; submitting needs an API key) · **Market Lab** (Datasets, Order Book Replay, Market Data Workbench at
`/market`) · **Execution Analysis** (Runs & Forensics, Comparisons, Risk & P&L) · **System** (worker, queue, readiness).

- **Event-complete replay.** Run timelines are cursor-paged and byte-bounded; a replay window is loaded completely
  or labelled incomplete, never silently capped.
- **ITCH 5.0 local reconstruction.** `blackbox itch-reconstruct` rebuilds the order-by-order (L3) book of every
  stock in a NASDAQ ITCH file, checks its invariants, normalizes chosen symbols to Level 2 and writes a manifest.
  Local use only.
- **Synthetic-only hosted data.** The hosted service serves the generated `synthetic-demo` dataset; vendor and
  exchange data stay on your machine (their terms do not allow hosting).

**Research and simulation only.** BlackBox cannot connect to an exchange account, stores no trading credentials and
cannot place an order; simulated orders do not affect historical prices ([docs/LIMITATIONS.md](docs/LIMITATIONS.md)).
Rebuild record: [docs/rebuild/](docs/rebuild/) (start with [FINAL_REVIEW.md](docs/rebuild/FINAL_REVIEW.md)).

**Live system.** Console: **https://blackbox-hft.vercel.app** · API:
**https://blackbox-hft-api.onrender.com/api/v1** (OpenAPI at `/api/v1/openapi.json`). Anonymous
visitors get the read-only demo workspace; experiments are submitted as background jobs with an
API key and run on the hosted worker. Everything hosted is synthetic data on free tiers (cold
start ~1 minute after idle). Architecture and limits: [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md),
[docs/engineering/TARGET_ARCHITECTURE.md](docs/engineering/TARGET_ARCHITECTURE.md).

| What is real | What is simulated or modeled |
|---|---|
| The replay engine (hftbacktest 0.9.4), the recorded tape of every simulated order, the accounting, the job queue, persistence and integrity checks | Every order, fill and P&L figure; queue position (L2 data cannot reveal it); latency; no market impact; hosted data is synthetic |

## What it does

| Section | What you get |
|---|---|
| **Explore** | `/`: the latency experiment as one guided page: the question, the answer, a P&L breakdown (net = realized + unrealized − fees, checked against the stored values before it is shown), equity, the first divergence, the replay and order lifecycles, and the model assumptions in plain words. Scenario, order and time live in the URL, so deep links and Back keep them. `/experiments`: the catalogue of curated demo experiments. |
| **Research Studio** | **Strategy Lab** in Guided mode (strategy, order size, latency and fees, each explained) or Advanced mode: a strategy (touch maker, Avellaneda-Stoikov, GLFT, imbalance taker, scripted) and every execution assumption (exchange and queue model, constant / jittered / log-normal / heavy-tailed latency with separate cancel legs, think time, fees, risk limits, seed). **Jobs & Sweeps**: submit as a background job and follow it through queued, running and completed, or cancel it; `/api/v1/sweeps` accepts `SweepSpec` grid, seeded random and seed-ensemble designs, with optional in-/out-of-sample partitions. **Reports**: exportable, with configuration fingerprint, dataset version hash, platform class and result hash. |
| **Market Lab** | **Datasets**: dataset identity and a data-quality report per version. **Order Book Replay**: heatmap reconstructed from L2 updates, best bid/ask, trades, a depth ladder and microstructure metrics (spread, depth, imbalance, microprice, order-flow imbalance, realised volatility), with a run's own orders and fills on the same clock. |
| **Market Data Workbench** | `/market`: an order-level (L3) view of a generated synthetic ITCH session (`l3-demo`): event timeline, the book with per-order queue position at any event, native L3 metrics and an L3-versus-L2 comparison of the same session. Hosted and anonymous access is synthetic only; private ITCH files are inspected locally with `blackbox market workbench`. [docs/research/MARKET_DATA_WORKBENCH.md](docs/research/MARKET_DATA_WORKBENCH.md), [L3_L2_VALIDATION.md](docs/research/L3_L2_VALIDATION.md). |
| **Execution Model Validation** | `blackbox study exec-validation` changes one execution assumption (queue model, latency shape and direction, cancel delay, feed staleness) or the synthetic liquidity regime at a time against the `maker_baseline` preset. The `execution_validation` demo carries the same contrasts on `synthetic-demo`, `synthetic-stress-depletion` and `synthetic-stress-wide-spreads`; the order page's evidence ledger separates what the simulator knows from what it estimates. A model sensitivity study, not validation against real executions: [docs/research/EXECUTION_MODEL_VALIDATION.md](docs/research/EXECUTION_MODEL_VALIDATION.md). |
| **Execution Analysis** | **Runs & Forensics**: click any order and ask *why did this fill?* or *why did this not fill?* A two-clock timeline, the modeled queue trajectory, and a step-by-step explanation in which every statement is tagged Observed, Modeled, Derived or Unobservable. **Comparisons**: eligibility first (dataset, instrument, strategy, decision grid), then decision-aligned, timing-aware pair classes and the exact first divergence; sweep reports include selection-bias caveats and, where available, backtest-overfitting estimates or per-seed dispersion. **Risk & P&L**: equity, inventory, drawdown, fees and execution analytics (effective and realised spread, adverse selection, implementation shortfall, fill probability by queue position) with units and caveats; no annualised Sharpe where it would be meaningless. |
| **System** | Worker (idle / busy / not responding), queue and readiness status; admin tabs need a key. |

## Screens

All screenshots show the generated **synthetic** dataset; vendor market data is never published.
Apart from the latency experiment, they were taken before the rebuild and show the earlier navigation shell
(two-letter rail codes, inspector open); the current rail is labelled by section and the inspector starts closed.
More current screenshots: [docs/rebuild/img/](docs/rebuild/img/). The Phase 3.1 responsiveness and accessibility
pass (before/after at 360–1440 px) is recorded in [docs/rebuild/PHASE31_UI_REVIEW.md](docs/rebuild/PHASE31_UI_REVIEW.md).

| | |
|---|---|
| ![The public latency experiment at /: question, answer and P&L breakdown](docs/rebuild/img/final-1440-landing.webp) **Latency experiment (`/`)**: 1 ms versus 50 ms order latency, answer first. | ![Latency experiment: first divergence in the replay](docs/rebuild/img/final-1440-first-divergence.webp) **Latency experiment, first divergence**: the replay jumps to the first order whose timing differs. |
| ![Order Book Replay: reconstructed book with the run's own resting orders and simulated fills](docs/screenshots/terminal.png) **Order Book Replay (with run)**: the book reconstructed from L2 updates, with the run's own orders and simulated fills on one playback clock. | ![Runs & Forensics: two-clock timeline, modeled queue position and explanation](docs/screenshots/forensics.png) **Runs & Forensics (order)**: why one order filled, on the strategy and exchange clocks, with its modeled queue position. |
| ![Comparisons, Set: 1 ms versus 50 ms order latency](docs/screenshots/latency.png) **Comparisons (Set)**: the same strategy at 1 ms and 50 ms order latency, aligned decision by decision. | ![Comparisons, Pair](docs/screenshots/compare.png) **Comparisons (Pair)** |
| ![Comparisons, Sweep](docs/screenshots/grid.png) **Comparisons (Sweep)**: every cell is one stored simulation. | ![Order Book Replay, market only: heatmap and depth ladder](docs/screenshots/replay.png) **Order Book Replay (market only)** |
| ![Datasets: dataset identity and quality](docs/screenshots/datasets.png) **Datasets** | ![Runs & Forensics: run blotter, metrics and charts](docs/screenshots/run.png) **Runs & Forensics (run)** |
| ![Strategy Lab](docs/screenshots/lab.png) **Strategy Lab** | |

## The flight recorder

The engine exposes no event hooks, so each engine model (queue, latency, fees, order responses) is
wrapped in a recording type that delegates the decision to the real model and reports what
happened. No simulation logic is re-implemented. For each order the tape holds:

```
order_intent      strategy   local     what the strategy knew, and its stated reason
request_sent      engine     local     modeled entry latency, scheduled exchange arrival
exch_queued       engine     exchange  displayed quantity at the price, modeled quantity ahead
queue_update      engine     exchange  the trade or depth change that moved the estimate
exch_fill         engine     exchange  price, quantity, maker/taker, fee
exch_response     engine     exchange  status, modeled response latency
local_response    engine     local     when the strategy host learned of it
```

Every event names its clock and its source, so engine-generated facts are never confused with
BlackBox annotations or post-run derivations.

## Engine findings

Building the recorder meant reading the pinned engine closely. Six defects in hftbacktest 0.9.4
were found, each pinned by a test against the unwrapped engine, and each handled without forking
it: non-reproducible ordering of simultaneous fills, an abort after an exactly-completing partial
fill, an under-counting local account under partial fills, `submit_order` ignoring the order
side, market orders that can execute at a sentinel price or stop sweeping after 100 ticks, and a
rejected cancel of an already-filled order booking the fill a second time in the engine's own
account (found by the property tests).
Details and evidence: [docs/EXECUTION_MODEL.md](docs/EXECUTION_MODEL.md).

## Quick start

Prerequisites and platform notes are in [docs/BUILDING.md](docs/BUILDING.md).

With Docker (Postgres 18 and the all-in-one image, [docker-compose.yml](docker-compose.yml)):

```
docker compose up --build
```

The first start migrates the database, computes the demo runs, stores the curated demo comparisons and saved
reports, prints one research API key (`docker compose logs app | grep bbk_`; it is shown only once) and serves the
console and `/api/v1` on http://127.0.0.1:8787. Paste the key into the console to run your own experiments.

Without Docker, against an existing (empty) Postgres database you run yourself, one command builds the binary and the
console, migrates, seeds the demo workspace and serves the console and `/api/v1` on http://127.0.0.1:8787
([docs/DEPLOYMENT.md](docs/DEPLOYMENT.md#local) has the details and the individual steps):

```
# bash (Git Bash on Windows)
DATABASE_URL=postgres://user:password@127.0.0.1:5432/blackbox scripts/local-stack.sh
# Windows PowerShell
$env:DATABASE_URL = 'postgres://user:password@127.0.0.1:5432/blackbox'; scripts/local-stack.ps1
```

To run your own experiments, mint a key in a second terminal (same `DATABASE_URL`) and paste it into the console:
`target/release/blackbox --data-dir data keys mint --workspace lab --scopes read,run --label me`.

### Command line

```
blackbox serve | worker             API with in-process worker, or a worker only
blackbox migrate                    apply database migrations
blackbox keys mint|revoke|list      API keys (only their hash is stored)
blackbox demo seed [--wait]         register the synthetic dataset, queue (or with --wait compute) the demo
                                    experiments, then store the curated comparisons and saved reports
blackbox openapi --out spec.json    write the OpenAPI document
blackbox synth                      generate the deterministic synthetic dataset
blackbox ingest-tardis ...          convert Tardis.dev CSV files into a dataset (local use only)
blackbox itch-reconstruct <file>    rebuild a NASDAQ ITCH 5.0 order book and print its manifest (local use only)
blackbox run --preset <id>          run a simulation offline (no database)
blackbox market l3-synth --out <new dir>     write synthetic ITCH bytes and a workbench artefact
blackbox market workbench --input <itch> ... inspect a private ITCH file locally (outputs stay private)
blackbox study exec-validation --out <dir>   Phase 5 execution-model sensitivity study (offline)
blackbox study queue-shadow ...     L3 queue versus L2 queue-model shadow study (ITCH input stays local)
blackbox explain <run> <order>      explain one order
blackbox compare <run_a> <run_b>    compare two runs
```

Tests: `cargo test --workspace` (set `BLACKBOX_TEST_DATABASE_URL` to a Postgres database to run the
database and API suites), `cd web && npx vitest run && npx playwright test`.

### Real market data (optional)

```
scripts/fetch-tardis-sample.sh      # Tardis.dev sample day; run from Git Bash on Windows
blackbox ingest-tardis --trades ... --book ... --id ... --symbol BTCUSDT \
    --exchange binance-futures --tick-size 0.01 --lot-size 0.001 --max-duration-s 600
```

Vendor data is never committed or redistributed by this repository; `data/` is git-ignored.

Read the vendor's terms before using their data. As read on the survey date, the Tardis.dev terms
apply to the free samples too: they do not allow sharing the files, or files converted from them,
with others, and they do not allow using the data to offer others a backtesting or charting
service. Running BlackBox locally on files you downloaded yourself is research use; hosting a
public instance over vendor data is a different matter that needs the vendor's agreement. The
bundled demo and every test fixture use synthetic data only. Quotes and links:
[docs/OPEN_SOURCE_RESEARCH.md](docs/OPEN_SOURCE_RESEARCH.md). This is a summary, not legal advice.

## Reproducibility

A run is identified by a hash of the dataset bytes, the engine version, BlackBox's simulation
revision and the full configuration including the seed. On the same platform, running the same
experiment again returns the same `run_id` and the same `result_sha256` over the tape, fills and
equity series (verified across fresh processes and in CI). Across operating systems results are
identical only where the result path uses exact arithmetic: the engine's probability queue models
call the platform math library, and one configuration was observed to differ between Windows and
Linux. Details and evidence: [docs/EXPERIMENT_REPRODUCIBILITY.md](docs/EXPERIMENT_REPRODUCIBILITY.md).

## Repository layout

```
crates/bb-core        shared type contract: configs, flight events, results, manifests, limits, sweeps
crates/bb-data        ingestion, validation, book reconstruction, quality reports, synthetic generator, slicing
crates/bb-strategies  deterministic strategies (touch maker, Avellaneda-Stoikov, GLFT, imbalance taker, scripted)
crates/bb-sim         engine adapter, flight recorder, ledger, risk harness, run control and budgets
crates/bb-forensics   order records, explanations, metrics, comparison eligibility, sweep analysis
crates/bb-analytics   market microstructure, execution and risk analytics with documented fixtures
crates/bb-db          Postgres migrations, repositories, leased job queue
crates/bb-artifacts   verified paged run bundles; filesystem and Vercel Blob stores
crates/bb-app         /api/v1, auth, worker, CLI (binary: blackbox)
web/                  React + TypeScript research console
docs/                 design, models, data, testing, sources and licences; docs/engineering and docs/research
```

## Documentation

Start with [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md); [docs/INTERVIEW_GUIDE.md](docs/INTERVIEW_GUIDE.md)
explains the concepts from scratch and has a five-minute demo script.

| Document | What it covers |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | Layers, crates, data flow, determinism design, storage, why Rust/axum |
| [MARKET_DATA.md](docs/MARKET_DATA.md) | Sources, Tardis schema and units, normalization, quality checks, synthetic generator, provenance |
| [EXECUTION_MODEL.md](docs/EXECUTION_MODEL.md) | How the engine is used and observed, its six defects, the workarounds |
| [QUEUE_MODELS.md](docs/QUEUE_MODELS.md) | Every queue-position model's formula, a worked example, what L2 cannot reveal |
| [LATENCY_MODEL.md](docs/LATENCY_MODEL.md) | Feed, entry and response latency, the two clocks, jitter, the order bus |
| [STRATEGIES.md](docs/STRATEGIES.md) | The bundled strategies, Avellaneda-Stoikov derivation and scaling, no look-ahead |
| [PNL_ACCOUNTING.md](docs/PNL_ACCOUNTING.md) | Ledger, fees, markouts, spread capture, engine cross-check, reconciliation |
| [EXPERIMENT_REPRODUCIBILITY.md](docs/EXPERIMENT_REPRODUCIBILITY.md) | Run identity, result hash, manifests, how to reproduce a run |
| [TESTING.md](docs/TESTING.md) | Test inventory, what each suite proves, what is not tested |
| [LIMITATIONS.md](docs/LIMITATIONS.md) | What the simulation assumes and cannot know |
| [OPEN_SOURCE_RESEARCH.md](docs/OPEN_SOURCE_RESEARCH.md) | Prior art, inspected versions, licences, reuse decisions, data terms |
| [PERFORMANCE.md](docs/PERFORMANCE.md) | Measured replay cost versus the stock engine, resource use, API latency, scaling |
| [SECURITY.md](docs/SECURITY.md) | Threat model, controls with their tests, review findings, residual risks |
| [API.md](docs/API.md) | HTTP API |
| [BUILDING.md](docs/BUILDING.md) | Build requirements and Windows notes |
| [INTERVIEW_GUIDE.md](docs/INTERVIEW_GUIDE.md) | Concepts explained from scratch, and a demo script |
| [rebuild/](docs/rebuild/) | The product rebuild: [FINAL_REVIEW](docs/rebuild/FINAL_REVIEW.md), tracker, [next phases](docs/rebuild/NEXT_PHASES_TRACKER.md), baseline, prototype review, validation, latency sensitivity, performance, security review, reuse matrix |
| [research/](docs/research/) | Market Data Workbench, L3/L2 validation, execution model validation, queue-model shadow experiment, microstructure |

## Acknowledgements

Simulation engine: [HftBacktest](https://github.com/nkaz001/hftbacktest) by nkaz001 (MIT). The
Tardis CSV conversion and event-ordering algorithms are ports of its Python reference
implementation. Charts: [TradingView Lightweight Charts](https://github.com/tradingview/lightweight-charts)
(Apache-2.0).
