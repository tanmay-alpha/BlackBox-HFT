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

## Sweeps and overfitting

A sweep is a registry entry, not an optimiser. `bb_core::sweep::SweepSpec` declares before
anything runs every configuration that will be tried, the one objective that will rank them
and, optionally, an in-sample (IS) / out-of-sample (OOS) split. `bb_forensics::analyze_sweep`
then reports how many configurations were tried, what the objective selected and how much of
its apparent edge selection alone can explain. It never searches further, never reports an
annualised Sharpe ratio (§5 of [MARKET_MICROSTRUCTURE.md](MARKET_MICROSTRUCTURE.md)) and never
drops a failed configuration from the count.

### The spec

- **Design.** `grid`: one or two `GridAxis` (the same parameters and semantics as the existing
  sensitivity grid, applied by value), cells row-major with the first axis varying fastest.
  `random`: `samples` draws over declared domains (integer `range` with a step or `set`;
  fee `uniform` or `set`; queue-model `set`) with a `seed`. Discrete combinations are drawn
  uniformly without replacement by rejection; when `samples` exceeds the discrete combinations
  (only possible with a continuous parameter) every combination is used once per pass before
  any repeats. Continuous parameters draw from one seeded stream per parameter name, so adding
  a parameter never changes another's values. The generator is the project's own xoshiro256**
  (`bb_core::rng`); the same spec gives the same cells on every platform, and a test pins the
  first draws of seed 42. A repeated value, a parameter swept twice, or `order_latency_ns` with
  `entry_latency_ns` is refused: a duplicate configuration is a wasted trial that still counts.
- **Partition.** Two half-open exchange-clock windows `[start, end)` relative to the dataset
  origin. OOS must start at or after IS ends (a gap is allowed); overlap or OOS before IS is
  refused, so the selection never sees OOS data.
- **Objective.** One of `net_pnl`, `realized_pnl`, `fees`, `spread_capture`,
  `pnl_per_turnover_bps`, `max_drawdown`, `fill_count`, `fill_ratio_orders`,
  `mean_abs_position`, with `maximize` or `minimize`. Only the first three are running totals
  over time ("time-additive") and can be split into per-block contributions.
- **Fingerprint.** SHA-256 of `("blackbox-sweep-spec", SWEEP_SPEC_VERSION, spec)` in canonical
  JSON. The objective, seed, partition and CSCV blocks are part of it: changing the objective
  after seeing results is a different sweep.
- **Admission** (`SweepSpec::admit`). Structural checks first and alone (more than 100
  configurations is refused before a single value is examined), then the deployment's cell cap
  (hosted 25, local 100), every cell's `RunConfig::validate`, the windows against the dataset's
  span, `estimate_work` per run against the per-run `ExecutionLimits`, and the sum over all runs
  against a sweep total of `max_cells ×` the per-run limit (ticks, equity samples, tape events,
  output bytes). A partitioned sweep runs each cell twice on shorter windows and stays inside
  the same envelope. A windowed run is estimated on the parent manifest with `start`/`end`
  clipped to the window and the parent's full row count, an upper bound.

### Windows run on derived datasets

`RunConfig` has no window. `bb_data::slice::slice(parent, window)` derives a self-contained
dataset whose id `TimeWindow::slice_dataset_id(parent_id, parent_events_sha256)` is a pure
function of the parent's content hash, the window and `DATASET_SLICE_VERSION`;
`SweepSpec::admit` writes that id into each windowed run's configuration. Rules:

- The parent's exchange-clock book after every exchange row with `exch_ts < start`, and its
  local-clock book after every local row with `local_ts <= start`, are emitted first as a
  clear plus one snapshot row per level at `exch_ts = start`, `local_ts = start + 1`. Equal
  books share one snapshot; when updates are in flight at `start` the two clocks get separate
  snapshots, so the strategy's view after `start` is the parent's, not one that sees in-flight
  updates early.
- Exchange-clock rows: exactly those with `start <= exch_ts < end`. Local-clock rows: those
  with `local_ts > start` and `exch_ts < end`, including rows that happened before `start` but
  arrive after it (kept on the local clock only, counted in the provenance).
- Rows keep the parent's order, values and origin, so relative timestamps mean the same instant
  in the parent, every slice and the sweep's windows. The manifest's `generator` holds
  `{"slice": {slicing_version, parent_id, parent_events_sha256, window, snapshot level counts,
  carried_in_local_rows, parent_generator}}`; `source.inputs` names the parent's event file and
  hash.
- Quality checks that are computed from the events (crossed book, gaps, feed latency, trade
  diagnostics, event order) are recomputed on the slice. Checks that came from the source files
  (out-of-order source rows, trade ids, duplicate rows, precision, the local-time shift) cannot
  be recomputed and are copied from the parent, labelled as whole-span parent counts. A
  `slice.boundary` check describes the snapshot and carried-in rows.
- Limits. The snapshot reproduces `L2Book`'s literal levels; after a crossed parent feed the
  engine's own depth can differ. A strategy starts each window with fresh state (no warm-up
  from earlier data): this is what keeps OOS independent of IS, and it means indicators with
  long memory start cold in both windows.

Tests (`crates/bb-data/src/slice_tests.rs`): a slice of the whole span equals the parent row
for row and has the same BBO series; at an odd window start with updates in flight, the full
book on both clocks equals the parent's at the start and at every timestamp afterwards (until
the parent delivers rows from after the window); no exchange row lies outside `[start, end)`
and every local-only row before the start is a counted carry-in; slicing twice and saving
gives identical events, manifest and event-file hash, and the derived dataset loads verified.
`crates/bb-forensics/tests/sweep_slices.rs` admits a partitioned 3 × 2 grid on generated data,
slices both windows, simulates all 12 runs, checks that nothing a run records precedes its
window and that the per-block values sum to each IS run's `net_pnl`, and builds the report.

### The report

Inputs per cell: the objective's IS value (on the whole dataset without a partition), its OOS
value, optional per-block IS values and an error. Every declared cell is a trial whatever
happened to it; cells with neither value nor error count as missing.

- **Selected cell.** Largest oriented IS value (values negated for `minimize`), lowest index
  on ties, with the number of tied cells. **OOS rank**: one plus the number of cells strictly
  better out of sample, out of the cells with an OOS value. **Degradation**: IS minus OOS in
  the objective's direction (positive means worse OOS), relative to `|IS|` when non-zero, and
  for time-additive objectives also per second of each window, because a 80 s total and a 40 s
  total do not compare.
- **Rank correlation.** Spearman (1904): Pearson correlation of average ranks (ties share the
  mean rank), over cells with both values; at least 3 cells; unavailable when either side is
  entirely tied. Hand fixture: IS `[1,2,3,4,5]`, OOS `[5,6,7,8,7]` gives `8/√95 = 0.820783`.
- **Dispersion.** Per set (IS, OOS): n, min, quartiles by linear interpolation between order
  statistics (Hyndman & Fan 1996, type 7), max, mean and sample standard deviation.
- **Selection bias.** `selected_z`: how many IS standard deviations the selected value lies
  above the mean of the completed cells. `expected_max_z_under_noise`: the expected maximum of
  `N` independent standard normals, `E[max_N] ≈ (1 − γ)·Φ⁻¹(1 − 1/N) + γ·Φ⁻¹(1 − 1/(N·e))`
  (Bailey, Borwein, López de Prado & Zhu 2014, Proposition 2.1, eq. 2.4; read 2026-11-14 in the
  April 2014 preprint). It is asymptotic: 1.575 against the exact 1.539 for N = 10 (the paper
  quotes 1.57), 2.531 against 2.508 for N = 100. `Φ⁻¹` is Acklam's rational approximation
  (relative error below 1.15e-9), tested against reference quantiles. The paper notes that
  independence makes this conservative; neighbouring grid cells are far from independent, so
  the yardstick is a scale, not a test.
- **PBO by CSCV** (Bailey, Borwein, López de Prado & Zhu 2017, Algorithm 2.3; read 2026-11-14
  in the February 2015 revision). Only when the spec declares `cscv_blocks = S` (even,
  4 ≤ S ≤ 16), the objective is time-additive, every one of the N ≥ 4 declared cells has S
  block values, and each block holds at least one equity sample. The selection window is cut
  into S equal time blocks (`TimeWindow::blocks`); `objective_blocks` turns a run's equity
  samples into the change of the running total within each block (a sample on a boundary opens
  the next block; the last block runs to the closing sample; the blocks sum to the run's total).
  For each of the C(S, S/2) choices of training blocks: the cell with the largest training sum
  is selected (lowest index on ties), its testing sum's average rank `r` (1 = worst) gives
  `ω = r/(N + 1)` and `λ = ln(ω/(1 − ω))`; PBO is the fraction with `λ ≤ 0`, the paper's
  `∫_{-∞}^0 f(λ)dλ` (its Definition 2.2 writes a strict `r < N/2`; the `λ ≤ 0` form is the one
  with expectation exactly 1/2 under noise for even N). Choices the paper leaves open and are
  ours: the performance statistic is the sum of the oriented objective (the paper allows any
  statistic computable on sub-samples); tie handling as above. PBO is computed on the selection
  window and is separate from the OOS holdout. The paper prints 12,780 combinations for
  S = 16; C(16, 8) is 12,870.
  Fixtures: constant per-cell rankings give PBO 0 (λ = ln 4 for N = 4); an identity matrix
  (each cell earns in its own block only) gives PBO 1 (λ = ln(3/7)); a hand-worked 4 × 4
  matrix gives exactly 2/6; 400 fixed-seed pure-noise matrices (N = 10, S = 8) average within
  0.05 of 1/2; a strong signal with small noise gives 0.
- **Not implemented.** White's (2000) reality check, Hansen's (2005) SPA test and the deflated
  Sharpe ratio. The report gives counts, ranks, the yardstick above and CSCV instead of a
  p-value.

### Caveats in the report

Each carries a stable `code`. `selection_bias` changes with the number of trials and the
holdout: one configuration ("nothing was selected"); several without a partition (the value
is chosen on the data that scores it, bias unmeasured); several with a partition (the OOS value
and its rank are the less biased numbers, but one holdout is one draw); OOS missing. From 10
trials on it adds the expected noise maximum and the selected cell's `selected_z`.
`incomplete_cells` counts failed and missing trials. `window_lengths` appears when the windows
differ in length (per-second values for time-additive objectives; otherwise not comparable).
`few_cells` below 3 completed cells. `not_a_significance_test` always. `cscv_assumptions` with
a PBO (exchangeable blocks, sum statistic, coarse below 10 cells). `no_annualised_sharpe`
always.

### References

- Acklam, P. J. (2003). An algorithm for computing the inverse normal cumulative distribution
  function. Technical note.
- Bailey, D. H., Borwein, J. M., López de Prado, M. and Zhu, Q. J. (2014). Pseudo-mathematics
  and financial charlatanism: the effects of backtest overfitting on out-of-sample performance.
  *Notices of the AMS* 61(5), 458–471.
- Bailey, D. H., Borwein, J. M., López de Prado, M. and Zhu, Q. J. (2017). The probability of
  backtest overfitting. *Journal of Computational Finance* 20(4), 39–69.
- Hansen, P. R. (2005). A test for superior predictive ability. *Journal of Business &
  Economic Statistics* 23(4), 365–380. (Not implemented.)
- Hyndman, R. J. and Fan, Y. (1996). Sample quantiles in statistical packages. *The American
  Statistician* 50(4), 361–365.
- Spearman, C. (1904). The proof and measurement of association between two things. *The
  American Journal of Psychology* 15(1), 72–101.
- White, H. (2000). A reality check for data snooping. *Econometrica* 68(5), 1097–1126. (Not
  implemented.)
