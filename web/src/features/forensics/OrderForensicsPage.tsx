import { Fragment, useContext, useMemo, useState, type ReactNode } from "react";
import { Link, useParams } from "react-router-dom";
import { ApiError, api } from "../../api/client";
import type {
  DatasetManifest,
  ExecutionConfig,
  FlightEvent,
  InstrumentMeta,
  OrderDetail,
  Question,
} from "../../api/types";
import { exchangeModelLabel, latencyLabel, queueModelLabel, tifLabel } from "../../components/describe";
import { PaneTabs, usePhone } from "../../components/PaneTabs";
import { SplitPane } from "../../components/SplitPane";
import { Tabs } from "../../components/Tabs";
import { Term } from "../../components/Tooltip";
import { fmtBp, fmtDuration, fmtInt, fmtPnl, fmtPrice, fmtQty } from "../../components/fmt";
import { digitsForUnit, type NsDigits } from "../../components/ns";
import {
  Badge,
  Empty,
  ErrorBox,
  Notice,
  Panel,
  Pill,
  SideBadge,
  Spinner,
  TimeDigitsContext,
  useAsync,
} from "../../components/ui";
import { useInspector } from "../../shell/inspector";
import { paths } from "../../shell/paths";
import { Workspace } from "../../shell/Workspace";
import { ClockText, EvidenceLegend } from "./evidence";
import { eventFields, eventTitle } from "./eventFormat";
import {
  OUTCOME_MEANING,
  VERDICT_TEXT,
  fmtSince,
  outcomeTone,
  queueModelMeaning,
  words,
} from "./labels";
import { findNeighbours } from "./neighbours";
import type { Neighbours } from "./neighbours";
import { DecisionContext, EventList, ExplanationSteps, FillsTable, LatencyTable } from "./panels";
import { EvidenceLedger, LEDGER_HINT } from "./EvidenceLedger";
import { buildLedger, type LedgerTarget } from "./ledger";
import { Lifecycle } from "./Lifecycle";
import { RunContextLinks } from "../runs/RunContextLinks";
import { QueueChart } from "./QueueChart";
import { TwoClockTimeline } from "./TwoClockTimeline";
import type { AxisMode } from "./timeline";
import { useElementSize } from "./useElementSize";
import "./forensics.css";

const QUESTION_TEXT: Record<Question, string> = {
  why_filled: "Why did this order fill?",
  why_not_filled: "Why did this order not fill?",
  why_partially_filled: "Why did this order only partly fill?",
};

type BottomTab = "ledger" | "lifecycle" | "explanation" | "context" | "latency" | "fills" | "events";
/** Panes of the phone layout (below 600 px), one at a time. */
type PhoneTab = "order" | "timeline" | "evidence";
/** The two charts of the docked layout when they share one pane as tabs. */
type ChartTab = "timeline" | "queue";
/** Docked charts pane height (px) that shows the timeline canvas above the queue plot; a shorter
 *  pane shows one chart at a time on tabs instead of squeezing both. */
const BOTH_CHARTS_MIN = 640;

export default function OrderForensicsPage() {
  const { runId = "", orderId = "" } = useParams();
  const parsed = /^\d+$/.test(orderId) ? Number(orderId) : NaN;
  if (runId === "" || !Number.isSafeInteger(parsed)) {
    return (
      <Workspace context={null} scroll>
        <Empty>
          This address does not name an order. Order ids are non-negative integers.{" "}
          <Link to={paths.runs()}>Back to runs</Link>
        </Empty>
      </Workspace>
    );
  }
  // Keyed so selection and zoom never carry over from one order to the next.
  return <OrderForensics key={`${runId}/${parsed}`} runId={runId} orderId={parsed} />;
}

function OrderForensics({ runId, orderId }: { runId: string; orderId: number }) {
  const detail = useAsync((signal) => api.orderDetail(runId, orderId, { signal }), [runId, orderId]);
  const neighbours = useAsync((signal) => findNeighbours(runId, orderId, signal), [runId, orderId]);
  const run = useAsync((signal) => api.run(runId, { signal }), [runId]);
  const runDataset = run.data ? run.data.dataset : null;
  // Timestamp provenance labels feed latency as measured or not; the unit sets clock precision.
  // Both come from the exact dataset version the run used.
  const dataset = useAsync<DatasetManifest | null>(
    (signal) =>
      runDataset === null
        ? Promise.resolve(null)
        : api.datasetVersion(runDataset.id, runDataset.version, { signal }).then((v) => v.manifest),
    [runDataset?.id, runDataset?.version],
  );

  const data = detail.data && detail.data.order.order_id === orderId ? detail.data : null;
  const digits = dataset.data ? digitsForUnit(dataset.data.timestamps.input_unit) : 9;
  const phone = usePhone();

  const blotterLink = (
    <Link className="btn" to={paths.run(runId)} title={`Order blotter of run ${runId}`}>
      Run blotter <span className="mono">{runId.slice(0, 12)}</span>
    </Link>
  );
  const openReplay = (
    <Link
      className="btn btn-primary"
      to={paths.bookRun(runId, { t: data ? data.order.decision_ts : null, order: orderId })}
    >
      Open in Order Book Replay
    </Link>
  );
  const runLinks = <RunContextLinks runId={runId} omit={["overview", "replay"]} />;

  return (
    <TimeDigitsContext.Provider value={digits}>
      <Workspace
        context={`Order ${orderId} \u00b7 run ${runId}`}
        layout
        toolbar={
          phone ? (
            // Phones: the order, the one primary action and order stepping stay in view; the run's
            // other views sit behind one disclosure so the toolbar stays two rows.
            <>
              <span className="mono">Order {orderId}</span>
              {openReplay}
              <OrderNav runId={runId} neighbours={neighbours.data} loading={neighbours.loading} compact />
              <details className="ws-more">
                <summary>Run links</summary>
                <div className="ws-more-body">
                  {neighbours.data && neighbours.data.position !== null ? (
                    <span className="dim mono">
                      {fmtInt(neighbours.data.position)} of {fmtInt(neighbours.data.total)}
                    </span>
                  ) : null}
                  {blotterLink}
                  {runLinks}
                </div>
              </details>
            </>
          ) : (
            <>
              {blotterLink}
              <span className="mono">Order {orderId}</span>
              <OrderNav runId={runId} neighbours={neighbours.data} loading={neighbours.loading} />
              {openReplay}
              {runLinks}
            </>
          )
        }
      >
        {data ? (
          <OrderView
            runId={runId}
            detail={data}
            execution={run.data ? run.data.config.execution : null}
            runError={run.error}
            dataset={dataset.data}
          />
        ) : detail.error ? (
          detail.error instanceof ApiError && detail.error.status === 404 ? (
            <Empty>
              Run <span className="mono">{runId}</span> has no order{" "}
              <span className="mono">{orderId}</span>, or the run no longer exists.{" "}
              <Link to={paths.run(runId)}>Open the run</Link>
            </Empty>
          ) : (
            <ErrorBox error={detail.error} onRetry={detail.reload} />
          )
        ) : (
          <Spinner label="Loading order forensics" />
        )}
      </Workspace>
    </TimeDigitsContext.Provider>
  );
}

/** `compact` (phones): short "Prev" / "Next" labels and no position, which moves into the
 *  toolbar's disclosure; accessible names stay "Previous order <id>" / "Next order <id>". */
function OrderNav({
  runId,
  neighbours,
  loading,
  compact = false,
}: {
  runId: string;
  neighbours: Neighbours | null;
  loading: boolean;
  compact?: boolean;
}) {
  const link = (id: number | null, label: string, short: string) =>
    id === null ? (
      <button type="button" disabled title={`No ${label.toLowerCase()} in this run`}>
        {compact ? short : label}
      </button>
    ) : (
      <Link className="btn" to={paths.order(runId, id)} aria-label={compact ? `${label} ${id}` : undefined}>
        {compact ? short : label} <span className="mono dim">{id}</span>
      </Link>
    );
  if (neighbours) {
    return (
      <>
        {!compact && neighbours.position !== null ? (
          <span className="dim mono">
            {fmtInt(neighbours.position)} of {fmtInt(neighbours.total)}
          </span>
        ) : null}
        {link(neighbours.prev, "Previous order", "Prev")}
        {link(neighbours.next, "Next order", "Next")}
      </>
    );
  }
  return (
    <span className="dim">
      {loading ? "locating neighbouring orders" : "neighbouring orders unavailable"}
    </span>
  );
}

function ExecutionAssumptions({ execution }: { execution: ExecutionConfig }) {
  const latency = execution.order_latency;
  return (
    <dl className="kv">
      <dt>Exchange model</dt>
      <dd className="mono">{exchangeModelLabel(execution.exchange_model)}</dd>
      <dt>Queue model</dt>
      <dd className="mono">{queueModelLabel(execution.queue_model)}</dd>
      <dt>
        Order <Term term="latency">latency</Term>
      </dt>
      <dd className="mono">
        {latency.type === "constant" || latency.type === "jitter"
          ? `entry ${fmtDuration(latency.entry_ns)}, response ${fmtDuration(latency.response_ns)}${
              latency.type === "jitter" ? `, jitter up to ${fmtDuration(latency.jitter_ns)}` : ""
            }`
          : `${latency.type}: ${latencyLabel(latency)}`}
      </dd>
      <dt>
        <Term term="feed_latency">Feed latency</Term> offset
      </dt>
      <dd className="mono">{fmtDuration(execution.feed_latency_offset_ns)}</dd>
      <dt>
        <Term term="fees">Fees</Term> (maker / taker)
      </dt>
      <dd className="mono">
        {fmtBp(execution.fees.maker * 1e4)} / {fmtBp(execution.fees.taker * 1e4)}
      </dd>
    </dl>
  );
}

function OrderContext({
  detail,
  execution,
  runError,
  ledger,
}: {
  detail: OrderDetail;
  execution: ExecutionConfig | null;
  runError: Error | null;
  /** The phone layout shows the evidence ledger here, right under the verdict. */
  ledger?: ReactNode;
}) {
  const { order, explanation, instrument } = detail;
  const originNs = order.decision_ts;
  return (
    <Panel title={`Order ${order.order_id}`} basis="modeled" fill>
      <div className="col fx-context-body">
        <div className="row fx-order-line">
          <SideBadge side={order.side} />
          <span className="mono fx-order-size">
            {fmtQty(order.qty, instrument)} @ {fmtPrice(order.price_tick, instrument)}
          </span>
          <span className="mono dim">
            {tifLabel(order.tif)} {order.ord_type}
          </span>
        </div>
        <div className="row">
          <span className="dim">simulated outcome</span>
          <Pill tone={outcomeTone(order.outcome)} title={OUTCOME_MEANING[order.outcome]}>
            {words(order.outcome)}
          </Pill>
        </div>
        <p className="dim">{OUTCOME_MEANING[order.outcome]}</p>
        <h3 className="fx-question">{QUESTION_TEXT[explanation.question]}</h3>
        <p className="fx-headline">{explanation.headline}</p>
        {ledger ? (
          <>
            <h3 className="fx-subhead">What the simulator knows vs. estimates</h3>
            {ledger}
          </>
        ) : null}
        <p className="note">
          Verdict: <span title={explanation.verdict}>{VERDICT_TEXT[explanation.verdict]}</span>. Rule-based: assembled
          deterministically from the tape events below. This order was simulated on historical
          replay; it was never sent to an exchange and did not affect the historical prices.
        </p>
        <dl className="kv">
          <dt>Filled (simulated)</dt>
          <dd className="mono">
            {fmtQty(order.filled_qty, instrument)} of {fmtQty(order.qty, instrument)}{" "}
            <span className="dim">
              maker {fmtQty(order.maker_qty, instrument)}, taker{" "}
              {fmtQty(order.taker_qty, instrument)}, {fmtInt(order.fill_count)} fill
              {order.fill_count === 1 ? "" : "s"}
            </span>
          </dd>
          <dt>Average fill price</dt>
          <dd className="mono">
            {fmtPrice(order.avg_fill_tick, instrument)} <span className="dim">quantity-weighted</span>
          </dd>
          <dt>
            <Term term="queue_position">Queue ahead</Term> at entry
          </dt>
          <dd className="mono">
            {fmtQty(order.queue_ahead_at_entry, instrument)} <span className="dim">modeled</span>
          </dd>
          <dt>Minimum raw queue estimate (modeled)</dt>
          <dd>
            <span className="mono">{fmtQty(order.min_queue_ahead, instrument)}</span>{" "}
            <span className="dim">
              Negative means traded quantity beyond the modeled front, not negative displayed depth. Fill events
              establish executions.
            </span>
          </dd>
          <dt>Traded at price while resting</dt>
          <dd className="mono">
            {fmtQty(order.traded_at_price_qty, instrument)}{" "}
            <span className="dim">observed market trades</span>
          </dd>
          <dt>
            <Term term="fees">Fees</Term> <span className="unit">quote</span>
          </dt>
          <dd className="mono">
            {fmtPnl(order.fees, 4)} <span className="dim">negative is a rebate</span>
          </dd>
          <dt>Last exchange status</dt>
          <dd>
            {words(order.final_status)}{" "}
            <span className="dim">
              {order.terminal_ts === null
                ? "not terminal at the exchange"
                : `terminal ${fmtSince(order.terminal_ts, originNs)} after decision`}
            </span>
          </dd>
          {order.cancel_sent_ts !== null ? (
            <>
              <dt>Cancel sent</dt>
              <dd>
                <span className="mono">{fmtSince(order.cancel_sent_ts, originNs)}</span> after
                decision{" "}
                <span className="dim">
                  {order.cancel_reason ?? "no reason recorded"}
                  {order.cancel_rejected ? "; rejected, the order was already gone" : ""}
                </span>
              </dd>
            </>
          ) : null}
        </dl>
        <h3 className="fx-subhead">Run context: modeled execution assumptions</h3>
        {execution ? (
          <ExecutionAssumptions execution={execution} />
        ) : runError ? (
          <ErrorBox error={runError} />
        ) : (
          <Spinner label="Loading run configuration" />
        )}
        <EvidenceLegend sources />
      </div>
    </Panel>
  );
}

function SelectedEvent({
  event,
  instrument,
  originNs,
  citingSteps,
  digits,
}: {
  event: FlightEvent;
  instrument: InstrumentMeta;
  originNs: number;
  citingSteps: number[];
  /** The inspector renders outside this workspace's TimeDigitsContext. */
  digits: NsDigits;
}) {
  return (
    <div className="col">
      <div className="row">
        <span className="mono">#{event.seq}</span>
        <Badge kind={event.source}>{event.source}</Badge>
        <span className="fx-clock-tag" data-clock={event.clock}>
          {event.clock} clock
        </span>
      </div>
      <strong>{eventTitle(event)}</strong>
      <div className="row">
        <ClockText ns={event.ts} digits={digits} />
        <span className="mono dim">{fmtSince(event.ts, originNs)} from decision</span>
      </div>
      <p className="dim">
        {citingSteps.length === 0
          ? "not cited by any explanation step"
          : `cited by explanation step ${citingSteps.join(", ")}`}
      </p>
      <dl className="kv">
        {eventFields(event, instrument).map(([label, value]) => (
          <Fragment key={label}>
            <dt>{label}</dt>
            <dd className="mono">{value}</dd>
          </Fragment>
        ))}
      </dl>
    </div>
  );
}

function OrderView({
  runId,
  detail,
  execution,
  runError,
  dataset,
}: {
  runId: string;
  detail: OrderDetail;
  execution: ExecutionConfig | null;
  runError: Error | null;
  dataset: DatasetManifest | null;
}) {
  const { order, events, fills, explanation, instrument } = detail;

  const [mode, setMode] = useState<AxisMode>("linear");
  const [selectedSeq, setSelectedSeq] = useState<number | null>(null);
  const [activeStep, setActiveStep] = useState<number | null>(null);
  const phone = usePhone();
  // The ledger is the desktop's first evidence tab; phones show it in the Order pane instead.
  const [tab, setTab] = useState<BottomTab>(phone ? "lifecycle" : "ledger");
  const bottomTab: BottomTab = phone && tab === "ledger" ? "lifecycle" : tab;
  const digits = useContext(TimeDigitsContext);
  const [phoneTab, setPhoneTab] = useState<PhoneTab>("order");
  const [chartTab, setChartTab] = useState<ChartTab>("timeline");
  const [attachCharts, chartsSize] = useElementSize<HTMLDivElement>();
  const chartsTabbed = chartsSize.height < BOTH_CHARTS_MIN;

  const ledgerRows = useMemo(
    () =>
      buildLedger({
        detail,
        execution,
        dataset: dataset ? { id: dataset.id, kind: dataset.kind } : null,
      }),
    [detail, execution, dataset],
  );
  const openTab = (next: BottomTab) => () => {
    setTab(next);
    if (phone) setPhoneTab("evidence");
  };
  const ledgerTargets: Partial<Record<LedgerTarget, () => void>> = {
    context: openTab("context"),
    latency: openTab("latency"),
    fills: openTab("fills"),
    explanation: openTab("explanation"),
    // Phones show the queue chart on the Timeline pane; a docked layout on its chart tab (a no-op
    // when the pane is tall enough to show both charts).
    queue: phone ? () => setPhoneTab("timeline") : () => setChartTab("queue"),
  };
  const ledger = (
    <EvidenceLedger
      rows={ledgerRows}
      selectedSeq={selectedSeq}
      onSelect={setSelectedSeq}
      targets={ledgerTargets}
      intro={phone}
    />
  );

  const citedSeqs = useMemo<ReadonlySet<number>>(() => {
    const step = activeStep === null ? undefined : explanation.steps[activeStep];
    return new Set<number>(step ? step.event_seqs : []);
  }, [activeStep, explanation]);

  const originNs = order.decision_ts;
  const selectedEvent: FlightEvent | undefined =
    selectedSeq === null ? undefined : events.find((e) => e.seq === selectedSeq);
  const citingSteps = explanation.steps.flatMap((step, i) =>
    selectedSeq !== null && step.event_seqs.includes(selectedSeq) ? [i + 1] : [],
  );
  const truncations = events.flatMap((ev) =>
    ev.body.type === "queue_truncated" ? [ev.body.dropped] : [],
  );

  useInspector(
    selectedEvent ? `Event #${selectedEvent.seq}` : `Order ${order.order_id}`,
    selectedEvent ? (
      <SelectedEvent
        event={selectedEvent}
        instrument={instrument}
        originNs={originNs}
        citingSteps={citingSteps}
        digits={digits}
      />
    ) : (
      <p className="note">
        Select an event on the two-clock timeline, the queue chart or in the Flight recorder tab
        to see its recorded fields here. Run {runId}, order {order.order_id}.
      </p>
    ),
    [selectedEvent, citingSteps.join(","), instrument, originNs, runId, order.order_id, digits],
  );

  const axisToggle = (
    <span className="fx-toggle" role="group" aria-label="Time axis">
      <button type="button" aria-pressed={mode === "linear"} onClick={() => setMode("linear")}>
        Linear time
      </button>
      <button type="button" aria-pressed={mode === "ordinal"} onClick={() => setMode("ordinal")}>
        Event order
      </button>
    </span>
  );

  const timeline = (
    <Panel title="Two-clock timeline" fill actions={axisToggle}>
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
      <p className="note" aria-live="polite">
        {selectedEvent
          ? `Selected #${selectedEvent.seq}: ${eventTitle(selectedEvent)}, ${fmtSince(selectedEvent.ts, originNs)} from decision. Details in the inspector.`
          : "No event selected."}
      </p>
    </Panel>
  );

  // The chart comes first so a short pane still shows it; the model note follows.
  const queue = (
    <Panel
      title="Modeled queue position"
      basis="modeled"
      fill
      actions={!phone && chartsTabbed ? axisToggle : undefined}
    >
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
      {truncations.map((dropped, i) => (
        <Notice key={i} tag="TRUNCATED" tone="info">
          <Badge kind="recorder">recorder</Badge> The per-order cap on recorded queue updates was
          reached: {dropped} later updates that changed the estimate by less than one lot were not
          kept.
        </Notice>
      ))}
      <p className="note">
        {execution ? (
          <>
            Queue model configured for this run:{" "}
            <code className="mono">{queueModelLabel(execution.queue_model)}</code>.{" "}
            {queueModelMeaning(execution.queue_model)}
          </>
        ) : runError ? (
          "The run's configured queue model could not be loaded."
        ) : (
          "Loading the run's configured queue model."
        )}{" "}
        Level-2 data does not show individual orders, so the position in the queue is an estimate
        and is never observed.
      </p>
    </Panel>
  );

  const bottom = (
    <Tabs<BottomTab>
      label="Order evidence"
      className="fx-tabs"
      active={bottomTab}
      onChange={setTab}
      tabs={[
        ...(phone ? [] : [{ id: "ledger" as const, label: "Known vs. estimated" }]),
        { id: "lifecycle", label: "Lifecycle" },
        { id: "explanation", label: "Explanation", count: explanation.steps.length },
        { id: "context", label: "Decision context" },
        { id: "latency", label: "Latency breakdown" },
        { id: "fills", label: "Fills", count: fills.length },
        { id: "events", label: "Flight recorder", count: events.length },
      ]}
    >
      {bottomTab === "ledger" ? (
        <Panel
          title="What the simulator knows vs. estimates"
          fill
          actions={
            <span className="fx-ledger-hint" title={LEDGER_HINT}>
              Expand a row for values, assumption and tape events
            </span>
          }
        >
          {ledger}
        </Panel>
      ) : bottomTab === "lifecycle" ? (
        <Panel title="Order lifecycle" basis="modeled" fill>
          <p className="note">
            Strategy decision and responses run on the local (strategy) clock; arrival, queue, fills and cancels on the
            exchange clock. Every step is simulated on historical replay.
          </p>
          <Lifecycle
            events={events}
            instrument={instrument}
            originNs={originNs}
            selectedSeq={selectedSeq}
            onSelect={setSelectedSeq}
          />
        </Panel>
      ) : bottomTab === "explanation" ? (
        <Panel title="Explanation" fill>
          <ExplanationSteps
            explanation={explanation}
            selectedSeq={selectedSeq}
            activeStep={activeStep}
            onStep={setActiveStep}
            onSelect={setSelectedSeq}
          />
        </Panel>
      ) : bottomTab === "context" ? (
        <Panel title="Decision context" fill>
          <DecisionContext events={events} order={order} instrument={instrument} />
        </Panel>
      ) : bottomTab === "latency" ? (
        <Panel title="Latency breakdown" fill>
          <LatencyTable
            latency={explanation.latency}
            localTs={dataset ? dataset.timestamps.local_ts : null}
            feedOffsetNs={execution ? execution.feed_latency_offset_ns : null}
          />
        </Panel>
      ) : bottomTab === "fills" ? (
        <Panel title="Simulated fills" basis="modeled" flush fill>
          <FillsTable
            fills={fills}
            instrument={instrument}
            originNs={originNs}
            selectedSeq={selectedSeq}
            onSelect={setSelectedSeq}
          />
        </Panel>
      ) : (
        <Panel title="Flight recorder events" flush fill>
          <EventList
            events={events}
            instrument={instrument}
            originNs={originNs}
            selectedSeq={selectedSeq}
            citedSeqs={citedSeqs}
            onSelect={setSelectedSeq}
          />
        </Panel>
      )}
    </Tabs>
  );

  const context = (
    <OrderContext detail={detail} execution={execution} runError={runError} ledger={phone ? ledger : undefined} />
  );
  if (phone) {
    return (
      <PaneTabs<PhoneTab>
        label="Order forensics panes"
        active={phoneTab}
        onChange={setPhoneTab}
        tabs={[
          { id: "order", label: "Order", content: context },
          {
            id: "timeline",
            label: "Timeline",
            content: (
              <SplitPane
                id="forensics.order.phone"
                orientation="vertical"
                panes={[
                  { id: "timeline", label: "Two-clock timeline", stackHeight: "max(320px, 60dvh)", content: timeline },
                  { id: "queue", label: "Modeled queue position", stackHeight: "max(300px, 50dvh)", content: queue },
                ]}
              />
            ),
          },
          { id: "evidence", label: "Evidence", count: explanation.steps.length, content: bottom },
        ]}
      />
    );
  }

  return (
    <SplitPane
      id="forensics.order"
      orientation="vertical"
      panes={[
        {
          id: "top",
          label: "Order and charts",
          defaultSize: "58%",
          minSize: "25%",
          content: (
            <SplitPane
              id="forensics.order.top"
              panes={[
                {
                  id: "context",
                  label: "Order context",
                  defaultSize: "30%",
                  minSize: "18%",
                  collapsible: true,
                  content: context,
                },
                {
                  id: "charts",
                  label: "Timeline and queue",
                  defaultSize: "70%",
                  minSize: "35%",
                  stackHeight: "max(480px, 60dvh)",
                  content: (
                    <div className="fx-charts" ref={attachCharts}>
                      {chartsTabbed ? (
                        <Tabs<ChartTab>
                          label="Order charts"
                          className="fx-tabs fx-chart-tabs"
                          active={chartTab}
                          onChange={setChartTab}
                          tabs={[
                            { id: "timeline", label: "Two-clock timeline" },
                            { id: "queue", label: "Modeled queue position" },
                          ]}
                        >
                          {chartTab === "timeline" ? timeline : queue}
                        </Tabs>
                      ) : (
                        <SplitPane
                          id="forensics.order.charts"
                          orientation="vertical"
                          panes={[
                            { id: "timeline", label: "Two-clock timeline", defaultSize: "50%", minSize: 320, content: timeline },
                            { id: "queue", label: "Modeled queue position", defaultSize: "50%", minSize: 300, content: queue },
                          ]}
                        />
                      )}
                    </div>
                  ),
                },
              ]}
            />
          ),
        },
        { id: "evidence", label: "Order evidence", defaultSize: "42%", minSize: "15%", content: bottom },
      ]}
    />
  );
}
