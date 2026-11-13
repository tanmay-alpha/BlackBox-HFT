# Market microstructure and execution metrics: specification and audit (Phase 0)

Research input for the analytics BlackBox will implement in Rust (in `bb-forensics` or a new
crate), plus four audits: the market-making formulas in `crates/bb-strategies`, the statistical
validity of Sharpe-type metrics at our sample sizes, the "trades outside the reconstructed book"
finding on the Tardis sample, and which real datasets a public hosted service could legally serve.

Written 2026-11-14 against HEAD `5fce27d`. Nothing in this document changes code. Numbers marked
"measured" come from the scratch analysis described in [Methods](#9-methods-and-reproducibility),
run locally on the git-ignored Tardis sample; only aggregate statistics are reported, as the Tardis
terms permit for research reports (clause 9.3, quoted in
[OPEN_SOURCE_RESEARCH.md](../OPEN_SOURCE_RESEARCH.md#redistribution-and-sharing-terms-of-service)).
Statements that are my reasoning and not something I read or measured are marked **[INFERENCE]**.

## Contents

0. [Findings in brief](#0-findings-in-brief)
1. [Conventions shared by every metric](#1-conventions-shared-by-every-metric)
2. [Market metrics](#2-market-metrics) (M1–M11)
3. [Execution metrics](#3-execution-metrics) (E1–E11)
4. [Avellaneda–Stoikov and the other bundled strategies](#4-avellanedastoikov-and-the-other-bundled-strategies)
5. [Sharpe ratios and risk metrics](#5-sharpe-ratios-and-risk-metrics)
6. [Tardis sample: trades outside the reconstructed BBO](#6-tardis-sample-trades-outside-the-reconstructed-bbo)
7. [Real data a public hosted service could serve](#7-real-data-a-public-hosted-service-could-serve)
8. [References](#8-references)
9. [Methods and reproducibility](#9-methods-and-reproducibility)

---

## 0. Findings in brief

1. **The 659/2,212 "outside the book" trades are mostly not a data defect.** 655 of the 659 print
   on the aggressor's side, beyond the touch: buys above the ask, sells below the bid. 62.8% (414)
   are walks through the pre-trade book: the price is a resting level, and earlier prints in the
   same millisecond, on the same side, used up all the displayed depth in front of it. Another
   34.4% (227) fall inside the BBO of a book state at most 50 ms later. The depth channel's
   exchange timestamp trails the trade channel's by a median of 10 ms (p90 18 ms). 2.7% (18) stay
   unexplained. An independent best-bid/offer stream recorded by the same vendor, Binance
   `bookTicker`, matches our reconstructed BBO within ±10 ms in 99.5% of its updates. It flags
   27.7% of trades as outside, about the same rate. The reconstruction is therefore sound. The
   check's definition ("more than one tick outside the pre-trade touch") is what produces the
   headline. [Section 6](#6-tardis-sample-trades-outside-the-reconstructed-bbo).
2. **The reference instant for "mid at fill" decides the sign of spread capture on recorded data.**
   For `maker_baseline` on the Tardis sample, capture is −6.30 ticks per unit against the mid *at*
   the fill's exchange timestamp, which is what the code uses today. Against the mid strictly before
   that timestamp it is −2.28, against the mid 20 ms earlier +1.09, and 100 ms earlier +7.16
   (measured). Every execution metric below is defined on a **pre-fill** reference (E1).
3. **Avellaneda–Stoikov.** The code reproduces AS (2008) equations (29)–(30) exactly. Three points
   still need correcting or documenting:
   - The "stationary variant" (constant `T − t`) is neither the paper's own infinite-horizon
     solution (AS §2.3) nor the Guéant–Lehalle–Fernandez-Tapia asymptotic quotes. GLFT show that
     the AS expansion is a Taylor expansion near `t → T`.
   - The code's formulas assume each fill trades **one base unit** (1 BTC), but the presets quote
     0.01 BTC. Guéant (2017) shows the liquidity term and the inventory half-spread term both scale
     with the fill size Δ.
   - σ is estimated from a mid that bounces across a median 16-tick spread on the real sample.
   [Section 4](#4-avellanedastoikov-and-the-other-bundled-strategies).
4. **No Sharpe ratio should be displayed for current runs.** Equity is sampled every 100 ms, and on
   the Tardis runs 80–88% of the increments are zero. First-order autocorrelation is +0.14 to
   +0.26, and a run spans at most 600 s. Annualising multiplies the per-sample ratio by about
   17,758. The Lo (2002) aggregation factor is itself unstable when the aggregation lag nears the
   sample size. [Section 5](#5-sharpe-ratios-and-risk-metrics).
5. **No real dataset I checked can legally back a public hosted backtester.** Binance Vision
   forbids hosting (clause 4.3). Bybit, OKX and Coinbase forbid redistribution or display to third
   parties. The LOBSTER samples carry no grant. FI-2010 is CC BY 4.0, but it holds normalised
   feature vectors, not an event stream. Recommendation: the hosted service stays **synthetic
   only**, and real data remains a local, bring-your-own workflow.
   [Section 7](#7-real-data-a-public-hosted-service-could-serve).

---

## 1. Conventions shared by every metric

### 1.1 Notation

| Symbol | Meaning | Unit |
|---|---|---|
| `Pb, Pa` | best bid / best ask price | price (quote currency per base unit) |
| `Qb, Qa` | displayed quantity at the best bid / ask | base units (instrument quantity) |
| `M = (Pb + Pa)/2` | mid | price |
| `S = Pa − Pb` | spread | price; also stated in ticks |
| `s` | side of *our* order or fill: `+1` buy, `−1` sell | — |
| `p`, `x` | fill price, fill quantity | price, base units |
| `t⁻` | the last completed exchange-time batch **strictly before** `t` | — |
| `m0` | pre-fill mid `M(t_fill⁻)` | price |
| `m_h` | mid at `t_fill + h` (state after all batches with timestamp ≤ `t_fill + h`) | price |
| tick, lot | `InstrumentMeta.tick_size`, `lot_size` | price, base units |

Prices are held as **integer ticks** and quantities as **integer lots** wherever possible. The mid
is held as `2·M` in ticks (`bid_tick + ask_tick`), so that it is an exact integer. Conversion to
price, basis points or currency happens once, at the end.

### 1.2 Clocks and sampling

BlackBox has two clocks ([LATENCY_MODEL.md](../LATENCY_MODEL.md)): the **exchange clock**
(`exch_ts`, the dataset's book and trades) and the **local clock** (what the strategy host has
received). Every metric below states which clock it uses:

- **Market metrics** (section 2) are computed on the exchange clock, from the book that `bb-data`
  reconstructs. The book is read only *after a complete exchange-time batch*, because
  intermediate states never existed at the exchange (`crates/bb-data/src/book.rs:182-200`,
  `bbo_series`).
- **Decision-time** quantities (for example the decision mid used by implementation shortfall)
  use the local view the strategy had (`MarketView`, `crates/bb-core/src/strategy.rs:37`).
- **Event-time sampling** evaluates a metric at every completed batch that changes its inputs.
  **Clock-time sampling** evaluates it on a fixed grid `t_k = t_0 + kΔ` by *previous-tick*
  interpolation (the state in force at `t_k`). The two give different numbers (see the
  realised-variance fixture, M8), so every stored series records which one it used and `Δ`.

### 1.3 Missing data and invalid books

- A metric that needs both sides is **`None`** (never 0) while the book is one-sided or
  crossed/locked (`Bbo::is_valid`, `crates/bb-data/src/book.rs:43`). This matches the existing
  `mid_tick_at` (`crates/bb-core/src/market.rs:35-41`).
- Clock-sampled series never carry a value forward across an invalid interval. A grid point whose
  in-force state is invalid is `None`.
- Horizon metrics (`m_h`) are `None` when `t + h` lies after the dataset's last book state. They are
  never extrapolated, as markouts already are (`crates/bb-forensics/src/metrics.rs:16-36`).
- Aggregates report `n` (the number of contributing observations) next to the value, and the
  number excluded as `None`.

### 1.4 Numerical rules

- Integer arithmetic for ticks and lots. Comparisons against bands (for example "within k bps")
  are done in cross-multiplied integers, not in floating point (M3).
- Long sums (OFI, volumes over hours of data) use `i64` lots or compensated (Kahan) summation for
  `f64` values.
- Ratios with a possibly zero denominator (imbalances, relative spreads, fill ratios) return
  `None` when the denominator is 0.
- `ln(1 + x)` uses `ln_1p` (already done in `avellaneda_stoikov.rs:149`).
- Quantiles use the existing nearest-rank definition (`crates/bb-forensics/src/metrics.rs:38-56`).

### 1.5 What our data and tape actually record

| Source | Content | Code |
|---|---|---|
| Dataset events | L2 market-by-price: absolute quantity per price level, depth clears, snapshots, trades with aggressor side if stated. **No order ids** (no L3). | `crates/bb-data/src/tardis.rs`, `synthetic.rs` |
| `BboSeries` | per completed exchange batch: best bid/ask ticks **and quantities** | `crates/bb-core/src/market.rs:10-17` |
| `TradeSeries` | exchange ts, price tick, qty, `side: Option<Side>` (aggressor) | `crates/bb-core/src/market.rs:46-52` |
| `DepthFrame` / `L2Book::top_bids/top_asks` | N levels per side at an instant | `market.rs:57`, `crates/bb-data/src/book.rs:133-150` |
| Tape `OrderIntent` / `CancelIntent` | decision tick, **local L1 `MarketView`** (bid/ask ticks and qty, feed timestamps), known position | `crates/bb-core/src/flight.rs:72-92` |
| Tape `RequestSent` | local send ts, drawn entry latency, scheduled exchange arrival | `flight.rs:107-114` |
| Tape `ExchQueued` | modeled `queue_ahead`, displayed `level_qty`, exchange BBO ticks when the order starts resting | `flight.rs:120-128` |
| Tape `QueueUpdate` | cause (trade qty / depth prev→new / fill reset), queue ahead before/after | `flight.rs:130-136` |
| Tape `ExchFill` | price tick, qty, `maker` flag, fee, value, leaves qty | `flight.rs:138-148` |
| Tape `ExchResponse` / `LocalResponse` | status, rejection flag, response latency, local arrival | `flight.rs:150-170` |
| `FillRecord` | exch ts, side, price, qty, maker, fee, position after, `mid_tick` **at** fill ts, markouts at 0.1/1/5/30 s | `crates/bb-core/src/result.rs:62-84`, `:11-12` |
| `OrderRecord` | decision ts, exchange arrival, first response, queue ahead at entry / min, traded-at-price qty, maker/taker qty, first/last fill ts, `cancel_sent_ts`, `terminal_ts` | `result.rs:113-158` |
| `EquitySample` | one per strategy tick (100 ms in every preset), local clock, marked at the **local** mid | `result.rs:88-109`, `crates/bb-sim/src/lib.rs:568-596` |

**Not recorded:** full local depth at decision time (only L1 is in `MarketView`), the hidden or
iceberg share of liquidity (L2 data cannot reveal it), and any market reaction to our orders.
Replayed prices never move ([LIMITATIONS.md](../LIMITATIONS.md)).

### 1.6 Shared fixture book `F`

Tick size 0.5, lot 1, contract size 1. Every market-metric fixture starts from this book:

| Level | Bid price × qty | Ask price × qty |
|---|---|---|
| 1 | 100.0 × 3 (tick 200) | 101.0 × 1 (tick 202) |
| 2 | 99.5 × 5 (tick 199) | 101.5 × 4 (tick 203) |
| 3 | 99.0 × 2 (tick 198) | 102.0 × 6 (tick 204) |

`2·M = 402` ticks, so `M = 100.5`. All expected values below were also recomputed by script
(see [Methods](#9-methods-and-reproducibility)).

---

## 2. Market metrics

### M1. Quoted spread

- **Formula:** `S = Pa − Pb`. Store it in ticks: `ask_tick − bid_tick`.
- **Units / sign:** price or ticks; it is ≥ 1 tick on a valid book.
- **Sampling:** event time (every BBO change) for distributions. Clock time, or time-weighted
  (each value weighted by how long it held), for "typical spread". The existing quality report
  samples at every top-of-book change, *not* time-weighted (`MARKET_DATA.md` "Spread"). The two
  are different statistics and both must be labelled.
- **Missing:** `None` if one-sided or crossed.
- **Data:** `BboSeries`.
- **Limits:** a quoted spread says nothing about depth. On the Tardis sample its median is 16
  ticks at 0.01 (dataset quality report). Binance's own `bookTicker` stream shows a median of 20
  ticks at its update instants (measured, section 6). That sample is a wide-spread, thin market.
- **Fixture (F):** `S = 101.0 − 100.0 = 1.0` = **2 ticks**.
  - *Time-weighted:* spread 2 ticks for 300 ms, then 4 ticks for 100 ms, over a 400 ms window:
    `(2·300 + 4·100)/400` = **2.5 ticks**.
  - *Event-sampled mean* of the same two states: **3 ticks**.

### M2. Relative spread

- **Formula:** `S_rel = (Pa − Pb)/M × 10⁴`.
- **Units:** basis points of the mid.
- **Numerics:** compute as `2·(ask_tick − bid_tick)/(bid_tick + ask_tick) × 10⁴`. It is
  independent of tick size and exact up to the final division.
- **Sampling / missing / data:** as M1.
- **Limits:** compares instruments of different price levels. It does not adjust for tick
  constraints: a 1-tick spread on a low-priced asset is large in bps.
- **Fixture (F):** `2·2/402 × 10⁴` = **99.5025 bps**.

### M3. Depth within k bps of mid

- **Formula:** `D_bid(k) = Σ q_i` over bid levels `p_i` with `M − p_i ≤ k·M/10⁴`, and likewise
  `D_ask(k)` for `p_i − M ≤ k·M/10⁴`. Report quantity and notional `Σ p_i·q_i`.
- **Units:** base units; quote currency for notional.
- **Boundary:** inclusive.
- **Numerics:** in ticks, include bid tick `b` iff `(2M − 2b)·10⁴ ≤ k·(2M)`, where `2M` is the
  integer `bid_tick + ask_tick`. Ask ticks are tested the same way. This avoids floating-point
  boundary flips.
- **Sampling:** event or clock time. The band is anchored to the **current** mid.
- **Missing:** `None` if the mid is undefined. Levels beyond those the dataset ever published
  are unknown. The Tardis Binance snapshot covers the top 1,000 levels
  ([OPEN_SOURCE_RESEARCH.md](../OPEN_SOURCE_RESEARCH.md#is_snapshot)), so a band that extends past
  the deepest level ever seen must be flagged *truncated*, not reported as complete.
- **Data:** full L2 book (`L2Book`). L1-only data, such as a `bookTicker` feed, cannot produce
  this metric.
- **Limits:** displayed depth only; hidden liquidity is invisible in MBP data.
- **Fixture (F, k = 100 bps):** the band is `M ± 1.005` = [99.495, 101.505].
  - Integer test: bid tick 199 gives `(402−398)·10⁴ = 40,000 ≤ 100·402 = 40,200`, so it is
    included. Tick 198 gives `60,000`, so it is excluded.
  - **`D_bid = 3 + 5 = 8`**, notional **797.5**. **`D_ask = 1 + 4 = 5`**, notional **507.0**.

### M4. Order book imbalance, levels 1..N

- **Formula:** `OBI_N = (Σ_{i≤N} Qb_i − Σ_{i≤N} Qa_i) / (Σ_{i≤N} Qb_i + Σ_{i≤N} Qa_i)`, in
  `[−1, 1]`. Level `i` is the i-th *non-empty* price level, not the i-th tick from the touch. A
  tick-distance variant must say so.
- **Sign:** positive means more resting buy interest.
- **Relation to Stoikov's** `I = Qb/(Qb+Qa) ∈ [0,1]` (Stoikov 2018 eq. 3): `I = (OBI_1 + 1)/2`.
  The existing `MarketView::imbalance` is `OBI_1` (`crates/bb-core/src/strategy.rs:66-70`).
- **Sampling:** event time for signal research, clock time for aggregation.
- **Missing:** `None` if both sums are 0, or if one side has fewer than N levels and the metric
  is configured as strict.
- **Data:** L2 for N > 1; L1 for N = 1.
- **Limits:** predictive power differs by asset and by spread regime. Stoikov (2018) conditions
  on both imbalance and spread. Cartea, Donnelly & Jaimungal (2018) model L1 imbalance as a
  Markov signal. Cao, Hansch & Wang (2009) study the information in levels beyond the first. A raw
  threshold on `OBI_1` ignores the spread.
- **Fixture (F):**
  - **`OBI_1 = (3−1)/(3+1) = 0.5`**
  - **`OBI_2 = (8−5)/13 = 0.230769`**
  - **`OBI_3 = (10−11)/21 = −0.047619`**
  - Stoikov's `I = 0.75`.

### M5. Weighted mid (the simple size-weighted "microprice")

- **Formula** (Stoikov 2018, eqs. 2–3): `W = I·Pa + (1 − I)·Pb` with `I = Qb/(Qb + Qa)`.
  Equivalently `W = (Pb·Qa + Pa·Qb)/(Qb + Qa)`: each price is weighted by the *opposite* side's
  size. A large bid queue pulls the fair price toward the ask.
- **Do not confuse with** the same-side weighting `(Pb·Qb + Pa·Qa)/(Qb+Qa)`, which moves the
  wrong way. It must not be implemented under this name.
- **Units:** price. `Pb ≤ W ≤ Pa`.
- **Sampling:** event time. It changes on every L1 quantity update, which is why Stoikov calls it
  noisy.
- **Missing:** `None` on an invalid book or when `Qb + Qa = 0`.
- **Data:** L1 with quantities (`BboSeries` has them).
- **Limits:** this is a heuristic, not a martingale. Stoikov (2018) shows it is the micro-price
  only under a special Brownian imbalance model.
- **Fixture (F):** `I = 0.75`, so `W = 0.75·101.0 + 0.25·100.0` = **100.75**. The wrong same-side
  formula would give 100.25.

### M6. Micro-price (Stoikov 2018 definition)

- **Definition:** `P_micro(t) = lim_{i→∞} E[M_{τ_i} | F_t]`, where `τ_i` are the times of the next
  `i` mid changes and `F_t` is generated by `(M_t, I_t, S_t)`. Stoikov's Assumption 2 requires
  mid increments to be independent of the mid level, so `P_micro = M + g(I, S)`.
- **Discrete estimator** (Stoikov 2018 §3):
  - Discretise `I` into `n` buckets and `S` into `m` tick values.
  - From data, estimate:
    - `Q` (no mid change, transition to state `y`),
    - `R` (mid change of size `k ∈ K`),
    - `T` (mid change and next state `y`).
  - Compute `G¹ = (1 − Q)⁻¹ R K`, `B = (1 − Q)⁻¹ T`.
  - Then `P_micro = M + Σ_{j≥0} Bʲ G¹`, which converges when `B* G¹ = 0` (his Theorem 3.1).
    Stoikov **symmetrises** the training data (each observation is paired with its mirror
    `(1−I, S, −ΔM)`) to guarantee this.
- **Units:** price (the adjustment is often reported in ticks).
- **Sign:** an adjustment above the mid means an expected upward drift.
- **Sampling:** the Markov chain is estimated on a time grid (Stoikov uses discrete steps) or on
  events. The estimate depends on that choice, which must be recorded with the calibration.
- **Missing:** `None` outside calibrated `(I, S)` states. Spreads wider than the largest bucket
  are not extrapolated.
- **Numerics:** solve `(1 − Q) X = R K` with LU decomposition; never invert explicitly. Stop the
  series when `‖Bʲ G¹‖_∞ < 10⁻¹² · tick`, and fail if no convergence within a bound.
- **Data:** L1 with quantities and enough mid changes per state. **Train/test separation is
  mandatory:** a micro-price calibrated on the same data it is evaluated on is look-ahead.
- **Limits:** this is a *calibrated model output*. It is valid only for the regime it was fitted
  on. On a 10-minute sample with a median 16-tick spread, most `(I, S)` cells would be sparsely
  populated **[INFERENCE]**.
- **Fixture:** a hand-solvable chain with `n = 2` imbalance buckets (L: `I ≤ 0.5`, H: `I > 0.5`),
  a fixed spread, and mid moves `K = {−½, +½}` tick.
  - From H, per step: `P(+½) = 0.4`, `P(−½) = 0.1`, `P(no move, stay H) = 0.3`,
    `P(no move, go to L) = 0.2`. L is the mirror image.
  - After a move: from H, `P(move ∧ next H) = 0.3` and `P(move ∧ next L) = 0.2`. L is mirrored.
  - So `Q = T = [[0.3, 0.2], [0.2, 0.3]]` and `R K = [+0.15, −0.15]` ticks.
  - `(1−Q)⁻¹ = (1/0.45)·[[0.7, 0.2], [0.2, 0.7]]`, which gives:
    - **`G¹ = [+1/6, −1/6]` tick**
    - **`B = [[5/9, 4/9], [4/9, 5/9]]`**
    - **`G² = B G¹ = [+1/54, −1/54]`**
  - The antisymmetric eigenvalue is `1/9`, so the adjustment is
    `G¹·Σ (1/9)ʲ = (1/6)·(9/8)` = **+3/16 tick in state H**.
  - With book F (`I = 0.75`, so state H, tick 0.5): `P_micro = 100.5 + 0.09375` = **100.59375**.
    Compare `W = 100.75` (M5) and `M = 100.5`.

### M7. Order flow imbalance (Cont, Kukanov & Stoikov 2014)

- **Formula** (CKS §2.1). For consecutive L1 states `n−1 → n`:

  ```text
  e_n = 1{Pb_n ≥ Pb_{n−1}}·Qb_n − 1{Pb_n ≤ Pb_{n−1}}·Qb_{n−1}
      − 1{Pa_n ≤ Pa_{n−1}}·Qa_n + 1{Pa_n ≥ Pa_{n−1}}·Qa_{n−1}
  OFI_k = Σ_{n ∈ (t_{k−1}, t_k]} e_n
  ```

  CKS relate `ΔP_k` (the mid change in ticks) to `OFI_k` linearly, `ΔP_k = β·OFI_k + ε_k`, with
  `β` inversely related to average depth.
- **Units:** base units (lots). **Sign:** positive means net buying pressure at the touch.
- **Sampling:** each `e_n` is event time. `OFI_k` is summed over clock intervals. CKS use
  10-second intervals for the regression (their Appendix tables).
- **Batching rule (ours):** `n` indexes **completed exchange-time batches**, not raw rows. Intra-batch
  states never existed (§1.2). OFI is *not* invariant to batching, so record the rule.
- **Missing:** a transition into or out of an invalid book contributes nothing, and the interval
  is flagged.
- **Numerics:** integer lots, so the sum is exact.
- **Data:** L1 with quantities (`BboSeries`).
- **Limits:**
  - CKS note that OFI includes the price-changing events themselves. The OFI–ΔP regression is
    partly tautological at the same horizon (CKS footnote 3).
  - Use lagged OFI for prediction claims.
  - Hidden orders are not captured.
- **Fixture** (states `(Pb, Qb, Pa, Qa)` starting from book F, at 0, 10, 60, 130 and 180 ms):

  | n | t | state | e_n |
  |---|---|---|---|
  | 0 | 0 | (100.0, 3, 101.0, 1) | — |
  | 1 | 10 ms | (100.0, 5, 101.0, 1) | `5 − 3 − 1 + 1` = **+2** |
  | 2 | 60 ms | (100.0, 5, 101.5, 4) | `5 − 5 − 0 + 1` = **+1** |
  | 3 | 130 ms | (100.5, 2, 101.5, 4) | `2 − 0 − 4 + 4` = **+2** |
  | 4 | 180 ms | (100.0, 5, 101.5, 6) | `0 − 2 − 6 + 4` = **−4** |

  - Totals: `OFI(0, 180] = +1`. On a 100 ms grid, `OFI(0,100] = +3` and `OFI(100,200] = −2`.
  - Mid: 100.5 → 100.75, so **ΔP = +0.5 tick**.

### M8. Short-horizon realised variance and volatility (event vs clock sampling)

- **Formula:** `RV = Σ_j (M_{t_j} − M_{t_{j−1}})²` over the sampling times, and `σ̂² = RV / T_span`.
  Use price (or tick) changes for intraday horizons and log returns only for cross-asset
  comparison; state which one. Realised variance as an estimator of integrated variance is from
  Andersen, Bollerslev, Diebold & Labys (2003).
- **Units:** price²/s (or ticks²/s). Volatility `σ̂ = √σ̂²` is in price/√s.
- **Sampling:**
  - *Event:* every mid change.
  - *Clock:* previous-tick on a Δ grid.
  - Mid quotes contain microstructure noise (bid–ask bounce, discreteness), so event-sampled RV is
    biased upward and diverges as sampling gets finer. Zhang, Mykland & Aït-Sahalia (2005)
    document this and propose two-scale estimators. Bandi & Russell (2008) derive an optimal
    sampling frequency.
  - The Avellaneda–Stoikov strategy uses clock sampling at its 100 ms tick (§4).
- **Missing:** skip increments that span an invalid book, and record how many were skipped.
- **Data:** `BboSeries`.
- **Limits:** on a book whose spread is often many ticks wide, the mid jumps by large amounts
  when one side moves. Its variance is dominated by spread dynamics, not by the efficient price
  **[INFERENCE]**.
- **Fixture:** mid 100.5 at 0 ms, 101.0 at 40 ms, 100.5 at 120 ms, 101.0 at 130 ms, 101.5 at
  250 ms. Window [0, 300] ms.
  - *Event:* changes `+0.5, −0.5, +0.5, +0.5` give **RV = 1.0 price²**, so **σ̂² = 3.333 price²/s**.
  - *Clock (Δ = 100 ms):* samples 100.5, 101.0, 101.0, 101.5 give changes `+0.5, 0, +0.5`, so
    **RV = 0.5**, **σ̂² = 1.667 price²/s**.
  - The bounce at 120/130 ms doubles the event estimate.

### M9. Trade aggressor imbalance

- **Formula:** `TAI = (V_buy − V_sell)/(V_buy + V_sell)` over an interval, using **only trades
  whose aggressor side is recorded**. Report `coverage = (V_buy + V_sell)/V_total` with it.
- **Units:** dimensionless in [−1, 1]. **Sign:** positive means buyer-initiated volume dominates.
- **Sampling:** clock intervals or trade-count buckets.
- **Missing:** never infer a side. The tick rule and Lee & Ready (1991) are inference methods
  with known error rates and are out of scope unless labelled as such. Tardis states the aggressor
  side ("liquidity taker side"). The synthetic generator emits buy/sell flags
  (`crates/bb-data/src/synthetic.rs:204-205`). `TradeSeries.side` is `Option`
  (`market.rs:46-52`). Binance Vision trades carry `is_buyer_maker` (section 7).
- **Data:** `TradeSeries`.
- **Limits:**
  - A large order split into many prints counts by volume, not by order.
  - On Binance Futures a single aggressive order produces one print per maker order and level.
    Of the 2,220 sample trades, 1,395 belong to multi-print same-millisecond, same-side groups
    (1,227 groups; measured).
- **Fixture:** trades buy 2, buy 1, sell 4, unknown 3.
  - **TAI = (3 − 4)/(3 + 4) = −0.142857**, **coverage = 7/10 = 0.7**.

### M10. Liquidity depletion and replenishment

- **Definitions:** for one side, and a band anchored to the **pre-event mid** `M_0` (k bps, or the
  first N levels at `t_0`), over a window:
  - *trade depletion* `D_tr` = traded quantity at in-band levels;
  - *cancel depletion* `D_cx` = Σ quantity decreases at in-band levels, minus the trade-attributed
    part (clamped ≥ 0, matched by timestamp and price);
  - *replenishment* `R` = Σ quantity increases at in-band levels (including new levels);
  - *spread recovery time* `τ_S` = time from the batch that widened the spread (by a trade
    consuming the touch) to the first batch with spread ≤ the pre-event spread;
  - *depth recovery time* `τ_D` = time until in-band depth is again ≥ its pre-event value.
- **Units:** base units; ns for times. **Sign:** all quantities ≥ 0. Net flow is `R − D_tr − D_cx`.
- **Sampling:** event time (batches). Recovery times are right-censored at the window end and
  must be reported as censored, not dropped.
- **Missing:** an invalid book during the window marks the event as unusable.
- **Data:** L2 rows plus trades.
  - The split between trade and cancel needs both on the same clock. On Tardis Binance data the
    depth update reflecting a trade arrives a median of 10 ms *after* the trade's timestamp
    (section 6).
  - The matching window must allow for that lag. Report the window used.
- **Limits:**
  - MBP data cannot separate a cancel from a modification, or one participant from another.
  - "Replenishment" includes any new order, not only liquidity providers reacting to the trade.
- **Fixture** (ask side, book F, `M_0 = 100.5`, band 100 bps covers asks 101.0 and 101.5):
  - **Events:**
    - t = 0: buy trade 1 @ 101.0; the 101.0 level goes to 0.
    - t = 20 ms: ask 101.5 goes from 4 to 2.
    - t = 30 ms: a new ask 101.0 × 2 appears.
    - t = 50 ms: ask 101.5 goes from 2 to 5.
  - **Flows:** `D_tr = 1`, `D_cx = 2`, `R = 2 + 3 = 5`, net **+2**.
  - **In-band ask depth:** 5 → 4 → 2 → 4 → 7.
  - **Recovery:** the spread is 2 ticks before, 3 ticks after the trade, and 2 ticks again at
    30 ms, so **τ_S = 30 ms**. Depth is first ≥ 5 at 50 ms, so **τ_D = 50 ms**.

### M11. Note on the spread, OBI and microprice inputs on the local clock

Strategies see these quantities on the **local** clock, delayed by the feed latency: median
128.9 ms on the Tardis sample, which was captured in London in 2020 (dataset quality report). An
analytics view that shows "what the strategy saw" must recompute M1–M7 from the local-clock book,
not reuse the exchange-clock series. The tape stores only L1 of the local book at each intent
(`flight.rs:81-82`). Deeper local features need a local-clock replay of the dataset.

---

## 3. Execution metrics

Shared conventions:

- Our side `s`, fill `(p, x)`.
- Pre-fill mid `m0 = M(t_fill⁻)`: the last completed exchange batch *strictly before* the fill's
  exchange timestamp.
- Costs are positive when bad for us, unless stated otherwise.
- The existing `FillRecord.mid_tick` uses the state *at* `t_fill`, including the same-timestamp
  batch (`market.rs:29-31`, `partition_point(|x| x <= t)`).
- On recorded data that choice changes the sign of results. Measured on `maker_baseline`
  (Tardis, 305 maker fills), capture in ticks per unit is:

  | Reference mid | Capture |
  |---|---|
  | at `t` | −6.298 |
  | `t⁻` | −2.282 |
  | `t − 20 ms` | +1.093 |
  | `t − 100 ms` | +7.164 |

- For `avellaneda_stoikov` (70 maker fills) the same references give +3.73, +8.89, +12.77 and
  +31.77.
- For 156 of the 305 `maker_baseline` maker fills, no `QueueUpdate` precedes the `ExchFill`, and
  168 of the 305 occur at the exchange timestamp of a market trade (measured from the tape). A
  reference that includes the same-timestamp batch therefore sometimes includes the book move that
  caused the fill **[INFERENCE]**.
- Recommendation:
  - Define all spread metrics on `m0 = M(t⁻)`.
  - Keep a sensitivity row at `t − Δ` for Δ = the dataset's measured trade-to-depth lag (§6).
  - Document that `spread_capture` as currently implemented is "at `t`".

The fixtures share one scenario. **Decision:** buy `X = 5`, decided at local time `t_d` with
decision mid `P_d = 100.5` (book F as seen locally).
- **Arrival:** the IOC limit buy at 101.5 reaches the exchange 1 ms later. The exchange book is
  then bids 100.5 × 2, 100.0 × 3; asks 101.0 × 1, 101.5 × 4, 102.0 × 6, so `M_arr = m0 = 100.75`.
- **Fills:** 1 @ 101.0 and 3 @ 101.5 (taker). VWAP = 405.5/4 = 101.375. The remaining 1 is
  unfilled (IOC remainder expires).
- **Mid afterwards:** 101.25 at `t_fill + 1 s`; 102.0 when the remainder is abandoned (`P_end`).
- **Fees:** taker 4 bp of value. A separate **maker scenario:** a resting buy 2 @ 100.0 is filled
  with `m0 = 100.5`; then `m_{100ms} = 100.25` and `m_{1s} = 99.75`. Tick 0.5 throughout.

### E1. Effective spread

- **Formula:** per fill, `ES½ = s·(p − m0)` (effective half-spread). The full effective spread is
  `ES = 2·ES½`, and relative `ES½_bps = ES½/m0 × 10⁴`. An order's value is the quantity-weighted
  mean over its fills. This is the effective-spread construct used by Huang & Stoll (1996).
- **Sign:** positive is a cost, as for takers. Maker fills are normally negative: we earned the
  half-spread.
- **Clock:** exchange. **Missing:** `None` if `m0` is undefined.
- **Data:** `ExchFill` plus `BboSeries`. Change `enrich_fills` to use `t⁻`.
- **Limits:**
  - In simulation our fills never move the replayed book, so a taker's effective spread
    understates real cost for sizes beyond the displayed level.
  - The engine sweeps at most 100 ticks ([EXECUTION_MODEL.md](../EXECUTION_MODEL.md#5-market-orders)).
- **Fixture:**
  - Taker fills: `ES½ = 0.25` and `0.75`, so qty-weighted **ES½ = 0.625** (1.25 ticks),
    **62.03 bps** of `m0`.
  - Maker: **ES½ = 100.0 − 100.5 = −0.5** (1 tick earned).

### E2. Realised spread at horizon h

- **Formula:** `RS½(h) = s·(p − m_h)`.
- **Identity:** `ES½ = RS½(h) + s·(m_h − m0)`. The second term is the price impact, or drift in
  our favour.
- **Relation to existing markouts:** `markout(h) = s·(m_h − p) = −RS½(h)`
  (`crates/bb-forensics/src/metrics.rs:32`). Keep the existing positive-is-favourable markout
  convention for display, and the cost convention internally. Both are documented.
- **Horizons:** the existing 0.1/1/5/30 s (`result.rs:11-12`). Also report the count measured at
  each horizon.
- **Missing:** `None` past the data end.
- **Limits:** the horizon choice drives the result. Short horizons are dominated by bounce;
  long ones by unrelated price moves.
- **Fixture:**
  - Taker, h = 1 s: `RS½ = −0.25` and `+0.25`, weighted **+0.125**. Drift `s·(m_h − m0) = +0.5`,
    and `0.625 = 0.125 + 0.5` holds. Markout **−0.125 = −0.25 tick**.
  - Maker, h = 1 s: **RS½ = 100.0 − 99.75 = +0.25**; markout **−0.25**.

### E3. Adverse selection (mid move after fill)

- **Formula:** `AS(h) = −s·(m_h − m0) = s·(m0 − m_h)`. Positive means the market moved against
  us after the fill. For a maker this is the classic adverse-selection component; for a taker,
  negative values are the impact or alpha it captured.
- **Report:** the quantity-weighted mean curve over h ∈ {0.1, 1, 5, 30} s, split maker/taker,
  with `n` and a confidence band. Fills cluster in time, so use a block bootstrap over fill
  clusters, not an IID standard error.
- **Limits:**
  - Replay has no reaction to our order.
  - Adverse selection measured on replay is the adverse selection of the *historical* flow at
    the times our model says we filled. It is only as good as the queue model.
  - Lehalle & Mounjid (2017) analyse how queue position and latency interact with adverse
    selection.
- **Fixture:** maker buy at 100.0, `m0 = 100.5`. **AS(100 ms) = +0.25**, **AS(1 s) = +0.75**.
  Identity check: `ES½ = RS½ − AS`, so `−0.5 = 0.25 − 0.75`.

### E4. Implementation shortfall (Perold 1988)

- **Concept** (Perold 1988): the shortfall is the difference between the return of a *paper*
  portfolio, traded at the decision price without cost, and the *real* one.
- **Our formalisation for one parent decision** (the decomposition into execution and opportunity
  cost is standard practice; this exact formula is our definition):

  ```text
  IS = Σ_j s·x_j·(p_j − P_d)          execution cost   (= delay + trading, below)
     + s·(X − Σ_j x_j)·(P_end − P_d)  opportunity cost of the unfilled part
     + Σ fees
  delay   = s·(Σ x_j)·(M_arr − P_d)   decision-to-arrival drift (feed + order latency)
  trading = s·Σ x_j·(p_j − M_arr)
  ```

  `P_d` is the **decision mid**: local clock, `OrderIntent.market`. `M_arr` is the exchange mid at
  arrival. `P_end` is the mid when the remainder is abandoned (cancel or expiry `terminal_ts`), or a
  fixed evaluation horizon; the choice must be stated.
- **Units:** quote currency; bps of `|X|·P_d`. **Sign:** positive is a cost.
- **Data:**
  - `OrderIntent` gives `P_d`, `X` and side.
  - `RequestSent` / `ExchQueued` / the first exchange event give arrival.
  - `ExchFill` and `terminal_ts` complete it. All are recorded.
  - *"Parent order"* needs a grouping key. Today one intent equals one order; a multi-order
    parent needs a strategy-supplied id (not present).
- **Limits:**
  - Market-maker quotes have no natural decision price or target quantity, so IS is meaningful
    for directional orders only (imbalance taker, scripted).
  - Opportunity cost depends on `P_end`.
- **Fixture:**
  - Execution `1·0.5 + 3·1.0 = 3.5`, of which delay `4·0.25 = 1.0` and trading `4·0.625 = 2.5`.
  - Opportunity `1·(102.0 − 100.5) = 1.5`. Fees `0.0004·405.5 = 0.1622`.
  - **IS = 5.1622**, which is **102.73 bps** of 502.5.

### E5. Slippage vs arrival mid

- **Formula:** `slip_arr = s·(VWAP − M_arr)`. Also `slip_dec = s·(VWAP − P_d)`, and
  `slip_dec − slip_arr = s·(M_arr − P_d)` is the latency drift.
- **Units:** price, ticks, bps. **Sign:** positive is a cost.
- **Clock:** `M_arr` on the exchange clock, `P_d` on the local clock.
- **Limits:** for taker orders, `slip_arr` equals the qty-weighted `ES½` when `m0 = M_arr`.
- **Fixture:** **`slip_arr = 101.375 − 100.75 = 0.625`**; **`slip_dec = 0.875`**; latency drift
  **0.25**.

### E6. Fill probability vs modeled queue position

- **Covariate:** relative queue position at entry `ρ = queue_ahead / level_qty` (from
  `ExchQueued`, `flight.rs:120-128`, already in `OrderRecord.queue_ahead_at_entry`), bucketed.
  Use absolute lots ahead as a second covariate.
- **Outcome:** "filled (any quantity) within horizon H of resting".
- **Estimator:** **Kaplan–Meier** (Kaplan & Meier 1958) per bucket. Orders cancelled or still
  working before H, without a fill, are *right-censored*, not failures. A naive fraction treats
  them as failures; dropping them biases the estimate upward.
- **Units:** probability in [0, 1], with a Greenwood confidence interval and the bucket's `n`.
- **Limits:** queue position is **modeled**: the queue model's output, not an observation
  ([QUEUE_MODELS.md](../QUEUE_MODELS.md)). This curve characterises the model plus the data. It is
  not evidence that the model is right. It is useful for comparing queue models on the same orders.
- **Fixture:** one bucket, H = 1 s.
  - Orders: A filled at 0.2 s; B cancelled unfilled at 0.4 s; C filled at 0.8 s; D unfilled and
    still working at 1.0 s.
  - Naive `2/4 = 0.5`; dropping censored `2/3`.
  - **Kaplan–Meier:** `S(0.2) = 3/4`; B is censored, leaving a risk set of 2; `S(0.8) = 3/4·1/2`,
    so **F(1 s) = 1 − 3/8 = 0.625**.

### E7. Time to first and to complete fill

- **Formula:** `TTFF = first_fill_ts − exch_arrival_ts` (already computed,
  `crates/bb-forensics/src/metrics.rs:167-169`). `TTCF = ts of the fill that makes leaves_qty 0 −
  exch_arrival_ts`.
- **Clock:** exchange. **Units:** ns.
- **Missing:** unfilled or partially filled orders are censored. Report quantiles over filled
  orders *and* the censored count. Better, use a Kaplan–Meier median.
- **Fixture:** arrival 1,000 ms; fills 2 @ 1,300 ms and 3 @ 1,750 ms for qty 5. **TTFF = 300 ms**,
  **TTCF = 750 ms**.

### E8. Cancel latency

- **Formulas:**
  - *Cancel-to-exchange* = exchange time the cancel was processed (`terminal_ts` of a cancelled
    order, or the `ExchResponse` ts) − `cancel_sent_ts`.
  - *Cancel round trip* = `LocalResponse` ts − `cancel_sent_ts`.
  - *In-flight fill exposure* = quantity filled between `cancel_sent_ts` and the exchange
    processing time.
  - *Late cancel* = cancel rejected because the order was already gone
    (`OrderRecord.cancel_rejected`).
- **Clocks:** in simulation both clocks share one epoch time base, so cross-clock differences are
  exact. In live data they would need clock synchronisation.
- **Limits:** in replay the latencies are whatever the latency model drew. Only the exposure they
  create is informative.
- **Fixture:** cancel sent 5,000.0 ms (local); processed 5,001.0 ms (exchange); acknowledged
  5,002.0 ms (local); a 1-unit fill at 5,000.4 ms.
  - **Cancel-to-exchange 1.0 ms**, **round trip 2.0 ms**, **in-flight fill 1 unit**.

### E9. Maker/taker decomposition of P&L

- **Identity** (marking at a single mid series `M`, final mark `M_T`):

  ```text
  equity_T = Σ_fills s·x·(m0 − p)          instantaneous edge vs pre-fill mid
           + Σ_fills s·x·(M_T − m0)        inventory / post-fill drift
           − Σ fees
  ```

  The two sums together equal cash plus the marked position
  (`−Σ s·x·p + (Σ s·x)·M_T`), so the decomposition is exact. Each term is split by `maker` flag.
- **Caveat:** the existing equity series marks at the **local** mid (`result.rs:94-95`). The
  identity holds only if every term uses the same mark. Use the exchange-clock dataset mid and say
  so.
- **Units:** quote currency.
- **Fixture:**
  - Maker buy 2 @ 100.0 (`m0 = 100.5`, 2 bp fee); taker sell 2 @ 100.0 (`m0 = 100.25`,
    4 bp fee); `M_T = 100.25`.
  - Edge: maker **+1.0**, taker **−0.5**.
  - Drift: maker `2·(100.25 − 100.5) = −0.5`, taker **0**.
  - Fees **0.04 + 0.08**.
  - **Net −0.12**, which equals cash 0 + position 0 − fees 0.12.

### E10. Opportunity cost of unfilled orders

- **Formula:** for each order with unfilled quantity `u` that became terminal (cancelled,
  expired) or was working at the end, `OC(h) = s·u·(m_{t_term+h} − P_d)`. Use h = 0 for "at
  abandonment", as in E4, or fixed horizons. Report the sum and the distribution.
- **Sign:** positive means the price moved away, so not filling cost us.
- **Limits:**
  - This is meaningful for directional intents only. For passive market-making quotes an
    unfilled quote is not a loss.
  - It depends on `P_d`, which is local and stale by the feed latency.
- **Fixture** (decision scenario): **OC(0) = 1·(102.0 − 100.5) = 1.5**.

### E11. Counts, ratios and their caveats (already implemented)

`fill_ratio_orders`, `fill_ratio_qty`, maker/taker fill counts and order-age quantiles already
exist (`crates/bb-forensics/src/metrics.rs:122-261`). Two gaps:

- Every ratio must expose its numerator and denominator.
- Order age is right-censored for orders working at the end, as in E6 and E7.

---

## 4. Avellaneda–Stoikov and the other bundled strategies

### 4.1 What the papers say (read 2026-11-14)

**Avellaneda & Stoikov (2008)**, from the journal PDF (`math.nyu.edu/~avellane/HighFrequencyTrading.pdf`).

**Model:**
- The mid is arithmetic Brownian motion, `dS = σ dW` (eq. 1).
- The agent has CARA utility over terminal wealth at `T`.
- The agent quotes and "is committed to respectively buy and sell **one** share" per fill (§2.4).
- Fills arrive as Poisson with intensity `λ(δ) = A·e^{−kδ}` (eq. 12, eq. 20).
- Quotes "can be continuously updated at no cost".

**Results:**
- Frozen-inventory reservation price, eq. (8):

  ```text
  r(s, q, t) = s − q·γ·σ²·(T − t)
  ```

- After an expansion in `q`, and a *linear* approximation of the arrival term (eq. 26), they
  obtain the same reservation price (eq. 29) and a total spread (eq. 30):

  ```text
  δᵃ + δᵇ = γσ²(T − t) + (2/γ)·ln(1 + γ/k)
  ```

- **Infinite horizon (§2.3)** is solved differently. With a discount parameter `ω`, the
  stationary reservation prices are

  ```text
  rᵃ(s, q) = s + (1/γ)·ln(1 + (1 − 2q)γ²σ² / (2ω − γ²q²σ²))
  ```

  and similarly for `rᵇ`, with `ω > ½γ²σ²q²`. The authors suggest `ω = ½γ²σ²(q_max + 1)²`.
  Setting `T − t` to a constant is **not** what the paper does.
- **Simulation note (§3.3):** `dt` "must be larger than the typical tick time".

**Guéant, Lehalle & Fernandez-Tapia (2013)**, from arXiv:1105.3115:
- Footnote 8 restates the AS approximations
  `δᵇ ≈ (1/γ)ln(1+γ/k) + ((1+2q)/2)·γσ²(T−t)` and `δᵃ ≈ (1/γ)ln(1+γ/k) + ((1−2q)/2)·γσ²(T−t)`.
  It shows they are "nothing but the Taylor expansions of the optimal quotes for t close to T".
- §4 shows the optimal quotes are "almost independent of t, as soon as t is far from the
  terminal time T".
- They give closed-form asymptotic approximations:

  ```text
  δᵇ∞(q) ≈ (1/γ)ln(1 + γ/k) + ((2q + 1)/2)·√( σ²γ/(2kA) · (1 + γ/k)^{1 + k/γ} )
  δᵃ∞(q) ≈ (1/γ)ln(1 + γ/k) − ((2q − 1)/2)·√( σ²γ/(2kA) · (1 + γ/k)^{1 + k/γ} )
  ```

  These depend on `A`. AS's approximations do not depend on `A`.

**Guéant (2017)**, from arXiv:1605.01862:
- For transactions of size Δ with exponential intensities (eq. 4.6), the liquidity term becomes
  `(1/(ξΔ))·ln(1 + ξΔ/k)`.
- The inventory factor becomes `(2q + Δ)/2`, with a `√(γσ²/(2AΔk)·…)` scale.

### 4.2 The code (`crates/bb-strategies/src/avellaneda_stoikov.rs`)

| Item | Code | Paper | Verdict |
|---|---|---|---|
| Reservation price | `reservation = mid − position·γ·σ²·horizon_s` (`:147-148`) | AS eq. 29 | **matches** with `T − t := horizon_s` |
| Half spread | `0.5·γσ²·horizon + ln_1p(γ/κ)/γ` (`:149`) | AS eq. 30 ÷ 2 | **matches**; `ln_1p` is the numerically right choice |
| Quotes | `floor((r − δ)/tick)`, `ceil((r + δ)/tick)`, with a 1e-11 relative grid tolerance (`:42-55`, `:157-162`) | continuous prices | rounding outward widens each side by < 1 tick (documented) |
| Horizon | constant `horizon_s` | finite T (eqs. 29–30), or §2.3 with ω, or GLFT asymptotics | **discrepancy 1** |
| Fill size | `q` = position in base units; `order_qty` = 0.01 BTC in presets | one unit per fill (AS §2.4); Δ explicit in Guéant (2017) | **discrepancy 2** |
| σ estimate | EWMA of squared 100 ms clock-sampled mid changes (local book), ÷ interval (`:110-123`, `:146`) | σ of the mid's BM, assumed constant | **caveat 3** |
| κ, A | κ preset per tick; A absent | `k` from the order-size and impact distribution (AS eq. 12) | parameter, not estimated (documented in STRATEGIES.md) |
| Inventory limit | hard gate in `quoting.rs:94-103` | unconstrained (AS); GLFT has bounds ±Q | documented simplification |

**Discrepancy 1: the "stationary variant" is ad hoc.** STRATEGIES.md calls the constant `T − t`
"the common infinite-horizon variant, not the paper's finite-horizon solution". The paper's
infinite-horizon solution is §2.3, which has a different functional form and parameter `ω`, and it
is not implemented. GLFT show the constant-`T − t` formulas are a near-terminal Taylor expansion.
Far from `T` the true optimal quotes converge to the GLFT asymptotics, whose inventory skew scales
as `√(σ²γ/(kA))`, not as `γσ²(T − t)`.

Recommendation:
- Rename the strategy's documentation of the horizon to "AS near-terminal approximation evaluated
  at a fixed remaining time `horizon_s`".
- Optionally add a `glft` variant implementing the closed form above. It needs `A`, which can be
  estimated from simulated fill intensities at several distances.

**Discrepancy 2: fill-size units.**
- With Δ = 1 base unit the code is consistent. The presets quote 0.01 BTC, so Δ = 0.01.
  Following Guéant (2017) eq. 4.6 with ξ = γ:
  - the liquidity term would be `ln(1 + γΔ/κ)/(γΔ)`;
  - the inventory part of the half-spread would be `Δ·γσ²τ/2`, not `γσ²τ/2`.
- On the Tardis preset (γ = 10, κ = 50, both per price unit):
  - the liquidity term is 1.998 ticks, versus the code's 1.823;
  - the volatility term of the half-spread shrinks by a factor of 100.
- The reservation-price shift `q·γσ²τ` is unchanged (it is linear in `q`) **[INFERENCE: derived
  from AS §2.2 with ±Δ instead of ±1]**.
- Either document that the code's γ is "per unit of fill size", or introduce Δ = `order_qty`
  explicitly.

**Caveat 3: σ from a bouncing mid.** AS assume σ is the volatility of the *efficient* price. The
code estimates it from squared 100 ms changes of the *local* mid. That clock sampling is the less
noise-prone choice (M8). On the Tardis sample, though, the spread median is 16 ticks, and a
one-sided touch change moves the mid by many ticks. The estimate there includes spread
fluctuation. A micro-price or weighted-mid input (M5/M6) would reduce bounce **[INFERENCE]**.

**Other checks that pass:**
- σ² is converted per second (`ewma_var / interval_s`).
- The EWMA is seeded with the first squared change and not blended with 0 (`:115-119`).
- A gap spanning several ticks is normalised by elapsed ticks (`:112-114`).
- Warm-up suppresses quoting (`:140-142`).
- Non-finite parameters quote nothing (`:150-155`).

**Fixture** (AS formulas as coded): s = 100, q = 0.5, γ = 0.1, σ² = 4 price²/s, τ = 1 s,
κ = 1.5, tick 0.5.
- **r = 100 − 0.5·0.1·4 = 99.8**
- **δ = 0.2 + ln(1 + 0.1/1.5)/0.1 = 0.2 + 0.645385 = 0.845385**
- Bid `98.954615`, which is `197.909` ticks, floored to **197 (98.5)**.
- Ask `100.645385`, which is `201.291` ticks, ceiled to **202 (101.0)**.
- With fill size Δ = 0.1 (Guéant), the liquidity term would be **0.664454** and the inventory
  half-spread term **0.02**.
- GLFT with A = 140 and q = 1: `√(4·0.1/(2·1.5·140)·(1.0667)^{16}) = 0.051717`, so
  **δᵇ = 0.722961** and **δᵃ = 0.619527**.

### 4.3 Touch maker (`crates/bb-strategies/src/touch_maker.rs`)

No paper formula to check against. The rule is
`bid = best_bid − offset − round(k·position)`, `ask = best_ask + offset − round(k·position)`
(`:50-53`), then the shared clamp `bid ≤ best_ask − 1`, `ask ≥ best_bid + 1` (`quoting.rs:41-55`)
and the inventory gate (`quoting.rs:94-103`). The code matches STRATEGIES.md. Points to document:

- `f64::round` rounds half away from zero, so the skew is symmetric for long and short.
- A long position lowers the ask *into* the spread (`ask = best_ask − skew`). That is a deliberate
  price improvement to shed inventory. The clamp stops it at `best_bid + 1`.
- Quotes are pegged to the **local** touch. On data where the touch flickers across a wide spread,
  "keep the order if the price is unchanged" (`quoting.rs:147-156`) rarely keeps queue position.
  The keep-or-replace rate is a useful E6 covariate **[INFERENCE]**.

### 4.4 Imbalance taker (`crates/bb-strategies/src/imbalance_taker.rs`)

- **Signal:** `OBI_1` on the local book (`strategy.rs:66-70`), compared with a fixed threshold
  (`:67-87`). This is M4 with N = 1 and is computed correctly, including the `total > 0` guard.
- **Literature:**
  - L1 imbalance does predict the next mid move. Stoikov (2018) and Cartea, Donnelly & Jaimungal
    (2018) both condition on it.
  - Stoikov conditions on **spread as well**. A threshold on `OBI_1` alone treats a 1-tick and a
    100-tick spread the same, though the expected move per unit of imbalance is a property of the
    spread regime (Stoikov's `g(I, S)`).
  - Not a coding error; a modelling caveat for documentation.
- **Execution:**
  - IOC at the opposite touch of a book seen ~129 ms late (Tardis feed latency). Expected cost
    per round trip is ≥ the effective spread (E1) plus fees.
  - Its directional orders are the right population for implementation shortfall (E4) and
    opportunity cost (E10).
- No formula discrepancies found.

---

## 5. Sharpe ratios and risk metrics

### 5.1 What Lo (2002) establishes (read 2026-11-14)

Lo, *The Statistics of Sharpe Ratios* (FAJ 2002):

- Under IID returns the estimator `SR̂ = μ̂/σ̂` has asymptotic variance `V_IID = 1 + SR²/2`
  (his eqs. 8–9; Table 1 note "VIID = 1 + 1/2SR²"). So `SE(SR̂) ≈ √((1 + SR̂²/2)/T)` for `T`
  observations.
- Aggregating `q` periods multiplies SR by `√q` **only under IID returns** (eq. 17).
- With autocorrelations `ρ_k`, `Var(R(q)) = σ²·[q + 2 Σ_{k=1}^{q−1} (q − k)ρ_k]` (eq. 19), so

  ```text
  SR(q) = η(q)·SR,   η(q) = q / √( q + 2 Σ_{k=1}^{q−1} (q − k)·ρ_k )      (eq. 20)
  ```

  Positive serial correlation makes `η(q) < √q`. Naive `√q` scaling then overstates the
  aggregated ratio.

Two further results are relevant to BlackBox's parameter grids (`blackbox grid`):

- the *Probabilistic Sharpe Ratio* (Bailey & López de Prado 2012);
- the *Deflated Sharpe Ratio* (Bailey & López de Prado 2014), which corrects for non-normality
  and for the number of trials when the best of many configurations is reported.

**Fixtures:**
- Per-period returns `[1, −1, 2, 0, 1, 3, −2, 0, 1, 1]`: mean 0.6, sample s.d. 1.429841,
  **SR̂ = 0.419627**, **SE_IID = √((1 + 0.419627²/2)/10) = 0.329855**. The estimate is about one
  standard error from zero.
- AR(1) returns with `ρ_k = 0.3ᵏ`, q = 10: **η(10) = 2.401**, versus `√10 = 3.162`.

### 5.2 Measured properties of our equity series

Six local runs: the presets `maker_baseline`, `avellaneda_stoikov` and `imbalance_taker` on
`synthetic-demo` (120 s) and on the Tardis 10-minute sample. P&L increments are taken between
consecutive 100 ms equity samples (`EquitySample`, local clock, local-mid mark).

| Run | n | zero increments | ρ₁ | per-sample SR̂ | η(10) vs √10 | "annualised" SR (√(periods/yr) = 17,758) |
|---|---|---|---|---|---|---|
| touch maker, Tardis | 6,012 | 80.1% | +0.262 | −0.127 | 2.35 vs 3.16 | −2,251 |
| A-S, Tardis | 6,012 | 87.8% | +0.144 | −0.041 | 2.61 vs 3.16 | −723 |
| imbalance taker, Tardis | 6,012 | 80.0% | +0.240 | −0.268 | 3.11 vs 3.16 | −4,762 |
| touch maker, synthetic | 1,200 | 6.8% | −0.055 | −1.817 | 3.12 vs 3.16 | −32,259 |
| A-S, synthetic | 1,200 | 35.8% | −0.014 | −0.191 | 2.98 vs 3.16 | −3,393 |
| imbalance taker, synthetic | 1,200 | 27.9% | +0.169 | −0.314 | 3.20 vs 3.16 | −5,571 |

What these numbers show:

- **The sample is short.** A 600 s run has 10 one-minute buckets and 60 ten-second buckets. No
  normal-approximation inference is defensible on 10 observations.
- **The distribution is not normal.** 80–88% zero increments (no position, unchanged mark) make a
  point mass. The IID-normal SE formula does not apply.
- **The data are serially dependent.**
  - Positive `ρ₁` on every Tardis run. A Bartlett-truncated long-run variance factor of 2.65 for
    the touch maker means ≈ 2,271 effective observations, not 6,012.
  - The mark is the *local mid*, which on the real sample bounces across a wide spread. Part of
    the dependence is mark noise, not P&L **[INFERENCE]**.
- **η(q) is unstable when q approaches n.** For the synthetic imbalance taker (n = 1,200), the
  sample-ACF plug-in gives η(600) = 142.8 against √600 = 24.5, a nonsensical value caused by
  summing ~600 noisy autocorrelations. Lo's correction needs `q ≪ n` and a truncated or
  kernel-weighted ACF (Newey & West 1987).
- **Annualising is meaningless.** It multiplies the 100 ms ratio by 17,758 and extrapolates
  10 minutes of one market regime to a year.
- **Fees dominate some runs.** The synthetic touch maker's per-sample SR of −1.8 reflects a near-
  deterministic fee drain, not risk-adjusted skill.

### 5.3 Recommendation: what to show

| Show | How | Caveat to print |
|---|---|---|
| Net P&L with the E9 decomposition (edge maker/taker, drift, fees) | exact identity on one mark | "Replay: no market impact; queue position modeled" |
| P&L per unit traded: net P&L ÷ turnover, bps | from fills | fee-dominated at small edge |
| Max drawdown and time under water | on the equity series as sampled; state Δ = 100 ms and the mark | path-dependent; one realisation; not a risk forecast |
| Inventory risk: mean \|position\|, time at the position limit, max long/short | existing fields plus time-at-limit | — |
| Markout / adverse-selection curves (E2, E3) | with `n` and a block-bootstrap CI over fill clusters | horizon-dependent |
| A Sharpe-type statistic, **only if** the run has ≥ 30 non-overlapping buckets at the chosen horizon | mean/sd of bucket P&L, HAC (Newey–West) SE with truncation lag ≪ number of buckets, **no annualisation**, horizon in the label ("1-min SR") | below 30 buckets show "insufficient sample"; in grids show the number of trials and prefer the Deflated SR |

**Do not show:** an annualised Sharpe or Sortino, Calmar, or any ratio extrapolated beyond the
data span. Also never show IID standard errors on 100 ms increments.

---

## 6. Tardis sample: trades outside the reconstructed BBO

### 6.1 The finding being explained

The dataset `binance-futures-btcusdt-2020-02-01-10m` contains Binance Futures BTCUSDT, the first
601 s of 2020-02-01, tick 0.01 (`data/datasets/.../dataset.json`). Its quality check
`trades.outside_book` reports: "659 of 2212 trades printed more than one tick outside the
reconstructed best bid and ask at their exchange timestamp (8 trades occurred while the book was
one-sided or crossed and were not checked)".

The check (`crates/bb-data/src/quality.rs:331-347`) compares each trade with the book after all
earlier exchange-time batches. Trades sort before depth rows of the same timestamp
(`tardis.rs:6`, `:501`). A trade is flagged when `tick > ask + 1 || tick < bid − 1`.

My independent Python reconstruction from the raw CSVs reproduces **exactly 659 of 2,212 checked
(29.79%), 8 unchecked**. All results below come from that reconstruction ([Methods](#9-methods-and-reproducibility)).

### 6.2 Shape of the violations (measured)

- **655 of 659** are on the aggressor's far side (buy above the ask, sell below the bid); 4 are
  on the "wrong" side.
- Distance beyond the touch: p50 **40 ticks**, p90 212, p99 423, max 469 ticks.
- **616 (93.5%)** belong to a group of prints with the same millisecond and the same aggressor
  side. The 659 flagged trades fall in only **213** distinct (millisecond, side) groups.
- **641 (97.3%)** print at a price where the pre-trade book had a resting level on the passive
  side.

### 6.3 Hypotheses tested

| Hypothesis | Test | Result | Share of the 659 explained |
|---|---|---|---|
| **H1. Multi-level sweep evaluated against the pre-trade touch** (a definitional artefact, not a data error) | trade price is a resting passive-side level at `T⁻`, **and** earlier same-ms same-side prints consumed ≥ all displayed depth in front of it | 414 strict; 420 with 10% depth tolerance; 641 if only "price is a resting level" | **62.8%** (strict) |
| **H2. Timestamp alignment, exchange clock:** depth stamped later than trades | lag from trade `T` to the first depth row at that price on the passive side: p10 5 ms, **p50 10 ms**, p75 13 ms, p90 17.9 ms, p99 72 ms, n = 2,212; only 1.5% at 0 ms | trades not explained by H1 but inside `[bid−1, ask+1]` of some completed batch in `(T, T+50 ms]` | **34.4%** (227) sequentially after H1; 93.0% marginally |
| H2b. Window ±k ms around `T` | any completed batch in the window has the trade inside | ±1 ms 2.3%, ±5 ms 7.1%, ±10 ms 26.1%, **±50 ms 93.3%**, ±250 ms 96.2%, ±1 s 97.1%. Forward-only [T, T+k] ≈ the full window (93.0% at 50 ms); backward-only [T−k, T] only 15.8% at 50 ms | asymmetry: the book lags the trades, not the reverse |
| H2c. A constant shift (book at `T + d`) | — | d = +20 ms resolves 57.4% but moves 214 previously-inside trades outside; d = +50 ms: 47.0% resolved, 566 newly outside | not a fix: a constant shift trades one error for another |
| H3. Local clock: each channel by arrival time | book from rows that arrived strictly before the trade row arrived | 614/2,212 outside (27.8%); 585 of the 659 still outside. Trade-channel feed latency p50 144.2 ms vs depth 128.7 ms (15.5 ms later) | **≈ 11%** (74 of 659) |
| H4. Snapshot / initial state (REST snapshot spliced into the WebSocket diffs, Tardis binance-futures page) | flag rate by time; agreement of our BBO with Binance's independent `bookTicker` stream (Tardis `book_ticker`, same day) | flag rate 73% in 0–1 s, 56% in 0–10 s, 33% in 10–60 s, 30% in 60–300 s, 19% in 300–601 s. **But** `bookTicker` BBO equals a reconstructed state within ±10 ms in 99.2% of updates in the first 10 s (99.5–99.6% later), and the strict-sweep share is similar (66% in 0–10 s vs 62% later) | **≈ 0%**: the early excess is more sweeping, not a bad book |
| H5. Liquidation trades (Binance Futures specific) | Tardis `liquidations` file for the same day | first liquidation 4,408 s after the window start; **0 in the window** | **0%** |
| H6. Price-level aggregation | trade prices on the 0.01 grid (quality check: 0 off-grid); flagged prices are book levels | 641/659 at a level present in the pre-trade book; of the other 18, 17 resolve within 50 ms (H2) | **0%** |
| H7. Missing book updates | an independent BBO source should then disagree with the reconstruction | `bookTicker` agrees within ±10 ms in **99.5%** of 13,378 updates; outside rate vs `bookTicker` (last update received before the trade) **612/2,212 = 27.7%**, 584 of them among our 659 | **not supported** |

The **sequential attribution** (H1 first, then H2) sums to 100%: **62.8% sweep + 34.4% depth-stamp
lag + 2.7% unexplained (18 trades: 7 in the first 10 s, 11 later)**.

### 6.4 Interpretation

- **Timestamps.** Tardis's open-source normaliser stamps Binance Futures trades with `T`, the
  transaction time, and depth updates with `E`, the event time
  ([OPEN_SOURCE_RESEARCH.md](../OPEN_SOURCE_RESEARCH.md#which-binance-futures-field-is-the-exchange-timestamp)).
  Three measurements here fit that:
  - the median 10 ms trade→depth lag;
  - the 15.5 ms higher feed latency of trade rows than depth rows;
  - Binance Vision's own `bookTicker` files, which carry both fields; in a 2024 file
    `event_time − transaction_time` has a median of 7 ms (measured on 200k rows of
    `BTCUSDT-bookTicker-2024-03-30`; see section 7).

  That the CSV export uses the same mapping remains an **[INFERENCE]**.
- **The book is not wrong.** Binance's own L1 stream agrees with the reconstruction, and it flags
  the same share of trades as outside. The headline 29.8% mostly measures how often aggressive
  orders walked more than one tick into a thin book (median spread 16 ticks).

### 6.5 Conclusion and what the quality report should say

**Conclusion.**
- About 63% of the flagged trades are ordinary multi-level sweeps compared with the pre-trade
  touch.
- About 34% are explained by the depth channel's timestamps trailing the trade channel's by about
  10 ms (median).
- About 3% (18 trades) are unexplained.
- Snapshot problems, liquidations, price aggregation and missing book updates explain
  approximately nothing.
- A constant timestamp shift is not a correct repair.

**Proposed rewording of `trades.outside_book`** (the implementer owns the code change). Split it
into three findings:

1. `trades.beyond_touch_sweep` (info): "N trades printed beyond the pre-trade best bid/ask at
   levels that were resting in the book, after earlier prints of the same aggressor in the same
   timestamp had consumed the depth in front of them (a multi-level sweep)." Tardis sample: 414.
2. `trades.book_lags_trades` (warn): "N trades fall outside the pre-trade book, but inside it
   within L ms after their timestamp. The book channel's exchange timestamps trail the trade
   channel's: trade → depth-update lag p50 X ms, p90 Y ms." Report the lag distribution as a
   dataset statistic. Tardis sample: 227; p50 10 ms, p90 17.9 ms.
3. `trades.outside_book` (warn): only the remainder that neither explains. Tardis sample: 18 of
   2,212.

**Consequences for the simulator (documentation, not a fix):**
- hftbacktest processes a trade at `T` before the depth update stamped about 10 ms later. The
  exchange-side book that simulated orders see therefore trails trades by that lag.
- Trade-triggered maker fills are evaluated against a book that does not yet reflect the trade.
- This is one candidate cause of the negative spread capture recorded in
  [PNL_ACCOUNTING.md](../PNL_ACCOUNTING.md) (section 3 shows the sensitivity). It is not proven.

---

## 7. Real data a public hosted service could serve

The decided deployment is a public service: anyone runs backtests and sees charts and tapes
derived from the data. The question is whether any real dataset may legally back it. Tardis may
not (ToS 9.2(2), [MARKET_DATA.md](../MARKET_DATA.md#redistribution-rules)). Below, quotes, file
schemas and measurements come from a source survey done for this document on 2026-11-14:
documents read, one sample file downloaded per source and inspected. I re-read Binance Vision
clauses 3.1, 4.1, 4.3 and 4.5 myself.

| Source | Licence / terms (date) | L2 incremental? | Timestamps | Hosted service | Engine fit |
|---|---|---|---|---|---|
| **Binance Vision** (data.binance.vision), USDⓈ-M | `TERMS_AND_CONDITIONS.md` v1.0, "Last Updated: August 26, 2026": CC BY-NC-SA 4.0 (3.1). Backtesting only "for purely personal non-production research" (4.1). "You shall not license, sub-license, sell, rent, lease, **host**, or commercially exploit the Datasets or any derivative real-time or historical data feeds" (4.3). Share-alike and attribution (4.5). <https://raw.githubusercontent.com/binance/binance-public-data/master/TERMS_AND_CONDITIONS.md> | **No.** `trades` (`id,price,qty,quote_qty,time,is_buyer_maker`, ms; to 2026-11-12); `bookTicker` (`update_id,best_bid_price,best_bid_qty,best_ask_price,best_ask_qty,transaction_time,event_time`; every BBO change; **last file 2024-03-30**); `bookDepth` (`timestamp,percentage,depth,notional`: cumulative depth in ±0.2/1/2/3/4/5% bands, **≈30 s** snapshots, 1 s resolution) | trades `time` ms; bookTicker carries both `transaction_time` and `event_time` (event − transaction median 7 ms) | **No** (4.3 "host"; personal-only backtesting) | L1 only and discontinued; bookDepth is useless for queues |
| **Bybit** (public.bybit.com trades; `quote-saver.bycsi.com/orderbook/linear/BTCUSDT/{date}_BTCUSDT_ob500.data.zip`) | No terms on the file hosts. Bybit T&C (effective 2 Sep 2026): market data "was provided for the User only and is not to be further distributed without the written consent of the Company"; no derivative works (6.4). API terms 6.7: no repackaging of "Service Data" | **Yes:** NDJSON snapshot + deltas, 500 levels, ≈100 ms cadence; files seen 2023-03-01 … 2025-08-20 (404 from 2025-08-25 at that path; newer location unverified) | `cts` = matching-engine time, `ts` = system time (Bybit v5 docs); trades in seconds with fractions | **No** | technically good (MBP-500 at 100 ms; no L3) |
| **OKX** | Historical-data T&C (26 Oct 2023): "used only for your personal use"; re-distribution to end users counts as commercial. API Agreement (28 Jul 2026) §9.4: may not "publish, display, or otherwise make Market Data available to any third party, whether for commercial gain or otherwise" | L2 history "from March 2023" per OKX's page; **schema unverified** (okx.com unreachable from this network) | trades `created_time` ms | **No** | probably; unverified |
| **Coinbase** | Market Data Terms (7 Aug 2026): use "exclusively for … personal or research purposes"; may not "Redistribute, display, or disseminate the Market Data—or any data, charts, analytics … derived from" it to third parties | no official historical L2/L3 archive found (search only) | — | **No** | — |
| **LOBSTER samples** (php.lobsterdata.com/info/DataSamples.php) | no licence grant found for the samples; footer "All rights reserved"; terms "issued with your LOBSTER contract"; derived from NASDAQ TotalView-ITCH | **L3 message file + 1–50-level book**, one day (2012-06-21), AMZN/AAPL/GOOG/INTC/MSFT (30/50 levels: AAPL/MSFT/SPY, 09:30–10:30 only) | seconds after midnight, ns precision, exchange (ITCH) time | **No grant; would need written permission** | excellent (exact queue position possible) |
| **FI-2010** (Etsin / Fairdata) | CC BY 4.0, open access | normalised feature vectors only; no raw prices, sizes or events | — | legally yes | **unusable** for replay |

The OPEN_SOURCE_RESEARCH.md entry for Binance stated that the `bookTicker`/`bookDepth` schemas
were unverified. They are now verified as above. The new fact is that `bookTicker` publication
ended on 2024-03-30.

### 7.1 L1-only data (if ever used locally)

A `bookTicker`-plus-trades replay, for local bring-your-own-data use only, has these limits
**[INFERENCE from the engine's queue models, [QUEUE_MODELS.md](../QUEUE_MODELS.md)]**:

- The queue model needs the displayed quantity at the order's price on every change. L1 gives it
  only while that price is the touch. An order one tick behind the touch has an unknown queue, and
  its position cannot be advanced by depth changes.
- Trades that sweep beyond L1 consume liquidity the book never showed. As section 6 shows, this
  is common on Binance Futures. Taker fills would see an empty book behind L1, and the 100-tick
  sweep would find nothing.
- Depth metrics (M3, M4 with N > 1, M10) are impossible.

### 7.2 Recommendation

**The hosted service uses synthetic data only.** Reasons:

- No openly licensed real L2 event dataset was found.
- Every exchange archive that has L2 forbids hosting or display.
- The only L3 sample, LOBSTER, carries no grant.

Real data stays a local workflow: the user downloads it, and it is never served. That is the
current Tardis workflow, and a Binance Vision or Bybit importer would follow the same pattern.

Two options, each requiring a decision outside this document:

1. **Ask LOBSTER for written permission** to serve the 2012 sample files. They are L3, so the
   queue model could be *validated* against true queue position, which is valuable for a research
   lab. Until permission is granted they are local-only.
2. **Calibrate the synthetic generator** to aggregate statistics (spread distribution, print-size
   distribution, sweep frequency, trade→depth lag). Tardis clause 9.3 allows Derived Data in any
   form if "aggregated over periods of at least 10 minutes". Whether calibration parameters
   derived from one 10-minute window qualify needs a legal reading. **[INFERENCE]**: treat as not
   yet permitted.

---

## 8. References

Papers (DOI metadata checked against Crossref on 2026-11-14; text read where noted):

- Andersen, T. G., Bollerslev, T., Diebold, F. X. and Labys, P. (2003). Modeling and forecasting
  realized volatility. *Econometrica* 71(2), 579–625. <https://doi.org/10.1111/1468-0262.00418>
- Avellaneda, M. and Stoikov, S. (2008). High-frequency trading in a limit order book.
  *Quantitative Finance* 8(3), 217–224. <https://doi.org/10.1080/14697680701381228>. Text read:
  <https://www.math.nyu.edu/~avellane/HighFrequencyTrading.pdf>.
- Bailey, D. H. and López de Prado, M. (2012). The Sharpe ratio efficient frontier. *Journal of
  Risk* 15(2), 3–44. <https://doi.org/10.21314/JOR.2012.255>
- Bailey, D. H. and López de Prado, M. (2014). The deflated Sharpe ratio: correcting for
  selection bias, backtest overfitting, and non-normality. *Journal of Portfolio Management*
  40(5), 94–107. <https://doi.org/10.3905/jpm.2014.40.5.094>
- Bandi, F. M. and Russell, J. R. (2008). Microstructure noise, realized variance, and optimal
  sampling. *Review of Economic Studies* 75(2), 339–369.
  <https://doi.org/10.1111/j.1467-937X.2008.00474.x>
- Cao, C., Hansch, O. and Wang, X. (2009). The information content of an open limit-order book.
  *Journal of Futures Markets* 29(1), 16–41. <https://doi.org/10.1002/fut.20334> (Crossref issue
  date 2008-11 online).
- Cartea, Á., Donnelly, R. and Jaimungal, S. (2018). Enhancing trading strategies with order book
  signals. *Applied Mathematical Finance* 25(1), 1–35.
  <https://doi.org/10.1080/1350486X.2018.1434009>
- Cont, R., Kukanov, A. and Stoikov, S. (2014). The price impact of order book events. *Journal
  of Financial Econometrics* 12(1), 47–88. <https://doi.org/10.1093/jjfinec/nbt003>. Text read:
  arXiv:1011.6402.
- Guéant, O. (2017). Optimal market making. *Applied Mathematical Finance* 24(2), 112–154.
  <https://doi.org/10.1080/1350486X.2017.1342552>. Text read: arXiv:1605.01862.
- Guéant, O., Lehalle, C.-A. and Fernandez-Tapia, J. (2013). Dealing with the inventory risk: a
  solution to the market making problem. *Mathematics and Financial Economics* 7(4), 477–507.
  <https://doi.org/10.1007/s11579-012-0087-0>. Text read: arXiv:1105.3115.
- Huang, R. D. and Stoll, H. R. (1996). Dealer versus auction markets: a paired comparison of
  execution costs on NASDAQ and the NYSE. *Journal of Financial Economics* 41(3), 313–357.
  <https://doi.org/10.1016/0304-405X(95)00867-E>. Not re-read; cited for the effective/realised
  spread construct.
- Kaplan, E. L. and Meier, P. (1958). Nonparametric estimation from incomplete observations.
  *Journal of the American Statistical Association* 53(282), 457–481.
  <https://doi.org/10.1080/01621459.1958.10501452>
- Lee, C. M. C. and Ready, M. J. (1991). Inferring trade direction from intraday data. *Journal
  of Finance* 46(2), 733–746. <https://doi.org/10.1111/j.1540-6261.1991.tb02683.x>
- Lehalle, C.-A. and Mounjid, O. (2017). Limit order strategic placement with adverse selection
  risk and the role of latency. *Market Microstructure and Liquidity* 3(1), 1750009.
  <https://doi.org/10.1142/S2382626617500095>
- Lo, A. W. (2002). The statistics of Sharpe ratios. *Financial Analysts Journal* 58(4), 36–52.
  <https://doi.org/10.2469/faj.v58.n4.2453>. Text read from a copy hosted at
  traders.studentorg.berkeley.edu; equations quoted by number.
- Newey, W. K. and West, K. D. (1987). A simple, positive semi-definite, heteroskedasticity and
  autocorrelation consistent covariance matrix. *Econometrica* 55(3), 703–708.
  <https://doi.org/10.2307/1913610>
- Perold, A. F. (1988). The implementation shortfall: paper versus reality. *Journal of Portfolio
  Management* 14(3), 4–9. <https://doi.org/10.3905/jpm.1988.409150>. Not read in full (paywalled);
  cited for the concept only.
- Stoikov, S. (2018). The micro-price: a high-frequency estimator of future prices. *Quantitative
  Finance* 18(12), 1959–1966. <https://doi.org/10.1080/14697688.2018.1489139>. Text read:
  preprint dated April 18, 2018 (SSRN 2970694); equation numbers refer to the preprint.
- Zhang, L., Mykland, P. A. and Aït-Sahalia, Y. (2005). A tale of two time scales: determining
  integrated volatility with noisy high-frequency data. *JASA* 100(472), 1394–1411.
  <https://doi.org/10.1198/016214505000000169>

Data sources and terms (read 2026-11-14): Binance Vision terms (URL above); Bybit v5 order-book
docs <https://bybit-exchange.github.io/docs/v5/websocket/public/orderbook>; Bybit T&C PDF
<https://www.bybit.com/common-static/compliance/legal/BYBIT/8016641686b360b415bf61cc24e22fbb.pdf>;
OKX historical-data T&C and API Agreement (via Wayback captures of
`okx.com/help/historicaldata-terms-and-conditions` and `okx.com/help/okx-api-agreement`); Coinbase
Market Data Terms <https://www.coinbase.com/legal/market_data>; LOBSTER
<https://php.lobsterdata.com/info/DataSamples.php>; FI-2010 metadata
<https://metax.fairdata.fi/rest/v2/datasets?preferred_identifier=urn:nbn:fi:csc-kata20170601153214969115>;
Tardis datasets API <https://api.tardis.dev/v1/exchanges/binance-futures> (channels include
`bookTicker`, `forceOrder`).

---

## 9. Methods and reproducibility

All analysis ran locally in `target/scratch-QuantResearch2/` (git-ignored), and that directory was
deleted afterwards. No vendor rows are reproduced here.

1. **BBO reconstruction (Python 3.11, numpy/pandas).**
   - Read `trades.csv.gz` and the first ~208k rows of `incremental_book_L2.csv.gz` (to 615 s
     past the first book row) from `data/raw/binance-futures/BTCUSDT/2020-02-01/`.
   - Prices are converted to integer ticks (0.01).
   - The book is replayed on the exchange clock, with trades before depth rows at equal
     timestamps and the snapshot clearing the book. Each trade is compared with the state after
     earlier batches.
   - This reproduced the quality report's 659/2,212 and 8 unchecked.
   - The same replay recorded:
     - the pre-trade passive-side level quantity and the depth in front of each trade price;
     - the post-batch BBO series used for the shift and window tests;
     - a separate replay ordered by `local_timestamp` (H3).
2. **Lag measurement.** For each checked trade, the time from its `timestamp` to the first
   depth row at the same price on the passive side with timestamp ≥ the trade's, capped at 1 s.
3. **Independent BBO.** I streamed the first rows of the Tardis free samples for the same day,
   `book_ticker` (Binance `bookTicker`, whose `timestamp` equals `local_timestamp` in every row)
   and `liquidations`, from `datasets.tardis.dev` with no API key, under the same terms as the
   existing sample. They were used locally and deleted. `bookTicker` updates were compared with
   the local-clock reconstruction at ±10/50/100 ms. Why the `book_ticker` file is independent of
   the depth file: its first row predates the first depth row (the REST snapshot) by about 1.2 s,
   and its timestamps are receipt times, not depth timestamps. Tardis's `quotes` file, by
   contrast, starts exactly at the snapshot's timestamp and was not used. That `book_ticker` is the
   raw `bookTicker` channel is an **[INFERENCE]** from the channel list in Tardis's exchange API
   and these properties; the Tardis data-type page was not read for it.
4. **Runs.**
   - I ran `target/release/blackbox.exe --data-dir <scratch> run --preset {maker_baseline,
     avellaneda_stoikov, imbalance_taker} --dataset {synthetic-demo,
     binance-futures-btcusdt-2020-02-01-10m}`. The synthetic dataset came from `blackbox synth`;
     the Tardis dataset was a local copy.
   - The `maker_baseline` Tardis run reproduces the documented spread capture of −6.298 ticks
     per unit (PNL_ACCOUNTING.md), which confirms that my timestamp alignment matches the code's.
   - Equity increments, sample ACFs, Lo's η(q) and the Bartlett-truncated long-run variance factor
     (lag 100) were computed from `run.json`.
5. **Fixtures.** Every expected value in sections 2–5 was recomputed by a short script (numpy,
   including the micro-price chain by matrix power and the Kaplan–Meier product).
6. **Papers.** Text was extracted from the PDFs with PyMuPDF; equations above are transcribed from
   that text.
7. **Data-source survey.** One sample file per source was downloaded and inspected (header, row
   count, time span, sampling interval). Terms were read at the URLs listed in section 8.
