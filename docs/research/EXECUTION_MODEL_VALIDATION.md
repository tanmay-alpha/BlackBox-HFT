# Execution Model Validation Lab

## Question

How sensitive is the existing touch-maker simulation to queue-position assumptions, the shape and direction of latency, cancellation delay, feed staleness, and explicitly synthetic liquidity regimes? This is a controlled model sensitivity study, not a validation against real executions and not a search for a profitable configuration.

## Reproduction and output contract

The study is an offline CLI command, with no HTTP route and no database writes. It only generates synthetic data inside a fresh output directory; existing output directories are refused. No restricted vendor data is loaded or redistributed.

```sh
CARGO_TARGET_DIR="D:/black box/target/exec" cargo build --release --locked -p bb-app --bin blackbox
target/exec/release/blackbox.exe study exec-validation --out data/studies/execution-validation
```

This run uses a separate Cargo target directory because other local research servers hold
the usual debug and release executables open on Windows. The ordinary release build command
(`cargo build --release -p bb-app --bin blackbox`) compiled the crates but could not replace
the locked executable; no server was stopped and no existing run artifacts were changed.

`--quick` selects a tiny smoke grid, not the research design. Outputs:

- `study.json`: design and baseline manifest; each cell's full run manifest (including configuration, dataset hash, config hash, result hash and platform), factor values, changed configuration fields, baseline-relative scalar deltas, first divergence and lifecycle class counts; aggregates; two-factor sensitivity tables stratified by regime.
- `study.md`: readable sensitivity tables. Stochastic entries are means with percentile bootstrap intervals; deterministic entries are single values.
- `cells/<run_id>.json`: full dataset and run manifests, ordinary run metrics, modeled queue-entry statistics, first/complete-fill distributions and Kaplan–Meier estimates, and the existing capped order-alignment comparison. The comparison preserves its truncation metadata.
- `datasets/<dataset_id>/`: generated manifest and hash-verified event archive.

All of these outputs live under `data/`, which version control ignores, so the figures below are reproducible by
re-running the commands (same binary platform, same seeds) rather than from committed files. `study.json` carries
wall-clock provenance, so its whole-file hash is not stable; every cell's config and result hashes are.

Each cell passes the ordinary local pipeline's configuration validation, work admission and execution/output budgets. Execution is sequential; only the current dataset, its baseline tape and one alternative tape are resident. The bounded cell index retains summaries, not tapes. No simulation behavior, existing default configurations, simulation revision or existing run identities are changed.

## Experimental design

The baseline is the existing `maker_baseline` preset. Strategy parameters, decision interval, instrument, exchange/fill model, fees, risk limits, recording policy, generator seed and session duration remain fixed in the full study. The manifests, rather than unstated defaults, are authoritative.

One-factor contrasts on normal liquidity cover every supported queue model, latency shape, entry/response asymmetry, a separate cancel-entry delay and a feed-latency offset. Power-model exponents are held fixed. A bounded factorial crosses the conservative and baseline queue models with constant and heavy-tail latency, separately on normal, wide-spread and depleted-liquidity sessions. Other regimes receive baseline executions. These are sensitivity tables over two factors, stratified by regime, not a fitted response surface or evidence of a causal effect in a real market.

The constant, uniform-jitter, log-normal and clipped cubic heavy-tail shape contrasts match their *marginal* entry and response medians. The cubic minimum is solved from its analytic median and rounded to integer nanoseconds. The engine's FIFO message bus can change realized latency through blocking, so matched marginal medians do not imply matched realized latency. The asymmetric and cancel-delay contrasts intentionally do not match all legs.

Only non-degenerate jitter, log-normal and heavy-tail configurations receive multiple run seeds. The run seed affects stochastic latency draws only; the generated market is held fixed. The cancel-delay configuration uses log-normal legs with zero dispersion and is reported once, despite its distribution tag. Deterministic queue-model alternatives are not replicated with artificial seed changes.

For each scalar measure, stochastic-seed aggregation reports a mean and a fixed-seed, nearest-rank percentile bootstrap interval for that mean, resampling run seeds with replacement. This is conditional Monte Carlo dispersion over the assumed latency model, not market-sampling uncertainty or a confidence interval for live returns. Missing metrics are not replaced with zero; an interval requires multiple defined values. Deterministic cells have no bootstrap interval. The small seed ensemble cannot establish tail risk or robust distributional calibration.

## Synthetic regime assumptions

These named profiles are synthetic assumptions. **None reproduces a historical crash.** The absent/default regime retains the original generator byte for byte; explicitly selecting normal liquidity produces the same events.

| Profile | Intervention |
|---|---|
| `normal_liquidity` | Original seeded integer-book process, with generated trades, cancels and additions. |
| `widening_spreads` | A wider opening spread and a floor on quote improvements; other flow parameters remain fixed. This is a persistently wide-spread session, not a historical widening episode. |
| `liquidity_depletion` | Lower freshly generated level sizes throughout the session. This is a persistently depleted session, not an endogenous withdrawal or historical crash. |
| `volatility_burst` | A middle-session interval with faster event generation and a higher aggressive-trade probability. Any resulting volatility is generated by the same integer-book mechanics, not calibrated to a real burst. |
| `thin_book` | Fewer initial levels and smaller level sizes; the original minimum-level safety floor and replenishment remain. |
| `feed_staleness` | A middle-session feed-delay spike; local receipt times remain nondecreasing within each stream, and backlog persists until caught up. Exchange events are not moved backwards or inspected early. |
| Cancellation delay | An **execution setting**, not a market-data regime: only the cancel-entry leg is delayed relative to the normal deterministic entry/response legs. |

Generator tests compute spread and touch-depth differences from generated output, check repeatability and book invariants across the regimes, and pin the original default event-archive hash. They do not assert a claim about any historical market.

## Evidence classes and metric semantics

| Evidence class | What this study can say |
|---|---|
| Observed market events | None here: the input events and receipt times are generated. Recorded venue events would be observations in a different study. |
| Modeled order placement | Strategy intent and exchange arrival under the stated decision grid, risk policy and latency model. |
| Estimated queue position | Engine queue-ahead estimates, including cancellation-allocation assumptions. These are not measured positions. |
| Simulated fill | Engine fill events under the queue and fill model; maker/taker counts and shares describe those events only. |
| Derived performance | Fees, fill rate, sampled mean absolute inventory, maximum absolute inventory, P&L and fill-time summaries computed from simulated execution. |
| Unobservable counterfactual | Whether the corresponding real order would fill, the actual priority behind aggregate depth, market impact and a real strategy's opportunity set. |

Fill rate is the share of submitted orders with any fill. Unfilled orders are submitted orders without any fill; risk-blocked intents are excluded, and partial fills are not counted as wholly unfilled. Fill-time quantiles condition on simulated fills; the accompanying Kaplan–Meier curves preserve the existing analytics' censoring treatment. Their modeled fill probabilities are **not empirical venue fill probabilities**. Inventory means use recorded equity samples, not a continuous-time integral. Net P&L uses the existing local-clock accounting/marking convention, not an actual liquidation at the exchange.

First-divergence and lifecycle comparisons use the existing order alignment **within the same regime**, with identical event hashes and strategy decision points. Cross-regime event hashes differ, so per-order alignment against the normal-liquidity baseline is unavailable rather than invented; scalar deltas and separate within-regime baselines remain available. After the first changed order, later strategy differences may be knock-on effects rather than separable direct parameter effects.

## Executed study and results

Actual commands (release binary, not a mocked or precomputed result):

```sh
target/exec/release/blackbox.exe study exec-validation --out data/studies/execution-validation-quick --quick
# exec-validation: 8 cells; 127 ms; data/studies/execution-validation-quick
target/exec/release/blackbox.exe study exec-validation --out data/studies/execution-validation
# exec-validation: 57 cells; 2885 ms; data/studies/execution-validation
```

The full run produced **57 cells and 25 aggregate groups**. Timing is the runner's reported
elapsed time before final report serialization, not a benchmark claim. Primary evidence:
`data/studies/execution-validation/study.json` and `study.md`, plus `cells/*.json`.
The recorded `study.json` SHA-256 is
`e57e5b765d2fc3b4291706c01c9b0ce5c29ad2d9bb5d794c8625860037c2c9e1`.
Wall-clock provenance means this whole-report hash need not repeat on a new execution.
The platform is `windows-x86_64-gnu+libm-0352e099`.

### Factor levels and held constants

The executed design uses 30-second generated sessions, generator seed 7 and stochastic latency
seeds **1, 7, 19, 31, 43**. Bootstrap: **2000** resamples, seed **3105**, percentile **95%**
intervals. Queue levels: `risk_adverse`, `log_prob`, `log_prob2`, `power_prob`, `power_prob2`,
`power_prob3`; all power exponents are **3**. The factorial is the two queues `power_prob3`
and `risk_adverse` × `constant` and `heavy_tail`, stratified by normal, wide and depleted regimes.

Held constant: `touch_maker`, 100 ms decisions, order quantity 0.01, position limit 0.1,
offset 0 ticks, inventory skew 20 ticks per unit, `no_partial_fill`, maker fee 0.0002,
taker fee 0.0004, maximum order quantity 0.05, maximum open orders 8, no drawdown halt,
equity recorded every tick and at most 2000 retained depth-queue updates per order.
The dataset's instrument and other generator settings remain the original defaults.
The baseline run is `ccc243055c1a7e59`; full configuration hash:
`ccc243055c1a7e592fece46e0f24d086fb9bbc3289a0d6f6dd42f4450ef73fda`.

| Latency factor | Executed entry / response / cancel assumptions |
|---|---|
| `constant` | 1 ms / 1 ms; cancels share entry |
| `jitter` | Each leg: 0.5 ms + uniform [0, 1 ms]; marginal median 1 ms |
| `log_normal` | Each leg: median 1 ms, sigma 0.7; independent cancel stream using entry parameters |
| `heavy_tail` | Each leg: minimum 768920 ns, jitter 0.5, clip 0.1, unit 10; marginal median within 1 ns of 1 ms |
| `entry_slow` | 10 ms / 1 ms |
| `response_slow` | 1 ms / 10 ms |
| `cancel_delay` | Deterministic 1 ms / 1 ms / 20 ms, implemented with zero-sigma log-normal legs |
| Feed offset | Additional 20 ms on all local feed receipts, independent of order latency |

Regime interventions are fixed generator assumptions: wide spreads start with quotes 5 ticks
either side of the initial mid and stop quote improvements at an 8-tick spread; depletion
divides the base level size by 10; thin book uses 6 initial levels per side and base size
divided by 20. The burst occupies the middle quarter of the session, with mean event gap
divided by 8, trade probability 0.65 and cancellation probability 0.20. Staleness adds
100 ms to feed receipts during that middle quarter, preserving ordered stream backlog.
These are not calibrated historical-event parameters.

### Sensitivity tables

Transcribed from `study.md` produced by the command above. Net is in quote currency;
fill rate is a fraction. Brackets are bootstrap intervals for stochastic-seed means only.
Every deterministic row is one execution, not an independent-seed ensemble.

| Regime | Queue | Latency | Feed offset ns | Runs | Fill rate | Net P&L |
|---|---|---|---:|---:|---|---|
| normal_liquidity | power_prob3 | constant | 0 | 1 | 0.610169 | -19.508749 |
| normal_liquidity | risk_adverse | constant | 0 | 1 | 0.610169 | -19.508749 |
| normal_liquidity | log_prob | constant | 0 | 1 | 0.610169 | -19.508749 |
| normal_liquidity | log_prob2 | constant | 0 | 1 | 0.610169 | -19.508749 |
| normal_liquidity | power_prob | constant | 0 | 1 | 0.609023 | -19.508749 |
| normal_liquidity | power_prob2 | constant | 0 | 1 | 0.610487 | -19.621749 |
| normal_liquidity | power_prob3 | jitter | 0 | 5 | 0.606954 [0.606118, 0.607940] | -19.334354 [-19.358153, -19.321055] |
| normal_liquidity | power_prob3 | log_normal | 0 | 5 | 0.605322 [0.601132, 0.609600] | -19.205860 [-19.324857, -19.086864] |
| normal_liquidity | power_prob3 | heavy_tail | 0 | 5 | 0.582807 [0.576381, 0.590570] | -18.272467 [-18.524571, -18.068965] |
| normal_liquidity | power_prob3 | entry_slow | 0 | 1 | 0.558317 | -17.578770 |
| normal_liquidity | power_prob3 | response_slow | 0 | 1 | 0.607692 | -19.017755 |
| normal_liquidity | power_prob3 | cancel_delay | 0 | 1 | 0.529183 | -16.376789 |
| normal_liquidity | power_prob3 | constant | 20000000 | 1 | 0.479008 | -15.066289 |
| normal_liquidity | risk_adverse | heavy_tail | 0 | 5 | 0.582268 [0.576381, 0.588953] | -18.248667 [-18.453171, -18.068965] |
| widening_spreads | power_prob3 | constant | 0 | 1 | 0.609943 | -18.058696 |
| widening_spreads | power_prob3 | heavy_tail | 0 | 5 | 0.612412 [0.603888, 0.619637] | -17.954501 [-18.143884, -17.765117] |
| widening_spreads | risk_adverse | constant | 0 | 1 | 0.609943 | -18.058696 |
| widening_spreads | risk_adverse | heavy_tail | 0 | 5 | 0.609552 [0.601058, 0.616828] | -17.865106 [-18.063491, -17.666721] |
| liquidity_depletion | power_prob3 | constant | 0 | 1 | 0.607807 | -19.731194 |
| liquidity_depletion | power_prob3 | heavy_tail | 0 | 5 | 0.583108 [0.577977, 0.588194] | -18.781615 [-18.968591, -18.585538] |
| liquidity_depletion | risk_adverse | constant | 0 | 1 | 0.607807 | -19.731194 |
| liquidity_depletion | risk_adverse | heavy_tail | 0 | 5 | 0.583108 [0.577977, 0.588194] | -18.781615 [-18.968591, -18.585538] |
| volatility_burst | power_prob3 | constant | 0 | 1 | 0.666667 | -21.780807 |
| thin_book | power_prob3 | constant | 0 | 1 | 0.663004 | -21.757276 |
| feed_staleness | power_prob3 | constant | 0 | 1 | 0.540643 | -17.128811 |

### Execution, inventory and queue summaries

The regime-baseline records in `study.json` supply the following rounded values.
Queue and inventory quantities are in base units; fees are in quote currency.
These are simulated maker/taker fill-event counts, not empirical trade classifications.

| Regime | Maker/taker fills | Unfilled orders | Filled-only TTF p50 / p90 ms | Mean estimated queue at entry | Mean / max absolute inventory | Fees |
|---|---:|---:|---:|---:|---:|---:|
| normal_liquidity | 324 / 0 | 207 | 50.056 / 117.127 | 0.596554 | 0.016877 / 0.05 | 19.439749 |
| widening_spreads | 319 / 0 | 204 | 48.330 / 116.757 | 0.559398 | 0.015482 / 0.05 | 19.138696 |
| liquidity_depletion | 327 / 0 | 211 | 42.553 / 103.211 | 0.059579 | 0.018505 / 0.07 | 19.617694 |
| volatility_burst | 362 / 0 | 181 | 29.299 / 103.455 | 0.488632 | 0.023355 / 0.08 | 21.716807 |
| thin_book | 362 / 0 | 184 | 33.189 / 94.696 | 0.051231 | 0.016877 / 0.05 | 21.717276 |
| feed_staleness | 286 / 0 | 243 | 50.386 / 119.954 | 0.605086 | 0.025548 / 0.09 | 17.159811 |

The normal baseline Kaplan–Meier estimate covers 531 orders with 207 censored and no
excluded orders. Its estimated median is 88.348 ms, versus the filled-only median of
50.056 ms. The terminal KM modeled fill estimate is 0.923963, **not** the raw fill rate
0.610169 and not an empirical probability of filling a real order. Cancellation censoring
and the modeled observation window matter; neither number should replace the other.

### Where paths first differ

Selected normal-liquidity comparisons, seed 1. The timestamp is the decision point's
relative time; it is **not** the time the first differing lifecycle event occurred.
Complete differences and timing deltas are in each cell's existing comparison payload.
H/T/O/D are counts of same-fills/different-history, same-outcome/different-timing,
same-decision/different-outcome and different-decision pairs, respectively.

| Alternative | Run id | First order / decision ms | First class | H / T / O / D |
|---|---|---:|---|---:|
| risk_adverse, constant | `aa360fc2699dec69` | 4097 / 1700 | same fills, different history | 64 / 4 / 0 / 0 |
| log_prob, constant | `9fd4e8706cdfff2b` | 3073 / 1300 | same fills, different history | 86 / 2 / 0 / 0 |
| power_prob2, constant | `80a1420d89e23339` | 3073 / 1300 | same fills, different history | 95 / 4 / 0 / 3 |
| power_prob3, jitter | `7ded836f06325de6` | 0 / 100 | same outcome, different timing | 0 / 518 / 2 / 9 |
| power_prob3, log_normal | `17e7d7a48e761a73` | 0 / 100 | same outcome, different timing | 0 / 511 / 4 / 14 |
| power_prob3, heavy_tail | `fe145ad394ef9d5a` | 0 / 100 | same outcome, different timing | 0 / 482 / 11 / 25 |
| power_prob3, entry_slow | `0a4520e7138da4fb` | 0 / 100 | same outcome, different timing | 0 / 416 / 36 / 62 |
| power_prob3, response_slow | `499a881f52282207` | 0 / 100 | same outcome, different timing | 0 / 496 / 3 / 18 |
| power_prob3, cancel_delay | `4188ce9b2d40a618` | 1 / 100 | same outcome, different timing | 74 / 269 / 42 / 77 |
| power_prob3, feed offset | `1a480c7a3c78758f` | 0 / 100 | same fills, different history | 258 / 10 / 12 / 210 |

The risk-adverse and log-probability alternatives can change lifecycle history without
changing the displayed aggregate P&L. The `power_prob` row also illustrates why a fill-rate
denominator must be retained: its fill rate changes while the rounded net remains equal.
On normal liquidity, the heavy-tail mean fill rate is lower than constant latency despite
matched marginal medians; on the wide-spread session it is slightly higher. This is
conditional model sensitivity, not a universal latency ordering. Less-negative net in
some delayed configurations is not evidence that delay helps a real strategy.

## Limitations

Synthetic data; no market impact, hidden liquidity, empirical queue calibration or historical-crash reconstruction; one strategy and fee schedule; one generator realization per regime; short sessions; fixed factor levels and power exponents; a small stochastic ensemble; no out-of-sample test; no annualisation. Whole-order fill assumptions can be optimistic about size. Neither model agreement nor a narrow Monte Carlo interval establishes real execution quality. No claim that a particular queue model is correct or that lower losses make a latency assumption preferable is supported.

The study itself remains CLI-only. A small curated subset is visible in the application as the
demo `execution_validation` (next section).

## In-app demo `execution_validation`

The demo reuses the existing demo mechanism (`crates/bb-app/src/demos.rs`, `blackbox demo seed`):
its runs are admitted and computed through the ordinary job queue in the `demo` workspace, and its
curated comparisons and saved report are stored with the same code as `POST /comparisons` and
`POST /reports`. Visitors read it through the existing `GET /api/v1/demos`,
`GET /api/v1/comparisons`, `/compare/set?demo=execution_validation` and `/compare/pair` pages;
no endpoint, migration or web change was needed. The canonical demos, their run ids
(`253bbc23b342f373`, `d6a57aed972e18d0`, `bc2d1df47e2b91fe`, `130d67fc371bd27f`,
`6bcdf19136fd3059`, `586374188aae19b2`, pinned by a unit test) and their curated comparisons are
unchanged.

**Datasets.** The normal-liquidity session is the existing default `synthetic-demo` dataset
(120 s, generator seed 7). `demo seed` additionally registers two synthetic stress datasets
through the existing registry path (`storage = 'regenerate'`, hosted 500 000-row cap applies):
`synthetic-stress-depletion` (`liquidity_depletion`) and `synthetic-stress-wide-spreads`
(`widening_spreads`), with every other generator setting equal to the default. Their ids sort
after `synthetic-demo`, which stays the default catalogue dataset. These sessions are 120 s long,
not the study's 30 s, so the numbers below are not the study tables' numbers.

**Runs (7, one of them the existing 1 ms baseline run; 6 new computations).** All use the
`maker_baseline` preset (touch maker, 100 ms decisions, seed 1, PowerProb3 n = 3, constant 1 ms
entry/response latency, 2 / 4 bp fees). Each label names the one assumption changed against the
normal-liquidity baseline, or (run 6) against the depleted-liquidity run. Measured on a fresh local
database with the release binary (`windows-x86_64-gnu+libm-0352e099`). Hosted limits
(`ExecutionLimits::hosted`): 500 000 rows, 150 000 ticks, 150 000 tape events; every run's
admission estimate is 1201 ticks and at most 79 550 tape events (unit test
`execution_validation_fits_hosted_limits_and_changes_one_assumption_per_pair`).

| # | Run id | Label (changed assumption) | Dataset (rows) | Ticks | Tape events | Fills / orders | Fill ratio | Fees | Net P&L |
|---|---|---|---|---:|---:|---:|---:|---:|---:|
| 0 | `253bbc23b342f373` | Normal-liquidity baseline | synthetic-demo (62 929) | 1200 | 25 242 | 1241 / 2060 | 0.602427 | 74.461053 | -74.709553 |
| 1 | `08200b05ca35723f` | Heavy-tail order latency (1 ms median, seed 1) | synthetic-demo | 1200 | 24 661 | 1186 / 2035 | 0.582801 | 71.161060 | -71.448060 |
| 2 | `5a063572329e1c80` | Cancel-entry latency 20 ms (entry/response 1 ms) | synthetic-demo | 1200 | 24 015 | 1106 / 2028 | 0.545365 | 66.361028 | -66.594028 |
| 3 | `bcf5c37532e25266` | Feed latency offset +20 ms | synthetic-demo | 1200 | 23 104 | 1029 / 2070 | 0.497101 | 61.740933 | -61.909433 |
| 4 | `88e0971baa6491ab` | Depleted-liquidity regime | synthetic-stress-depletion (63 109) | 1200 | 25 324 | 1296 / 2116 | 0.612476 | 77.751089 | -78.025089 |
| 5 | `69ca873c448e2dd1` | Depleted-liquidity regime, risk-adverse queue | synthetic-stress-depletion | 1200 | 25 301 | 1294 / 2114 | 0.612110 | 77.631100 | -77.917100 |
| 6 | `4d0de6620234a7fe` | Widening-spreads regime | synthetic-stress-wide-spreads (62 848) | 1200 | 25 813 | 1284 / 2101 | 0.611138 | 77.025689 | -72.896689 |

The heavy-tail and cancel-delay latency levels are exactly the study's `heavy_tail` and
`cancel_delay` levels (`study::latency`). Heavy-tail latency is stochastic: run 1 is **one seed**,
not an ensemble; the five-seed intervals are in the study tables above.

**Curated comparisons (6) and one saved report** (of the first comparison). Aligned pairs share
the event hash; H/T/O/D as above, plus orders present only in A / only in B. The class counts
cover every order; only the stored pair list is capped at 500.

| A vs B | Status | Identical | H / T / O / D | Only A / only B |
|---|---|---:|---:|---:|
| 0 vs 1 (heavy-tail latency) | aligned | 0 | 0 / 1873 / 43 / 103 | 41 / 16 |
| 0 vs 2 (cancel-entry 20 ms) | aligned | 265 | 219 / 1169 / 149 / 197 | 61 / 29 |
| 0 vs 3 (feed +20 ms) | aligned | 0 | 1081 / 46 / 48 / 729 | 156 / 166 |
| 4 vs 5 (queue model on depleted data) | aligned | 1843 | 259 / 4 / 2 / 6 | 2 / 0 |
| 0 vs 4 (regime: depleted liquidity) | metrics_only, not like-for-like | – | – | – |
| 0 vs 6 (regime: widening spreads) | metrics_only, not like-for-like | – | – | – |

The two regime pairs replay different events (`dataset.sha256` differs), so the existing
eligibility gate stores them metrics-only with its not-like-for-like explanation; their deltas
mix the regime with knock-on strategy effects. The depleted-liquidity queue contrast was kept
although the study's 30 s scalars were identical: on the 120 s session the risk-adverse queue
changes 259 orders' queue history and a handful of outcomes (1296 versus 1294 fills), which the
lifecycle view shows. The depleted risk-adverse run is not curated against the normal baseline
(two assumptions changed at once); `/compare/set?demo=execution_validation` lists that pair as
not compared.

**Seeding and resolution.** `demo seed --wait` on a fresh database (local profile, release
binary, one worker slot) registered the three datasets with their market analytics, computed all
12 distinct demo runs (6 canonical, 6 new) and stored 11 comparisons and 4 reports in **9.25 s**
wall time; each new run took 28–32 ms of simulation (`wall_time_ms`). `GET /api/v1/demos`
resolves each run's id on its own dataset: the selected catalogue dataset's manifest exactly as
before, each other dataset's current manifest read once per request from its version row (never
its events). When a stress dataset is unknown or invisible, `execution_validation` is left out of
the list; the other demos are unaffected. The anonymous demo snapshot uses the same function.

## Targeted verification

Commands executed locally:

```sh
cargo test -p bb-data synthetic::tests -- --nocapture
# 13 passed, 0 failed (includes original default archive hash and stress-regime invariants)
cargo test -p bb-forensics ensemble::tests -- --nocapture
# 4 passed, 0 failed (includes fixed-seed bootstrap fixtures)
cargo test -p bb-app study::tests --lib -- --nocapture
# 2 passed, 0 failed (matched medians, deterministic cancel, tiny study output)
cargo test -p bb-app --lib -- demos:: v1::datasets::
# 3 passed (hosted limits and one-assumption pairs of execution_validation, canonical pairs,
# canonical demo run ids pinned and per-dataset resolution)
BLACKBOX_TEST_DATABASE_URL=$TEST_DB cargo test -p bb-app --test demo_workflow -- demo_seed_curates
# 1 passed (seed: 15 experiments, 11 comparisons, 4 reports, 12 runs; idempotent re-seed)
CARGO_TARGET_DIR="D:/black box/target/exec" cargo test --locked -p bb-app --test golden
# 2 passed, 0 failed
cargo clippy -p bb-app -p bb-data -p bb-forensics -- -D warnings
# Passed, no warnings
```

The pinned default `synthetic-demo` event-archive SHA-256 remains
`0bc26426a4c945630737db01e82921c28edf45c37f8de1671d45ce921e818f38`.
