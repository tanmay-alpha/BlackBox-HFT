// "What the simulator knows vs. estimates": one ledger row per evidence class of a single order,
// mapped from the order-detail response and the run's execution config. Pure, so every value on
// screen is traceable to an API field in the unit tests. Wording follows bb-core `explain.rs`,
// `flight.rs` and docs/rebuild/QUANT_SEMANTICS_REVIEW.md (signed queue minimum).

import type {
  Basis,
  Clock,
  DatasetKind,
  ExecutionConfig,
  FlightEvent,
  OrderDetail,
} from "../../api/types";
import { MARKOUT_HORIZONS_NS } from "../../api/types";
import {
  EXCHANGE_MODEL_INFO,
  exchangeModelLabel,
  fractionToBps,
  latencyLabel,
  queueModelLabel,
} from "../../components/describe";
import { fmtBp, fmtDuration, fmtInt, fmtPnl, fmtPrice, fmtQty, fmtTicks } from "../../components/fmt";
import { VERDICT_TEXT, fmtSince, queueModelMeaning, words } from "./labels";

export type LedgerId = "observed" | "placement" | "queue" | "fill" | "performance" | "counterfactual";

/** Existing views a row can point to instead of repeating their detail. */
export type LedgerTarget = "context" | "latency" | "queue" | "fills" | "explanation";

export interface LedgerFact {
  label: string;
  /** Formatted API value with its unit; `null` when the value does not exist for this order. */
  value: string | null;
  /** A recorded timestamp (relative to dataset origin), rendered as a clock reading. */
  time?: { ns: number; clock: Clock };
  note?: string;
  /** The value is words, not a number: set in the reading face instead of mono. */
  prose?: boolean;
}

export interface LedgerRow {
  id: LedgerId;
  /** Evidence basis badge. */
  basis: Basis;
  /** The finer class named in the row: observed, modeled, estimated, simulated, derived, unobservable. */
  evidence: string;
  title: string;
  /** What this class of evidence is, one sentence. */
  what: string;
  /** One-line value summary for the collapsed row. */
  summary: string;
  facts: LedgerFact[];
  /** Short name of the assumption (model names from the run config). */
  restsOn: string;
  /** The assumption in full. */
  assumption: string;
  /** Tape events the row's values come from. */
  seqs: number[];
  link: { target: LedgerTarget; label: string } | null;
}

export interface LedgerInput {
  detail: OrderDetail;
  /** `null` while the run configuration is loading or unavailable. */
  execution: ExecutionConfig | null;
  /** Dataset identity of the run, `null` while unknown. */
  dataset: { id: string; kind: DatasetKind } | null;
}

type Body<T extends FlightEvent["body"]["type"]> = Extract<FlightEvent["body"], { type: T }>;

function first<T extends FlightEvent["body"]["type"]>(
  events: FlightEvent[],
  type: T,
  pred: (b: Body<T>) => boolean = () => true,
): { ev: FlightEvent; body: Body<T> } | null {
  for (const ev of events) {
    if (ev.body.type === type && pred(ev.body as Body<T>)) return { ev, body: ev.body as Body<T> };
  }
  return null;
}

function plural(n: number, one: string, many = `${one}s`): string {
  return `${fmtInt(n)} ${n === 1 ? one : many}`;
}

function uniqueSorted(seqs: number[]): number[] {
  return [...new Set(seqs)].sort((a, b) => a - b);
}

/** Meaning of the signed minimum raw queue estimate (QUANT_SEMANTICS_REVIEW §1). */
export function minQueueMeaning(min: number | null, fmt: (q: number) => string): string {
  if (min === null) return "No queue estimate was recorded for this order.";
  if (min < 0) {
    return `Negative: ${fmt(-min)} traded beyond the modeled front. Not negative displayed depth and not this order's fill quantity; fill events establish executions.`;
  }
  if (min === 0) return "Zero: modeled at the front of the queue, which is not by itself proof of a fill.";
  return "The modeled quantity ahead never fell below this, so the order never reached the modeled front.";
}

export function buildLedger({ detail, execution, dataset }: LedgerInput): LedgerRow[] {
  const { order, events, fills, explanation, instrument } = detail;
  const qty = (q: number | null) => fmtQty(q, instrument);
  const price = (t: number | null) => fmtPrice(t, instrument);
  const origin = order.decision_ts;
  const sinceDecision = (ts: number) => `${fmtSince(ts, origin)} after decision`;
  const latency = explanation.latency;

  const intent = first(events, "order_intent");
  const sentNew = first(events, "request_sent", (b) => b.req === "new");
  const sentCancel = first(events, "request_sent", (b) => b.req === "cancel");
  const queued = first(events, "exch_queued");
  const firstResponse = first(events, "exch_response");
  const truncated = events.reduce(
    (n, ev) => (ev.body.type === "queue_truncated" ? n + ev.body.dropped : n),
    0,
  );
  const tradeUpdates = events.filter(
    (ev) => ev.body.type === "queue_update" && ev.body.cause.type === "trade",
  );
  const depthUpdates = events.filter(
    (ev) => ev.body.type === "queue_update" && ev.body.cause.type === "depth",
  );
  const filled = order.filled_qty > 0;
  const rested = queued !== null;

  // ----- 1. Source market events of the dataset ---------------------------------------------------
  const observedFacts: LedgerFact[] = [];
  if (intent) {
    const m = intent.body.market;
    const side = (tick: number | null, q: number, name: string) =>
      tick === null ? `no ${name}` : `${price(tick)} \u00d7 ${qty(q)}`;
    observedFacts.push({
      label: "Best bid / ask received by the strategy at decision",
      value: `${side(m.best_bid_tick, m.best_bid_qty, "bid")} / ${side(m.best_ask_tick, m.best_ask_qty, "ask")}`,
      time: m.feed_exch_ts === null ? undefined : { ns: m.feed_exch_ts, clock: "exchange" },
      note: "price \u00d7 displayed base quantity; time is the exchange time of the last feed event received",
    });
  }
  observedFacts.push(
    queued
      ? {
          label: "Displayed at the order's price on arrival",
          value: `${qty(queued.body.level_qty)} at ${price(order.price_tick)}`,
          time: { ns: queued.ev.ts, clock: "exchange" },
          note: `best bid ${price(queued.body.best_bid_tick)}, best ask ${price(queued.body.best_ask_tick)}; the arrival time is modeled, the book at that time is recorded`,
        }
      : {
          label: "Displayed at the order's price on arrival",
          value: null,
          note: "The order never rested at the simulated exchange, so no book state was recorded for it.",
        },
    {
      label: "Traded at the order's price while it rested",
      value: `${qty(order.traded_at_price_qty)}${
        rested && truncated === 0 ? ` in ${plural(tradeUpdates.length, "trade print")}` : ""
      }`,
      note: rested ? "base quantity, trades in the dataset" : "The order never rested.",
    },
  );
  if (rested) {
    observedFacts.push({
      label: "Changes in displayed quantity at the price",
      value: truncated === 0 ? fmtInt(depthUpdates.length) : `${fmtInt(depthUpdates.length)} recorded`,
      note:
        truncated === 0
          ? "while the order rested"
          : `the recorder dropped ${fmtInt(truncated)} later queue updates of under one lot`,
    });
  }
  const datasetNote = dataset
    ? dataset.kind === "synthetic"
      ? `Dataset ${dataset.id} is SYNTHETIC: generated by BlackBox from a seed, not exchange market data. `
      : `Dataset ${dataset.id}, recorded market data. `
    : "";
  const observed: LedgerRow = {
    id: "observed",
    basis: "observed",
    evidence: "observed",
    title: "Market events in the dataset (source data)",
    what: "Book and trades at the order's price, read from the dataset around the order.",
    summary: queued
      ? `${qty(queued.body.level_qty)} displayed at ${price(order.price_tick)} on arrival \u00b7 ${qty(order.traded_at_price_qty)} traded at price`
      : intent
        ? `${observedFacts[0].value} at decision \u00b7 never rested`
        : "No market event was recorded for this order.",
    facts: observedFacts,
    restsOn: dataset ? `${dataset.id} (${dataset.kind})` : "dataset loading",
    assumption: `${datasetNote}Replay is exact: prices, depth and trades are as recorded and never react to the simulated order.`,
    seqs: uniqueSorted([
      ...(intent ? [intent.ev.seq] : []),
      ...(queued ? [queued.ev.seq] : []),
      ...tradeUpdates.map((ev) => ev.seq),
      ...depthUpdates.map((ev) => ev.seq),
    ]),
    link: intent ? { target: "context", label: "Open the Decision context tab" } : null,
  };

  // ----- 2. Modeled order placement ------------------------------------------------------------
  const entryNs = sentNew ? sentNew.body.entry_latency_ns : latency.entry_latency_ns;
  const placementFacts: LedgerFact[] = [
    {
      label: "Decision (local clock)",
      value: `strategy tick ${order.tick_no}, slot ${order.slot}`,
      time: { ns: order.decision_ts, clock: "local" },
    },
    {
      label: "Modeled entry latency",
      value: entryNs === null ? null : entryNs < 0 ? "rejected before the matching engine" : fmtDuration(entryNs),
      prose: entryNs !== null && entryNs < 0,
      note: entryNs === null ? "No new-order request was sent." : undefined,
    },
    order.exch_arrival_ts === null
      ? {
          label: "Arrival at the simulated exchange",
          value: null,
          note: `Never reached the simulated exchange: ${words(order.outcome)}.`,
        }
      : {
          label: "Arrival at the simulated exchange (exchange clock)",
          value: sinceDecision(order.exch_arrival_ts),
          time: { ns: order.exch_arrival_ts, clock: "exchange" },
        },
  ];
  if (order.first_response_ts !== null) {
    placementFacts.push({
      label: "First response reached the strategy (local clock)",
      value: sinceDecision(order.first_response_ts),
      time: { ns: order.first_response_ts, clock: "local" },
      note:
        latency.response_latency_ns === null
          ? undefined
          : `modeled response latency ${fmtDuration(latency.response_latency_ns)}`,
    });
  }
  if (order.cancel_sent_ts !== null) {
    placementFacts.push({
      label: "Cancel sent (local clock)",
      value: sinceDecision(order.cancel_sent_ts),
      time: { ns: order.cancel_sent_ts, clock: "local" },
      note: [
        latency.cancel_entry_latency_ns === null
          ? null
          : `modeled cancel entry latency ${fmtDuration(latency.cancel_entry_latency_ns)}`,
        order.cancel_rejected ? "rejected, the order was already gone" : null,
      ]
        .filter((s) => s !== null)
        .join("; "),
    });
  }
  const lat = execution?.order_latency;
  const placement: LedgerRow = {
    id: "placement",
    basis: "modeled",
    evidence: "modeled",
    title: "Modeled order placement",
    what: "When the order was decided, how long the latency model took to deliver it, and when it reached the simulated exchange.",
    summary:
      order.exch_arrival_ts === null
        ? `Never reached the simulated exchange (${words(order.outcome)})`
        : `Arrival ${fmtSince(order.exch_arrival_ts, origin)} after decision${
            entryNs !== null && entryNs >= 0 ? ` \u00b7 entry latency ${fmtDuration(entryNs)}` : ""
          }`,
    facts: placementFacts,
    restsOn: lat ? `${words(lat.type)} latency ${latencyLabel(lat)}` : "run config loading",
    assumption: execution && lat
      ? `Latency model: ${words(lat.type)}, entry / response ${latencyLabel(lat)}.${
          execution.think_time_ns ? ` Strategy think time ${fmtDuration(execution.think_time_ns)}.` : ""
        } Feed latency offset ${fmtDuration(execution.feed_latency_offset_ns)}. Real network and gateway delays are not in the data.`
      : "The run's latency model is not loaded.",
    seqs: uniqueSorted([
      ...(intent ? [intent.ev.seq] : []),
      ...(sentNew ? [sentNew.ev.seq] : []),
      ...(firstResponse ? [firstResponse.ev.seq] : []),
      ...(sentCancel ? [sentCancel.ev.seq] : []),
    ]),
    link: { target: "latency", label: "Open the Latency breakdown tab" },
  };

  // ----- 3. Estimated queue position -----------------------------------------------------------
  const path = explanation.queue_path;
  const qm = execution ? queueModelLabel(execution.queue_model) : null;
  const queueFacts: LedgerFact[] = [
    { label: "Queue model", value: qm, note: qm === null ? "run config loading" : undefined },
    {
      label: "Initial estimate, at arrival",
      value: order.queue_ahead_at_entry === null ? null : `${qty(order.queue_ahead_at_entry)} ahead`,
      note:
        order.queue_ahead_at_entry === null
          ? "No estimate: the order never rested."
          : queued
            ? `${qty(queued.body.level_qty)} was displayed at the price`
            : undefined,
    },
    {
      label: "Estimate path",
      value:
        path.length === 0
          ? null
          : `${qty(path[0].ahead)} \u2192 ${qty(path[path.length - 1].ahead)} over ${plural(path.length, "recorded estimate")}`,
      note:
        path.length === 0
          ? "No queue path: the order never rested."
          : truncated > 0
            ? `${fmtInt(truncated)} later updates of under one lot were not kept`
            : "base quantity ahead, exchange clock",
    },
    {
      label: "Minimum raw estimate (signed)",
      value: order.min_queue_ahead === null ? null : qty(order.min_queue_ahead),
      note: minQueueMeaning(order.min_queue_ahead, (q) => qty(q)),
    },
  ];
  const queue: LedgerRow = {
    id: "queue",
    basis: "modeled",
    evidence: "estimated",
    title: "Estimated queue position",
    what: "Quantity modeled ahead of the order at its price. Level-2 data shows totals per price, never individual orders.",
    summary:
      order.queue_ahead_at_entry === null
        ? "No estimate: the order never rested"
        : `${qty(order.queue_ahead_at_entry)} ahead at arrival \u00b7 minimum raw ${qty(order.min_queue_ahead)}`,
    facts: queueFacts,
    restsOn: qm ?? "run config loading",
    assumption: execution
      ? `Queue model ${qm}: the order joins behind everything displayed at its price, and trades there reduce the estimate. ${queueModelMeaning(execution.queue_model)}`.trim()
      : "The run's queue model is not loaded.",
    seqs: uniqueSorted([...(queued ? [queued.ev.seq] : []), ...path.map((p) => p.seq)]),
    link: path.length > 0 ? { target: "queue", label: "Show the queue chart" } : null,
  };

  // ----- 4. Simulated fill -------------------------------------------------------------------
  const fillEvents = events.filter((ev) => ev.body.type === "exch_fill");
  const liquidity =
    order.maker_qty > 0 && order.taker_qty > 0 ? "maker and taker" : order.maker_qty > 0 ? "maker" : "taker";
  const fillFacts: LedgerFact[] = [
    {
      label: "Fill condition under the exchange model",
      value: VERDICT_TEXT[explanation.verdict],
      prose: true,
    },
    {
      label: "Filled",
      value: `${qty(order.filled_qty)} of ${qty(order.qty)}`,
      note: plural(order.fill_count, "fill"),
    },
  ];
  if (filled) {
    if (order.first_fill_ts !== null) {
      fillFacts.push({
        label: "First fill (exchange clock)",
        value: sinceDecision(order.first_fill_ts),
        time: { ns: order.first_fill_ts, clock: "exchange" },
        note:
          latency.time_to_first_fill_ns === null
            ? undefined
            : `${fmtDuration(latency.time_to_first_fill_ns)} after arrival`,
      });
    }
    fillFacts.push(
      { label: "Maker / taker", value: `${qty(order.maker_qty)} / ${qty(order.taker_qty)}` },
      { label: "Average fill price", value: price(order.avg_fill_tick), note: "quantity-weighted" },
      { label: "Fee (quote)", value: fmtPnl(order.fees, 4), note: "positive is charged, negative is a rebate" },
    );
  } else {
    fillFacts.push({
      label: "Last exchange status",
      value: words(order.final_status),
      prose: true,
      time: order.terminal_ts === null ? undefined : { ns: order.terminal_ts, clock: "exchange" },
      note: order.terminal_ts === null ? "not terminal at the exchange when the data ended" : sinceDecision(order.terminal_ts),
    });
  }
  const fill: LedgerRow = {
    id: "fill",
    basis: "modeled",
    evidence: "simulated",
    title: "Simulated fill",
    what: "Whether the configured fill model's condition was met, and the execution it then simulated.",
    summary: filled
      ? `${qty(order.filled_qty)} of ${qty(order.qty)} filled as ${liquidity} at ${price(order.avg_fill_tick)}${
          order.first_fill_ts === null ? "" : `, ${fmtSince(order.first_fill_ts, origin)} after decision`
        }`
      : `No fill: ${VERDICT_TEXT[explanation.verdict].replace(/^not filled: /, "")}`,
    facts: fillFacts,
    restsOn: execution ? exchangeModelLabel(execution.exchange_model) : "run config loading",
    assumption: execution
      ? `Exchange model ${exchangeModelLabel(execution.exchange_model)}: ${EXCHANGE_MODEL_INFO[execution.exchange_model].help} A resting order also fills when the market trades through its price or the opposite best crosses it, regardless of queue position.`
      : "The run's exchange model is not loaded.",
    seqs: uniqueSorted(
      fillEvents.length > 0
        ? fillEvents.map((ev) => ev.seq)
        : events.filter((ev) => ev.body.type === "exch_response").map((ev) => ev.seq).slice(-1),
    ),
    link: { target: "fills", label: "Open the Fills tab" },
  };

  // ----- 5. Derived performance -----------------------------------------------------------------
  const realized = fills.reduce((s, f) => s + f.realized_pnl, 0);
  const perfFacts: LedgerFact[] = [
    {
      label: "Fees (quote)",
      value: fmtPnl(order.fees, 4),
      note: fills.length === 0 ? "No fill, so no fee." : "positive is charged, negative is a rebate",
    },
  ];
  if (fills.length > 0) {
    perfFacts.push({
      label: "Realized P&L from its fills (quote, before fees)",
      value: fmtPnl(realized, 4),
      note: "Unrealized P&L of open inventory is marked at run level, not attributed to one order.",
    });
    for (const f of fills) {
      perfFacts.push({
        label: `Markouts of fill #${f.seq} (ticks)`,
        value: MARKOUT_HORIZONS_NS.map((h, i) => `+${fmtDuration(h)} ${fmtTicks(f.markout_ticks[i] ?? null)}`).join(" \u00b7 "),
        note: `mid at fill ${price(f.mid_tick)}; a dash means the dataset ended first or the book was not valid`,
      });
    }
  } else {
    perfFacts.push({ label: "Markouts and realized P&L", value: null, note: "No simulated fill, so nothing to measure." });
  }
  const fees = execution?.fees;
  const performance: LedgerRow = {
    id: "performance",
    basis: "derived",
    evidence: "derived",
    title: "Derived performance",
    what: "Fees, markouts and realized P&L computed after the run from the simulated fills and dataset mids.",
    summary:
      fills.length === 0
        ? "Nothing to derive: no simulated fill"
        : `fee ${fmtPnl(order.fees, 4)} quote \u00b7 realized ${fmtPnl(realized, 4)} before fees${
            fills.length === 1
              ? ` \u00b7 markout +${fmtDuration(MARKOUT_HORIZONS_NS[0])} ${fmtTicks(fills[0].markout_ticks[0] ?? null)} ticks`
              : ""
          }`,
    facts: perfFacts,
    restsOn: fees ? `fees ${fmtBp(fractionToBps(fees.maker))} / ${fmtBp(fractionToBps(fees.taker))}` : "run config loading",
    assumption: `${
      fees
        ? `Fee model: maker ${fmtBp(fractionToBps(fees.maker))}, taker ${fmtBp(fractionToBps(fees.taker))} of traded value. `
        : ""
    }A markout is the side-signed dataset mid at fill time plus the horizon minus the fill price, in ticks. Realized P&L comes from the run's position accounting, before fees. All inherit the assumptions of the simulated fill.`,
    seqs: uniqueSorted(fills.map((f) => f.seq)),
    link: fills.length > 0 ? { target: "fills", label: "Open the Fills tab" } : null,
  };

  // ----- 6. Unobservable counterfactual: plain words, never numbers -------------------------------
  const counterfactual: LedgerRow = {
    id: "counterfactual",
    basis: "unobservable",
    evidence: "unobservable",
    title: "Unobservable counterfactual",
    what: "What a real order would have met. This replay cannot answer it, so no number is given.",
    summary: "Real fill, true queue place, others' reaction and market impact are unknowable",
    facts: [
      {
        label: "Real fill",
        value: null,
        note: filled
          ? "Whether a real order at this price and time would also have filled, and when."
          : "Whether a real order at this price and time would have filled where the simulated one did not.",
      },
      {
        label: "True queue position",
        value: null,
        note: "Market-By-Price data shows the total at each price, not individual orders, so the real place in the queue is unknown.",
      },
      {
        label: "Other participants' reaction",
        value: null,
        note: "How other traders would have changed or cancelled their orders after seeing this one.",
      },
      {
        label: "Market impact",
        value: null,
        note: "How prices, depth and trades would have moved; the replay keeps them exactly as recorded.",
      },
    ],
    restsOn: "nothing testable",
    assumption:
      "The simulator assumes no reaction and no impact; the fill model stands in for the unknown real outcome.",
    seqs: uniqueSorted(
      explanation.steps.flatMap((s) => (s.basis === "unobservable" ? s.event_seqs : [])),
    ),
    link: { target: "explanation", label: "Open the Explanation tab and its caveats" },
  };

  return [observed, placement, queue, fill, performance, counterfactual];
}
