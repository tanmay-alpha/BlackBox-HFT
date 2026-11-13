# P&L accounting

How BlackBox books simulated fills, what each P&L and execution-quality number means, and how the
books are checked against the engine. Every figure is simulated on historical replay.

Measured examples below come from runs in the lead's local run store on the Tardis sample (2020-02-01
BTCUSDT, first 10 minutes), created before `SIMULATION_REVISION` entered the run identity; re-running
a preset with current code gives it a new run id ([EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md#keeping-identity-honest)).
Vendor data cannot be redistributed, so these runs are reproducible only by someone who downloads
the sample themselves.

## Value and fees (the engine's arithmetic)

For a linear contract (`LinearAsset`, the only asset type BlackBox uses), hftbacktest 0.9.4
computes per execution (`State::apply_fill`, `src/backtest/state.rs`):

```text
value = contract_size × exec_price × exec_qty
fee   = rate × value          rate = maker or taker fee (TradingValueFeeModel<CommonFees>)
```

A negative rate is a rebate. BlackBox records `value` and `fee` exactly as the engine computed them,
from the exchange-side fee hook that sees every execution (`TapFee`, `crates/bb-sim/src/taps.rs`).
Funding, margin, liquidation and borrowing costs are not modeled
([LIMITATIONS.md](LIMITATIONS.md#accounting)).

## The ledger

`crates/bb-sim/src/ledger.rs`, `Book::apply`, for a fill with `sign = +1` (buy) or `−1` (sell),
using the same operations in the same order as the engine's `apply_fill` so the two can be compared
exactly:

```text
position += sign × qty
cash     −= sign × value
fees     += fee
```

Average-cost P&L, with `prev` the position before the fill and a half-lot tolerance for "flat":

| Case | Effect |
|---|---|
| opening from flat (`|prev| < lot/2`) | `avg_entry = price` |
| adding (`sign(prev) = sign`) | `avg_entry = (|prev|·avg_entry + qty·price) / (|prev| + qty)` |
| reducing, closing or flipping | `closed = min(qty, |prev|)`; `realized += closed × (price − avg_entry) × sign(prev) × contract_size`; on a flip the remainder opens at `avg_entry = price` |

```text
unrealized = position × (mark − avg_entry) × contract_size      (0 while flat)
equity     = realized + unrealized − fees
```

Realized and unrealized P&L are **before fees**; `equity` (reported as net P&L) is after.

### The cash identity

With `value = contract_size × price × qty`, average-cost accounting satisfies, at any mark,

```text
realized + unrealized = cash + position × mark × contract_size
```

Gross P&L computed two independent ways must agree. This is checked by
`ledger::tests::realized_plus_unrealized_equals_cash_plus_marked_position`, at every equity sample of
every run in the invariants sweep (`crates/bb-sim/tests/invariants.rs`, tolerance 1e-6), and on
generated markets by the property test `fills_reconcile_to_the_ledger`
(`crates/bb-sim/tests/properties.rs`).

## Two ledgers: true and known

The tape keeps two `Book`s (`crates/bb-sim/src/tape.rs`):

- **truth** is updated when an `exch_fill` is committed: every fill that has happened at the
  exchange.
- **known** is updated when a `local_response` reaches the strategy host: it releases that order's
  fills with exchange time up to the response's exchange time.

| Consumer | Ledger |
|---|---|
| `EquitySample.position`, `cash`, `fees`, realized, unrealized, `equity`; run metrics | truth (the measurement) |
| `EquitySample.known_position` | known |
| `LocalView.position` given to the strategy; pre-trade position rule; drawdown halt | known (decision inputs) |
| Engine cross-check | known |

The difference between `position` and `known_position` in an equity sample is the inventory the
strategy did not yet know about because its fill notification was still travelling
([LATENCY_MODEL.md](LATENCY_MODEL.md)).

**Marking.** Equity is marked at the mid of the **local** book (what the strategy host saw). While
the local book is one-sided the last two-sided mid is reused; before any mid exists unrealized P&L is
zero (`Harness::mark`, `crates/bb-sim/src/lib.rs`). Marking at mid ignores the cost of actually
closing the position.

**Sampling.** One `EquitySample` every `record.sample_every_ticks` ticks (default 1), plus a closing
sample one interval after the final tick, taken after everything the last `elapse` processed, so the
last sample reflects every fill on the tape.

## Run metrics

`crates/bb-forensics/src/metrics.rs`, `compute_metrics`; definitions in `Metrics`
(`crates/bb-core/src/result.rs`).

| Metric | Definition |
|---|---|
| `realized_pnl`, `unrealized_pnl`, `net_pnl` | values of the last equity sample; `net = realized + unrealized − fees` |
| `fees` | sum of fill fees (negative with rebates) |
| `volume`, `turnover` | Σ qty; Σ value |
| `max_drawdown` | largest peak-to-trough fall of sampled equity |
| `final_position`, `max_long`, `max_short` | from fills' `position_after` (magnitudes) |
| `mean_abs_position` | mean of `|position|` over equity samples |
| `fill_ratio_orders`, `fill_ratio_qty` | orders with a fill ÷ orders submitted; filled ÷ submitted quantity |
| timing quantiles | nearest rank, `sorted[ceil(q·n) − 1]` with integer arithmetic |

### Spread capture

```text
spread_capture        = Σ_maker fills  side × (mid_at_fill − price_tick) × qty × tick_size × contract_size
spread_capture_ticks  = Σ_maker fills  side × (mid_at_fill − price_tick) × qty  /  Σ_maker fills qty
```

`side` is `+1` for a buy. `mid_at_fill` is the mid of the **dataset's** reconstructed book at the
fill's exchange timestamp (`enrich_fills`, using the exchange-clock top-of-book series), not the
strategy's local mid. A positive value is the half-spread earned at the instant of each passive fill.
Maker fills without a valid mid are excluded from both sums. Taker fills are not included.

### Markouts

For every fill and each horizon `h` in `MARKOUT_HORIZONS_NS` = 100 ms, 1 s, 5 s, 30 s:

```text
markout_ticks(h) = side × (mid(t_fill + h) − price_tick)
```

`None` when `t + h` lies after the dataset's last top-of-book entry or the book had no valid mid
then; markouts are never extrapolated. The run-level value per horizon is the quantity-weighted mean
over fills with a value, with the count reported. Negative means the market moved against the fill
afterwards: adverse selection.

### Spread capture versus net P&L

They answer different questions and are not an additive decomposition:

- **Spread capture** is measured at the instant of the fill against the mid then.
- **Markouts** are measured against the mid `h` later: a fill's markout equals its capture at the
  fill plus the mid's move since, i.e. what the fill earns if marked at mid at the horizon.
- **Net P&L** is what the ledger booked: realized on closes, unrealized on the open position at the
  final local mark, minus fees. It depends on when and at what price inventory was closed, which
  neither capture nor markouts describe.

A maker strategy can show positive capture and negative net P&L (adverse selection plus fees), or the
reverse. BlackBox reports all of them and does not claim any one explains the other.

Measured example (Tardis sample, preset `maker_baseline`, run `6e84d8b06ed6b959`, read with
`blackbox compare 6e84d8b06ed6b959 9891fe2628409dca`): 305 maker fills, spread capture −0.192100
quote (−6.298 ticks per unit), markouts −26.895 / −74.521 / −78.640 / −74.904 ticks at 0.1 / 1 / 5 /
30 s, realized −2.159128, unrealized 0.027378, fees 5.715603, net −7.847353. The negative capture
means the fills' prices were on the wrong side of the dataset mid at the fill's exchange time. Why is
not established; maker fills triggered by trade-throughs and the sample's trade/book timestamp
misalignment ([MARKET_DATA.md](MARKET_DATA.md#measured-findings-on-the-real-sample)) are both
candidate explanations, and neither is proven here.

## Engine cross-check

At the end of every run (`Harness::engine_check`, `crates/bb-sim/src/lib.rs`) the **known** ledger is
compared with the engine's own local account (`StateValues`): position, cash vs `balance`, fees vs
`fee`, fill count vs `num_trades`, each with relative tolerance `1e-9 × (1 + max(|a|, |b|))`. The
result (`EngineStateCheck`, `GET /api/runs/{id}/engine-check`) carries a note:

- **`no_partial_fill`: must agree.** Every execution produces exactly one `Filled` response, which
  the engine's local account applies. A disagreement is reported as "Unexpected … treat this run's
  accounting as suspect".
- **`partial_fill`: expected to disagree.** In 0.9.4 the engine's local account applies only
  responses whose status is `Filled`, using the last execution's quantity, so partial fills and
  multi-level taker sweeps are under-counted there
  ([EXECUTION_MODEL.md](EXECUTION_MODEL.md#3-the-local-account-under-counts-partial-fills)). BlackBox's
  ledger, fed by the exchange-side fee hook, sees every execution and is the figure reported
  everywhere.

Measured on the Tardis sample (local run store, `engine_check` of each `run.json`):

| Run | Exchange model | Ledger fills / engine trades | Ledger fees / engine fees | Ledger position / engine position | Agrees |
|---|---|---|---|---|---|
| `6e84d8b06ed6b959` `maker_baseline` | no_partial_fill | 305 / 305 | 5.715603 / 5.715603 | 0.03 / 0.03 | yes |
| `e2de975ebd9a5eba` `imbalance_taker` | no_partial_fill | 401 / 401 | 15.025573 / 15.025573 | −0.010 / −0.010 | yes |
| `299337d89c0227d1` touch maker | partial_fill | 311 / 305 | 5.715601 / 5.674366 | 0.03 / 0.03 | no (expected) |
| `5bd9c993e318de33` imbalance taker | partial_fill | 403 / 397 | 14.838152 / 14.736973 | −0.010 / −0.013 | no (expected) |

The invariants sweep requires agreement for every `no_partial_fill` run
(`crates/bb-sim/tests/invariants.rs`), and `partial_maker_fills_are_all_counted_and_the_engine_gap_is_reported`
(`crates/bb-sim/tests/engine_adapter.rs`) pins the expected disagreement. The engine sums fills in
arrival order, so its floating-point totals can differ in the last bit between runs; they are
excluded from the result hash ([EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md)).

## Reconciliation: what is proven, and by which tests

| Identity | Test(s) |
|---|---|
| Ledger arithmetic: round trip realizes the price difference; adding averages; partial close keeps the entry; a flip realizes only the closed part | `ledger::tests::*` (`crates/bb-sim/src/ledger.rs`) |
| `realized + unrealized = cash + position × mark × contract_size` | `ledger::tests::realized_plus_unrealized_equals_cash_plus_marked_position`; every sample in `invariants.rs`; `properties.rs::fills_reconcile_to_the_ledger` |
| `equity = realized + unrealized − fees` at every sample | `invariants.rs` |
| Σ signed fill quantity = final position; each fill's `position_after` is the running sum; fills in tape order | `invariants.rs` |
| Fill `value` = `contract_size × price × qty` | `invariants.rs` |
| Ledger = engine local account under `no_partial_fill` | `invariants.rs` (every `no_partial_fill` configuration of the sweep), real-data table above |
| Expected gap under `partial_fill` | `engine_adapter.rs::partial_maker_fills_are_all_counted_and_the_engine_gap_is_reported`, `engine_defects.rs::upstream_local_account_undercounts_partial_fills` |
| Metrics on a hand-computed three-fill example (spread capture, markouts) | `metrics::tests::three_fill_example_matches_hand_computation` |

## Attribution between scenarios

The comparison (`crates/bb-forensics/src/compare.rs`) never attributes a P&L difference to one
parameter unless the arithmetic proves it. The one case it can prove: when two runs' fills are
identical and only fees differ, `Δnet = −Δfees` exactly (test
`compare::tests::fee_only_difference_has_checked_exact_arithmetic`).

Measured: `maker_baseline` (2 bp maker fee) against `maker_rebate` (−1 bp) on the Tardis sample,
runs `6e84d8b06ed6b959` and `9891fe2628409dca`: 1,874 decisions identical, 305 fills identical,
fees 5.715603 versus −2.857801, net P&L −7.847353 versus 0.726051. `blackbox compare` reports "Fills
are identical. The net P&L difference (B − A) of 8.57340396 quote equals the negative fee difference
of 8.57340396 quote" with basis *derived*. With any other parameter changed, fills differ and the
comparison lists the differences without splitting the P&L between causes
([LIMITATIONS.md](LIMITATIONS.md#comparisons)).
