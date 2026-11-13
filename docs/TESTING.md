# Testing

What the test suite proves, what it pins, and what it does not cover. Run everything with:

```
cargo test --workspace
```

Every test uses synthetic or hand-built data; no vendor data is needed or used. Most adapter tests
run the real pinned engine, not a mock.

## Inventory

Counts from `cargo test --workspace --no-fail-fast` in the debug profile, Windows
(x86_64-pc-windows-gnu): **241 tests, 0 failed, 0 ignored** at the commit that added the static
viewer exporter and the input-validation fixes from the final review. The per-suite rows below
were taken earlier at 235 tests; re-run the command for current numbers.

| Crate | Suite | Tests | What it proves |
|---|---|---:|---|
| `bb-core` | unit | 11 | Clock and source are derived from the event body and cannot disagree; serialized tags match type names; SHA-256 known vector; SplitMix64 reference vector, PRNG ranges and stream independence; order id round-trips and sorts by tick; time units are inferred only when unambiguous and a wrong unit is rejected, not converted |
| `bb-data` | unit | 49 | Tardis conversion (snapshots, mid-file snapshots, a snapshot at end of file, missing initial snapshot, skewed clocks, wrong header, malformed rows with file and line, gzip, window limit, an over-long line refused); exact-decimal grid checks; normalization port (negative-latency shift, dual ordering, row splitting, tie-breaking, validation); every quality check's detection and severity; synthetic generator determinism and book invariants at every row; store round trip and hash integrity; book reconstruction and replay windows |
| `bb-strategies` | unit | 41 | Each strategy's quotes and intents on hand-built views: touch-maker join, offset, skew, replace-only-when-moved, gates; Avellaneda-Stoikov symmetry at zero inventory, inventory shading, γ widening, volatility scaling, warm-up, grid rounding, never crossing; imbalance taker entry, threshold, cooldown, hold and exit; scripted placement, price modes, cancel timing; shared rules (one place per slot, never cancel a non-cancellable order, same views → same intents) |
| `bb-sim` | unit | 9 | Ledger arithmetic and the cash identity; jitter is a pure function within bounds; the queue probe reads the real engine models' state bit for bit and reports unreadable formats as unreadable |
| `bb-sim` | `tests/engine_adapter.rs` | 20 | Recorder behaviour on hand-built markets where every value is worked out on paper: maker fills by trade-through and by queue exhaustion, taker IOC, post-only expiry, taker sweep one execution per level, cancels (resting, refused in flight, losing the race to a fill, late rejection after terminal), partial fills with `fill_reset`, queue-model differences, entry latency deciding fills, feed offset delaying only the view, risk rules before the engine and hard limits, drawdown halt and its use of locally known fills only, queue-update cap, in-flight at end of data |
| `bb-sim` | `tests/engine_defects.rs` | 9 | The five upstream defects reproduced on the **unwrapped** engine, and the adapter's workaround for each ([below](#upstream-defect-pins)) |
| `bb-sim` | `tests/determinism.rs` | 3 | Simultaneous fills recorded in a reproducible order (40 repeats per configuration); seed changes jitter and nothing else; tick grid independent of execution assumptions |
| `bb-sim` | `tests/invariants.rs` | 3 | The tape grammar across a sweep of strategies and models ([below](#invariants-sweep)); feed offset keeps the invariants; fill side moves position the right way |
| `bb-sim` | `tests/properties.rs` | 10 | Property-based tests on generated markets and request streams ([below](#property-based-tests)) |
| `bb-forensics` | unit | 55 | Order records from tapes; every explanation verdict and its wording rules (observed versus modeled versus derived, unknown queue is unknown, no invented counterfactuals); metrics against a hand-computed example, markouts stopping at end of data, nearest-rank quantiles; comparison eligibility (each refusal and not-like-for-like reason), timing-aware pair classes (decision time, exchange arrival, missing orders, shifted ids, divergent fills, identical fills under distinct histories, fee-only pairs, the audit's order-9472 fill-timing case), timing summaries, capped storage and the exact fee-only attribution; result hash stability |
| `bb-forensics` | `tests/real_tapes.rs` | 3 | Orders, explanations, metrics and a self-comparison over tapes produced by the real recorder, not hand-written events; a queue-model switch where an independent check confirms that a pair is `identical` exactly when records and lifecycles match, so nothing before the first divergence differs; a 5 ms versus 10 ms interval refused alignment |
| `bb-app` | unit | 19 | Host and origin admission for the local server; dataset ids cannot escape the data root; API error codes; preset catalogue (valid, unique, whole lots, one assumption per variant, the A-S half-spread scaling, the probe script); replay query defaults and caps; run-id shape enforced before the filesystem |
| `bb-app` | `tests/integration.rs` | 13 | Every preset executes; pipeline determinism and identity covering every assumption; run store round trip; grids store every cell; oversized grids and degenerate intervals refused; demos vary exactly their declared paths and run idempotently from the CLI; report and export reproduce the stored run; replay-terminal timeline windows; HTTP API end to end; web UI served with SPA fallback |
| `bb-app` | `tests/golden.rs` | 1 | Pinned result hashes of seven preset runs on a fixed synthetic dataset, tied to `SIMULATION_REVISION` ([EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md#keeping-identity-honest)) |

CI (`.github/workflows/ci.yml`, Ubuntu 24.04) runs `cargo test --workspace --locked`, a
cross-process determinism check of `maker_baseline` on a freshly generated synthetic dataset, and
the web build.

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
coverage claim is reproducible. The market-order defect (5) was found by these properties.

## What is not tested

- **Real vendor data in CI.** Tests never read the Tardis sample (it may not be redistributed).
  Real-data behaviour is evidenced by measured runs documented in
  [MARKET_DATA.md](MARKET_DATA.md#measured-findings-on-the-real-sample) and
  [PNL_ACCOUNTING.md](PNL_ACCOUNTING.md), not by automated tests.
- **Whether the models are right.** Tests prove the simulator does what the models say; no test can
  show that a queue or latency model matches a real venue.
- **The web UI.** The web build is checked in CI; there are no component or browser end-to-end
  tests yet (Playwright is planned, `TODO.md` M6). Browser verification is manual.
- **Cross-platform equality for Avellaneda-Stoikov**, and debug versus release hash equality
  ([EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md#caveats)).
- **Performance regressions.** Benchmarks are run by hand ([PERFORMANCE.md](PERFORMANCE.md)); no
  test fails on a slowdown.
- **The ingestion of the full 250 MB Tardis book file**, other exchanges, or other Tardis data
  types.
- **Engine paths BlackBox does not use**: L3 models, `IntpOrderLatency`, inverse contracts, the
  `live` feature.
