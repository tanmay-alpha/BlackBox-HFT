# Queue models

A resting limit order fills by price-time priority: at its price it waits behind every order that
arrived earlier. Level-2 (Market-By-Price) data shows only the total quantity at each price, so how
much of that quantity is *ahead* of a simulated order is never observed. hftbacktest estimates it
with a queue model. This page gives every model BlackBox exposes with its exact formula, read from
the hftbacktest 0.9.4 source (`src/backtest/models/queue.rs`), and shows what the estimate can and
cannot mean. Fill conditions that do not depend on the queue (trade-through, opposite best crossing)
are in [EXECUTION_MODEL.md](EXECUTION_MODEL.md#engine-behaviours-that-are-assumptions-not-defects).

## The state and its lifecycle

Each resting order carries one number, `front`: the modeled quantity ahead of it. The probability
models also carry `cum_trade`, traded quantity at the price since the last depth update.

| Engine call | When the exchange model makes it | Effect on `front` (every model) |
|---|---|---|
| `new_order` | the order is accepted to rest | `front = displayed quantity at the order's price`: the order joins behind everything displayed |
| `trade(qty)` | a market trade prints at the order's price | `front -= qty`; probability models also `cum_trade += qty` |
| `depth(prev, new)` | the displayed quantity at the order's price changes | model-specific, below |
| `is_filled` | after each `trade` | `exec = round(-front / lot)`; if `exec > 0`: `front = 0` and `exec` lots are executable |

So an order becomes fillable by the queue path once trades have pushed `front` to at most minus half
a lot. `NoPartialFillExchange` then fills the whole order; `PartialFillExchange` fills
`min(exec, leaves)` and the order, if it survives, stays at `front = 0` (see
[fill_reset](#fill_reset)).

Every one of our orders at a price is credited with the full traded quantity: liquidity is not
shared between our own orders. Our orders are not part of the displayed quantity either, because
replay has no market impact.

## Depth changes: the six models

`BlackBox config` is the `execution.queue_model` value (`crates/bb-core/src/config.rs`); the engine
type is what `crates/bb-sim/src/engine.rs` constructs.

### RiskAdverse (`RiskAdverseQueueModel`)

```text
front = min(front, new)
```

A decrease in displayed quantity is never credited as having happened ahead; `front` only falls
when the level shrinks below it. Trades are the only thing that advances the order. Most
conservative.

### Probability models (`ProbQueueModel<P>`)

For a depth change from `prev` to `new`:

```text
chg       = (prev - new) - cum_trade          # remove volume already counted from trades
cum_trade = 0
if chg < 0:                                   # level grew: newcomers join behind
    front = min(front, new)
else:
    back  = prev - front                      # modeled quantity behind the order
    p     = P.prob(front, back)               # share of the decrease assumed BEHIND (inf -> 1)
    est   = front - (1 - p) * chg + min(back - p * chg, 0)
    front = min(est, new)
```

`1 - p` of the cancelled quantity is assumed to have been ahead. The `min(back - p*chg, 0)` term
moves any part of the "behind" share that exceeds what is actually behind to the front. The final
`min(…, new)` keeps the estimate within the displayed level.

| BlackBox config | Probability function `P` | `p = prob(front, back)` |
|---|---|---|
| `log_prob` | `LogProbQueueFunc` | `f(back) / (f(back) + f(front))`, `f(x) = ln(1 + x)` |
| `log_prob2` | `LogProbQueueFunc2` | `f(back) / f(back + front)`, `f(x) = ln(1 + x)` |
| `power_prob { n }` | `PowerProbQueueFunc` | `f(back) / (f(back) + f(front))`, `f(x) = x^n` |
| `power_prob2 { n }` | `PowerProbQueueFunc2` | `f(back) / f(back + front)`, `f(x) = x^n` |
| `power_prob3 { n }` | `PowerProbQueueFunc3` | `1 - (front / (front + back))^n` |

hftbacktest's documentation states the intended shape: the probability at the head of the queue
should be 0 (all decreases happen behind you) and at the tail 1 (all happen ahead), citing Rigtorp
(2013) and Quantitative Finance Stack Exchange question 3782
([OPEN_SOURCE_RESEARCH.md](OPEN_SOURCE_RESEARCH.md#papers)). The presets use `power_prob3 { n: 3 }`
(`crates/bb-app/src/presets.rs`).

## Worked example

From `crates/bb-sim/tests/engine_adapter.rs::queue_models_differ_on_cancellations_ahead` (market
`Mkt::standard()` in `crates/bb-sim/tests/common/mod.rs`):

1. The bid at 100.0 shows 5. A buy for 1 at 100.0 arrives and rests: `front = 5`.
2. The level grows 5 → 8 (three join behind). `chg = -3 < 0`, so `front = min(5, 8) = 5`.
3. The level falls 8 → 4 with no trade: four were cancelled somewhere in the queue.
   `chg = 4`, `back = 8 - 5 = 3`.

The test asserts two values: **RiskAdverse → 4** (`min(5, 4)`), and **PowerProb3 with n = 1 → 2.5**
(`p = 1 - 5/8 = 3/8`, so `4 × 5/8 = 2.5` assumed ahead; `est = 5 - 2.5 + min(3 - 1.5, 0) = 2.5`).

The same arithmetic for the other models (computed from the formulas above; not pinned by a test):

| Model | `p` (behind) | Assumed ahead of the 4 cancelled | `front` after |
|---|---:|---:|---:|
| RiskAdverse | n/a | 1 (only the cap applies) | 4 |
| LogProb | 0.4362 | 2.255 | 2.745 |
| LogProb2 | 0.6309 | 1.476 | 3.524 |
| PowerProb n = 1 (= PowerProb2 n = 1 = PowerProb3 n = 1) | 0.375 | 2.5 | 2.5 |
| PowerProb n = 3 | 0.1776 | 3.289 | 1.711 |
| PowerProb2 n = 3 | 0.0527 | 3.789 | 1.211 |
| PowerProb3 n = 3 (preset default) | 0.7559 | 1 | 4 |

The last row shows the clamp at work: `back - p·chg = 3 - 3.02 < 0`, so
`est = front + back - chg = new = 4`. The same market event leaves the order anywhere from 1.2 to 4
lots from the front depending on an assumption the data cannot check. That spread is the point:
**the difference between models measures how much a result depends on unobservable queue
position; it is not noise and it does not say which model is right.**

## Trades and depth at the same timestamp

A trade at the order's price normally arrives with a depth update shrinking the level by the same
amount. The probability models subtract `cum_trade` from the next depth change so the volume is not
counted twice. That only works if the trade row comes first; `bb-data` guarantees trades precede
the depth update they caused ([MARKET_DATA.md](MARKET_DATA.md#normalization)).

## fill_reset

Inside `is_filled`, both engine models set `front = 0` whenever any quantity is executable. Under
`PartialFillExchange` an order can survive such a fill, and its remainder is then modeled at the
head of the queue. The recorder makes this visible as a `queue_update` with cause `fill_reset`,
placed directly after the `exch_fill` it belongs to, so the trade that exhausted the queue stays
directly before the fill on the tape (`crates/bb-sim/src/taps.rs`, `TapQueue::is_filled`; test
`partial_fill_records_the_queue_reset_and_rejected_cancel_has_no_execution`). Under
`NoPartialFillExchange` the order is always completely filled, so no reset is recorded. A
`fill_reset` is model bookkeeping, not market activity, and the explanations describe it that way.

## What L2 data cannot reveal

- **Where cancellations happened.** A level shrinking without a trade is consistent with any split
  between ahead and behind. Each model picks one split; none is observed.
- **Where additions went.** Every model assumes growth joins behind. Venues where some orders can
  gain priority (for example by size or by modification rules) are not modeled.
- **Hidden and iceberg liquidity, self-trade prevention, auctions.** Not in the data, not modeled.
- **Whether the displayed quantity included orders that would have left because of ours.** Replay
  has no reaction to our orders ([LIMITATIONS.md](LIMITATIONS.md)).
- **Exact position, even at entry.** Joining "behind everything displayed" is itself an
  assumption: by the time a real order arrived the queue could have changed between feed messages.

hftbacktest also has an L3 (Market-By-Order) queue model and exchange that track individual orders
FIFO. BlackBox does not use them because none of its data sources is L3.

## How BlackBox records and uses the estimate

- `exch_queued.queue_ahead` is `front` at acceptance; `queue_update` records `ahead_before`,
  `ahead_after` and the cause (`trade`, `depth`, `fill_reset`). The value is read from the engine's
  own state without re-implementing the model; if it cannot be read the tape says `null` instead of
  guessing ([EXECUTION_MODEL.md](EXECUTION_MODEL.md#how-the-flight-recorder-observes-the-engine)).
- Depth-driven updates beyond `record.max_queue_updates_per_order` (default 2,000) are kept only if
  they move the estimate by at least one lot; the rest are counted in one explicit
  `queue_truncated` marker. Trade-driven updates are always kept.
- Every queue number in the UI and in explanations is labeled **modeled**.
- To measure the dependence of a result on this assumption: the `maker_conservative_queue` preset
  (RiskAdverse versus the PowerProb3 baseline), the `queue_position` demo (RiskAdverse, LogProb,
  PowerProb3 n = 3 on identical everything else; `crates/bb-app/src/demos.rs`), or a sensitivity
  grid with the queue model on one axis.
