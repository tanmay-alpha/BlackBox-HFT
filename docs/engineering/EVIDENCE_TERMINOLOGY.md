# Evidence terminology

Every number, row and chart in the research UI belongs to exactly one evidence class. Visible copy
names the class; it never lets one class pass for another. This page is the reference for UI copy,
report wording and reviews. Code: `web/src/components/ui.tsx` (`BASIS_LABEL`, `BASIS_MEANING`,
`BasisBadge`), `web/src/components/evidence.ts` (`replayPhrase`), dataset kind badges on
Datasets, Book index, Mission Control, Replay and the Replay terminal.

## Evidence classes

| Class | What it is | Say | Never say |
|---|---|---|---|
| Recorded source event | A message from a recorded exchange or vendor feed, converted into a dataset of kind `recorded`. | "recorded market data", "recorded trade" | "historical" for anything else |
| Synthetic generated event | A message produced by BlackBox's generator from a seed and a specification; dataset kind `synthetic`. | "synthetic", "generated event", "synthetic dataset" | "historical", "observed market data", "market data" without "synthetic" |
| Reconstructed state | Book state (L3 orders, L2 levels, best bid/ask) rebuilt by replaying source events. As true as its source events; recorded or synthetic follows the dataset. | "reconstructed book", "reconstructed L2" | "observed book" |
| Estimated queue position | Where a simulated order stands in the queue, from the configured queue model. | "estimated queue position", "modeled queue" | "queue position" as a fact |
| Modelled decision | A strategy decision computed by the simulated strategy from the replayed data. | "modeled decision", "strategy decision (simulated)" | "order sent", "trade" |
| Simulated fill | A fill produced by the matching/queue model for a simulated order. Never reached an exchange. | "simulated fill" | "execution", "trade", "filled" without "simulated" |
| Derived metric | Arithmetic over the classes above (P&L, markouts, rates, spreads). Inherits the weakest input. | "derived" | presenting it as source data |
| Unobservable counterfactual | What the market would have done had the simulated orders existed; hidden liquidity; true queue position. | "unobservable", "not knowable from the data" | any estimate of it as fact |

The UI's four basis badges group these classes:

| Badge (visible) | Code value | Classes |
|---|---|---|
| source data | `observed` | recorded source event, synthetic generated event, reconstructed state |
| modeled | `modeled` | estimated queue position, modeled decision, simulated fill, latency draws |
| derived | `derived` | derived metric |
| unobservable | `unobservable` | unobservable counterfactual |

The code value `observed` is kept for API and CSS compatibility; it is never shown as the word
"observed", because a synthetic event was not observed anywhere.

## Source-data category labels

Shown wherever a dataset is named, read from the dataset's `kind` as served by the API:

| `kind` | Label | Badge |
|---|---|---|
| `recorded` | recorded market data (short: recorded) | `observed` tone |
| `synthetic` | synthetic, generated, not market data (short: synthetic) | `warn` tone |

When a page does not know the kind (reports, forensics, sweeps), copy uses the neutral
"a replay of the run's dataset"; `replayPhrase(kind)` returns the conditioned phrase when it does.

## Rules for copy

- "Historical" only describes recorded data, and only where the page knows the dataset is recorded.
- "Simulated on a replay of …", never "simulated historical replay".
- Simulated orders and fills are never "executions"; they "did not affect the replayed prices".
- Raw identifiers (`touch_maker`, `metrics_only`, `price4`, nanosecond counts, config paths) are
  shown in words via `components/labels.ts`, `components/describe.ts` and `labFieldLabels.ts`; the
  raw value stays in the `title` or an advanced/copyable detail.
- Configuration values are formatted by the single `configValueText` (`components/labels.ts`).
