import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { ApiError, api } from "../../api/client";
import type {
  DatasetManifest,
  ExecutionConfig,
  FlightEvent,
  OrderDetail,
  Question,
} from "../../api/types";
import { fmtDuration, fmtNum, fmtPrice, fmtQty } from "../../components/fmt";
import { Badge, Empty, ErrorBox, Panel, Spinner, Stat, useAsync } from "../../components/ui";
import { ClockText, EvidenceLegend } from "./evidence";
import { eventFields, eventTitle } from "./eventFormat";
import {
  OUTCOME_MEANING,
  fmtSince,
  outcomeKind,
  queueModelLabel,
  queueModelMeaning,
  words,
} from "./labels";
import { findNeighbours } from "./neighbours";
import type { Neighbours } from "./neighbours";
import { DecisionContext, EventList, ExplanationSteps, FillsTable, LatencyTable } from "./panels";
import { QueueChart } from "./QueueChart";
import { TwoClockTimeline } from "./TwoClockTimeline";
import type { AxisMode } from "./timeline";
import "./forensics.css";

const QUESTION_TEXT: Record<Question, string> = {
  why_filled: "Why did this order fill?",
  why_not_filled: "Why did this order not fill?",
  why_partially_filled: "Why did this order only partly fill?",
};

export default function OrderForensicsPage() {
  const { runId = "", orderId = "" } = useParams();
  const parsed = /^\d+$/.test(orderId) ? Number(orderId) : NaN;
  if (runId === "" || !Number.isSafeInteger(parsed)) {
    return (
      <div className="fx-page">
        <Empty>
          This address does not name an order. Order ids are non-negative integers.{" "}
          <Link to="/runs">Back to runs</Link>
        </Empty>
      </div>
    );
  }
  // Keyed so selection and zoom never carry over from one order to the next.
  return <OrderForensics key={`${runId}/${parsed}`} runId={runId} orderId={parsed} />;
}

function OrderForensics({ runId, orderId }: { runId: string; orderId: number }) {
  const detail = useAsync(() => api.orderDetail(runId, orderId), [runId, orderId]);
  const neighbours = useAsync(() => findNeighbours(runId, orderId), [runId, orderId]);

  const data = detail.data && detail.data.order.order_id === orderId ? detail.data : null;

  return (
    <div className="fx-page">
      <nav className="fx-crumbs" aria-label="Breadcrumb">
        <Link to="/runs">Runs</Link>
        <span aria-hidden="true">/</span>
        <Link to={`/runs/${runId}`}>
          Run <span className="mono">{runId}</span>
        </Link>
        <span aria-hidden="true">/</span>
        <span aria-current="page">
          Order <span className="mono">{orderId}</span>
        </span>
        <OrderNav runId={runId} neighbours={neighbours.data} loading={neighbours.loading} />
      </nav>

      {data ? (
        <OrderView runId={runId} detail={data} />
      ) : detail.error ? (
        detail.error instanceof ApiError && detail.error.status === 404 ? (
          <Empty>
            Run <span className="mono">{runId}</span> has no order{" "}
            <span className="mono">{orderId}</span>, or the run no longer exists.{" "}
            <Link to={`/runs/${runId}`}>Open the run</Link>
          </Empty>
        ) : (
          <div className="col">
            <ErrorBox error={detail.error} />
            <div>
              <button type="button" className="fx-btn" onClick={detail.reload}>
                Retry
              </button>
            </div>
          </div>
        )
      ) : (
        <Spinner />
      )}
    </div>
  );
}

function OrderNav({
  runId,
  neighbours,
  loading,
}: {
  runId: string;
  neighbours: Neighbours | null;
  loading: boolean;
}) {
  const link = (id: number | null, label: string) =>
    id === null ? (
      <span className="fx-btn is-disabled" aria-disabled="true">
        {label}
      </span>
    ) : (
      <Link className="fx-btn" to={`/runs/${runId}/orders/${id}`}>
        {label} <span className="mono dim">{id}</span>
      </Link>
    );
  return (
    <span className="fx-nav">
      {neighbours ? (
        <>
          {neighbours.position !== null ? (
            <span className="dim mono">
              {neighbours.position} of {neighbours.total}
            </span>
          ) : null}
          {link(neighbours.prev, "\u2190 Previous order")}
          {link(neighbours.next, "Next order \u2192")}
        </>
      ) : loading ? (
        <span className="dim">locating neighbouring orders</span>
      ) : (
        <span className="dim">neighbouring orders unavailable</span>
      )}
    </span>
  );
}

function ExecutionAssumptions({ execution }: { execution: ExecutionConfig }) {
  const latency = execution.order_latency;
  const bps = (fraction: number) => `${fmtNum(fraction * 1e4, 2)} bps`;
  return (
    <dl className="fx-assumptions">
      <div>
        <dt>Exchange model</dt>
        <dd className="mono">{execution.exchange_model}</dd>
      </div>
      <div>
        <dt>Queue model</dt>
        <dd className="mono">{queueModelLabel(execution.queue_model)}</dd>
      </div>
      <div>
        <dt>Order latency</dt>
        <dd className="mono">
          entry {fmtDuration(latency.entry_ns)}, response {fmtDuration(latency.response_ns)}
          {latency.type === "jitter" ? `, jitter up to ${fmtDuration(latency.jitter_ns)}` : ""}
        </dd>
      </div>
      <div>
        <dt>Feed latency offset</dt>
        <dd className="mono">{fmtDuration(execution.feed_latency_offset_ns)}</dd>
      </div>
      <div>
        <dt>Fees (maker / taker)</dt>
        <dd className="mono">
          {bps(execution.fees.maker)} / {bps(execution.fees.taker)}
        </dd>
      </div>
    </dl>
  );
}

function OrderView({ runId, detail }: { runId: string; detail: OrderDetail }) {
  const { order, events, fills, explanation, instrument } = detail;
  const run = useAsync(() => api.run(runId), [runId]);
  const datasetId = run.data ? run.data.manifest.dataset.id : null;
  // Only the timestamp provenance is needed, to label feed latency as measured or not.
  const dataset = useAsync<DatasetManifest | null>(
    () => (datasetId === null ? Promise.resolve(null) : api.dataset(datasetId)),
    [datasetId],
  );

  const [mode, setMode] = useState<AxisMode>("linear");
  const [selectedSeq, setSelectedSeq] = useState<number | null>(null);
  const [activeStep, setActiveStep] = useState<number | null>(null);

  const citedSeqs = useMemo<ReadonlySet<number>>(() => {
    const step = activeStep === null ? undefined : explanation.steps[activeStep];
    return new Set<number>(step ? step.event_seqs : []);
  }, [activeStep, explanation]);

  const originNs = order.decision_ts;
  const execution = run.data ? run.data.manifest.config.execution : null;
  const selectedEvent: FlightEvent | undefined =
    selectedSeq === null ? undefined : events.find((e) => e.seq === selectedSeq);
  const citingSteps = explanation.steps.flatMap((step, i) =>
    selectedSeq !== null && step.event_seqs.includes(selectedSeq) ? [i + 1] : [],
  );
  const truncations = events.flatMap((ev) =>
    ev.body.type === "queue_truncated" ? [ev.body.dropped] : [],
  );

  return (
    <>
      <header className="fx-header">
        <div className="fx-order-line">
          <Badge kind={order.side}>{order.side}</Badge>
          <span className="mono fx-order-size">
            {fmtQty(order.qty, instrument)} @ {fmtPrice(order.price_tick, instrument)}
          </span>
          <span className="mono dim">
            {order.tif} {order.ord_type}
          </span>
          <span className="dim">simulated outcome</span>
          <Badge kind={outcomeKind(order.outcome)}>{words(order.outcome)}</Badge>
          <span className="dim">{OUTCOME_MEANING[order.outcome]}</span>
        </div>
        <h1 className="fx-question">{QUESTION_TEXT[explanation.question]}</h1>
        <p className="fx-headline">{explanation.headline}</p>
        <p className="dim fx-verdict">
          Verdict <code className="mono">{explanation.verdict}</code>. Rule-based: assembled
          deterministically from the tape events below. This order was simulated on historical
          replay; it was never sent to an exchange and did not affect the historical prices.
        </p>
        <div className="fx-stats">
          <Stat
            label="Filled (simulated)"
            value={`${fmtQty(order.filled_qty, instrument)} of ${fmtQty(order.qty, instrument)}`}
            hint={`maker ${fmtQty(order.maker_qty, instrument)}, taker ${fmtQty(order.taker_qty, instrument)}, ${order.fill_count} fill${order.fill_count === 1 ? "" : "s"}`}
          />
          <Stat
            label="Average fill price"
            value={fmtPrice(order.avg_fill_tick, instrument)}
            hint="quantity-weighted"
          />
          <Stat
            label="Queue ahead at entry"
            value={fmtQty(order.queue_ahead_at_entry, instrument)}
            hint="modeled"
          />
          <Stat
            label="Smallest queue ahead"
            value={fmtQty(order.min_queue_ahead, instrument)}
            hint="modeled"
          />
          <Stat
            label="Traded at price while resting"
            value={fmtQty(order.traded_at_price_qty, instrument)}
            hint="observed market trades"
          />
          <Stat label="Fees" value={fmtNum(order.fees, 6)} hint="negative is a rebate" />
          <Stat
            label="Last exchange status"
            value={words(order.final_status)}
            hint={
              order.terminal_ts === null
                ? "not terminal at the exchange"
                : `terminal ${fmtSince(order.terminal_ts, originNs)} after decision`
            }
          />
          {order.cancel_sent_ts !== null ? (
            <Stat
              label="Cancel sent"
              value={`${fmtSince(order.cancel_sent_ts, originNs)} after decision`}
              hint={`${order.cancel_reason ?? "no reason recorded"}${order.cancel_rejected ? "; rejected, the order was already gone" : ""}`}
            />
          ) : null}
        </div>
      </header>

      <EvidenceLegend sources />

      <Panel title="Run context: modeled execution assumptions">
        {execution ? (
          <ExecutionAssumptions execution={execution} />
        ) : run.error ? (
          <ErrorBox error={run.error} />
        ) : (
          <Spinner />
        )}
      </Panel>

      <Panel
        title="Two-clock timeline"
        actions={
          <span className="fx-toggle" role="group" aria-label="Time axis">
            <button
              type="button"
              className="fx-btn"
              aria-pressed={mode === "linear"}
              onClick={() => setMode("linear")}
            >
              Linear time
            </button>
            <button
              type="button"
              className="fx-btn"
              aria-pressed={mode === "ordinal"}
              onClick={() => setMode("ordinal")}
            >
              Event order
            </button>
          </span>
        }
      >
        {events.length === 0 ? (
          <Empty>The tape holds no event for this order.</Empty>
        ) : (
          <TwoClockTimeline
            events={events}
            originNs={originNs}
            instrument={instrument}
            side={order.side}
            mode={mode}
            selectedSeq={selectedSeq}
            citedSeqs={citedSeqs}
            onSelect={setSelectedSeq}
          />
        )}
        <div className="fx-selected" aria-live="polite">
          {selectedEvent ? (
            <>
              <div className="fx-selected-head">
                <span className="mono">#{selectedEvent.seq}</span>
                <Badge kind={selectedEvent.source}>{selectedEvent.source}</Badge>
                <span className="fx-clock-tag" data-clock={selectedEvent.clock}>
                  {selectedEvent.clock} clock
                </span>
                <strong>{eventTitle(selectedEvent)}</strong>
                <ClockText ns={selectedEvent.ts} />
                <span className="mono dim">
                  {fmtSince(selectedEvent.ts, originNs)} from decision
                </span>
                <span className="dim">
                  {citingSteps.length === 0
                    ? "not cited by any explanation step"
                    : `cited by explanation step ${citingSteps.join(", ")}`}
                </span>
              </div>
              <div className="fx-kvs">
                {eventFields(selectedEvent, instrument).map(([label, value]) => (
                  <span key={label} className="fx-kv">
                    <span className="dim">{label}</span> <span className="mono">{value}</span>
                  </span>
                ))}
              </div>
            </>
          ) : (
            <span className="dim">No event selected.</span>
          )}
        </div>
      </Panel>

      <Panel title="Modeled queue position" actions={<Badge kind="modeled">modeled</Badge>}>
        <p className="fx-queue-model">
          {execution ? (
            <>
              Queue model configured for this run:{" "}
              <code className="mono">{queueModelLabel(execution.queue_model)}</code>.{" "}
              <span className="dim">{queueModelMeaning(execution.queue_model)}</span>
            </>
          ) : run.error ? (
            <span className="dim">The run's configured queue model could not be loaded.</span>
          ) : (
            <span className="dim">Loading the run's configured queue model.</span>
          )}{" "}
          <span className="dim">
            Level-2 data does not show individual orders, so the position in the queue is an
            estimate and is never observed.
          </span>
        </p>
        {truncations.map((dropped, i) => (
          <p key={i} className="fx-truncated">
            <Badge kind="recorder">recorder</Badge> The per-order cap on recorded queue updates was
            reached: {dropped} later updates that changed the estimate by less than one lot were
            not kept.
          </p>
        ))}
        <QueueChart
          path={explanation.queue_path}
          events={events}
          originNs={originNs}
          instrument={instrument}
          side={order.side}
          mode={mode}
          selectedSeq={selectedSeq}
          citedSeqs={citedSeqs}
          onSelect={setSelectedSeq}
        />
      </Panel>

      <div className="fx-two">
        <Panel title="Explanation">
          <ExplanationSteps
            explanation={explanation}
            selectedSeq={selectedSeq}
            activeStep={activeStep}
            onStep={setActiveStep}
            onSelect={setSelectedSeq}
          />
        </Panel>
        <div className="col">
          <Panel title="Decision context">
            <DecisionContext events={events} order={order} instrument={instrument} />
          </Panel>
          <Panel title="Latency breakdown">
            <LatencyTable
              latency={explanation.latency}
              localTs={dataset.data ? dataset.data.timestamps.local_ts : null}
              feedOffsetNs={execution ? execution.feed_latency_offset_ns : null}
            />
          </Panel>
        </div>
      </div>

      <Panel title="Simulated fills">
        <FillsTable
          fills={fills}
          instrument={instrument}
          originNs={originNs}
          selectedSeq={selectedSeq}
          onSelect={setSelectedSeq}
        />
      </Panel>

      <Panel title="Flight recorder events">
        <EventList
          events={events}
          instrument={instrument}
          originNs={originNs}
          selectedSeq={selectedSeq}
          citedSeqs={citedSeqs}
          onSelect={setSelectedSeq}
        />
      </Panel>
    </>
  );
}
