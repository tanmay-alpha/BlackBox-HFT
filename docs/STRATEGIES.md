# Strategies

The bundled strategies exist to exercise the execution model, not to make money. They are
deliberately simple, deterministic and explainable: every order carries a `reason` built from the
numbers the decision used, and the forensics view shows it verbatim. Their simulated P&L is not a
forecast ([LIMITATIONS.md](LIMITATIONS.md#strategies-and-risk)).

Code: `crates/bb-strategies`. Presets that size them for a dataset: `crates/bb-app/src/presets.rs`.

## The contract every strategy follows

A strategy is a pure state machine implementing `bb_core::strategy::Strategy`
(`crates/bb-core/src/strategy.rs`). On each tick of a fixed local-time grid (`strategy.interval_ns`,
100 ms in every preset) it receives a `LocalView` and appends `Intent`s:

| `LocalView` field | Content |
|---|---|
| `now`, `tick_no` | local time and index of this tick |
| `market` | best bid/ask ticks and quantities of the **local** book, plus the exchange and local timestamps of the last feed event received |
| `position` | position implied by fills whose notifications have **arrived locally** |
| `open_orders` | own non-terminal orders as known locally, ascending order id, with `request_pending` |
| `recent_trades` | market trades received locally since the previous tick |

`Intent::Place { slot, side, price_tick, qty, tif, ord_type, reason }` or
`Intent::Cancel { order_id, reason }`. The harness, not the strategy, assigns
`order_id = (tick_no << 8) | slot`, applies pre-trade risk rules, and calls the engine. Because the
tick grid is anchored to the dataset and not to execution assumptions, the same order id names the
same decision point in two scenarios ([EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md)).

Conventions shared by all strategies (`crates/bb-strategies/src/lib.rs`): at most one `Place` per
slot per tick; a `Cancel` only for a cancellable order (status `new` or `partially_filled` and no
request in flight), otherwise the order is looked at again next tick; quantities compared with a
half-lot tolerance; no I/O, no randomness, no engine access.

### No look-ahead

A strategy cannot see the future because it is never given anything else:

- `bb-strategies` depends on `bb-core` only (`crates/bb-strategies/Cargo.toml`); it cannot reach the
  engine, the dataset or the tape.
- The harness builds the view after `elapse(interval)` from the engine's **local** side: the local
  depth (updated only by events whose `local_ts` has passed), the local order map (updated only by
  responses that have arrived), and the local last-trades buffer (`crates/bb-sim/src/lib.rs`,
  `Harness::tick`).
- `position` is the ledger of *known* fills, not of all fills
  ([PNL_ACCOUNTING.md](PNL_ACCOUNTING.md#two-ledgers-true-and-known)).
- Given the same sequence of views, a strategy emits the same intents (test
  `tests::same_views_yield_identical_intents`).

The harness's risk rules obey the same boundary. The pre-trade position rule uses the known
position plus working orders; the `max_drawdown` halt evaluates equity from the **known** ledger
(fills whose response has reached the strategy host) marked at the locally known mid. Only the
recorded equity series uses the exchange-side truth, because it is a measurement, not a decision
input ([PNL_ACCOUNTING.md](PNL_ACCOUNTING.md#two-ledgers-true-and-known)).

## Shared quote maintenance (market makers)

`crates/bb-strategies/src/quoting.rs` turns "desired bid and ask" into the fewest intents:

1. **Clamp.** `bid = min(raw_bid, best_ask - 1)`, `ask = max(raw_ask, best_bid + 1)` (in ticks). A
   post-only order that would cross is expired by the exchange model, so quoting through the locally
   known book would only waste a request. When a clamp applies, the reason says so.
2. **Inventory gate.** A side is disabled when one more fill would exceed the limit:
   `sign(side) × position + order_qty > max_position + lot/2`. Its working orders are cancelled.
3. **Keep or replace.** An order already resting at the desired price is left alone, so it keeps its
   queue position; one at any other price is cancelled and a new post-only (`Gtx`) limit order is
   placed. There is no order modification: replace means rejoining at the back of the queue.
4. **One-sided book.** When the local book loses a side, every cancellable order is cancelled.

Slot 0 is the bid, slot 1 the ask.

## Touch maker (`touch_maker`)

Joins (or sits `offset_ticks` behind) the best bid and ask and leans against inventory
(`crates/bb-strategies/src/touch_maker.rs`):

```text
skew = round(skew_ticks_per_unit × position)
bid  = best_bid − offset_ticks − skew
ask  = best_ask + offset_ticks − skew
```

A long position lowers both quotes: the bid becomes less likely to add, the ask more likely to
reduce. Preset values: `offset_ticks = 0`, `skew_ticks_per_unit = 2 / max_position`, so quotes move
one tick from a quarter of the limit and two ticks at the limit; order size 10 lots, position limit
100 lots.

## Avellaneda-Stoikov (`avellaneda_stoikov`)

### The model

Avellaneda and Stoikov (2008), *High-frequency trading in a limit order book*, Quantitative Finance
8(3), 217–224, <https://doi.org/10.1080/14697680701381228>. A dealer with exponential utility
`−exp(−γ·wealth)` quotes a bid and an ask around a mid price `s` that follows arithmetic Brownian
motion with volatility `σ`. Market orders hit a quote at distance `δ` from the mid with Poisson
intensity `λ(δ) = A·exp(−κ·δ)`. The paper's approach, in outline:

1. With no quoting, the dealer's value function for inventory `q` at time `t` with horizon `T` is
   `v(x, s, q, t) = −exp(−γx)·exp(−γqs)·exp(γ²q²σ²(T−t)/2)`.
2. The **reservation price** is the price at which the dealer is indifferent to holding one more or
   one less unit. Averaging the bid and ask indifference prices gives

   ```text
   r(s, q, t) = s − q·γ·σ²·(T − t)
   ```

   A long dealer (`q > 0`) values the asset below the mid and shades both quotes down.
3. Optimising the expected utility of quoting distances `δᵃ`, `δᵇ` against the exponential
   intensity, and approximating to first order in inventory, gives quotes symmetric around `r` with a
   **total spread**

   ```text
   δᵃ + δᵇ = γ·σ²·(T − t) + (2/γ)·ln(1 + γ/κ)
   ```

   The first term is compensation for inventory risk over the remaining horizon; the second is the
   pure liquidity term set by how fast fill probability decays with distance.

The paper's full text was not re-read for this page (see the note under
[Papers](OPEN_SOURCE_RESEARCH.md#papers)); the two formulas above are the ones the strategy's module
documentation states and that market-maker-rs's README restates identically.

### BlackBox's implementation

`crates/bb-strategies/src/avellaneda_stoikov.rs`, an original implementation:

```text
mid        = (best_bid + best_ask) / 2 × tick_size          # local book, price units
σ²         = ewma_var / interval_s                          # per second
r          = mid − position × γ × σ² × horizon_s
δ          = 0.5 × γ × σ² × horizon_s + ln(1 + γ/κ) / γ     # half spread
bid_tick   = floor((r − δ) / tick_size)
ask_tick   = ceil((r + δ) / tick_size)
```

followed by the shared clamp, gate and keep-or-replace logic. `floor`/`ceil` treat a value within a
relative 1e-11 of an integer as on the grid, so `8.2 / 0.01 = 819.9999999999999` is not floored a
whole tick low.

**Volatility estimate.** On every tick with a two-sided local book the mid is one observation. The
squared change since the previous observation, divided by the number of ticks it spans (so a gap
during a one-sided book still estimates a one-tick variance), feeds an exponentially weighted mean
with `α = 1 − 0.5^(1/vol_halflife_ticks)`. The mean change is taken as zero. The first change seeds
the average. Nothing is quoted or cancelled until `vol_halflife_ticks` observations exist; after
warm-up a one-sided book cancels all quotes.

### Simplifications (stated, not hidden)

- **Stationary horizon.** `T − t` is held constant at `horizon_s`; there is no terminal time. This
  is the common infinite-horizon variant, not the paper's finite-horizon solution.
- **κ is a parameter, not estimated** from the data, and `A` does not appear in the quotes.
- **σ is estimated online** from the local mid, an observation process the paper does not specify.
- **Inventory in base units.** `q` is the position in the instrument's base unit (BTC on BTCUSDT),
  so γ is "per price unit per base unit".
- **One level each side, fixed size**, post-only, and a hard position gate that the paper's
  unconstrained solution does not have.
- **Discrete grid.** Quotes are rounded outward to ticks and re-evaluated every 100 ms, not
  continuously.

### Preset scaling

The presets express γ and κ per tick so the same catalogue works for any tick size
(`crates/bb-app/src/presets.rs`):

```text
γ = 0.1 / tick_size        κ = 0.5 / tick_size        horizon_s = 1        vol_halflife_ticks = 50
```

Substituting:

```text
liquidity term  = ln(1 + γ/κ) / γ = (tick / 0.1) × ln(1.2)           = 1.82 ticks
volatility term = 0.5 × γ × σ² × 1 s = 0.05 × (σ / tick)² ticks       (σ in price units per √s)
inventory shift = position × γ × σ² × 1 s = 0.1 × position × (σ / tick)² ticks
```

So the half spread is about two ticks when the market is quiet and widens as measured volatility
rises (0.05 ticks extra at σ = 1 tick/√s, 1.25 ticks at 5). The test
`presets::tests::avellaneda_stoikov_half_spread_is_about_two_ticks_for_any_tick_size` pins this. A
half-life of 50 ticks at 100 ms means volatility is estimated over roughly the last 5 seconds.
Because the inventory shift scales with position in base units, it is small on BTCUSDT at the preset
position limit of 0.1 BTC (0.01 × (σ/tick)² ticks).

## Imbalance taker (`imbalance_taker`)

Crosses the spread when the top of the local book is lopsided and flattens after a fixed holding
time (`crates/bb-strategies/src/imbalance_taker.rs`):

```text
imbalance = (bid_qty − ask_qty) / (bid_qty + ask_qty)      # best levels, local book, in [−1, 1]
```

- While any own order is open (in flight or working), do nothing: its outcome decides the next step.
- **Entry** (slot 0), when flat, the cooldown since the last entry has passed and both sides exist:
  buy `order_qty` at the best ask if `imbalance > threshold`, sell at the best bid if
  `imbalance < −threshold`, unless that alone would exceed `max_position`.
- **Exit** (slot 1), once `hold_ticks` have passed since the entry: an order for the whole known
  position at the opposite touch, repeated on later ticks while any of it remains. A position with
  no tracked entry is flattened immediately.
- Both are **IOC limit orders priced at the opposite touch**: they take what is displayed there and
  never rest. This is the supported equivalent of a market order, which the harness blocks
  ([EXECUTION_MODEL.md](EXECUTION_MODEL.md#5-market-orders)).

Preset values: threshold 0.6, hold 20 ticks (2 s), cooldown 10 ticks, 10 lots. Every fill pays the
taker fee, and whether the touch is still there on arrival depends on feed and order latency.

## Scripted (`scripted`)

Places exactly the listed orders, for curated forensics demonstrations and tests
(`crates/bb-strategies/src/scripted.rs`). Each `ScriptedOrder` names a tick, slot, side, price
(`tick` absolute, or `from_touch` relative to the same-side best at that tick: buy at
`best_bid + offset`, sell at `best_ask − offset`), size, time in force, order type, optional
`cancel_after_ticks` and a label used as the reason. If a `from_touch` reference is missing on its
tick the order is skipped permanently, because placing it later would give it a different order id
and it would no longer mean the same decision in two scenarios.

The `forensics_probe` preset uses five scripted orders, one per typical outcome: a post-only buy one
tick below the bid cancelled 50 ticks later; a buy joining the bid; a sell joining the ask; an IOC
buy priced `cross` ticks above the bid; a post-only buy priced the same, which would cross. `cross`
is the ceiling of the dataset's p99 spread from its quality report (1 tick when unknown), so the
crossing orders reach the ask on most of the data.

## The preset catalogue

`blackbox presets --dataset <id>` lists them sized for that dataset. All share: 100 ms interval,
`no_partial_fill`, `power_prob3 { n: 3 }`, 1 ms constant order latency each way, no feed offset,
2 bp maker and 4 bp taker fees, position limit 100 lots, order limit 50 lots, 8 open orders, no
drawdown rule, seed 1.

| Preset | Strategy | Differs from `maker_baseline` in |
|---|---|---|
| `maker_baseline` | touch maker | — |
| `maker_slow` | touch maker | order latency 50 ms each way |
| `maker_conservative_queue` | touch maker | queue model `risk_adverse` |
| `maker_rebate` | touch maker | maker fee −1 bp (a rebate) |
| `avellaneda_stoikov` | Avellaneda-Stoikov | strategy |
| `imbalance_taker` | imbalance taker | strategy |
| `forensics_probe` | scripted | strategy |

The three `maker_*` variants differ from the baseline in exactly one assumption each (test
`presets::tests::maker_variants_differ_from_the_baseline_in_one_assumption_each`), which is what
makes their comparisons interpretable.
