# Validation

## Analytics validation

`bb-analytics` implements the market/execution definitions in
[MARKET_MICROSTRUCTURE.md](MARKET_MICROSTRUCTURE.md), with its own
`ANALYTICS_VERSION`. Results are research measurements, not execution predictions.
Every scalar metric includes units, contributing and excluded counts, or an explicit
`Unavailable` reason. Inputs remain immutable; analytics never feed strategy decisions.

### Reference conventions

- Market states are observed after complete exchange-time batches. Clock grids use
  previous-tick interpolation, never a future state, and invalid intervals are not filled.
- Execution reference `m0` is strictly before a fill's exchange timestamp. A future
  horizon is unavailable beyond the dataset endpoint. The enriched `FillRecord.mid_tick`
  is not a valid replacement for this pre-fill reference.
- E3 is adverse-selection **cost**, not favourable drift: the specification's identity is
  **E1 = E2 − E3**, equivalently E1 = E2 + favourable drift. Tests use these signs.
- E9 uses one exchange-clock final mark. The existing simulator equity uses a local mark;
  reconciliation therefore re-marks the actual ledger cash and position at E9's mark,
  rather than claiming two different marking conventions are equal.
- Opportunity cost / implementation shortfall require directional intent and an explicit
  decision-price policy. An unfilled market-making quote is not automatically a loss.
- Modelled queue position and Kaplan–Meier describe the queue model plus replay data;
  they do not validate that queue model against real fills.

### Risk interpretation

Path statistics identify the supplied equity sampling and local-mid mark. Inventory is
weighted by elapsed time, not row count. Sharpe-type output requires at least 30 complete,
non-overlapping P&L buckets, reports the bucket horizon and number of trials, and uses a
Newey–West/Bartlett delta-method standard error with a short truncation lag. It is never
annualised. No IID error estimate is applied to 100 ms increments. A multiple-trial grid
still requires selection-bias analysis (prefer Deflated SR); no significance claim follows
from a single path. Invalid marks invalidate bucket increments that cross them.

### Quality-report diagnostics

`bb-data` retains the more-than-one-tick diagnostic threshold but now reports three
computed categories: `trades.beyond_touch_sweep` (info), `trades.book_lags_trades`
(warn), and residual `trades.outside_book` (warn). A sweep requires resting levels and
earlier same-side prints at the same exchange timestamp consuming all displayed depth
in front. The lag diagnostic searches completed book batches for at most 50 ms and
reports nearest-rank lag p50/p90 in the finding detail. A dataset ending early can truncate
this search. These are post-hoc diagnostics, not a timestamp repair or proof of bad data.

### Validation coverage

Unit fixtures derive their expected arithmetic in test comments, rather than obtaining
expected values from the analytics implementation. Market properties cover non-negative
uncrossed spread, imbalance bounds, the opposite-side weighted-mid bounds, and OFI
additivity. Execution properties cover monotone bounded KM survival and algebraic
spread/P&L identities.

`tests/pipeline.rs` generates and persists its own synthetic data, reloads it through
`DatasetCtx`, runs the existing `maker_baseline` preset using `pipeline::execute`, and
checks both per-order and per-run spread identities. It reconciles maker + taker net
P&L to the actual ledger cash, fees and inventory at the identical exchange final mark
to an absolute tolerance of `1e-9`. No recorded/vendor dataset is needed.

Risk tests cover elapsed-time weighting, invalid intervals, the insufficient-sample
gate on the ten-return research fixture, and a 30-bucket non-annualised ratio with HAC
uncertainty. Quality tests distinguish all three categories, require sufficient
same-timestamp same-aggressor sweep consumption, and test the inclusive 50 ms lag
boundary versus an update one nanosecond too late.

### Public API and bounds

- `market::compute_market(events, instrument, origin_ns, config)` computes summaries and
  optional capped event/clock series. M7/M9 interval views use the same bounded reduction
  policy; exact whole-window aggregates do not depend on those retained points.
- `market::calibrate_stoikov` and `StoikovModel::evaluate` require non-overlapping,
  explicitly recorded training/evaluation periods. M6 is unavailable without calibration,
  outside fitted states, or with insufficient state transitions. The LU solve and bounded
  convergent series are tested against the hand-solvable mirrored chain.
- `execution::compute_execution(run, bbo, instrument, config)` reads tape fills, returns
  per-order/run maker/taker results, configurable horizon cohorts, deterministic
  Poissonized time-block bootstrap bands, KM/Greenwood queue curves, cancel exposure,
  directional shortfall/opportunity distributions, and E9 components.
  `compute_execution_streaming(..., on_order)` delivers every order once to a sink and
  returns full-population run summaries without retaining the expanded details.
- `risk::compute_risk(equity, fills, instrument, net_pnl_quote, config)` takes an explicitly
  marked net P&L and reports sampled path statistics plus the gated bucket statistic.

Market memory is bounded by configured book-level, series-point, liquidity-trade matching,
and calibration-state limits; exceeding a resource bound is an error, not partial data.
Execution working memory is proportional to orders × configured horizons and bootstrap
replicates × horizons, not dataset rows. For host-budgeted callers, use the streaming
entry point and persist/select details in the sink. The collecting entry point explicitly
retains all per-order results and therefore adds output-size memory. The streaming
integration test checks every emitted order against collected output and checks every run
summary unchanged. Risk memory depends on the bounded HAC lag, not sample count.

The retained measurement example generates seed-7 synthetic data for 600 s, executes
`maker_baseline` once, and times three complete analytics calls over the same immutable
inputs. It enables 1,024-point series, both sides' 100 bp liquidity windows over [1,10] s
with zero matching lag (the synthetic generator stamps trades/depth together), and the
default execution horizons/512 bootstrap replicates. Every order detail is computed and
consumed by a non-retaining sink. It leaves M6 uncalibrated rather
than fitting/evaluating on the same data, and does not call market-making intents
directional for E4/E10. Dataset generation, persistence/loading and simulation are
outside each reported analytics duration. Peak process memory includes those inputs
and setup, not just the analytics working set; this is a host-budget measurement, not
an allocation proof.

### Verification (2026-11-14, Windows GNU)

- `cargo fmt --all`: passed.
- `cargo clippy --workspace --all-targets`: passed without warnings.
- `cargo test --workspace`: **303 passed**, including **45 analytics unit tests** and
  **3 generated-dataset pipeline integration tests**; no failures.
  The existing pinned result-hash/golden tests also passed.

### Memory finding and correction

The first release measurement retained all expanded per-order output. On the 600 s
workload it used **405.500 MiB peak working set** / **406.934 MiB peak private commit**,
with a best-of-three analytics duration of **1171.271 ms**. This exceeded the approximate
300 MB host target. The correction is a streaming detail sink, not dropped order
metrics or sampled run summaries: all E1–E10 order details are still computed, emitted
once, and eligible for persistence by the caller. Exact scalar distributions and KM
observations remain full-population. The collecting API remains available when a local
caller explicitly wants all expanded output resident at once.

### Corrected release measurement

One corrected workload invocation (three timed iterations), Windows x86-64 GNU release:

| Measurement | Observed value |
|---|---:|
| Synthetic duration / seed | 600 s / 7 |
| Event rows / tape events | 317,644 / 126,567 |
| Fills / equity samples | 6,273 / 6,001 |
| Analytics iteration 1 | 942.808 ms |
| Analytics iteration 2 | 922.644 ms |
| Analytics iteration 3 | 946.835 ms |
| **Analytics best of 3** | **922.644 ms** |
| Whole process (setup + simulation + 3 analytics calls) | 4,116.077 ms |
| **Peak working set** | **135,368,704 bytes (129.098 MiB)** |
| **Peak private commit** | **167,415,808 bytes (159.660 MiB)** |

Both measured memory high-water marks are below the approximate 300 MB host budget for
this workload; this does not establish a universal bound for arbitrary order counts or
caller-retained output. A process listing immediately before the corrected invocation
showed no other `cargo`/`rustc` process. Other agents' `rustc` compilations **were visible
during the initial collecting-output measurement**, so its runtime is not an isolated
performance comparison. No claim is made that the speed difference is solely the fix.

Reproduce with:

```sh
cargo build --release -p bb-analytics --example measure
target/release/examples/measure.exe
```

The example prints all three analytics durations and their minimum. The memory numbers
above were measured externally from process creation using Windows
`GetProcessMemoryInfo(PROCESS_MEMORY_COUNTERS_EX)` every 5 ms (604 successful samples):
`PeakWorkingSetSize` and `PeakPagefileUsage`, retaining the maximum reported values.
These OS-maintained high-water counters include dataset generation, store load, the
engine run, caller-owned inputs and all three analytics iterations. They exclude the
compiler and the monitoring process. No benches or vendor-data runs were performed.
