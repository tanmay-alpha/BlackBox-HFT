// The order's lifecycle as a readable sequence of stages: what the strategy decided, what it sent,
// when the request reached the simulated exchange, how the modeled queue moved, how the order
// ended there and when the strategy heard back. Each stage keeps its clock and its raw recorder
// evidence one click away.

import { Fragment } from "react";

import type { Basis, FlightEvent, InstrumentMeta } from "../../api/types";
import { fmtQty } from "../../components/fmt";
import { Badge, BasisBadge, ClockText } from "../../components/ui";
import { eventFields, eventTitle } from "./eventFormat";
import { fmtSince, words } from "./labels";

type Stage = "decision" | "submission" | "arrival" | "queue" | "outcome" | "response" | "risk" | "recorder";

const STAGE_LABEL: Record<Stage, string> = {
  decision: "Strategy decision",
  submission: "Submission",
  arrival: "Arrival at the simulated exchange",
  queue: "Queue behaviour",
  outcome: "Outcome at the exchange",
  response: "Response to the strategy",
  risk: "Risk control",
  recorder: "Recorder note",
};

/** What a stage's content rests on: the strategy and engine are simulated; queue moves come
 *  from observed trades and depth through a modeled queue. */
const STAGE_BASIS: Record<Stage, Basis> = {
  decision: "modeled",
  submission: "modeled",
  arrival: "modeled",
  queue: "modeled",
  outcome: "modeled",
  response: "modeled",
  risk: "modeled",
  recorder: "derived",
};

function stageOf(ev: FlightEvent): Stage {
  switch (ev.body.type) {
    case "order_intent":
    case "cancel_intent":
      return "decision";
    case "request_sent":
    case "request_refused":
      return "submission";
    case "exch_queued":
      return "arrival";
    case "queue_update":
      return "queue";
    case "exch_fill":
      return "outcome";
    case "exch_response":
      // An accepted new order's acknowledgement belongs to its arrival, not to how it ended.
      return ev.body.status === "new" && !ev.body.request_rejected ? "arrival" : "outcome";
    case "local_response":
      return "response";
    case "risk_blocked":
    case "risk_halt":
      return "risk";
    default:
      return "recorder";
  }
}

interface Step {
  stage: Stage;
  events: FlightEvent[];
}

/** Tape order, with consecutive queue updates folded into one step. */
function steps(events: FlightEvent[]): Step[] {
  const out: Step[] = [];
  for (const ev of [...events].sort((a, b) => a.seq - b.seq)) {
    const stage = stageOf(ev);
    const last = out[out.length - 1];
    if (stage === "queue" && last && last.stage === "queue") last.events.push(ev);
    else out.push({ stage, events: [ev] });
  }
  return out;
}

function headline(step: Step, inst: InstrumentMeta): string {
  const ev = step.events[0];
  const b = ev.body;
  if (step.stage === "queue") {
    const ups = step.events.flatMap((e) => (e.body.type === "queue_update" ? [e.body] : []));
    const first = ups[0];
    const last = ups[ups.length - 1];
    const trades = ups.filter((u) => u.cause.type === "trade").length;
    const range = first && last ? `; signed queue estimate ${fmtQty(first.ahead_before, inst)} \u2192 ${fmtQty(last.ahead_after, inst)}` : "";
    return `${ups.length} queue update${ups.length === 1 ? "" : "s"} (${trades} from market trades at the price)${range}`;
  }
  if (b.type === "request_sent") {
    return b.req === "cancel" ? "Cancel request sent toward the exchange" : `${words(b.req)} order request sent toward the exchange`.replace(/^./, (c) => c.toUpperCase());
  }
  if (b.type === "exch_response") {
    if (b.request_rejected) return "Simulated exchange rejected the request";
    if (b.status === "canceled") return "Cancelled at the simulated exchange";
    if (b.status === "filled") return "Order complete at the simulated exchange";
    if (b.status === "new") return "Exchange acknowledged: order resting (new)";
    return `Exchange status: ${words(b.status)}`;
  }
  if (b.type === "local_response") {
    if (b.request_rejected) return "Strategy learned of the rejection";
    return b.status === "new" ? "Strategy learned the order is resting" : `Strategy learned status: ${words(b.status)}`;
  }
  return eventTitle(ev);
}

export function Lifecycle({
  events,
  instrument,
  originNs,
  selectedSeq,
  onSelect,
}: {
  events: FlightEvent[];
  instrument: InstrumentMeta;
  originNs: number;
  selectedSeq: number | null;
  onSelect: (seq: number) => void;
}) {
  const list = steps(events);
  if (list.length === 0) return <p className="dim">The tape holds no event for this order.</p>;
  return (
    <ol className="fx-life" aria-label="Order lifecycle">
      {list.map((step, i) => {
        const ev = step.events[0];
        const lastEv = step.events[step.events.length - 1];
        const selected = step.events.some((e) => e.seq === selectedSeq);
        return (
          <li
            key={ev.seq}
            className={`fx-life-step fx-life-${step.stage}${selected ? " is-selected" : ""}`}
            data-clock={ev.clock}
          >
            <span className="fx-life-n" aria-hidden="true">
              {i + 1}
            </span>
            <div className="fx-life-body">
              <div className="fx-life-head">
                <span className="fx-life-stage">{STAGE_LABEL[step.stage]}</span>
                <span className="fx-clock-tag" data-clock={ev.clock}>
                  {ev.clock} clock
                </span>
                <BasisBadge basis={STAGE_BASIS[step.stage]} />
                <Badge kind={ev.source}>{ev.source}</Badge>
              </div>
              <div className="fx-life-line">
                <button type="button" className="fx-life-title btn-link" onClick={() => onSelect(ev.seq)} aria-pressed={selected}>
                  {headline(step, instrument)}
                </button>
                <span className="fx-life-time">
                  <ClockText ns={ev.ts} /> <span className="dim">(<span className="mono">{fmtSince(ev.ts, originNs)}</span>
                  {lastEv !== ev ? (
                    <>
                      {" "}
                      to <span className="mono">{fmtSince(lastEv.ts, originNs)}</span>
                    </>
                  ) : null}{" "}
                  from decision)</span>
                </span>
              </div>
              <details className="fx-life-raw">
                <summary>
                  Raw evidence ({step.events.length} event{step.events.length === 1 ? "" : "s"})
                </summary>
                {step.events.map((e) => (
                  <dl key={e.seq} className="kv fx-life-kv">
                    <dt>event</dt>
                    <dd>
                      <span className="mono">#{e.seq}</span> {e.body.type} <span className="dim">({e.clock} clock)</span>
                    </dd>
                    {eventFields(e, instrument).map(([label, value]) => (
                      <Fragment key={label}>
                        <dt>{label}</dt>
                        <dd className="mono">{value}</dd>
                      </Fragment>
                    ))}
                  </dl>
                ))}
              </details>
            </div>
          </li>
        );
      })}
    </ol>
  );
}
