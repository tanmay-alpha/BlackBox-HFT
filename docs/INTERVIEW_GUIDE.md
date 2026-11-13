# Interview guide

The concepts behind BlackBox HFT explained from scratch, each with a pointer to where the project
treats it precisely, followed by a five-minute demo script. Nothing here is investment advice;
everything BlackBox shows is simulated on historical replay.

## Concepts

### Limit order book

An exchange keeps, for each instrument, the resting **limit orders**: offers to buy (bids) or sell
(asks) a quantity at a stated price or better. The highest bid and the lowest ask are the **best
bid** and **best ask** (the "touch"); the gap between them is the **spread**; their average is the
**mid**. A **market order**, or a limit order priced through the opposite touch, **takes
liquidity**: it trades immediately against resting orders. A limit order that does not cross
**provides liquidity**: it rests until a taker arrives or it is cancelled. Prices move in **ticks**
(BTCUSDT futures: 0.01) and quantities in **lots** (0.001 BTC).

### L2 versus L3 data

- **L3 / Market-By-Order** shows every individual order: who is first in line is visible.
- **L2 / Market-By-Price** shows only the total quantity at each price, plus trades.

Most crypto venues publish L2. BlackBox's data is L2 with trades
([MARKET_DATA.md](MARKET_DATA.md)), so anything about individual queue position is an estimate.

### FIFO queue position

At one price, most venues fill resting orders first in, first out. A new order joins the back. It
fills only once everything ahead of it has traded or been cancelled. On L2 data you see the level
shrink but not whether the cancelled orders were ahead of yours or behind it. A **queue model**
guesses; BlackBox exposes six and shows how much results depend on the choice
([QUEUE_MODELS.md](QUEUE_MODELS.md)).

### Market making

A market maker quotes both a bid and an ask and earns the spread when both get filled, paying (or
receiving, with rebates) a fee each time. Its risks are **inventory** (holding a position while the
price moves) and **adverse selection** (below). The touch maker and the Avellaneda-Stoikov strategy
in BlackBox are market makers ([STRATEGIES.md](STRATEGIES.md)).

### Latency arbitrage and adverse selection

Your resting quote is a free option to everyone faster than you. When the price is about to move
up, fast participants buy from your stale ask before you can cancel it; you are filled exactly when
being filled is bad. That is **adverse selection**, and **latency arbitrage** is profiting from
being faster than the quote you trade against. BlackBox measures its footprint with **markouts**:
the mid price some time after each fill relative to the fill price; negative markouts mean the
market moved against you after you traded ([PNL_ACCOUNTING.md](PNL_ACCOUNTING.md#markouts)).
BlackBox cannot model other participants reacting to your quotes; replay is fixed history
([LIMITATIONS.md](LIMITATIONS.md)).

### Feed latency versus order latency

- **Feed latency**: exchange event → your machine sees it. Your view of the book is always old.
- **Order entry latency**: your machine sends → the matching engine processes.
- **Order response latency**: the matching engine responds → your machine learns the outcome,
  including fills.

They act at different times on different clocks: the exchange clock and your local clock. BlackBox
records every event with its clock and keeps all three latencies separate
([LATENCY_MODEL.md](LATENCY_MODEL.md)).

### Why HFT backtests mislead

- **No market impact.** Replay cannot react: your orders do not move prices or remove liquidity,
  and nobody fades your quotes.
- **Queue position is invented** on L2 data, and fill rates for passive orders depend heavily on it.
- **Latency is assumed**, and the recorded feed latency was measured where the data vendor sat, not
  where you would.
- **Data artifacts** (timestamp conventions, gaps, missing updates) look like opportunities. The
  real sample in BlackBox has 659 of 2,212 trades printing more than a tick outside the
  reconstructed book ([MARKET_DATA.md](MARKET_DATA.md#measured-findings-on-the-real-sample)).
- **Engine bugs.** The pinned engine has five defects that would silently corrupt results if
  unhandled ([EXECUTION_MODEL.md](EXECUTION_MODEL.md#defects-in-094-and-what-blackbox-does-about-each)).

BlackBox's answer is not a better forecast but transparency: every assumption is a parameter, every
fill has an explanation, and two scenarios can be compared decision by decision.

### Spread capture versus net P&L

**Spread capture** is how far inside the mid a passive fill was at the moment it happened: what the
quote "earned" on paper. **Net P&L** is what the books say after inventory moved and fees were
paid. A maker can capture spread on every fill and still lose money to adverse selection and fees.
BlackBox reports both, and markouts in between, without claiming one explains the other
([PNL_ACCOUNTING.md](PNL_ACCOUNTING.md#spread-capture-versus-net-pl)).

### Inventory risk

Holding a position exposes you to price moves. Market makers lean their quotes against inventory
(the touch maker's skew; Avellaneda-Stoikov's reservation price `r = s − q·γ·σ²·(T − t)`) and cap it
with position limits enforced before any order reaches the engine
([STRATEGIES.md](STRATEGIES.md)).

### Model uncertainty

When the data cannot decide between assumptions, the honest output is a range. BlackBox runs the
same strategy under different queue models, latencies or fees, aligns the decisions, finds the first
divergence, and attributes a P&L difference to one cause only when arithmetic proves it (for
example: identical fills, different fees). Differences between queue models measure dependence on
an unobservable assumption, not which model is right
([QUEUE_MODELS.md](QUEUE_MODELS.md#worked-example), [PNL_ACCOUNTING.md](PNL_ACCOUNTING.md#attribution-between-scenarios)).

### Rust for performance-sensitive work

The engine is Rust; BlackBox's recorder, ledger and forensics run in the same process with no
garbage collector and no serialisation boundary. The cost of recording is measured, not assumed:
on the 10-minute BTCUSDT sample the stock engine replays 9.39 M rows/s and the adapter with an idle
strategy 8.53 M rows/s ([PERFORMANCE.md](PERFORMANCE.md)). Why Rust instead of the brief's
Python/FastAPI is in [ARCHITECTURE.md](ARCHITECTURE.md#why-rust-and-axum-rather-than-python-and-fastapi).

### Efficient trace recording

The engine has no event hooks. BlackBox wraps each engine model (queue, latency, fees, order
responses) in a type implementing the same trait, delegating the decision to the real model and
appending a typed event to a tape. Events inside one engine step are buffered and committed in a
canonical order; queue updates beyond a per-order cap are counted, never silently dropped
([EXECUTION_MODEL.md](EXECUTION_MODEL.md#how-the-flight-recorder-observes-the-engine)).

### Deterministic replay

Same dataset bytes + same configuration + same code revision → same `run_id` and byte-identical
results, even though the engine iterates a randomized hash map internally. That is what makes a
"what changed?" question answerable at all
([EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md)).

### Why there is no live trading

It is a research tool. The engine's `live` feature is not compiled in, there are no credentials and
no exchange connectivity code in the binary, and the server binds to loopback by default
([ARCHITECTURE.md](ARCHITECTURE.md#what-runs-where)). A simulator that could also send real orders
would invite treating simulated results as real ones.

### What is ours and what is upstream

| Upstream (used unmodified) | BlackBox |
|---|---|
| hftbacktest 0.9.4: event loop, order bus, exchange models, queue models, constant latency, fees, depth | Flight recorder, ledger, risk harness, jitter latency, strategies, data pipeline and quality checks, explanations, comparisons, grids, run store, API, web console |
| Two py-hftbacktest data-prep algorithms (ported, attributed) | Five upstream defects found, pinned and worked around |
| Lightweight Charts (npm) | Every other UI component |

Details and licences: [OPEN_SOURCE_RESEARCH.md](OPEN_SOURCE_RESEARCH.md#reuse-boundaries-and-licence-obligations).

## Five-minute demo script

Start the server with `blackbox demo` (synthetic data, no network needed) or `blackbox serve` with
the Tardis sample ingested locally, and open the printed address. Mission Control, Replay Terminal
and Latency Lab are being built as this is written (`TODO.md`, M3 and M4); steps marked
**[re-check]** describe the planned screens and must be verified against the finished UI before the
demo is given.

| Time | Screen (route) | Do | Say |
|---|---|---|---|
| 0:00 | **Mission Control** (`/`) **[re-check]** | Point at the dataset card and its quality badge; start the one-click sample experiment. | "This replays recorded L2 data through an unmodified open-source engine. Everything is simulated; nothing here can trade. The badge is the data-quality report: we check the data before we trust it." |
| 0:45 | **Run overview** (`/runs/:runId`) | Show net P&L, fees, spread capture and markouts side by side. | "Spread capture says what quotes earned at the instant of the fill; markouts say what happened next; net P&L is what the books say. They disagree, which is the point." |
| 1:30 | **Replay Terminal** (`/runs/:runId/replay`) **[re-check]** | Play the run: book, own orders, fills and inventory on one clock. Jump to a fill. | "Every order is on a two-clock tape: when the strategy decided, when the exchange saw it, when the fill came back." |
| 2:15 | **Explain** (`/runs/:runId/orders/:orderId`) | Open a filled order, then an unfilled one. Read the explanation steps and their basis tags. | "Each statement is tagged observed, modeled, derived or unobservable and links to the recorded events. The queue position is modeled; the trades at our price are observed." |
| 3:15 | **Latency Lab** (`/latency`) **[re-check]** | Run the `cost_of_latency` demo (1 ms versus 50 ms each way); show the varied and controlled lists, then the first divergence. | "Only one variable differs. The comparison finds the first decision where the runs part ways, and it refuses to attribute the P&L difference to latency unless the arithmetic proves it." |
| 4:15 | **Latency Lab**, `gross_vs_net` **[re-check]** | Show fills identical across fee schedules and the exact fee arithmetic. | "This is the one case where attribution is certain: same fills, different fees, so the P&L difference is exactly the fee difference." |
| 4:45 | Close | Mention the queue-position demo and the five engine defects. | "On L2 data queue position is unknowable; we show how much results depend on it instead of hiding it." |

If a step's screen is not finished, the same facts are available from the CLI: `blackbox presets`,
`blackbox run --preset maker_baseline`, `blackbox explain <run> <order>`,
`blackbox compare <run_a> <run_b>`.
