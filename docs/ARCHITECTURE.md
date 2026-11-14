# Architecture

BlackBox HFT replays recorded or synthetic Level-2 market data through the pinned
`hftbacktest =0.9.4` engine, records every simulated order's lifecycle, and explains and compares
the outcomes. This page describes how the code is split, how a run flows through it, and why it is
built this way. Engine internals are in [EXECUTION_MODEL.md](EXECUTION_MODEL.md); the models are
in [QUEUE_MODELS.md](QUEUE_MODELS.md) and [LATENCY_MODEL.md](LATENCY_MODEL.md).

## Layers

```mermaid
flowchart TB
  WEB["web/ (React + TypeScript)"] -->|"HTTP JSON /api/v1"| APP
  CLI["blackbox CLI"] --> APP
  subgraph APP["bb-app"]
    SERVER["v1/ (axum, auth, admission)"] --> DB["bb-db (Postgres queue and metadata)"]
    DB --> WORKER["worker.rs"]
    WORKER --> PIPE["pipeline.rs"]
    WORKER --> STORE["bb-artifacts (verified bundles)"]
    PIPE --> PRESETS["presets.rs, demos.rs"]
    PIPE --> DS["datasets.rs (dataset cache)"]
  end
  PIPE --> FOR["bb-forensics"]
  PIPE --> SIM["bb-sim"]
  PIPE --> STRAT["bb-strategies"]
  DS --> DATA["bb-data"]
  SIM --> HBT["hftbacktest 0.9.4 (backtest feature)"]
  DATA --> HBT
  FOR --> CORE["bb-core (type contract)"]
  SIM --> CORE
  STRAT --> CORE
  DATA --> CORE
  APP --> CORE
```

| Crate | Responsibility | Depends on | Start reading at |
|---|---|---|---|
| `bb-core` | The shared type contract: configurations, flight events, results, manifests, explanations, comparisons, hashing, PRNG, time units. No engine dependency, so strategies and forensics stay engine-agnostic. | `serde`, `sha2` | `src/lib.rs` (conventions), `src/flight.rs`, `src/config.rs` |
| `bb-data` | Tardis CSV ingestion, normalization (ported from py-hftbacktest), data-quality checks, synthetic generator, `.npz` store, book reconstruction for replay views. | `bb-core`, `hftbacktest` (event type and `.npz` format) | `src/lib.rs`, `src/tardis.rs`, `src/quality.rs` |
| `bb-strategies` | Deterministic strategies: touch maker, Avellaneda-Stoikov, GLFT, imbalance taker, scripted. Pure state machines over a `LocalView`. | `bb-core` only | `src/lib.rs`, [STRATEGIES.md](STRATEGIES.md) |
| `bb-sim` | Assembles the engine with recording wrappers, runs the strategy on a fixed tick grid, applies pre-trade risk rules, keeps the ledger, emits the tape. | `bb-core`, `hftbacktest` | `src/lib.rs` (`simulate`), `src/engine.rs`, `src/taps.rs`, `src/tape.rs` |
| `bb-forensics` | Pure functions over a finished tape: order records, metrics, markouts, rule-based explanations, scenario comparison. | `bb-core` only | `src/lib.rs`, `src/explain.rs`, `src/compare.rs` |
| `bb-app` | v1 HTTP/auth/admission, leased worker, presets and demos, sweeps, offline pipeline and `blackbox` CLI. | all of the above, `bb-db`, `bb-artifacts`, `bb-analytics` | `src/v1/mod.rs`, `src/worker.rs`, `src/main.rs` |
| `web/` | Console: Mission Control, datasets and market replay, strategy lab, runs, replay terminal, order forensics, latency lab, compare, grid. | the HTTP API ([API.md](API.md)) | `web/src/App.tsx` |
| `bb-db` | Postgres metadata, workspace-scoped access, keys, quotas and leased job queue. | `bb-core` | `src/queue.rs`, `src/api_reads.rs` |
| `bb-artifacts` | Verified paged bundles, filesystem and private Blob storage, byte-bounded cache. | `bb-core` | `src/bundle.rs`, `src/store.rs` |
| `bb-analytics` | Market microstructure, execution and risk analytics. | `bb-core`, `bb-data` | `src/lib.rs` |

The dependency rule is visible in the `Cargo.toml` files: only `bb-sim` and `bb-data` link the
engine, and `bb-forensics` and `bb-strategies` depend on nothing but `bb-core`. A forensics bug can
therefore never change a fill, and a strategy cannot call the engine.

## The flow of one run

```mermaid
sequenceDiagram
  participant C as Client
  participant A as v1 admission
  participant D as Postgres
  participant W as Worker
  participant E as bb-sim + engine
  participant B as Bundle store
  C->>A: POST /api/v1/experiments {config}
  A->>A: validate; identity = sha256(contract, simulation revision, engine, dataset sha256, config)
  A->>D: atomic workspace/quota/single-flight submission
  D-->>C: existing run (200) or queued job (202)
  W->>D: claim with lease
  W->>E: simulate admitted config with budgets and cancellation
  E-->>W: tape, fills, equity, engine cross-check
  W->>W: forensics, metrics and result hash
  W->>B: write verified paged bundle
  W->>D: commit run metadata and job completion
  C->>A: GET /api/v1/runs/{id}
  A-->>C: verified stored result
```

The v1 path is `admission` → `bb-db` queue → `worker`; the DB-free `blackbox run` path uses
`pipeline::find_or_execute` → `execute`. Both call `bb_sim::simulate`. Inside `simulate` the loop is:
`elapse(interval)` on the engine; read the local depth, own orders and trades received since the
last tick; build a `LocalView`; call `Strategy::on_tick`; check each intent against the risk rules;
submit survivors with `submit_buy_order` / `submit_sell_order` / `cancel`; sample equity. Every
engine callback that matters writes to the shared `Tape` through a wrapper
([EXECUTION_MODEL.md](EXECUTION_MODEL.md#how-the-flight-recorder-observes-the-engine)).

After the run, every view is computed from stored data: an order's explanation
(`bb_forensics::explain_order`), a comparison of two runs (`compare_runs`), the replay terminal's
timeline. None of them re-runs the engine.

## Why Rust and axum rather than Python and FastAPI

The brief preferred a Python/FastAPI backend. BlackBox deliberately deviates (recorded in
`TODO.md`, "Deliberate deviations"):

- **The engine is a Rust crate.** The recording wrappers have to implement the engine's Rust
  traits (`QueueModel`, `LatencyModel`, `FeeModel`, `Processor`, `LocalProcessor`) and sit inside
  the engine's own call stack ([EXECUTION_MODEL.md](EXECUTION_MODEL.md#how-the-flight-recorder-observes-the-engine)).
  A Python backend would still need this Rust layer and would only add a boundary around it.
- **One process, no serialisation boundary.** With a Python service the tape would be produced in
  Rust and serialised across a process or FFI boundary before the ledger, forensics and metrics
  could see it. Keeping everything in Rust means the ledger is updated at the exact engine step
  where the fill happens ([PNL_ACCOUNTING.md](PNL_ACCOUNTING.md)) and the types the UI receives are
  the types the simulator produced (`crates/bb-core`, serialized by serde).
- **Correctness checks are cheap to keep on.** Determinism, invariants and reconciliation tests
  run the real engine in the normal `cargo test` ([TESTING.md](TESTING.md)).
- **axum** is a thin async HTTP layer. Submitted simulations run on the worker's blocking
  execution path, not in an HTTP handler (`crates/bb-app/src/worker.rs`).

What is given up: Python's notebook ecosystem. A keyed export
(`GET /api/v1/runs/{id}/export`) returns a verified `.bbz` gzip-NDJSON bundle, and the dataset event file is the engine's own `.npz` format, which
py-hftbacktest reads (`crates/bb-data/src/store.rs`).

## Determinism by design

Reproducibility is a property of the design, not of luck. The mechanisms, each explained where it
lives:

| Source of nondeterminism | Design answer | Where |
|---|---|---|
| Engine iterates its order `HashMap` when one event fills several orders | Recorder buffers each engine step and commits it sorted by order id | `crates/bb-sim/src/tape.rs`; [EXECUTION_MODEL.md](EXECUTION_MODEL.md) defect 1 |
| Random latency drawn from a stream in engine-dependent order | Jitter is a pure function of `(seed, order id, time)` | `crates/bb-sim/src/taps.rs`; [LATENCY_MODEL.md](LATENCY_MODEL.md) |
| Strategy decisions timed by execution speed | Fixed local-time tick grid anchored to the dataset; order id = `(tick << 8) \| slot` | `crates/bb-core/src/strategy.rs` |
| Hash-map ordering in serialized output | Hashed types use structs, `Vec` and `BTreeMap` only; JSON field order is declaration order | `crates/bb-core/src/hash.rs` |
| Float parsing drift when a stored run is reloaded | `serde_json` with `float_roundtrip` | `crates/bb-app/Cargo.toml` |
| Platform math libraries | Results carry a libm-fingerprinted platform class; probability queues and `ln`/`exp` paths can differ across platforms | `crates/bb-core/src/platform.rs`; [EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md#platform-policy) |

How identity and result hashes are formed and what evidence exists is in
[EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md).

## Storage layout

The following layout is for the **offline/deprecated file-store pipeline**, not hosted v1.
It is under `--data-dir` (default `data/`), which is git-ignored because vendor data must not be committed.

```
data/
  raw/<exchange>/<symbol>/<day>/     downloaded vendor files (scripts/fetch-tardis-sample.sh)
  datasets/<id>/dataset.json         DatasetManifest: provenance, instrument, quality report, hashes
  datasets/<id>/events.npz           engine event array (zip with one data.npy); its SHA-256 is the dataset hash
  runs/<run_id>/summary.json         RunSummary: manifest, metrics, counts, label
  runs/<run_id>/run.json             RunResult: everything, including the tape
```

The run store is behind the `RunStore` trait (`crates/bb-app/src/store.rs`); `FsRunStore` keeps one
directory per run and decodes at most 16 full runs in memory. Run ids are validated as 16 lowercase
hex characters before they touch the filesystem. Datasets are loaded once, hash-verified, and shared
by every run, replay request and grid cell (`crates/bb-app/src/datasets.rs`).

## What runs where

- **Hosted:** the Vercel SPA calls the Render `/api/v1` service and its in-process worker;
  metadata and leases live in Postgres, verified bundles in private Blob storage.
- **Local v1:** `blackbox serve` uses Postgres plus filesystem artefacts and serves `web/dist`.
  [Docker Compose or the local commands](DEPLOYMENT.md#local) start this supported console stack.
- **Offline/legacy:** `blackbox run`, `explain`, `compare` and `grid` use local files without a
  database. The deprecated `/api` router is only a local-profile `serve` fallback.
- **Network:** hosted storage and database connections are outbound network traffic. The engine
  has no `live` feature or exchange trading connectivity.
- **Threads:** each simulation is single-threaded; queued sweeps fan out into ordinary jobs on
  the same worker pool. Offline grids have a separate local threaded CLI path.
- **Browser:** the UI renders API results; it computes no fills, P&L or explanations.

The hosted stack and Docker image are implemented. See [DEPLOYMENT.md](DEPLOYMENT.md),
[TARGET_ARCHITECTURE.md](engineering/TARGET_ARCHITECTURE.md) for queue/bundle design and
[PROJECT_STATE.md](engineering/PROJECT_STATE.md) for deployment provenance.

## Conventions that cross every boundary

Defined once in `crates/bb-core/src/lib.rs` and summarised for API users in [API.md](API.md):
timestamps are nanoseconds relative to the dataset's `origin_ns` (absolute epoch nanoseconds do not
fit a JavaScript number); every event names its clock (exchange or local) and its source (engine,
strategy, recorder, derived); prices are integer ticks; quantities are multiples of the lot size;
nothing in these types represents a real exchange execution.
