# Quantitative semantics review — Phase 3

Reviewed 2026-11-15 against `phase3/quant`, based on `origin/main` `418addd`. Scope: queue telemetry and financial wording in `web/src/features/{explore,risk,forensics,terminal}`. No web files, Rust code, schemas, stored results or numerical contracts were changed.

## 1. Negative modeled queue ahead: legitimate signed model state

**Finding:** the negative minimum is real, intentional engine telemetry, not a model defect and not a floating-point artifact. The UI's unqualified phrase “Smallest queue ahead” is misleading because a physical quantity waiting ahead cannot be negative. Preserve the signed raw value; explain its meaning rather than clamp it.

### Reproduced from the local demo API

Requests used `Origin: http://localhost:5173`; no research key was needed. Only run metadata, the first 50 orders and one order detail were requested (no full run download):

- `GET http://127.0.0.1:8787/api/v1/runs/253bbc23b342f373`
- `GET http://127.0.0.1:8787/api/v1/runs/253bbc23b342f373/orders?limit=50&offset=0`
- `GET http://127.0.0.1:8787/api/v1/runs/253bbc23b342f373/orders/4608`

Run fingerprint: `253bbc23b342f373dff766b97128f82253bf135a6eb8a5a4462cd2f42f373aaa`; dataset `synthetic-demo`; simulation revision 2; hftbacktest 0.9.4; queue `power_prob3(n=3)`; exchange `no_partial_fill`; lot size 0.001. This is **SYNTHETIC historical replay, simulated executions, no market impact**, not live or observed exchange trading.

Order **4608**, buy 0.010 at 29998.8, has `queue_ahead_at_entry = 0.013000000000000001` and `min_queue_ahead = -0.771`. The following are API event values, not reconstructed chart pixels. Times are exchange-clock nanoseconds after dataset origin:

| Tape seq | Time (ns) | Event | Before → after |
|---|---:|---|---|
| 399 | 1901000000 | Accepted to rest | entry 0.013000000000000001 |
| 406 | 1902300298 | Trade 0.012 | 0.013000000000000001 → 0.0010000000000000009 |
| 407 | 1902300298 | Depth 0.013000000000000001 → 0.001 | 0.0010000000000000009 → 0.001 |
| 408 | 1903639432 | Trade 0.001 | 0.001 → 0 |
| 409 | 1903639432 | Depth 0.001 → 0 | 0 → 0 |
| 410 | 1915195506 | Depth 0 → 0.771 | 0 → 0 (new size joins behind) |
| 416 | 1934951725 | Trade 0.771 | 0 → **−0.771** |
| 417 | 1934951725 | Simulated maker fill | order filled 0.010; leaves 0 |

The 0.771 negative magnitude is **771 lots**, not rounding dust. The ~9e-19 difference between the first trade result and the subsequent depth result is ordinary floating-point precision; it does not explain the negative minimum. `traded_at_price_qty = 0.784` includes all three trades, not just the last one. The first fill notification reaches the local clock at 1935951725 ns (seq 419), 1 ms after the exchange fill.

### Exact semantics and source chain

Let `front` be the engine's signed front-quantity accumulator, in base quantity units:

- Entry: `front = displayed quantity at that price`.
- Trade at that price: `front -= trade_qty`. Depth changes use the configured queue model; growth does not move newcomers ahead of this order.
- `front > 0`: modeled quantity still ahead. `front = 0`: modeled at the front, **not itself proof of a fill**.
- `front < 0`: the modeled ahead quantity has been exhausted; `-front` is quantity beyond the modeled front. It is neither negative physical depth, short inventory, a rebate, nor necessarily this order's executed quantity.
- The engine tests `exec_lots = round(-front / lot_size)` and requires `exec_lots > 0`. Thus a tiny negative value below half a lot need not trigger execution. When executable, the engine resets its internal front state to zero. The tape retains the signed pre-reset trade update.
- `NoPartialFillExchange` fills the whole remaining order when its fill condition is met, not an execution equal to `-front`. `PartialFillExchange` uses executable quantity capped by the remainder; a surviving remainder gets a recorded `fill_reset` to zero. Trade-through/crossing conditions can fill independently of this queue path. Actual `exch_fill` events establish executions.

Evidence:

- hftbacktest 0.9.4 `src/backtest/models/queue.rs:175-178,207-215`: trade subtraction, lot rounding, executable test and reset; RiskAdverse has the same signed trade/reset contract at lines 76-94.
- `crates/bb-sim/src/queue_probe.rs:8-45`: reads the engine's round-tripping Debug value without numerical transformation. Its existing `prefix_matches_full_debug_bit_for_bit` test includes negative values.
- `crates/bb-sim/src/taps.rs:138-146`: records the result immediately after the engine's trade mutation, before `is_filled`; lines 169-184 handle executable quantity and partial-fill resets.
- `crates/bb-core/src/flight.rs:138-142`: explicitly documents negative `ahead_after` as queue exhaustion. `QueueCause::FillReset` is documented at lines 50-53.
- `crates/bb-forensics/src/orders.rs:146-158`: minimum is over readable raw queue estimates, including signed updates; existing test `records_are_sorted_by_order_id_and_cover_the_lifecycle` explicitly asserts `min_queue_ahead == Some(-0.45)` at line 254.
- `crates/bb-forensics/src/explain.rs` retains the raw path and explains exhaustion; the actual API explanation for this order says the trade went 0.771 beyond the front and filled 0.010.
- `crates/bb-analytics/src/execution.rs:1137-1143` consumes recorded entry queue state; it does not produce or clamp the forensic minimum. `crates/bb-forensics/src/metrics.rs` computes run metrics, not this minimum.
- Existing model documentation: [Queue models](../QUEUE_MODELS.md#the-state-and-its-lifecycle), including trade subtraction, half-lot fill threshold and reset behavior.

**Disposition:** no Rust fix or new defect regression is warranted: a nonnegative invariant would contradict the engine contract and existing tests. Do not silently clamp either the API minimum or chart path. A future UI assertion should render order 4608's signed minimum and an adjacent explanation. If a separate nonnegative “quantity still ahead” presentation is ever introduced, it must be explicitly derived and must not replace the raw signed record.

## 2. Financial contracts checked

Source of truth: [P&L accounting](../PNL_ACCOUNTING.md), `crates/bb-sim/src/ledger.rs:35-81`, `crates/bb-sim/src/lib.rs:294-310,726-752`, `crates/bb-forensics/src/metrics.rs:138-173,227-278`, and `crates/bb-core/src/result.rs:70-119`.

| Concept | Contract / audit result |
|---|---|
| Net P&L / equity | `realized + unrealized − fees`, all in quote currency; equity starts from the simulated P&L ledger, not an initial account deposit or portfolio NAV. Realized and unrealized are **before fees**. |
| Gross P&L | `realized + unrealized = net + fees`; includes marked open inventory, not just realized closed-trade P&L, turnover or spread capture. Risk's numerical expression is correct. |
| Mark | Run equity uses exchange-truth fills marked at the **local** mid; last valid two-sided local mid is reused while one-sided, and unrealized is zero before any valid mid. It is not a liquidation/close-out value. |
| Fee sign | `fee = rate × traded value`. Positive is a charge; negative is a maker rebate; subtracting a negative fee increases net. Do not invert raw fee values or style positive fees as gains. |
| Drawdown | Largest running-peak minus subsequent sampled net equity, an **absolute quote-currency amount**, not a percentage or return. Downsampled risk chart uses a labeled upper bound; it is not exact intrabucket ordering. |
| Fill ratio, orders | Number of submitted orders with **any** fill divided by submitted orders (full and partial fills both count). Risk-blocked and synchronously refused orders are excluded from submitted; in-flight-at-end orders are included. It is an end-of-run fraction, not a time rate or Kaplan–Meier probability. |
| Fill ratio, quantity | Sum of filled quantity divided by submitted quantity, not average per-order fill percentage. Neither ratio extrapolates unfilled outstanding orders. |
| Execution P&L decomposition | Analytics uses `edge + drift − fees` at the **exchange end mid**, with edge from the strict pre-fill exchange mid. It can differ from run P&L at the local mark; missing marks make the decomposition unavailable. Maker/taker splits attribute fills, not independently funded strategy accounts. |
| Clocks | Order decisions, request sends, local responses and equity samples: **local/strategy** clock. Queue updates, arrivals, fills and terminal exchange state: **exchange** clock. Relative timestamps share dataset origin; `created_at_ns` is run creation wall time, not replay event time. |

Actual demo reconciliation (API metadata): realized −0.24849999999907613 + unrealized 0 − fees 74.46105319999984 = net −74.70955319999891 quote. Gross is approximately −0.2485, not +74.46 and not the turnover 372305.266. Drawdown is 74.71005319999894 quote. Order fill ratio is 1241 / 2060 = 0.6024271844660194. Both gross and net lose money; a smaller negative result must remain a smaller loss, not a profit.

## 3. UI mislabeling / proposed wording (no web edits)

Locations below refer to the audited baseline; the lead should map them onto concurrent Phase 3 UI edits. “Clarify” means the calculation is correct but the current label omits an important contract.

| Priority | File:line (under `web/src/features/`) | Problem | Proposed wording / treatment |
|---|---|---|---|
| High | `forensics/OrderForensicsPage.tsx:274-276` | “Smallest queue ahead” presents a legitimate signed accumulator as negative physical quantity. | **“Minimum raw queue estimate (modeled)”**; adjacent persistent help: **“Negative means traded quantity beyond the modeled front, not negative displayed depth. Fill events establish executions.”** |
| High | `forensics/QueueChart.tsx:231,345` | Accessible chart label / legend say only “quantity ahead”; the chart correctly includes negative values but does not explain them. | **“Signed queue estimate (modeled, base units)”**; retain zero line and signed path; explain **“Below zero: quantity beyond the modeled front; not the order's fill quantity.”** Do not clamp the chart. |
| High | `risk/RiskPage.tsx:118` | “Realized … + unrealized … (fees included)” omits subtraction and can suggest the two before-fee components already include fees. | **“Net = realized P&L + unrealized P&L − fees (quote currency). Realized and unrealized are before fees.”** Include the signed fee amount when listing the components. |
| Medium | `explore/Results.tsx:12`; `explore/data.ts:166`; `explore/LifecyclePanel.tsx:156`; `explore/data.ts:127` | “Fees paid”, “Fee paid” and “paid −X” misdescribe a rebate. Current demo rates are positive, but the signed contract permits rebates. | **“Fees / rebates”**, **“Fee (negative = rebate)”**, and **“fee balance X (negative = rebate)”** or conditional “received X in rebates”. Keep raw fee sign. |
| Medium | `explore/StepGuide.tsx:104-106` | Row displays `-fees` but calls it “Fees (subtracted)”; this is the additive **fee impact**, not the signed raw fee displayed elsewhere. | **“Fee impact (−fees; rebates positive)”**. Keep the existing arithmetic and the correct formula at line 76. |
| Medium | `risk/RiskPage.tsx:120`; `terminal/ReplayTerminalPage.tsx:834-837`; `forensics/panels.tsx:420,424` | Fee sign and before-fee component semantics are not visible in these local contexts. Forensics summary already correctly says “negative is a rebate” at `OrderForensicsPage.tsx:287`. | Fee help: **“Quote currency; positive is charged, negative is a rebate.”** Realized/unrealized help: **“Before fees.”** Fill table: **“Realized P&L from this fill (before fees)”**, not cumulative P&L. |
| Medium | `terminal/ReplayTerminalPage.tsx:837,840-841` | “Equity” can be read as funded NAV or the strategy's known account; it actually uses exchange-truth fills at a local mark, even though the sample clock itself is correctly labeled local. | **“Net P&L / equity”**; note **“Exchange-truth fills marked at the local mid; realized + unrealized − fees. ‘Known to strategy’ includes only received fill notifications.”** |
| Medium | `explore/Results.tsx:14`; `explore/data.ts:171` | “Orders filled” / “Share of orders that filled” can imply fully completed orders. | **“Orders with any fill”**, help **“Fully or partially filled orders ÷ submitted orders, as of run end.”** Risk's numerator description at `RiskPage.tsx:93` is correct. |
| Medium | `risk/RiskPage.tsx:93,95` | Right-censoring wording mentions only `orders_working_at_end`; that outcome counter excludes partially filled working orders, and orders still in flight can also be unresolved. | **“End-of-run fraction, not a censoring-adjusted probability. Outstanding, partially filled and in-flight orders may still change after the replay ends.”** Do not call the existing counter a count of all unresolved orders. |
| Medium | `risk/RiskPage.tsx:175-193` | Drawdown chart has no visible quote-currency unit; generic dataset-origin time note omits the equity sample's local clock. Numerical drawdown and table units are correct. | Caption **“Running peak minus sampled net equity, in quote currency (not %). Local sample clock, relative to dataset origin.”** Keep the existing downsampled upper-bound warning. |
| Medium | `risk/AnalyticsPanels.tsx:170,225` | Generic “net” in decomposition can be confused with run Net P&L; exchange end mid is printed but not the possibility of a different local mark. | **“Net at exchange end mid”**; caption **“Edge + drift − fees at exchange end mid; may differ from run Net P&L marked at local mid.”** |
| Low | `risk/RiskPage.tsx:119,126` | Correct gross formula is only “Net P&L + fees”; final position caveat says “last mid” without specifying local. | Gross help **“Realized + unrealized P&L, before fees; includes open inventory.”** Final position help **“Open inventory; unrealized P&L uses the last valid local mid.”** |
| Low | `risk/AnalyticsPanels.tsx:232,234` | “Time to … fill (observed)” can blur the Observed/Modeled distinction; “observed” here means uncensored simulated event durations, not real executions. | **“Time to … simulated fill (uncensored)”**. Retain the panel's MODELED basis and Kaplan–Meier censoring counts. |
| Medium | `explore/data.ts:185-190` | Additional financial wording found during audit: total spread capture and quantity-weighted capture are both called “per fill”; markout is called a price move after a fill, omitting execution price and side. | `spread_capture`: **“Total maker spread capture (quote)”**; `spread_capture_ticks`: **“Maker spread capture per unit (ticks)”**; markouts: **“Side-signed mid minus fill price at +[h] (ticks)”**. Markout includes capture, not only post-fill mid drift. |

### Clock labels already correct

- `explore/LifecyclePanel.tsx:31-38`: decision/response/learned-fill use strategy clock; arrival/first fill/terminal use exchange clock; the learned fill is located from an explicit execution-bearing local response.
- `forensics/panels.tsx:159-215`: feed staleness local; feed/response latency exchange → local; order entry local → exchange; arrival-to-fill and terminal intervals exchange. Fill table explicitly says exchange time (line 409); event list includes a clock column and dataset-origin note (lines 509-520,556-557).
- `forensics/QueueChart.tsx:355-358`: exchange-clock axis and decision-relative labels are correct.
- `terminal/ReplayTerminalPage.tsx:701-702,752-753,840-841,1089-1105`: exchange cursor, local decision, exchange arrival/fill/terminal and local account sample labels are correct. An exchange cursor and nearest earlier local-clock account sample are different views; keep the explicit clock note rather than relabeling the account exchange-time.
- `terminal/EventLog.tsx` exposes per-event clock badges and explains the shared origin; no incorrect exchange/local swap found.

## 4. Validation and handoff

- Observed validation: successful local API metadata, first-order-page and order-detail responses; exact event arithmetic and ledger reconciliation above; source inspection through the recorder, engine, forensics, analytics and UI.
- No invalid model behavior was found; no Rust files changed. Therefore the conditional Rust fmt / clippy / `cargo test --workspace --locked` gate was not invoked. No web builds, tests or screenshots were run for this documentation-only assignment.
- The existing negative-queue assertions were inspected, not executed; no new test result is claimed. UI wording changes and browser acceptance are intentionally left to the lead/owning UI agents, per assignment.
