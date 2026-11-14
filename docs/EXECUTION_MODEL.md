# Execution model: hftbacktest 0.9.4 inside BlackBox

BlackBox uses the Rust crate `hftbacktest` pinned at exactly `=0.9.4` (crates.io, MIT, published
2025-12-10) with only the `backtest` feature. The `live` feature (exchange connectivity) is
disabled, so the binary contains no code that can reach an exchange.

Everything below was established by reading the 0.9.4 source and is pinned by a test where stated.
Statements about upstream `master` refer to commit `5f3ec40b2a` (2025-12-23).

This page is the single home for engine internals: what is reused, how the flight recorder
observes the engine, the engine's defects and what BlackBox does about them. The models
themselves have their own pages: [QUEUE_MODELS.md](QUEUE_MODELS.md) (queue-position formulas),
[LATENCY_MODEL.md](LATENCY_MODEL.md) (feed, entry and response latency, jitter, order bus) and
[PNL_ACCOUNTING.md](PNL_ACCOUNTING.md) (ledger and engine cross-check). How the pieces fit
together is in [ARCHITECTURE.md](ARCHITECTURE.md).

## What BlackBox uses from the engine, unmodified

| Concern | Engine item |
|---|---|
| Event loop, clock, order bus | `Backtest`, `order_bus`, `Local` |
| Matching | `NoPartialFillExchange`, `PartialFillExchange` |
| Queue position | `RiskAdverseQueueModel`, `ProbQueueModel` with `LogProbQueueFunc`, `LogProbQueueFunc2`, `PowerProbQueueFunc`, `PowerProbQueueFunc2`, `PowerProbQueueFunc3` |
| Order latency | `ConstantLatency` (the jitter model is BlackBox's own, see below) |
| Feed latency | the dataset's own `local_ts - exch_ts`, plus `FeedLatencyAdjustment` for a constant offset |
| Fees, accounting arithmetic | `TradingValueFeeModel<CommonFees>`, `LinearAsset` |
| Book | `HashMapMarketDepth` |

No engine source is copied or patched. The adapter (`crates/bb-sim`) assembles the same objects
the engine's `L2AssetBuilder` would, by hand, so that recording wrappers can be placed around them.

## How the flight recorder observes the engine

The engine has no event hooks. It does call four public traits at exactly the moments that matter,
so each engine model is wrapped in a type that implements the same trait, delegates the decision
to the real model, and reports what happened.

| Recorded event | Wrapper | Engine call site |
|---|---|---|
| request sent, entry latency | `LatencyModel::entry` | `LocalToExch::request` |
| order accepted to rest, initial queue estimate | `QueueModel::new_order` | exchange `ack_new` |
| queue estimate moved by a trade / depth change | `QueueModel::trade`, `QueueModel::depth` | exchange `process` |
| every execution | `FeeModel::amount` on the exchange-side `State` | exchange `fill` → `State::apply_fill` |
| response emitted, response latency | `LatencyModel::response` | `ExchToLocal::respond` |
| response received locally | `Local::process_recv_order_` with its handler enabled | `Local` |

**Reading the queue estimate.** `ProbQueueModel` keeps its per-order state in `QueuePos`, whose
fields are private. Re-implementing the model to expose them would mean BlackBox's arithmetic, not
the engine's, decides fills. Instead the wrapper reads the value from `QueuePos`'s derived `Debug`
output, which for `f64` is the shortest string that round-trips exactly, so the number is recovered
bit for bit. A unit test pins the format against the real model; if it ever changes the probe
returns "unreadable" and the tape records `queue_ahead: null`. It never guesses.

**What cannot be observed.** There is no hook for "the exchange considered this order and did
nothing". The absence of a queue update means no trade or depth change occurred at the order's
price.

## Defects in 0.9.4 and what BlackBox does about each

Each is pinned by a test in `crates/bb-sim/tests/engine_defects.rs` that drives the unwrapped
engine. If an engine upgrade fixes one, that test fails and the workaround should be removed.

### 1. Simultaneous fills are emitted in a non-reproducible order

Both exchange models keep the strategy's orders in a `HashMap` and iterate it when a trade or a
best-price move may fill several orders. `HashMap` iteration order is randomized per instance, so
two runs of the same configuration emit those fills, and their responses, in different orders.
Each order's own outcome is unaffected (every order is evaluated independently against the same
market event), but the tape sequence, the running position and any sequential random draw are.

Observed with the ordering fix disabled: eight simultaneous fills came out as
`[4, 3, 7, 6, 1, 5, 0, 2]` and two identical runs produced different result hashes.

**Handling.** The recorder buffers everything emitted inside one engine processing step and
commits it sorted by order id, preserving each order's internal sequence; ledger effects are
applied at commit. Latency jitter is a pure function of `(seed, order id, time)` rather than a
sequential stream, and response jitter depends only on the exchange timestamp, so the order bus's
"never overtake an earlier message" rule cannot reintroduce order dependence.
`tests/determinism.rs` repeats a sixteen-order scenario 40 times per configuration and requires an
identical hash every time.

The engine's own local account still sums fills in arrival order, so its floating-point totals can
differ in the last bit between runs. Those totals appear only in the informational engine
cross-check and are excluded from the result hash.

### 2. `PartialFillExchange` aborts after an exact completion

When a trade completes a resting order with exactly its remaining quantity, the exchange marks it
`Filled` but removes it from its book only if the executable quantity was strictly greater than
the remainder. The order stays in the book, and the next trade at its price fails with
`InvalidOrderStatus`, ending the backtest.

**Handling.** The queue wrapper's `is_filled` reports "remaining plus one lot" whenever the
engine's own model says the whole remainder is executable. That routes the exchange through its
removal branch. The executed quantity does not change, because the exchange caps it at the
remainder; tests cover overshoot, shortfall and exact completion. `NoPartialFillExchange` only
tests the value for being positive and is unaffected.

### 3. The local account under-counts partial fills

`Local` applies a response to its account only when the status is `Filled`, using that response's
`exec_qty`. Under `PartialFillExchange` a partially filled response is never applied, and a taker
order that sweeps several levels produces one response carrying only the last level's quantity.
Example from the tests: an order filled 1 then 2 leaves the engine reporting position 2; an 8-lot
sweep leaves it at 3. Still present on upstream `master`.

**Handling.** BlackBox never reports the engine's account. Its own ledger is fed by the
exchange-side execution hook, which sees every fill. The engine's figures are shown next to the
ledger's in each run's "engine cross-check": they must agree under `NoPartialFillExchange` (except
for the double counts of defect 6, which the check identifies) and are expected to disagree under
`PartialFillExchange`, with the reason stated.

The strategy's *known* position (what it may act on) is the ledger restricted to fills whose
response has reached the local side.

### 4. `Bot::submit_order` ignores the requested side

In 0.9.4 `submit_order(OrderRequest)` always submits a sell. Fixed on upstream `master` after the
release (#298).

**Handling.** The adapter calls only `submit_buy_order` and `submit_sell_order`.

### 5. Market orders

Both exchange models mishandle `OrdType::Market`:

- **No price check on an empty side.** `NoPartialFillExchange::ack_new` fills a market buy at
  `self.depth.best_ask_tick()` (`nopartialfillexchange.rs`, line 372) without checking that an
  ask exists. On an empty ask side that is the sentinel `INVALID_MAX = i64::MAX`, an absurd
  execution price; sells are symmetric with `INVALID_MIN`.
- **A 100-tick sweep window.** `PartialFillExchange` sweeps `best_ask..best_ask + 100` for a
  market buy and `(best_bid - 100..=best_bid).rev()` for a market sell (`partialfillexchange.rs`,
  lines 504 and 634, each under the engine's own comment `// todo: set the proper upper/lower
  bound`). On an empty side the bound arithmetic on the sentinel overflows `i64`: a panic in a
  debug build, a silently empty range (the order expires) in a release build, so the behaviour
  depends on the build profile. With liquidity present the sweep never reaches levels more than
  100 ticks from the touch. On BTCUSDT with tick 0.01 that is $1, while the p99 spread of the
  Tardis sample is 225 ticks ([MARKET_DATA.md](MARKET_DATA.md#measured-findings-on-the-real-sample)).

Found by the property-based tests (shrunk case: `PartialFill`, zero latency, a sell market order
on an ask-only book).

**Handling.** BlackBox rejects every market order before it reaches the engine: the tape records
`risk_blocked` with rule `invalid_order` and a detail explaining why. `RunConfig::validate`
rejects scripted market orders. A marketable IOC limit order (priced at or through the opposite
touch) is the supported equivalent; the bundled imbalance taker uses exactly that.

Pinned by `upstream_market_buy_on_empty_ask_fills_at_the_sentinel_tick`,
`upstream_partial_fill_market_order_scans_only_100_ticks` and
`adapter_blocks_market_orders_before_the_engine` in `crates/bb-sim/tests/engine_defects.rs`.

### 6. A rejected cancel of a filled order books the fill twice

`Local::cancel` (`proc/local.rs`, line 216) only checks that no request is pending, so it accepts a
cancel for an order the local side already holds as `Filled`, and it sends a clone of that local
order. The exchange no longer holds the order, so `NoPartialFillExchange::ack_cancel`
(`nopartialfillexchange.rs`, line 436) answers by echoing the request back with `req = Rejected`,
still carrying the clone's `status = Filled`, `exec_qty` and `exec_price_tick`.
`Local::process_recv_order_` (`local.rs`, line 102) calls `state.apply_fill` for every received
order whose status is `Filled` before it looks at `req`, so the echo books the execution again:
position, balance, fee and `num_trades` all count it twice. Upstream `master` was not checked.

Found twice, independently: by the property-based tests (`instrumentation_matches_the_unwrapped_engine`,
regression `cc a2fd814d…` in `crates/bb-sim/tests/properties.proptest-regressions`: zero response
latency, 10.001 ms entry latency, a resting buy filled while a first cancel is in flight, then a
second cancel of the now-filled order; the raw engine reports position 0.2 against one 0.1
execution), and when strategy think time was added
([LATENCY_MODEL.md](LATENCY_MODEL.md#think-time-decision-latency)): a cancel decided before a fill
notice arrives is sent after it, as decided.

**How BlackBox reaches it.** Only when the host sends a cancel for an order it already knows is
filled *and* the engine still holds the order (clearing is deferred while an earlier cancel of it
is in flight, or the order is not cleared until the next tick). Without think time the bundled
strategies never do this: they cancel only `cancellable()` orders (new or partially filled, no
request pending). With think time they can, because the order was cancellable at the decision and
the fill notice arrived during the think time; a custom script can at any time.

**Handling.** BlackBox's results are unaffected: its ledger is fed by the exchange-side fill hook,
which fires once per execution, and the echo is recorded faithfully on the tape as a rejected
`local_response` with status `filled` and `exec_qty` 0. The receive wrapper
(`RecLocal::process_recv_order`) counts the executions the engine's account applies beyond the fill
reports it delivered; when that count reconciles the difference exactly, the engine cross-check
reads "Expected: The engine's local account also applied N fill(s) a second time…" instead of the
"unexpected" note. The property test requires the raw engine's account to equal the ledger plus
exactly one duplicate per such echo.

Pinned by `upstream_rejected_cancel_of_a_filled_order_double_counts_the_fill` and
`adapter_ledger_counts_the_fill_once_when_a_filled_order_is_cancelled` in
`crates/bb-sim/tests/engine_defects.rs`; the think-time trigger by `think_time_applies_to_cancels`
in `crates/bb-sim/tests/latency_lab.rs`.

## Recorder pitfalls found in review

An independent review of the adapter against the engine source found four ways the first recorder
could misreport. Each is fixed and has a regression test in `crates/bb-sim/tests/engine_adapter.rs`.

- **A late cancel rejection could vanish.** `Local` reports a response only for orders it still
  holds, and clearing forgets terminal orders. A fill response also overwrites the local order's
  `req` field, so the engine itself cannot tell a cancel is still in flight. The tape now counts
  unanswered cancels per order, and clearing is deferred while a terminal order has one.
- **A rejected cancel carried a stale execution.** The engine leaves the previous execution
  fields on the rejected request; the recorder now reports zero execution on any rejection. The
  status on a rejection is the request's copy of the order (as known when it was sent) and is not
  used as the order's state.
- **Risk limits had half a lot of headroom.** They are now hard limits with only floating-point
  tolerance.
- **The queue reset after a partial fill was invisible.** Both engine queue models reset a
  surviving order's estimate to zero inside `is_filled`. The tape now records this as a
  `queue_update` with cause `fill_reset`, placed after the fill so the exhausting trade stays
  directly before it.

## Engine behaviours that are assumptions, not defects

These are what the engine models; the UI labels them as modeled.

- **No market impact.** Simulated orders never change replayed prices or displayed quantity. A
  taking order executes against displayed depth without consuming it, so a second taker an instant
  later sees the same depth.
- **Queue position is estimated.** On Market-By-Price data individual orders are invisible. A new
  order is assumed to join behind everything displayed at its price. Trades at the price reduce
  the estimate one for one. For a decrease in displayed quantity with no trade, the configured
  model decides what share was ahead. Formulas and a worked example:
  [QUEUE_MODELS.md](QUEUE_MODELS.md).
- **Fill conditions (resting orders).** Filled when the market trades through the price, when the
  opposite best price crosses it, or when trades at the price exhaust the estimated queue ahead.
  `NoPartialFillExchange` then fills the whole order regardless of the traded quantity.
- **Each of our orders sees the full trade.** When a trade prints at a price where several of our
  orders rest, each order's queue is advanced by the full traded quantity. Liquidity is not shared
  between our own orders.
- **Messages never overtake.** The order bus delivers requests and responses in send order, even
  when a later message was assigned a shorter latency.
- **Trade before depth.** At equal exchange timestamps the dataset must present a trade before the
  depth update it caused, otherwise the probability models subtract the traded quantity twice. The
  data pipeline enforces this ordering.

## BlackBox additions that are not the engine's

- **Jitter latency.** `OrderLatencyConfig::Jitter` is a BlackBox model implementing the engine's
  `LatencyModel` trait: base latency plus a seeded uniform draw. It is labeled as such; why it is a
  pure function rather than a random stream is explained in [LATENCY_MODEL.md](LATENCY_MODEL.md).
- **Strategy tick grid.** Strategies are evaluated every `interval_ns` of local time starting from
  the dataset's first timestamp. The grid does not depend on execution assumptions, which is what
  lets an order id (`tick << 8 | slot`) name the same decision point in two scenarios.
- **End of run.** A run ends at the first tick after the last feed event's local receipt time.
  Requests still in flight stay unresolved on the tape.
- **Pre-trade risk rules and the drawdown halt** are enforced by the harness before a request
  reaches the engine.
