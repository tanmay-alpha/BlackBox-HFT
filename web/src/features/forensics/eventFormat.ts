// Per-type rendering of flight-recorder events. One formatter feeds the timeline tooltip, the
// selected-event strip and the raw event list, so an event reads the same everywhere.

import type { FlightEvent, InstrumentMeta, MarketView } from "../../api/types";
import { fmtClock, fmtDuration, fmtNum, fmtPrice, fmtQty } from "../../components/fmt";
import { strategyName } from "../../components/describe";
import { EM_DASH, words } from "./labels";

export type EventField = [label: string, value: string];

function touch(market: MarketView, inst: InstrumentMeta): string {
  const bid =
    market.best_bid_tick === null
      ? "no bid"
      : `${fmtPrice(market.best_bid_tick, inst)} x ${fmtQty(market.best_bid_qty, inst)}`;
  const ask =
    market.best_ask_tick === null
      ? "no ask"
      : `${fmtPrice(market.best_ask_tick, inst)} x ${fmtQty(market.best_ask_qty, inst)}`;
  return `${bid} / ${ask}`;
}

/** A strategy's reason with its kind id prefix (`touch_maker: quote bid …`) named in words. */
function reasonText(reason: string): string {
  const m = /^([a-z][a-z0-9_]*): ([\s\S]*)$/.exec(reason);
  return m ? `${strategyName(m[1])}: ${m[2]}` : reason;
}

/** Human title of an event type. Fill and response wording always states that it is simulated. */
export function eventTitle(ev: FlightEvent): string {
  const b = ev.body;
  switch (b.type) {
    case "order_intent":
      return "Strategy decided to place the order";
    case "cancel_intent":
      return "Strategy decided to cancel";
    case "risk_blocked":
      return "Risk rule blocked the intent";
    case "risk_halt":
      return "Run-level risk halt";
    case "request_sent":
      return b.req === "cancel" ? "Cancel request sent" : `${words(b.req)} request sent`;
    case "request_refused":
      return "Engine refused the request";
    case "exch_queued":
      return "Accepted to rest at the simulated exchange";
    case "queue_update":
      return b.cause.type === "trade"
        ? "Market trade at the order's price"
        : b.cause.type === "fill_reset"
          ? "Queue estimate reset after a partial fill (modeled)"
          : "Displayed quantity changed at the order's price";
    case "exch_fill":
      return "Simulated fill";
    case "exch_response":
      return "Simulated exchange emitted a response";
    case "local_response":
      return "Response received locally";
    case "queue_truncated":
      return "Recorder truncated queue updates";
    default:
      return "Event";
  }
}

/**
 * Labelled fields of an event body. Types this build does not know are returned as raw JSON so a
 * newer backend never produces an empty or misleading row.
 */
export function eventFields(ev: FlightEvent, inst: InstrumentMeta): EventField[] {
  const b = ev.body;
  switch (b.type) {
    case "order_intent":
      return [
        ["side", b.side],
        ["price", fmtPrice(b.price_tick, inst)],
        ["qty", fmtQty(b.qty, inst)],
        ["tif", b.tif],
        ["type", b.ord_type],
        ["tick", String(b.tick_no)],
        ["slot", String(b.slot)],
        ["known position", fmtQty(b.known_position, inst)],
        ["local bid / ask", touch(b.market, inst)],
        ["reason", reasonText(b.reason)],
      ];
    case "cancel_intent":
      return [
        ["tick", String(b.tick_no)],
        ["known position", fmtQty(b.known_position, inst)],
        ["local bid / ask", touch(b.market, inst)],
        ["reason", reasonText(b.reason)],
      ];
    case "risk_blocked":
      return [
        ["rule", b.rule],
        ["tick", String(b.tick_no)],
        ["detail", b.detail],
      ];
    case "risk_halt":
      return [
        ["rule", b.rule],
        ["detail", b.detail],
      ];
    case "request_sent":
      return [
        ["request", b.req],
        [
          "entry latency (modeled)",
          b.entry_latency_ns < 0
            ? "rejected before reaching the matching engine"
            : fmtDuration(b.entry_latency_ns),
        ],
        [
          "scheduled exchange arrival",
          b.exch_arrival_ts === null ? EM_DASH : fmtClock(b.exch_arrival_ts),
        ],
      ];
    case "request_refused":
      return [
        ["request", b.req],
        ["error", b.error],
      ];
    case "exch_queued":
      return [
        [
          "queue ahead (modeled)",
          b.queue_ahead === null ? "not readable from the queue model" : fmtQty(b.queue_ahead, inst),
        ],
        ["displayed qty at price (observed)", fmtQty(b.level_qty, inst)],
        ["best bid", fmtPrice(b.best_bid_tick, inst)],
        ["best ask", fmtPrice(b.best_ask_tick, inst)],
      ];
    case "queue_update": {
      const cause: EventField =
        b.cause.type === "trade"
          ? ["trade qty (observed)", fmtQty(b.cause.qty, inst)]
          : b.cause.type === "fill_reset"
            ? ["cause (modeled)", "remainder moved to the front of the queue after a partial fill"]
            : [
                "displayed qty (observed)",
                `${fmtQty(b.cause.prev_qty, inst)} \u2192 ${fmtQty(b.cause.new_qty, inst)}`,
              ];
      return [
        cause,
        [
          "queue ahead (modeled)",
          `${fmtQty(b.ahead_before, inst)} \u2192 ${fmtQty(b.ahead_after, inst)}`,
        ],
      ];
    }
    case "exch_fill":
      return [
        ["price", fmtPrice(b.price_tick, inst)],
        ["qty", fmtQty(b.qty, inst)],
        ["liquidity", b.maker ? "maker (filled while resting)" : "taker (took liquidity on arrival)"],
        ["fee", fmtNum(b.fee, 6)],
        ["value", fmtNum(b.value, 4)],
        ["leaves", fmtQty(b.leaves_qty, inst)],
      ];
    case "exch_response":
      return [
        ["status", words(b.status)],
        ["request rejected", b.request_rejected ? "yes" : "no"],
        ["leaves", fmtQty(b.leaves_qty, inst)],
        ["response latency (modeled)", fmtDuration(b.response_latency_ns)],
        ["scheduled local arrival", fmtClock(b.local_arrival_ts)],
      ];
    case "local_response":
      return [
        ["status", words(b.status)],
        ["request rejected", b.request_rejected ? "yes" : "no"],
        ["exec qty", fmtQty(b.exec_qty, inst)],
        ["exec price", b.exec_qty > 0 ? fmtPrice(b.exec_price_tick, inst) : EM_DASH],
        ["leaves", fmtQty(b.leaves_qty, inst)],
        ["produced at exchange", fmtClock(b.exch_ts)],
      ];
    case "queue_truncated":
      return [["small updates not kept after this point", String(b.dropped)]];
    default:
      return [["raw", JSON.stringify(b)]];
  }
}

/** One-line rendering for dense tables. */
export function eventSummary(ev: FlightEvent, inst: InstrumentMeta): string {
  return eventFields(ev, inst)
    .map(([label, value]) => `${label}: ${value}`)
    .join("  \u00b7  ");
}
