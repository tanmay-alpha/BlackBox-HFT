import { useEffect, useRef, useState } from "react";
import type {
  Explanation,
  FillRecord,
  FlightEvent,
  InstrumentMeta,
  LatencyBreakdown,
  LocalTsProvenance,
  OrderRecord,
} from "../../api/types";
import { MARKOUT_HORIZONS_NS } from "../../api/types";
import { fmtDuration, fmtNum, fmtPrice, fmtQty, fmtSigned } from "../../components/fmt";
import { Badge, Empty } from "../../components/ui";
import { ClockText } from "./evidence";
import { eventFields, eventTitle } from "./eventFormat";
import { EM_DASH, fmtSince } from "./labels";
import type { BadgeKind } from "./labels";

// ---------------------------------------------------------------------------------------------
// Explanation
// ---------------------------------------------------------------------------------------------

export function ExplanationSteps({
  explanation,
  selectedSeq,
  activeStep,
  onStep,
  onSelect,
}: {
  explanation: Explanation;
  selectedSeq: number | null;
  activeStep: number | null;
  onStep: (index: number | null) => void;
  onSelect: (seq: number) => void;
}) {
  return (
    <div className="fx-explain">
      {explanation.steps.length === 0 ? (
        <Empty>The explanation for this order has no steps.</Empty>
      ) : (
        <ol className="fx-steps">
          {explanation.steps.map((step, i) => {
            const active = activeStep === i;
            const citesSelected = selectedSeq !== null && step.event_seqs.includes(selectedSeq);
            return (
              <li
                key={i}
                className={`fx-step${active ? " is-active" : ""}${citesSelected ? " is-citing" : ""}`}
              >
                <button
                  type="button"
                  className="fx-step-main"
                  aria-pressed={active}
                  title="Highlight the tape events this step rests on"
                  onClick={() => onStep(active ? null : i)}
                >
                  <span className="fx-step-no mono">{i + 1}</span>
                  <Badge kind={step.basis}>{step.basis}</Badge>
                  <span className="fx-step-text">{step.statement}</span>
                </button>
                <div className="fx-step-cites">
                  {step.event_seqs.length === 0 ? (
                    <span className="dim">rests on no tape event</span>
                  ) : (
                    <>
                      <span className="dim">rests on</span>
                      {step.event_seqs.map((seq) => (
                        <button
                          key={seq}
                          type="button"
                          className={`fx-chip mono${seq === selectedSeq ? " is-selected" : ""}`}
                          onClick={() => onSelect(seq)}
                        >
                          #{seq}
                        </button>
                      ))}
                    </>
                  )}
                </div>
              </li>
            );
          })}
        </ol>
      )}
      <section className="fx-caveats" aria-label="Caveats">
        <h3>Limits of this explanation</h3>
        {explanation.caveats.length === 0 ? (
          <p className="dim">The explanation lists no caveats.</p>
        ) : (
          <ul>
            {explanation.caveats.map((caveat, i) => (
              <li key={i}>{caveat}</li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Latency breakdown
// ---------------------------------------------------------------------------------------------

interface LatencyRow {
  label: string;
  value: number | null;
  clock: string;
  kind: BadgeKind;
  nature: string;
  note: string;
}

function feedLatencyNature(localTs: LocalTsProvenance | null): Pick<LatencyRow, "kind" | "nature" | "note"> {
  switch (localTs) {
    case "measured":
      return {
        kind: "observed",
        nature: "measured",
        note: "Recorded by the capturing host: a measurement at that host's location, which is generally not where a strategy would run.",
      };
    case "synthetic":
      return {
        kind: "neutral",
        nature: "synthetic",
        note: "Produced by the synthetic data generator from a configured latency. Not a measurement.",
      };
    case "assumed":
      return {
        kind: "warn",
        nature: "assumed",
        note: "The source had no receipt timestamps; they were set from an assumed latency. An assumption, not an observation.",
      };
    default:
      return {
        kind: "neutral",
        nature: "provenance not loaded",
        note: "The dataset manifest is not loaded, so this value is not labelled here as measured, synthetic or assumed.",
      };
  }
}

export function LatencyTable({
  latency,
  localTs,
  feedOffsetNs,
}: {
  latency: LatencyBreakdown;
  /** Provenance of the dataset's local timestamps; null when the manifest is not available. */
  localTs: LocalTsProvenance | null;
  feedOffsetNs: number | null;
}) {
  const feed = feedLatencyNature(localTs);
  const offsetNote =
    feedOffsetNs !== null && feedOffsetNs !== 0
      ? ` This run's configuration adds a modeled offset of ${fmtDuration(feedOffsetNs)} to every feed event's local receipt time.`
      : "";
  const rows: LatencyRow[] = [
    {
      label: "Feed staleness at decision",
      value: latency.feed_staleness_ns,
      clock: "local",
      kind: "derived",
      nature: "derived",
      note: "Decision time minus the local receipt time of the last feed event.",
    },
    {
      label: "Feed latency of that event",
      value: latency.feed_latency_ns,
      clock: "exchange \u2192 local",
      kind: feed.kind,
      nature: feed.nature,
      note: `Its local receipt time minus its exchange time. ${feed.note}${offsetNote}`,
    },
    {
      label: "Order entry latency",
      value: latency.entry_latency_ns,
      clock: "local \u2192 exchange",
      kind: "modeled",
      nature: "modeled",
      note: "Drawn by the configured order latency model for the new-order request.",
    },
    {
      label: "Response latency (first response)",
      value: latency.response_latency_ns,
      clock: "exchange \u2192 local",
      kind: "modeled",
      nature: "modeled",
      note: "Drawn by the configured order latency model for the first response.",
    },
    {
      label: "Arrival to first simulated fill",
      value: latency.time_to_first_fill_ns,
      clock: "exchange",
      kind: "derived",
      nature: "derived",
      note: "Difference of two simulated exchange-clock events.",
    },
    {
      label: "Arrival to terminal state",
      value: latency.resting_time_ns,
      clock: "exchange",
      kind: "derived",
      nature: "derived",
      note: "Difference of two simulated exchange-clock events.",
    },
    {
      label: "Cancel entry latency",
      value: latency.cancel_entry_latency_ns,
      clock: "local \u2192 exchange",
      kind: "modeled",
      nature: "modeled",
      note: "Drawn by the configured order latency model for the cancel request.",
    },
  ];

  return (
    <table className="table fx-latency">
      <caption className="fx-caption">
        A dash means one endpoint of the interval never occurred for this order.
      </caption>
      <thead>
        <tr>
          <th scope="col">Interval</th>
          <th scope="col" className="fx-num">
            Duration
          </th>
          <th scope="col">Clock</th>
          <th scope="col">Nature</th>
          <th scope="col">Meaning</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.label}>
            <th scope="row">{row.label}</th>
            <td className="mono fx-num">{fmtDuration(row.value)}</td>
            <td className="dim">{row.clock}</td>
            <td>
              <Badge kind={row.kind}>{row.nature}</Badge>
            </td>
            <td className="dim wrap">{row.note}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

// ---------------------------------------------------------------------------------------------
// Decision context
// ---------------------------------------------------------------------------------------------

export function DecisionContext({
  events,
  order,
  instrument,
}: {
  events: FlightEvent[];
  order: OrderRecord;
  instrument: InstrumentMeta;
}) {
  let intentEvent: FlightEvent | null = null;
  for (const ev of events) {
    if (ev.body.type === "order_intent") {
      intentEvent = ev;
      break;
    }
  }
  if (intentEvent === null || intentEvent.body.type !== "order_intent") {
    return <Empty>The tape holds no order_intent event for this order.</Empty>;
  }
  const decisionTs = intentEvent.ts;
  const { market, known_position: knownPosition, reason } = intentEvent.body;
  const bid = market.best_bid_tick;
  const ask = market.best_ask_tick;
  const spread = bid !== null && ask !== null ? ask - bid : null;
  const mid = bid !== null && ask !== null ? (bid + ask) / 2 : null;
  const depth = market.best_bid_qty + market.best_ask_qty;
  const imbalance =
    bid !== null && ask !== null && depth > 0
      ? (market.best_bid_qty - market.best_ask_qty) / depth
      : null;
  const sameSideBest = order.side === "buy" ? bid : ask;
  // Positive = more aggressive than the same-side best, as in ScriptedPrice::FromTouch.
  const fromTouch =
    sameSideBest === null
      ? null
      : order.side === "buy"
        ? order.price_tick - sameSideBest
        : sameSideBest - order.price_tick;

  return (
    <div className="fx-context">
      <p className="dim">
        What the strategy host had received when it decided. Market data still in transit at that
        moment is not included.
      </p>
      <table className="table">
        <tbody>
          <tr>
            <th scope="row">Decision time (local clock)</th>
            <td>
              <ClockText ns={decisionTs} />{" "}
              <span className="dim">
                tick {order.tick_no}, slot {order.slot}
              </span>
            </td>
          </tr>
          <tr>
            <th scope="row">Best bid known locally</th>
            <td className="mono">
              {bid === null
                ? "no bid in the local book"
                : `${fmtPrice(bid, instrument)} x ${fmtQty(market.best_bid_qty, instrument)}`}
            </td>
          </tr>
          <tr>
            <th scope="row">Best ask known locally</th>
            <td className="mono">
              {ask === null
                ? "no ask in the local book"
                : `${fmtPrice(ask, instrument)} x ${fmtQty(market.best_ask_qty, instrument)}`}
            </td>
          </tr>
          <tr>
            <th scope="row">Spread</th>
            <td className="mono">
              {spread === null
                ? EM_DASH
                : `${spread} ticks (${fmtPrice(spread, instrument)})`}
            </td>
          </tr>
          <tr>
            <th scope="row">Mid</th>
            <td className="mono">{fmtPrice(mid, instrument)}</td>
          </tr>
          <tr>
            <th scope="row">Top-of-book imbalance</th>
            <td className="mono">
              {fmtSigned(imbalance, 3)}{" "}
              <span className="dim">(bid qty - ask qty) / (bid qty + ask qty)</span>
            </td>
          </tr>
          <tr>
            <th scope="row">Order price vs same-side best</th>
            <td className="mono">
              {fromTouch === null ? EM_DASH : `${fromTouch > 0 ? "+" : ""}${fromTouch} ticks`}{" "}
              <span className="dim">positive is more aggressive</span>
            </td>
          </tr>
          <tr>
            <th scope="row">Last feed event, exchange time</th>
            <td>
              <ClockText ns={market.feed_exch_ts} />
            </td>
          </tr>
          <tr>
            <th scope="row">Last feed event, local receipt</th>
            <td>
              <ClockText ns={market.feed_local_ts} />{" "}
              <span className="dim mono">
                {market.feed_local_ts === null
                  ? ""
                  : `${fmtDuration(decisionTs - market.feed_local_ts)} before the decision`}
              </span>
            </td>
          </tr>
          <tr>
            <th scope="row">Known position</th>
            <td className="mono">
              {fmtQty(knownPosition, instrument)}{" "}
              <span className="dim">from fill notifications received by then</span>
            </td>
          </tr>
          <tr>
            <th scope="row">Strategy's stated reason</th>
            <td className="wrap">{reason}</td>
          </tr>
        </tbody>
      </table>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Fills
// ---------------------------------------------------------------------------------------------

export function FillsTable({
  fills,
  instrument,
  originNs,
  selectedSeq,
  onSelect,
}: {
  fills: FillRecord[];
  instrument: InstrumentMeta;
  originNs: number;
  selectedSeq: number | null;
  onSelect: (seq: number) => void;
}) {
  if (fills.length === 0) {
    return <Empty>The simulation produced no fill for this order.</Empty>;
  }
  return (
    <div className="fx-scroll-x">
      <table className="table fx-fills">
        <caption className="fx-caption">
          Simulated executions, not exchange executions. Mid at fill and markouts are{" "}
          <Badge kind="derived">derived</Badge> from the dataset after the run; the strategy never
          saw them. A markout is side-signed (mid at fill time plus the horizon, minus the fill
          price) in ticks; a dash means the dataset ended before the horizon or the book was not
          valid then.
        </caption>
        <thead>
          <tr>
            <th scope="col">Seq</th>
            <th scope="col">Exchange time</th>
            <th scope="col" className="fx-num">
              Price
            </th>
            <th scope="col" className="fx-num">
              Qty
            </th>
            <th scope="col">Liquidity</th>
            <th scope="col" className="fx-num">
              Fee
            </th>
            <th scope="col" className="fx-num">
              Position after
            </th>
            <th scope="col" className="fx-num">
              Realized P&amp;L
            </th>
            <th scope="col" className="fx-num fx-derived-col">
              Mid at fill
            </th>
            {MARKOUT_HORIZONS_NS.map((h) => (
              <th key={h} scope="col" className="fx-num fx-derived-col">
                Markout +{fmtDuration(h)}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {fills.map((f) => (
            <tr
              key={f.seq}
              className={f.seq === selectedSeq ? "is-selected" : undefined}
              onClick={() => onSelect(f.seq)}
            >
              <td>
                <button
                  type="button"
                  className="fx-chip mono"
                  aria-pressed={f.seq === selectedSeq}
                  aria-label={`Select fill event ${f.seq}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    onSelect(f.seq);
                  }}
                >
                  #{f.seq}
                </button>
              </td>
              <td>
                <ClockText ns={f.ts} />{" "}
                <span className="dim mono">{fmtSince(f.ts, originNs)}</span>
              </td>
              <td className="mono fx-num">{fmtPrice(f.price_tick, instrument)}</td>
              <td className="mono fx-num">{fmtQty(f.qty, instrument)}</td>
              <td>{f.maker ? "maker" : "taker"}</td>
              <td className="mono fx-num">{fmtNum(f.fee, 6)}</td>
              <td className="mono fx-num">{fmtQty(f.position_after, instrument)}</td>
              <td className="mono fx-num">{fmtSigned(f.realized_pnl, 4)}</td>
              <td className="mono fx-num fx-derived-col">{fmtPrice(f.mid_tick, instrument)}</td>
              {MARKOUT_HORIZONS_NS.map((h, i) => {
                const v = f.markout_ticks[i];
                return (
                  <td key={h} className="mono fx-num fx-derived-col">
                    {fmtSigned(v === undefined ? null : v, 2)}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Raw event list
// ---------------------------------------------------------------------------------------------

export function EventList({
  events,
  instrument,
  originNs,
  selectedSeq,
  citedSeqs,
  onSelect,
}: {
  events: FlightEvent[];
  instrument: InstrumentMeta;
  originNs: number;
  selectedSeq: number | null;
  citedSeqs: ReadonlySet<number>;
  onSelect: (seq: number | null) => void;
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [showQueue, setShowQueue] = useState(true);
  const queueCount = events.reduce((n, ev) => n + (ev.body.type === "queue_update" ? 1 : 0), 0);
  const shown = showQueue ? events : events.filter((ev) => ev.body.type !== "queue_update");

  // Scroll inside the list only: `scrollIntoView` would also scroll the page away from the
  // timeline the user just clicked.
  useEffect(() => {
    const box = scrollRef.current;
    if (!box || selectedSeq === null) return;
    const row = box.querySelector<HTMLElement>(`tr[data-seq="${selectedSeq}"]`);
    if (!row) return;
    const boxRect = box.getBoundingClientRect();
    const rowRect = row.getBoundingClientRect();
    if (rowRect.top < boxRect.top + 32 || rowRect.bottom > boxRect.bottom) {
      box.scrollTop += rowRect.top - boxRect.top - box.clientHeight / 2 + rowRect.height / 2;
    }
  }, [selectedSeq, showQueue]);

  if (events.length === 0) {
    return <Empty>The tape holds no event for this order.</Empty>;
  }

  return (
    <div className="fx-events">
      <div className="fx-events-bar">
        <span className="dim">
          {events.length} events in tape order. Time is relative to the dataset origin; the offset
          is from the decision.
        </span>
        <label className="fx-check">
          <input
            type="checkbox"
            checked={showQueue}
            onChange={(e) => setShowQueue(e.target.checked)}
          />
          show queue updates ({queueCount})
        </label>
      </div>
      <div className="fx-events-scroll" ref={scrollRef}>
        <table className="table fx-events-table">
          <thead>
            <tr>
              <th scope="col">Seq</th>
              <th scope="col">Clock</th>
              <th scope="col">Source</th>
              <th scope="col">Time</th>
              <th scope="col" className="fx-num">
                From decision
              </th>
              <th scope="col">Type</th>
              <th scope="col">Fields</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((ev) => {
              const selected = ev.seq === selectedSeq;
              const cited = citedSeqs.has(ev.seq);
              return (
                <tr
                  key={ev.seq}
                  data-seq={ev.seq}
                  className={`${selected ? "is-selected" : ""}${cited ? " is-cited" : ""}`}
                  onClick={() => onSelect(selected ? null : ev.seq)}
                >
                  <td>
                    <button
                      type="button"
                      className="fx-chip mono"
                      aria-pressed={selected}
                      aria-label={`Select event ${ev.seq}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        onSelect(selected ? null : ev.seq);
                      }}
                    >
                      #{ev.seq}
                    </button>
                  </td>
                  <td>
                    <span className="fx-clock-tag" data-clock={ev.clock}>
                      {ev.clock}
                    </span>
                  </td>
                  <td>
                    <Badge kind={ev.source}>{ev.source}</Badge>
                  </td>
                  <td>
                    <ClockText ns={ev.ts} />
                  </td>
                  <td className="mono fx-num">{fmtSince(ev.ts, originNs)}</td>
                  <td className="mono" title={eventTitle(ev)}>
                    {ev.body.type}
                  </td>
                  <td className="fx-kvs">
                    {eventFields(ev, instrument).map(([label, value]) => (
                      <span key={label} className="fx-kv">
                        <span className="dim">{label}</span> <span className="mono">{value}</span>
                      </span>
                    ))}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
