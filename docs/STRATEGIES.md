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

- **Constant horizon (caveat 1).** `T − t` is held constant at `horizon_s`; there is no terminal
  time. This is *not* the paper's infinite-horizon solution (AS §2.3 has a different functional form
  with a discount `ω`). Guéant, Lehalle & Fernandez-Tapia (2013, footnote 8) show the AS formulas
  are the Taylor expansion of the optimal quotes near the terminal time; far from it the optimal
  quotes converge to the GLFT asymptotics, whose skew scales as `√(σ²γ/(κA))`, not `γσ²(T − t)`.
  Read the implementation as "AS near-terminal approximation at a fixed remaining time
  `horizon_s`" ([research §4.2](research/MARKET_MICROSTRUCTURE.md#42-the-code-cratesbb-strategiessrcavellaneda_stoikovrs), discrepancy 1).
  The `glft` strategy below implements the asymptotic formulas.
- **Fill size (caveat 2).** The paper's dealer trades one unit per fill; the code uses the position
  in base units with the presets' 10-lot fills and no trade-size term, so γ is effectively "per
  unit of fill size". Guéant (2017) eq. 4.6 with `Δ = order_qty` would change the liquidity term to
  `ln(1 + γΔ/κ)/(γΔ)` and the inventory half-spread term to `Δγσ²τ/2` (discrepancy 2). `glft` takes
  `Δ` explicitly.
- **κ is a parameter, not estimated** from the data, and `A` does not appear in the quotes.
- **σ from a bouncing mid (caveat 3).** σ is estimated online from the local mid; AS assume the
  volatility of the efficient price. On a wide book the mid jumps by half the spread whenever one
  side moves, so the estimate includes spread fluctuation **[INFERENCE]**. `glft` can estimate σ
  from the size-weighted mid or from trade prices instead.
- **Inventory in base units.** `q` is the position in the instrument's base unit (BTC on BTCUSDT),
  so γ is "per price unit per base unit".
- **One level each side, fixed size**, post-only, and a hard position gate that the paper's
  unconstrained solution does not have.
- **Discrete grid.** Quotes are rounded outward to ticks and re-evaluated every 100 ms, not
  continuously; AS §3.3 warn that `dt` must be larger than the typical tick time.

These caveats are also in the `avellaneda_stoikov` preset's description. The implementation is
unchanged (its golden pins are unchanged).

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

## Guéant-Lehalle-Fernandez-Tapia (`glft`)

`crates/bb-strategies/src/glft.rs`, an original implementation of the closed-form asymptotic
quotes of Guéant, Lehalle & Fernandez-Tapia (2013, arXiv:1105.3115, §4) in the trade-size form of
Guéant (2017, arXiv:1605.01862, eqs. 4.6–4.7, Model A, `ξ = γ`):

```text
c1    = ln(1 + γΔ/k) / (γΔ)
ω     = √( γσ² / (2AΔk) × (1 + γΔ/k)^(k/(γΔ) + 1) )
δᵇ(q) = c1 + (2q + Δ)/2 × ω          bid_tick = floor((S − δᵇ) / tick)
δᵃ(q) = c1 − (2q − Δ)/2 × ω          ask_tick = ceil((S + δᵃ) / tick)
```

`S` is the local mid, `q` the known position and `Δ = order_qty`, both in base units; γ is per
(price unit × base unit), `k` per price unit, `A` fills per second at the reference price. With
`Δ = 1` this is GLFT (2013) exactly; the spread `2c1 + Δω` is independent of `q` and the skew is
linear in it (Guéant 2017, eqs. 4.8–4.9). The power is computed as
`exp((k/(γΔ) + 1) × ln_1p(γΔ/k))`. Then the shared clamp, inventory gate (GLFT's bound `Q` is
`max_position`) and keep-or-replace logic apply.

Parameters (`GlftParams`): `order_qty`, `max_position`, `gamma`, `kappa`, `intensity_a`,
`sigma_source` (`mid` | `weighted_mid` | `trade`), `vol_halflife_ticks`. σ uses the AS estimator
(EWMA of squared clock-sampled changes, per second) on the chosen series: the mid; the
size-weighted mid `(bid × ask_qty + ask × bid_qty)/(bid_qty + ask_qty)` (Stoikov 2018 eqs. 2–3,
not his calibrated micro-price); or the last trade price, carried forward. Trade prices avoid
quote flicker but bounce between bid and ask (Roll 1984); none of the three is the efficient price.

**Continuous model vs this implementation.** The formulas assume a Brownian reference price,
continuous costless quote revision, Poisson fills of exactly `Δ` at intensity `A e^(−kδ)`
independent of the price, and `T → ∞`. Here quotes change only on the tick grid through cancels
and new orders subject to latency and think time; fills come from the replay and the queue model,
not from the assumed intensity (so `A`, `k` shape the quotes but do not describe the simulated
fills); prices round outward to ticks; `S` and σ are what the host saw; a partial fill can leave
`q` off the `Δ` grid.

**Tests** (hand-computed): the GLFT fixture of research §4.2 (`γ = 0.1, k = 1.5, A = 140, σ² = 4,
q = 1, Δ = 1` → `δᵇ = 0.722961, δᵃ = 0.619527`); the same with `Δ = 0.1` → `c1 = 0.664454`,
`ω = 0.161167`, `δᵇ = 0.833679`, `δᵃ = 0.511346`, spread independent of `q`; quotes on a book whose
mid alternates by 0.10 (σ² = 0.1/s): flat `(99.80, 101.20)`, long 2 `(99.61, 101.01)`, short 2
`(99.99, 101.39)`; the three σ series on a one-sided ask move (2.025 / 0.081 / 6.561).

**Preset `glft_baseline`.** `γ = 0.1/(tick × Δ)`, `k = 0.5/tick`, `A = 1/s`, `Δ = 10 lots`,
`sigma_source = weighted_mid`, half-life 50 ticks: `c1 = 10 ln(1.2) = 1.82` ticks (the AS preset's
liquidity term), `ω = 0.546 × (σ/tick)` ticks per `Δ` of inventory. `A` is an assumption, not
calibrated to the data.

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
| `latency_lognormal` | touch maker | order latency: log-normal, median 1 ms, sigma 0.5, every leg |
| `latency_heavy_tail` | touch maker | order latency: ABIDES cubic, 1 ms minimum, 1.05–51 ms |
| `glft_baseline` | GLFT | strategy |

The `maker_*` and `latency_*` variants differ from the baseline in exactly one assumption each
(test `presets::tests::maker_variants_differ_from_the_baseline_in_one_assumption_each`), which is
what makes their comparisons interpretable. The latency variants keep the baseline's 1 ms as their
location parameter, so they isolate dispersion and tail from the median.
