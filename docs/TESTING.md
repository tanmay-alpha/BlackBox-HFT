# Testing

What the test suite proves, what it pins, and what it does not cover. Run everything with:

```
cargo test --workspace
```

Every test uses synthetic or hand-built data; no vendor data is needed or used. Most adapter tests
run the real pinned engine, not a mock.

## Inventory

Current recorded verification (2026-11-14): **422 Rust tests passed, 1 ignored; 93 Vitest tests passed**.
The Rust total includes the database/API suites with local Postgres configured; see
[FINAL_AUDIT §7](engineering/FINAL_AUDIT.md#7-fixes-after-the-audit) for the recorded run.
Playwright has **3 browser stories**. CI jobs and their actual commands are defined in
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml), summarised below; Vitest is a separate
`cd web && npm test` check, not currently a CI step.

| Area | Suites / coverage |
|---|---|
| `bb-core`, `bb-data`, `bb-strategies` | Wire types, hashes, config and sweep validation; ingestion and book reconstruction; strategy fixtures |
| `bb-sim` | Ledger, engine adapter and six upstream-defect pins; determinism, invariants, properties, run control and raw-engine parity |
| `bb-forensics`, `bb-analytics` | Explanations, comparison eligibility and timing, sweep/ensemble analysis; hand-computed market, execution and risk analytics |
| `bb-artifacts` | Bundle hashes, bounded decompression, storage keys, byte-bounded cache and Blob mock-server behaviour |
| `bb-db` | Real-Postgres repositories, queue races, scoped reads, keys, quotas, upload reservations and transactional audit rows |
| `bb-app` | Offline pipeline, golden hashes, budgets and hosted peak memory; v1 auth/IDOR, jobs/cancellation/restart, integrity, demo workflow, sweeps and hardening |
| `web/` (Vitest) | Formatting, API/query state and workstation view-model fixtures |
| `web/e2e/` (Playwright) | Anonymous demo reads; authenticated submit/complete/forensics/cancel/reload; rejected-key handling |

Database/API Rust tests skip when `BLACKBOX_TEST_DATABASE_URL` is unset; they require a disposable
Postgres 18 database with `CREATEDB`. CI supplies it. Playwright needs a built `blackbox` binary
(`BLACKBOX_BIN`) and its own `DATABASE_URL`; the `e2e` job supplies both.

`bb-app/tests/strategy_kinds.rs` submits a preset for every `StrategyKind` through
`POST /api/v1/experiments`, checks the stored strategy label, and waits for the real worker to
complete each job. Its exhaustive variant match and preset coverage assertion guard against
Rust strategy variants missing from PostgreSQL's `experiment_configs_strategy_kind_check`
(migration 0011 adds `glft`).

## CI

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) runs on every push to `main` and every pull request.
Its jobs are `rust`, `golden-windows`, `policy`, `contract`, `web`, `e2e`, `audit` and `docker`.
The gate scripts live in `scripts/ci/` and run locally from the repository root (Linux, macOS, or Git Bash on Windows).

| Job | What fails it |
|---|---|
| `rust` (Ubuntu 24.04, Postgres 18) | `cargo build`/`cargo test --workspace --locked` (determinism, upstream-defect pins, linux-x86_64 golden pins, hosted peak memory); two processes giving different result hashes for `maker_baseline`; a dataset-manifest number beyond 2^53 − 1 (AT-9) |
| `golden-windows` | The windows-x86_64-gnu golden pins and the Windows peak-memory check |
| `web` | `npm ci`; `npm audit --omit=dev --audit-level=high` (a high or critical advisory in a production dependency) and `npm audit signatures` (an unverifiable registry signature or attestation) (SEC-21); `npm run build`; `secret-scan.sh dist web/dist` (below) |
| `policy` | `check-pins.sh`, `secret-scan.sh repo`, `code-policy.sh` (below); `cargo deny --locked check` against `deny.toml` (cargo-deny 0.20.2, downloaded and SHA-256-checked); `cargo tree -e features -i hftbacktest` showing the engine's `live` feature (SEC-21) |
| `contract` | `check-contract.sh`: contract drift (AT-14, below) |
| `audit` | `rustsec/audit-check` on `Cargo.lock`, with the documented ignore list |
| `docker` | The local compose stack must build, serve `/api/v1/status` and datasets, expose 5 curated comparisons and 3 saved reports, serve the console HTML and print the first-start research key |
| `e2e` | The 3 Playwright v1 stories against `blackbox serve` (release build, `BLACKBOX_BIN=target/release/blackbox`) on a Postgres 18 service database `blackbox_e2e_ui` |

The gates:

- **cargo-deny** (`deny.toml`, SEC-21, SEC-24): *advisories*: the RustSec database, ignoring only the
  accepted zip 6 and bincode advisories documented in [SECURITY.md](SECURITY.md) (the `audit` job's
  `libsqlite3-sys` ignore is unnecessary here: cargo-deny resolves real features, where that crate
  never appears); *licences*: MIT, Apache-2.0 (incl. LLVM exception), BSD-2/3, ISC, Zlib,
  Unicode-3.0, CDLA-Permissive-2.0, MIT-0 and bzip2-1.0.6 only; *bans*: `openssl`/`openssl-sys`
  and the code-execution crates `libloading` (allowed only under `clang-sys`, a build-time
  dependency of `bindgen`), `wasmtime`, `wasmer`, `pyo3`, `rhai`, `mlua`, `deno_core`; *sources*:
  crates.io only, no git dependencies. Run locally with `cargo deny --locked check`.
- **Pins** (`scripts/ci/check-pins.sh`, SEC-21): every `uses:` in `.github/workflows/` names a full
  40-hex commit SHA (local `./` actions excepted; `docker://` needs `@sha256:`), and every
  `Dockerfile` `ARG *_IMAGE=` and literal `FROM` image carries an `@sha256:` digest.
- **Secret scan** (`scripts/ci/secret-scan.sh`, SEC-19). `repo [<base>]` greps every tracked file,
  and every line added by the commits `<base>..HEAD` (the pull request's base, or the push's
  previous head), for credential *values*: a Vercel Blob token (`vercel_blob_rw_<store>_<secret>`),
  a Neon password (`npg_…`), a full 43-character `bbk_` API key, a `postgres://user:password@` URL,
  or `DATABASE_URL*`/`BLOB_READ_WRITE_TOKEN`/`BB_API_KEY_PEPPER` assigned a literal. Docs and code
  that only name the variables, `$VAR` references, `<placeholder>`s, and the local development
  password `blackbox-local` (plus the placeholders `password`, `p`, `supersecret`) pass. `dist <dir>`
  is stricter: the bundle may not even contain `vercel_blob_rw_`, `postgres://`, `npg_`,
  `BLOB_READ_WRITE`, `DATABASE_URL` or `BB_API_KEY_PEPPER`; `web/src` may read no
  `import.meta.env.VITE_*` other than `VITE_API_BASE_URL`; and `web/vite.config.ts` may not set
  `envPrefix`, `define` or call `loadEnv`. Findings print file and line, never the matched text.
- **Code policy** (`scripts/ci/code-policy.sh`): no `process::Command`, `Command::new`,
  `libloading`, `dlopen` or `LoadLibrary` in non-test crate sources (SEC-24); SQL from literals only
  (SEC-30): `AssertSqlSafe` (the only way sqlx 0.9 accepts a non-`'static` query string) appears
  only in `crates/bb-db/src`, every `format!` in a file using it interpolates only UPPER_CASE
  constants or the `&'static str` fragments of `sort_sql`, and `QueryBuilder`/`raw_sql` are absent.
- **Contract drift** (`scripts/ci/check-contract.sh <blackbox binary>`, AT-14): `blackbox openapi
  --out` must equal the committed `web/openapi.json`, and `npm run gen:api` must leave
  `web/src/api/v1.gen.ts` unchanged. After an API change, regenerate both
  (`blackbox openapi --out web/openapi.json`, then `npm run gen:api` in `web/`) and commit them.

## Upstream-defect pins

`crates/bb-sim/tests/engine_defects.rs` drives hftbacktest 0.9.4 through its own public builder,
without BlackBox's wrappers, and asserts the defective behaviour. Each test is evidence for a
workaround in [EXECUTION_MODEL.md](EXECUTION_MODEL.md#defects-in-094-and-what-blackbox-does-about-each),
and is built to **fail when an engine upgrade fixes the defect**, which is the signal to remove the
workaround.

| Defect | Upstream pin | Adapter test |
|---|---|---|
| 1. Simultaneous fills in non-reproducible order | `upstream_simultaneous_fill_order_is_not_reproducible` | `determinism.rs::simultaneous_fills_are_recorded_in_a_reproducible_order` |
| 2. `PartialFillExchange` aborts after an exact completion | `upstream_partial_fill_exchange_aborts_after_an_exact_completion` | `adapter_survives_an_exact_completion`, `adapter_completion_workaround_leaves_other_cases_unchanged` |
| 3. Local account under-counts partial fills | `upstream_local_account_undercounts_partial_fills` | `engine_adapter.rs::partial_maker_fills_are_all_counted_and_the_engine_gap_is_reported` |
| 4. `submit_order` ignores the side | `upstream_submit_order_ignores_the_requested_side` | (the adapter never calls it) |
| 5. Market orders: sentinel price, 100-tick sweep | `upstream_market_buy_on_empty_ask_fills_at_the_sentinel_tick`, `upstream_partial_fill_market_order_scans_only_100_ticks` | `adapter_blocks_market_orders_before_the_engine`, `properties.rs::market_intents_are_blocked_before_the_engine` |
| 6. A rejected cancel of a filled order books the fill twice in the engine's account | `upstream_rejected_cancel_of_a_filled_order_double_counts_the_fill` | `adapter_ledger_counts_the_fill_once_when_a_filled_order_is_cancelled` (the cross-check's "Expected" note); `latency_lab.rs::think_time_applies_to_cancels` (the think-time trigger); `properties.rs::instrumentation_matches_the_unwrapped_engine` expects exactly one duplicate per rejected-and-filled response |

## Determinism

- `determinism.rs` builds a market where one trade fills eight resting orders at once, in the
  engine step where 0.9.4 iterates its order `HashMap`, and repeats each configuration (both
  exchange models, constant and jittered latency) 40 times requiring an identical
  `sha256(tape, fills, equity)`. It also asserts the canonical order (fills by order id, running
  position 1, 3, 6, … 36).
- The invariants sweep and `properties.rs::identical_inputs_have_identical_hashes` run every case
  twice and compare hashes.
- The integration test compares two full pipeline runs field by field; the golden test compares
  against hashes fixed in the source.

Cross-process and cross-platform evidence is collected in
[EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md#evidence).

## Invariants sweep

`crates/bb-sim/tests/invariants.rs::bundled_strategies_obey_the_tape_grammar_under_every_model`
generates one well-formed random L2 market (12,000 events, seed 11), then runs every combination of
3 strategies (touch maker, Avellaneda-Stoikov, imbalance taker) × 2 exchange models × 6 queue models
× 2 latency models (constant 0.8 ms; 2 ms + up to 9 ms jitter) = **72 configurations**, each twice.
For every run it checks the grammar the forensics layer relies on:

- `seq` contiguous; every event's clock and source match its body.
- Every intent has exactly one disposition directly after it; a blocked or refused intent has
  nothing else.
- Per order and per clock, time never goes backwards.
- A new resting order's queue estimate equals the displayed quantity (joins behind, every model);
  queue updates only while resting and always readable.
- No fill after a terminal state, no empty fill, maker flag equals "was resting", fill value
  `= contract_size × price × qty`; a queue-exhausted fill directly follows the trade update that
  exhausted it; every fill is followed by its response.
- Scheduled local arrival = exchange time + response latency; at most one terminal response;
  `Filled` only when fully filled; never overfilled; no local response without an exchange response.
- Ledger: Σ signed fills = final position; running `position_after`; fills in tape order;
  `equity = realized + unrealized − fees` and `realized + unrealized = cash + position × mark` at
  every sample; sampled position within the risk limit.
- Under `no_partial_fill`, the ledger equals the engine's own local account.

The sweep asserts that maker fills, taker fills and partial fills each occurred somewhere, so it
cannot pass vacuously.

## Property-based tests

`crates/bb-sim/tests/properties.rs` uses `proptest` with shrinkable generated books, request streams
and execution assumptions (exchange model, queue model and exponent, constant or jittered latency,
feed offset, maker and taker fees, position and order limits):

| Property | Checks |
|---|---|
| `simulation_accepts_well_formed_markets` | no error or panic on any well-formed market |
| `quantities_and_order_states_are_legal` | lifecycle transitions and quantities are legal |
| `fills_reconcile_to_the_ledger` | running position matches the fills; closing cash, fees and position match their sums; `equity = realized + unrealized − fees` and the marked cash identity at every sample |
| `hard_risk_limits_are_never_exceeded` | position and order limits hold with no extra lot |
| `identical_inputs_have_identical_hashes` | determinism on generated inputs |
| `instrumentation_matches_the_unwrapped_engine` | the wrapped run's fills match a raw engine replay of the same requests |
| `sparse_books_and_zero_trades_are_safe` | empty periods, one-sided books, no trades |
| `single_event_dataset_is_safe` | a dataset of one event |
| `market_intents_are_blocked_before_the_engine` | market orders never reach the engine |
| `generated_corpus_exercises_execution_paths` | 512 generated runs from a fixed seed reach every path of interest (maker, taker and partial fills, accepted and rejected cancels, `fill_reset`, risk blocks, refusals, working and in-flight at end, one-sided books) |

The `proptest!` properties run 256 cases each with proptest's default random seed, so each run
explores different inputs; `generated_corpus_exercises_execution_paths` uses a fixed seed so its
coverage claim is reproducible. The market-order defect (5) and the double-booked fill on a
rejected cancel (6) were found by these properties.

## What is not tested

- **Real vendor data in CI.** Tests never read the Tardis sample (it may not be redistributed).
  Real-data behaviour is evidenced by measured runs documented in
  [MARKET_DATA.md](MARKET_DATA.md#measured-findings-on-the-real-sample) and
  [PNL_ACCOUNTING.md](PNL_ACCOUNTING.md), not by automated tests.
- **Whether the models are right.** Tests prove the simulator does what the models say; no test can
  show that a queue or latency model matches a real venue.
- **Exhaustive web coverage.** Vitest and the 3 real-v1 Playwright stories cover selected flows,
  not every component, interaction or browser.
- **Cross-platform equality for Avellaneda-Stoikov**, and debug versus release hash equality
  ([EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md#caveats)).
- **Performance regressions.** Benchmarks are run by hand ([PERFORMANCE.md](PERFORMANCE.md)); no
  test fails on a slowdown.
- **The ingestion of the full 250 MB Tardis book file**, other exchanges, or other Tardis data
  types.
- **Engine paths BlackBox does not use**: L3 models, `IntpOrderLatency`, inverse contracts, the
  `live` feature.
