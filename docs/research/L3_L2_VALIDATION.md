# L3 → L2 observability experiment

## Question and method

What information disappears when the reconstructed order-level book is aggregated to price-level totals? This is an information-loss experiment on generated messages, not proof that an execution model is accurate on a market.

The experiment generates deterministic binary ITCH, reconstructs it with the existing decoder and `Reconstructor`, and compares each selected-symbol L3 price queue with its L2 total in exact integer shares. Book checkpoints are replayed forward at selected source indices and compared with uninterrupted replay. Aggregate equality tests inspect every event, not only chart samples. Same-timestamp messages stay in physical source order. Existing ITCH tests remain unchanged.

The L2 observation used here consists of side, price and displayed total after each message (and normalized prints). It contains no participant order identity or queue allocation. An alternative timestamp-batched observer is considered **only** for the explicit same-timestamp netting measure; those two observation models must not be conflated.

## Definitions

- **Order identity observations:** sum of the selected symbol's live resting-order counts after each selected-symbol event. This is a repeated state-exposure count, not the count of unique orders. Level totals alone cannot partition these quantities into identities.
- **Touch queue observations:** sum of order counts at the post-event best bid and best ask. Every such order has an exact one-based reconstructed arrival position and an exact number of displayed shares ahead. L2 reveals only its level total. These are not hypothetical fill predictions.
- **Execution identity:** accepted `E`/`C` messages name the consumed resting order. L2 prints and depth do not preserve that attribution. Non-cross `P` messages are not counted as resting-book executions.
- **Cancellation location:** for accepted `X`/`D`, take the pre-event queue at the affected price and select zero-based index `floor((length−1)/2)` as the reference. Count cancels of orders ahead of, at, or behind that reference. This reference is a clearly stated illustrative middle-order benchmark, not a trader's actual order. L2 cannot recover which identity lost shares from the level total alone.
- **Same-timestamp add/cancel level groups:** distinct `(source timestamp, side, price)` groups containing both an accepted `A`/`F` and an accepted `X`/`D`. A timestamp-batched L2 total can net gross opposing actions; per-event L2 updates still reveal gross total changes but not identity/queue location. A replace is not silently counted as an ordinary add/cancel pair for this measure.
- **L3 order-count imbalance:** `(bid order count − ask order count) / (bid order count + ask order count)` over the complete selected-symbol resting book; null when empty. This differs from the existing size/depth imbalance metrics.
- **Message flow:** accepted shares added, cancelled and executed by source event; buy is positive and sell negative. Replaces remove the old remaining quantity and add the replacement. Non-cross prints do not reduce resting depth. Counts/rates retain source message-type attribution.
- **Touch depletion/replenishment:** decrease/increase in displayed quantity at each pre-event best price; only genuine size changes at that same price contribute. A new best price alone is not interpreted as depletion of the old level. Native event-based changes are not the existing M10 recovery-window estimator.
- **Order lifetime:** source timestamp of full removal minus source timestamp of the accepted add/replace that created that identity. Partial execution/cancel does not end a lifetime. Resting orders at the end are right-censored and reported separately, not assigned zero or an invented completion time. Quantiles use nearest rank.
- **Queue position at execution:** one-based reconstructed FIFO position immediately before an accepted `E`/`C`, sampled once per execution message, not weighted by shares. An execution away from position one is reported as observed source behavior, never repaired to fit a matching assumption.
- **Rates:** counts divided by the first-to-last source-timestamp span in seconds. A zero/nonpositive span makes rates unavailable. The exchange/session clock is used, never local arrival time.

All native series carry the originating event index and session timestamp. Bounded chart views retain the last observation in event-index buckets and the final observation. A bucket endpoint summarizes completed observations; it must not be used as a feature available at an earlier event. The full summaries use all retained source observations, not the downsampled chart.

## Existing microstructure estimators

The reconstructed L2 rows are fed into `bb-analytics::market::compute_market` using integer-share lots and the ITCH price scale. This reuses M1 quoted spread, M2 relative spread, M3 depth/notional bands, M4 size imbalance, M5 weighted mid, M7 OFI, M8 sampled variance/volatility, and M9 aggressor imbalance with their original prerequisites, batching and exclusion reporting. M6 needs independent training/calibration and is not estimated from this evaluation session. M10 requires an explicitly specified liquidity shock/recovery window; none is inferred merely from a thin synthetic touch. See `MARKET_MICROSTRUCTURE.md` for the estimator definitions and edge cases.

## Results on the public synthetic session (seed 7)

Every figure below was produced on 2026-11-16 by the commands quoted; none is from a real venue. The source is 30,034 ITCH frames (SHA-256 `bfaa37069283d47d4d4527336802d726c954c862c8d077b43065a0462c1d30f4`) spanning 9.61 s of synthetic session time (34,200.000000000 s to 34,209.610400033 s after midnight); quality checks all pass with zero quarantined effects.

```sh
CARGO_TARGET_DIR=target/wb cargo test -p bb-data default_session_is_not_degenerate -- --nocapture
target/wb/debug/blackbox.exe market l3-synth --seed 7 --events 30000 --symbol SYNTH --out data/wbfinish/synth-seed7
```

**Generator statistics** (test output, measured through the unchanged `Reconstructor`):

| Message | Count | Share of the 30,000 order-flow messages |
|---|---:|---:|
| `A` add / `F` add with MPID | 12,619 / 1,020 (incl. 30 seed-book `A`) | 45.4% (13,609 excluding the seed book) |
| `D` delete / `X` partial cancel | 7,993 / 895 | 29.6% |
| `E` execute / `C` execute with price | 5,319 / 599 | 19.7% |
| `U` replace | 1,034 | 3.4% |
| `P` non-cross trade | 551 | 1.8% |

Prologue/epilogue: 2 `S`, 1 `R`, 1 `H` and the 30 seed-book adds. Best bid changed 1,014 times and best ask 1,237 times; 23 distinct best bids over a 22-tick ($0.22) range. Execution messages hit resting buys 3,823 times and resting sells 2,095 times. Resting orders per side averaged 708.6 over two-sided observations, with a maximum bid/ask order-count gap of 270; the session closes with 1,213 bids and 1,219 asks resting. The touch held 20.1 orders per side on average.

Post-event spread distribution (30,016 two-sided observations, one cent = one tick):

| Spread (ticks) | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8–10 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Observations | 13,243 | 10,687 | 3,960 | 996 | 767 | 215 | 107 | 41 |
| Share | 44.1% | 35.6% | 13.2% | 3.3% | 2.6% | 0.7% | 0.4% | 0.1% |

Adds exceed cancels by 1.53×, and cancels exceed executions; executions are deliberately far more frequent than in typical equity ITCH feeds so that depletion and queue effects appear in a short public session. The resting book grows over the session because adds outnumber removals.

**L3 → L2 comparison** (`comparison` in the CLI output; identical to `GET /api/v1/market/datasets/l3-demo/comparison`):

| Measure | Value |
|---|---:|
| Selected-symbol events | 30,034 |
| Order identity observations | 42,536,424 (mean 1,416 live orders per event) |
| Touch queue observations | 1,209,555 (mean 40.3 orders at the two touches per event) |
| Execution messages / with resting-order identity | 5,918 / 5,918 |
| Cancellations (`X` + `D`) | 8,888 |
| … ahead of / at / behind the middle-order reference | 2,943 (33.1%) / 2,325 (26.2%) / 3,620 (40.7%) |
| Same-timestamp add/cancel level groups | 125 |

"At reference" includes every cancellation from a single-order queue, where the only order is the reference by definition. The split is also partly mechanical: with an even queue length the reference is the earlier of the two middle orders, so one more position lies behind it than ahead (a two-order queue has no "ahead" position at all). The 40.7 % behind versus 33.1 % ahead therefore says nothing about where real cancellations happen; it only shows that the location exists in L3 and not in L2 (independent review, TechReview4). None of the 8,888 cancellation locations, 5,918 execution identities or 1,209,555 touch queue positions can be recovered from L2 totals; an L2 queue model has to assume them.

**Native metrics** (`counts` and distributions in `analytics.json`): order lifetime over 12,241 fully removed orders min 0 ns, median 97.0 ms, p90 1.7598 s, max 7.0864 s; 2,432 orders right-censored at the close. Queue position immediately before execution was 1 for all 5,918 executions: the generator always executes the head of the FIFO queue, so this checks reconstruction consistency rather than measuring anything about a venue. Touch depletion 3,935,400 shares; touch replenishment at an unchanged pre-event touch price 1,435,700 shares (replenishment that improves the price creates a new touch and is not counted, per the definition above).

**Reused L2 estimator (M1).** On completed equal-timestamp batches the existing `compute_market` reports quoted spread event-mean 194.98, min 100, max 1,000 (n = 11,768 batches). Its "tick" unit is the ITCH `price4` increment (`tick_size=0.0001`), so 100 = $0.01 and the event-mean is 1.95 cents, consistent with the per-event distribution above.

**Fault injection.** `market l3-synth --seed 7 --inject-unknown-execute --inject-duplicate-add` produced 30,036 frames with `quarantined_total` 2 (one `unknown_order`, one `duplicate_order_ref`, asserted in `faults_are_explicit_and_bounded`); fault runs are never mixed with the figures above.

## Restricted-data boundary

A names-only listing of `data/` (excluding `data/secrets/`, which was not read) was run:

```text
ls data/ data/raw data/datasets
find data -maxdepth 3 \( -iname "*itch*" -o -iname "*nasdaq*" \) -not -path "data/secrets/*"
=> data/raw: binance-futures; data/datasets: binance-futures-btcusdt-2020-02-01-10m, synthetic-demo; no ITCH/NASDAQ names
```

Consequently no local NASDAQ sample was processed for this experiment. Files mentioned in earlier research outside that tree were not opened, copied or uploaded. No recorded event-level data is included in this document or in the public workbench.

## Interpretation and limitations

A generated session validates arithmetic, source-order preservation, reconstruction diagnostics and the stated information-loss definitions under that generator. It does not establish how often a real exchange cancels ahead, how a real trader would fill, or which L2 queue model is best. Even agreement on one observed dataset would not establish those general claims: exchange allocation rules, hidden liquidity, feed completeness, trading regimes and participant conditioning differ.

L2 execution models therefore need explicit queue-position assumptions when allocating non-trade depth decreases ahead versus behind an order. L3 can expose the historical displayed queue for a known resting order; it still cannot establish the counterfactual market impact, hidden priority, order-entry latency or matching outcome of an order that never existed. No annualisation, real-execution claim, statistical confidence interval or cross-market conclusion is made here.

## Verification commands

All run on 2026-11-16 with `CARGO_TARGET_DIR=target/wb` (and `BLACKBOX_TEST_DATABASE_URL` pointing at the local development Postgres for the bb-app tests).

```sh
cargo test -p bb-data
```

100 unit tests passed, zero failed. This includes the unchanged ITCH suite, generator determinism/fault checks, the generator non-degeneracy tests (seed 7 at 30,000 messages; seeds 1–3 at 5,000; the minimum 8-message session), hand-computed L3 effects, checkpoint-versus-linear reconstruction and per-event L2 aggregation equality.

```sh
cargo test -p bb-analytics
```

52 unit tests and 3 pipeline integration tests passed, zero failed. Native fixtures independently specify counts, rates, nearest-rank distributions, same-timestamp ambiguity, physical-index joins and bounded causal reduction.

```sh
cargo test -p bb-app --test market_workbench
cargo test -p bb-app --test golden
```

1 and 2 tests passed respectively: endpoint bounds, pagination, cursor binding, book limits and the private-artifact 404 against the real middleware; the pinned golden hashes are unchanged.
