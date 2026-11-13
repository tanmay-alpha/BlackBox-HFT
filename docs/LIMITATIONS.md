# What BlackBox HFT does and does not claim

BlackBox is a research and simulation tool. It cannot connect to an exchange account, holds no
trading credentials and cannot submit an order: the engine's `live` feature is not compiled in.

Every number it shows comes from a simulation over historical data under stated assumptions. This
page lists those assumptions and the things the simulation cannot know.

## Historical replay is not a reactive market

- **No market impact.** Simulated orders never change the replayed prices, trades or displayed
  quantity. The market does what it did historically whatever the strategy does.
- **Taking does not consume depth.** A marketable order executes against the displayed book of
  that instant, and the book is unchanged afterwards. Results for taking orders that are large
  relative to displayed size are optimistic.
- **No reaction to resting orders.** Other participants do not see, join, fade or pick off a
  simulated quote.
- A reactive, agent-based simulation (for example ABIDES) is a different kind of model. BlackBox
  does not include one and never mixes such results with replay results.

## Level-2 data limits what can be known

- The datasets are Market-By-Price: quantity per price level plus trades. Individual orders are
  not visible, so **queue position is never observed**. It is estimated by the selected queue
  model and labeled "modeled" everywhere.
- A new order is assumed to join behind everything displayed at its price.
- When displayed quantity at a level falls without a trade, the data does not say whether the
  cancelled orders were ahead of or behind a simulated order. Each queue model makes a different
  assumption; the difference between them is a measure of that uncertainty, not noise.
- Hidden and iceberg liquidity, self-trade prevention, auctions and exchange-specific matching
  rules are not modeled.
- Whether a real order would have filled cannot be known from replay. "Why did this order fill?"
  is answered as "why the model filled it", with the evidence.

## Fill models

- `NoPartialFill`: once a resting order's fill condition is met the whole order fills, whatever
  quantity actually traded. Optimistic about size.
- `PartialFill`: a resting order fills by the traded quantity that exceeds its estimated queue.
  Each of the strategy's orders at a price is credited with the full trade; liquidity is not
  shared between them.
- Both: an order is filled with certainty when the market trades through its price or the
  opposite best price crosses it.

## Latency

- **Feed latency** is `local_timestamp - exchange_timestamp` from the dataset. For recorded data
  it was measured at the *vendor's* capture host, not where a user's strategy would run; for
  synthetic data it is generated; if a source lacks receipt timestamps it is an assumption. The
  dataset page states which. For Tardis Binance Futures data, Tardis's open-source normalizer
  stamps trades with the transaction time and depth updates with the later message event time
  (that the CSV export does the same is inferred, not documented). Depth feed latency may
  therefore be understated relative to matching-engine time, and trades and depth are stamped
  from different exchange fields.
  Measured on the Tardis sample (Binance Futures BTCUSDT, 2020-02-01, first 10 minutes): the
  quality report found 659 of 2,212 trades printing more than one tick outside the reconstructed
  best bid and ask at their exchange timestamp, which is what trades stamped earlier than the
  book updates they belong to would produce. Feed latency in that file is 116 ms at minimum and
  129 ms at the median (the vendor captured from London that year), with a p99 of 911 ms. Results
  on that data inherit both effects; the dataset page shows them.
- **Order latency** (entry and response) is a model: a constant, or a constant plus seeded
  jitter. It is not calibrated to any venue.
- Requests and responses never overtake one another.
- If the source contained events whose local time preceded their exchange time (clock skew), all
  local timestamps were shifted by a constant; the dataset manifest records the shift.

## Accounting

- Linear contracts only. Fees are a fraction of traded value with separate maker and taker rates.
  Funding payments, margin, liquidation and borrowing costs are not modeled.
- P&L uses average-cost accounting and is marked at the locally known mid price. Marking at mid
  ignores the cost of actually closing the position.
- The engine's own local account under-counts under `PartialFill`; BlackBox reports its own
  ledger and shows the engine's figures only as a cross-check (docs/EXECUTION_MODEL.md).

## Strategies and risk

- One instrument per run. No order modification: quotes are replaced by cancel and new order,
  which loses queue position, as it would on most venues.
- Market orders are not supported: the pinned engine mishandles them (docs/EXECUTION_MODEL.md,
  defect 5), so the harness blocks them. A marketable IOC limit order is the equivalent.
- Strategies are evaluated on a fixed time grid, not on every market event. A shorter interval
  reacts faster and costs more compute.
- The bundled strategies are deliberately simple and exist to exercise the execution model. They
  are not trading advice and their simulated P&L is not a forecast.
- The drawdown rule is checked on the strategy grid using the locally known position and mid.

## Comparisons

- Two scenarios are aligned by decision point (strategy tick and slot). The first point at which
  they differ is identified exactly. After it, the strategy's own state differs, so later
  differences are knock-on effects.
- A difference in P&L between two scenarios is **not attributed** to a single parameter unless
  that is arithmetically certain (for example, identical fills under different fee rates). When
  several parameters differ at once their contributions are not separated.
- A sensitivity grid shows how the model's output moves with its assumptions. It is not a
  prediction of live performance. Every cell is one completed simulation with its own run id.

## Determinism

- The same dataset hash, engine version, configuration and seed give the same result hash on the
  same binary and platform. The seed only affects jittered latency; nothing else is random.
- The engine iterates hash maps internally; BlackBox canonicalizes the order of simultaneous
  events so this does not leak into results (docs/EXECUTION_MODEL.md).
- Bit-identical results across different platforms are expected but not guaranteed for strategies
  that use transcendental functions (docs/BUILDING.md).

## Data

- Synthetic datasets are generated by BlackBox and are labeled as such. They are for tests and
  demonstrations of the mechanics and say nothing about any real market.
- Recorded vendor data is not included and must not be redistributed. A recorded dataset window
  always starts at the beginning of the source file, because the book has to be built from its
  initial snapshot.
- The ingestion pipeline reports duplicates, gaps, crossed books and precision violations. It
  does not repair them: nothing is interpolated and no missing book event is invented. Exact
  duplicate trades are dropped and counted.
