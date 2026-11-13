import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ApiError, api } from "../../api/client";
import type {
  Alignment,
  Comparison,
  DatasetManifest,
  DecisionOutcome,
  InstrumentMeta,
  MetricRow,
  OrderPair,
  PairClass,
  RunSummary,
} from "../../api/types";
import { ConfigValue, metricLabel } from "../../components/configValue";
import { fmtDuration, fmtNum, fmtPrice, fmtQty, fmtSigned } from "../../components/fmt";
import { Badge, Empty, ErrorBox, Panel, Spinner, useAsync } from "../../components/ui";
import { ClockText, EvidenceLegend } from "../forensics/evidence";
import {
  EM_DASH,
  fmtSignedDuration,
  outcomeKind,
  queueModelLabel,
  words,
} from "../forensics/labels";
import { EquityOverlay } from "./EquityOverlay";
import "./compare.css";

const PAIR_CLASSES: readonly PairClass[] = [
  "identical",
  "same_decision_different_outcome",
  "different_decision",
  "only_a",
  "only_b",
];

const CLASS_LABEL: Record<PairClass, string> = {
  identical: "Identical",
  same_decision_different_outcome: "Same decision, different outcome",
  different_decision: "Different decision",
  only_a: "Only in A",
  only_b: "Only in B",
};

// Definitions only. What a class implies about the scenarios is left to the interpretation notes
// the backend returns with their evidence basis.
const CLASS_MEANING: Record<PairClass, string> = {
  identical: "Same decision, same outcome and same filled quantity in both runs.",
  same_decision_different_outcome:
    "The strategy made the same decision (side, price, quantity) in both runs; the simulated execution of it differed.",
  different_decision:
    "The strategy made a different decision (side, price or quantity) at this tick and slot.",
  only_a: "The decision exists only in scenario A.",
  only_b: "The decision exists only in scenario B.",
};

const PAGE_ROWS = 200;

export default function ComparePage() {
  const [params, setParams] = useSearchParams();
  const a = params.get("a") ?? "";
  const b = params.get("b") ?? "";

  return (
    <div className="cmp-page">
      <header className="cmp-title">
        <h1>Scenario comparison</h1>
        <p className="dim">
          Two simulations side by side: which configuration fields differ, which measured outcomes
          differ, and where the two tapes first part. Both are simulations on historical replay.
        </p>
      </header>
      {a === "" || b === "" ? (
        <RunPicker a={a} b={b} onPick={(nextA, nextB) => setParams({ a: nextA, b: nextB })} />
      ) : (
        <CompareView
          key={`${a}|${b}`}
          a={a}
          b={b}
          onSwap={() => setParams({ a: b, b: a })}
          onChange={() => setParams({})}
        />
      )}
    </div>
  );
}

function runOptionText(run: RunSummary): string {
  const label = run.label === "" ? "(no label)" : run.label;
  return `${label} | ${run.manifest.config.strategy.kind.type} | ${run.manifest.dataset.id} | ${run.manifest.run_id}`;
}

function RunPicker({
  a,
  b,
  onPick,
}: {
  a: string;
  b: string;
  onPick: (a: string, b: string) => void;
}) {
  const runs = useAsync(() => api.runs(), []);
  const [pickA, setPickA] = useState(a);
  const [pickB, setPickB] = useState(b);

  if (runs.error) return <ErrorBox error={runs.error} />;
  if (!runs.data) return <Spinner />;
  if (runs.data.length < 2) {
    return (
      <Empty>
        A comparison needs two stored runs; {runs.data.length === 0 ? "none exists" : "one exists"}{" "}
        so far. <Link to="/lab">Run a simulation in the strategy laboratory</Link>
      </Empty>
    );
  }

  const list = runs.data;
  const select = (id: string, value: string, onChange: (v: string) => void, legend: string) => (
    <label className="cmp-pick" htmlFor={id}>
      <span className="cmp-pick-label">{legend}</span>
      <select id={id} value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Select a run</option>
        {value !== "" && !list.some((r) => r.manifest.run_id === value) ? (
          <option value={value}>{value} (not among stored runs)</option>
        ) : null}
        {list.map((r) => (
          <option key={r.manifest.run_id} value={r.manifest.run_id}>
            {runOptionText(r)}
          </option>
        ))}
      </select>
    </label>
  );
  const same = pickA !== "" && pickA === pickB;

  return (
    <Panel title="Choose two runs">
      <form
        className="cmp-picker"
        onSubmit={(e) => {
          e.preventDefault();
          if (pickA !== "" && pickB !== "" && !same) onPick(pickA, pickB);
        }}
      >
        {select("cmp-pick-a", pickA, setPickA, "Scenario A")}
        {select("cmp-pick-b", pickB, setPickB, "Scenario B")}
        <div className="cmp-picker-go">
          <button type="submit" className="cmp-btn" disabled={pickA === "" || pickB === "" || same}>
            Compare
          </button>
          {same ? <span className="dim">Choose two different runs.</span> : null}
        </div>
      </form>
    </Panel>
  );
}

function CompareView({
  a,
  b,
  onSwap,
  onChange,
}: {
  a: string;
  b: string;
  onSwap: () => void;
  onChange: () => void;
}) {
  const loaded = useAsync(
    () => Promise.all([api.compare(a, b), api.run(a), api.run(b)]),
    [a, b],
  );
  const equity = useAsync(() => Promise.all([api.equity(a), api.equity(b)]), [a, b]);

  const fresh =
    loaded.data && loaded.data[0].run_a === a && loaded.data[0].run_b === b ? loaded.data : null;
  const datasetA = fresh ? fresh[1].manifest.dataset.id : null;
  const datasetB = fresh ? fresh[2].manifest.dataset.id : null;
  // Instruments are needed only to print ticks as prices; ticks are shown until they arrive.
  const datasets = useAsync<[DatasetManifest, DatasetManifest] | null>(async () => {
    if (datasetA === null || datasetB === null) return null;
    const first = await api.dataset(datasetA);
    const second = datasetB === datasetA ? first : await api.dataset(datasetB);
    return [first, second];
  }, [datasetA, datasetB]);

  const actions = (
    <div className="cmp-actions">
      <button type="button" className="cmp-btn" onClick={onSwap}>
        Swap A and B
      </button>
      <button type="button" className="cmp-btn" onClick={onChange}>
        Choose other runs
      </button>
    </div>
  );

  if (!fresh) {
    if (loaded.error) {
      const missing = loaded.error instanceof ApiError && loaded.error.status === 404;
      return (
        <div className="col">
          {missing ? (
            <Empty>
              At least one of the runs <span className="mono">{a}</span> and{" "}
              <span className="mono">{b}</span> is not stored. {loaded.error.message}
            </Empty>
          ) : (
            <ErrorBox error={loaded.error} />
          )}
          <div className="cmp-actions">
            <button type="button" className="cmp-btn" onClick={loaded.reload}>
              Retry
            </button>
            <button type="button" className="cmp-btn" onClick={onChange}>
              Choose other runs
            </button>
          </div>
        </div>
      );
    }
    return <Spinner />;
  }

  const [cmp, runA, runB] = fresh;
  const instA = datasets.data ? datasets.data[0].instrument : null;
  const instB = datasets.data ? datasets.data[1].instrument : null;
  const first = cmp.alignment.first_divergence;

  return (
    <>
      {actions}
      <EvidenceLegend />

      <section className="cmp-scenarios" aria-label="Scenarios">
        <ScenarioCard tag="A" label={cmp.label_a} run={runA} />
        <ScenarioCard tag="B" label={cmp.label_b} run={runB} />
      </section>
      {cmp.same_dataset ? null : (
        <p className="cmp-warning" role="note">
          <Badge kind="warn">different datasets</Badge> The two runs did not replay the same
          dataset (their event-file hashes differ). The rows below compare simulations over
          different market data.
        </p>
      )}
      {cmp.same_strategy ? null : (
        <p className="cmp-warning" role="note">
          <Badge kind="warn">different strategies</Badge> The two runs did not use the same
          strategy configuration. The fields that differ are listed under "What changed".
        </p>
      )}

      <Panel title="What changed">
        <ConfigDiffTable cmp={cmp} />
      </Panel>

      <Panel title="Measured outcomes">
        <MetricsTable rows={cmp.metrics} />
      </Panel>

      <Panel title="Order alignment">
        <AlignmentSummary alignment={cmp.alignment} />
      </Panel>

      <Panel title="First divergence">
        {first === null ? (
          <Empty>
            No non-identical pair: every aligned decision has the same outcome and filled quantity
            in both runs.
          </Empty>
        ) : (
          <FirstDivergence pair={first} cmp={cmp} instA={instA} instB={instB} />
        )}
      </Panel>

      <Panel title="Diverging decisions">
        <PairsTable cmp={cmp} instA={instA} instB={instB} />
      </Panel>

      <Panel title="Equity, overlaid">
        {equity.error ? (
          <ErrorBox error={equity.error} />
        ) : equity.data ? (
          <EquityOverlay
            a={equity.data[0]}
            b={equity.data[1]}
            divergenceTs={first === null ? null : first.decision_ts}
            sameDataset={cmp.same_dataset}
          />
        ) : (
          <Spinner />
        )}
      </Panel>

      <Panel title="Interpretation">
        {cmp.interpretation.length === 0 ? (
          <Empty>The comparison returned no interpretation notes.</Empty>
        ) : (
          <ul className="cmp-notes">
            {cmp.interpretation.map((note, i) => (
              <li key={i}>
                <Badge kind={note.basis}>{note.basis}</Badge>
                <span>{readableNote(note.statement, instA !== null && instB !== null && instA.tick_size === instB.tick_size ? instA : null)}</span>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </>
  );
}

function ScenarioCard({ tag, label, run }: { tag: "A" | "B"; label: string; run: RunSummary }) {
  const { manifest } = run;
  const execution = manifest.config.execution;
  const latency = execution.order_latency;
  return (
    <article className={`cmp-scenario cmp-scenario-${tag.toLowerCase()}`}>
      <h2>
        <span className="cmp-tag">{tag}</span> {label === "" ? "(no label)" : label}
      </h2>
      <dl>
        <div>
          <dt>Run</dt>
          <dd>
            <Link className="mono" to={`/runs/${manifest.run_id}`}>
              {manifest.run_id}
            </Link>
          </dd>
        </div>
        <div>
          <dt>Strategy</dt>
          <dd className="mono">
            {manifest.config.strategy.kind.type}, every{" "}
            {fmtDuration(manifest.config.strategy.interval_ns)}
          </dd>
        </div>
        <div>
          <dt>Dataset</dt>
          <dd className="mono" title={manifest.dataset.sha256}>
            {manifest.dataset.id} <span className="dim">sha256 {manifest.dataset.sha256.slice(0, 12)}</span>
          </dd>
        </div>
        <div>
          <dt>Execution assumptions</dt>
          <dd className="mono">
            {execution.exchange_model}, {queueModelLabel(execution.queue_model)}, entry{" "}
            {fmtDuration(latency.entry_ns)} / response {fmtDuration(latency.response_ns)}
            {latency.type === "jitter" ? ` / jitter ${fmtDuration(latency.jitter_ns)}` : ""}
          </dd>
        </div>
        <div>
          <dt>Result hash</dt>
          <dd className="mono" title={manifest.result_sha256}>
            {manifest.result_sha256.slice(0, 16)}
          </dd>
        </div>
        <div>
          <dt>Orders / simulated fills</dt>
          <dd className="mono">
            {run.order_count} / {run.fill_count}
          </dd>
        </div>
      </dl>
    </article>
  );
}

function ConfigDiffTable({ cmp }: { cmp: Comparison }) {
  if (cmp.config_diffs.length === 0) {
    return <Empty>No configuration field differs between the two runs.</Empty>;
  }
  return (
    <table className="table cmp-diffs">
      <caption className="cmp-caption">
        Every field of the serialized run configuration whose value differs.
      </caption>
      <thead>
        <tr>
          <th scope="col">Configuration path</th>
          <th scope="col">A</th>
          <th scope="col">B</th>
        </tr>
      </thead>
      <tbody>
        {cmp.config_diffs.map((diff) => (
          <tr key={diff.path}>
            <th scope="row" className="mono">
              {diff.path}
            </th>
            <ConfigValue path={diff.path} value={diff.a} />
            <ConfigValue path={diff.path} value={diff.b} />
          </tr>
        ))}
      </tbody>
    </table>
  );
}

const UNIT_LABEL: Record<string, string> = {
  quote: "quote currency",
  qty: "base quantity",
  count: "count",
  ratio: "percent",
  ns: "time",
  ticks: "ticks",
};

function fmtMetric(value: number | null, unit: string): string {
  if (value === null) return EM_DASH;
  switch (unit) {
    case "count":
      return fmtNum(value, 0);
    case "ratio":
      return `${(value * 100).toFixed(2)}%`;
    case "ns":
      return fmtDuration(value);
    case "ticks":
      return fmtNum(value, 3);
    default:
      return fmtNum(value, 4);
  }
}

function fmtMetricDelta(value: number | null, unit: string): string {
  if (value === null) return EM_DASH;
  switch (unit) {
    case "count":
      return fmtSigned(value, 0);
    case "ratio":
      // A difference of two ratios is in percentage points, not percent.
      return `${fmtSigned(value * 100, 2)} pp`;
    case "ns":
      return fmtSignedDuration(value);
    case "ticks":
      return fmtSigned(value, 3);
    default:
      return fmtSigned(value, 4);
  }
}

function MetricsTable({ rows }: { rows: MetricRow[] }) {
  if (rows.length === 0) return <Empty>The comparison returned no metric rows.</Empty>;
  // Net P&L is the only row coloured, and only by its sign; no other difference is marked as
  // better or worse.
  const signClass = (row: MetricRow, value: number | null) =>
    row.key !== "net_pnl" || value === null || value === 0
      ? ""
      : value > 0
        ? " cmp-pos"
        : " cmp-neg";
  return (
    <table className="table cmp-metrics">
      <caption className="cmp-caption">
        Delta is B minus A. A dash means the value is undefined for that run. Only net P&amp;L is
        coloured, by sign.
      </caption>
      <thead>
        <tr>
          <th scope="col">Metric</th>
          <th scope="col" className="cmp-num">
            A
          </th>
          <th scope="col" className="cmp-num">
            B
          </th>
          <th scope="col" className="cmp-num">
            Delta (B - A)
          </th>
          <th scope="col">Unit</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <tr key={row.key}>
            <th scope="row" title={row.key}>
              {metricLabel(row)}
            </th>
            <td className={`mono cmp-num${signClass(row, row.a)}`}>{fmtMetric(row.a, row.unit)}</td>
            <td className={`mono cmp-num${signClass(row, row.b)}`}>{fmtMetric(row.b, row.unit)}</td>
            <td className={`mono cmp-num${signClass(row, row.delta)}`}>
              {fmtMetricDelta(row.delta, row.unit)}
            </td>
            <td className="dim">{UNIT_LABEL[row.unit] ?? row.unit}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function AlignmentSummary({ alignment }: { alignment: Alignment }) {
  const total = PAIR_CLASSES.reduce((sum, cls) => sum + alignment[cls], 0);
  if (total === 0) return <Empty>Neither run holds an order decision to align.</Empty>;
  return (
    <div className="cmp-align">
      <p className="dim">
        Orders are aligned by order id, which encodes the strategy tick and slot of the decision.{" "}
        {total} aligned decisions in total.
      </p>
      <div className="cmp-bar" aria-hidden="true">
        {PAIR_CLASSES.map((cls) =>
          alignment[cls] > 0 ? (
            <span
              key={cls}
              className={`cmp-seg cmp-seg-${cls}`}
              style={{ flexGrow: alignment[cls] }}
              title={`${CLASS_LABEL[cls]}: ${alignment[cls]}`}
            />
          ) : null,
        )}
      </div>
      <table className="table cmp-classes">
        <thead>
          <tr>
            <th scope="col">Class</th>
            <th scope="col" className="cmp-num">
              Decisions
            </th>
            <th scope="col" className="cmp-num">
              Share
            </th>
            <th scope="col">Definition</th>
          </tr>
        </thead>
        <tbody>
          {PAIR_CLASSES.map((cls) => (
            <tr key={cls}>
              <th scope="row">
                <span className={`cmp-swatch cmp-seg-${cls}`} aria-hidden="true" />{" "}
                {CLASS_LABEL[cls]}
              </th>
              <td className="mono cmp-num">{alignment[cls]}</td>
              <td className="mono cmp-num">{((alignment[cls] / total) * 100).toFixed(1)}%</td>
              <td className="dim">{CLASS_MEANING[cls]}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Ticks as a price once the dataset's instrument is known; raw ticks until then. */
function price(tick: number | null, inst: InstrumentMeta | null): string {
  if (tick === null) return EM_DASH;
  if (inst === null) return `tick ${Number.isInteger(tick) ? tick : fmtNum(tick, 2)}`;
  return fmtPrice(tick, inst);
}

/**
 * Server notes quote prices as raw ticks (`price tick 937160`, `average fill tick 937160`).
 * With one shared tick size they read as prices; otherwise the ticks stay.
 */
function readableNote(text: string, inst: InstrumentMeta | null): string {
  if (inst === null) return text;
  return text
    .replace(/\baverage fill tick (\d+(?:\.\d+)?)/g, (_, t: string) => `average fill price ${fmtPrice(Number(t), inst)}`)
    .replace(/\bprice tick (\d+(?:\.\d+)?)/g, (_, t: string) => fmtPrice(Number(t), inst));
}

function quantity(qty: number | null, inst: InstrumentMeta | null): string {
  return inst === null ? fmtNum(qty, 6) : fmtQty(qty, inst);
}

function OutcomeCard({
  tag,
  label,
  runId,
  orderId,
  outcome,
  inst,
}: {
  tag: "A" | "B";
  label: string;
  runId: string;
  orderId: number;
  outcome: DecisionOutcome | null;
  inst: InstrumentMeta | null;
}) {
  return (
    <article className={`cmp-outcome cmp-scenario-${tag.toLowerCase()}`}>
      <h3>
        <span className="cmp-tag">{tag}</span> {label === "" ? "(no label)" : label}
      </h3>
      {outcome === null ? (
        <p className="dim">No order was decided at this tick and slot in scenario {tag}.</p>
      ) : (
        <>
          <dl>
            <div>
              <dt>Decision</dt>
              <dd className="mono">
                <Badge kind={outcome.side}>{outcome.side}</Badge> {quantity(outcome.qty, inst)} @{" "}
                {price(outcome.price_tick, inst)}
              </dd>
            </div>
            <div>
              <dt>Simulated outcome</dt>
              <dd>
                <Badge kind={outcomeKind(outcome.outcome)}>{words(outcome.outcome)}</Badge>
              </dd>
            </div>
            <div>
              <dt>Filled (simulated)</dt>
              <dd className="mono">
                {quantity(outcome.filled_qty, inst)}
                {outcome.avg_fill_tick === null
                  ? ""
                  : ` at average ${price(outcome.avg_fill_tick, inst)}`}
              </dd>
            </div>
            <div>
              <dt>Queue ahead at entry (modeled)</dt>
              <dd className="mono">{quantity(outcome.queue_ahead_at_entry, inst)}</dd>
            </div>
            <div>
              <dt>Exchange arrival</dt>
              <dd>
                <ClockText ns={outcome.exch_arrival_ts} />
              </dd>
            </div>
            <div>
              <dt>First simulated fill</dt>
              <dd>
                <ClockText ns={outcome.first_fill_ts} />
              </dd>
            </div>
          </dl>
          <Link className="cmp-btn" to={`/runs/${runId}/orders/${orderId}`}>
            Open this order's forensics in {tag}
          </Link>
        </>
      )}
    </article>
  );
}

function FirstDivergence({
  pair,
  cmp,
  instA,
  instB,
}: {
  pair: OrderPair;
  cmp: Comparison;
  instA: InstrumentMeta | null;
  instB: InstrumentMeta | null;
}) {
  return (
    <div className="cmp-first">
      <p className="cmp-first-head">
        <span>
          Order <span className="mono">{pair.order_id}</span>
        </span>
        <span>
          strategy tick <span className="mono">{pair.tick_no}</span>
        </span>
        <span>
          decided at <ClockText ns={pair.decision_ts} /> <span className="dim">local clock</span>
        </span>
        <span className={`cmp-class cmp-seg-${pair.class}`}>{CLASS_LABEL[pair.class]}</span>
      </p>
      <p className="dim">
        The earliest non-identical pair in decision order. Every aligned pair before it is
        identical in both runs. {CLASS_MEANING[pair.class]}
      </p>
      <div className="cmp-outcomes">
        <OutcomeCard
          tag="A"
          label={cmp.label_a}
          runId={cmp.run_a}
          orderId={pair.order_id}
          outcome={pair.a}
          inst={instA}
        />
        <OutcomeCard
          tag="B"
          label={cmp.label_b}
          runId={cmp.run_b}
          orderId={pair.order_id}
          outcome={pair.b}
          inst={instB}
        />
      </div>
    </div>
  );
}

function PairSide({
  runId,
  orderId,
  outcome,
  inst,
  tag,
}: {
  runId: string;
  orderId: number;
  outcome: DecisionOutcome | null;
  inst: InstrumentMeta | null;
  tag: "A" | "B";
}) {
  if (outcome === null) {
    return (
      <td colSpan={3} className="dim cmp-side-start">
        no order in {tag}
      </td>
    );
  }
  return (
    <>
      <td className="mono cmp-side-start">
        <Link
          to={`/runs/${runId}/orders/${orderId}`}
          aria-label={`Order ${orderId} forensics in scenario ${tag}`}
        >
          {outcome.side} {quantity(outcome.qty, inst)} @ {price(outcome.price_tick, inst)}
        </Link>
      </td>
      <td>
        <Badge kind={outcomeKind(outcome.outcome)}>{words(outcome.outcome)}</Badge>
      </td>
      <td className="mono cmp-num">{quantity(outcome.filled_qty, inst)}</td>
    </>
  );
}

function PairsTable({
  cmp,
  instA,
  instB,
}: {
  cmp: Comparison;
  instA: InstrumentMeta | null;
  instB: InstrumentMeta | null;
}) {
  const { alignment } = cmp;
  const [filter, setFilter] = useState<PairClass | "all">("all");
  const [limit, setLimit] = useState(PAGE_ROWS);

  const nonIdentical =
    alignment.same_decision_different_outcome +
    alignment.different_decision +
    alignment.only_a +
    alignment.only_b;
  if (alignment.pairs.length === 0) {
    return (
      <Empty>
        {nonIdentical === 0
          ? "No decision diverges between the two runs."
          : "The comparison returned no pair rows."}
      </Empty>
    );
  }

  const matching =
    filter === "all" ? alignment.pairs : alignment.pairs.filter((p) => p.class === filter);
  const shown = matching.slice(0, limit);

  return (
    <div className="cmp-pairs">
      <div className="cmp-pairs-bar">
        <label className="cmp-pick" htmlFor="cmp-class-filter">
          <span className="cmp-pick-label">Class</span>
          <select
            id="cmp-class-filter"
            value={filter}
            onChange={(e) => {
              const value = e.target.value;
              setFilter(PAIR_CLASSES.find((cls) => cls === value) ?? "all");
              setLimit(PAGE_ROWS);
            }}
          >
            <option value="all">All non-identical ({alignment.pairs.length} listed)</option>
            {PAIR_CLASSES.filter((cls) => cls !== "identical").map((cls) => (
              <option key={cls} value={cls}>
                {CLASS_LABEL[cls]} ({alignment.pairs.filter((p) => p.class === cls).length} listed)
              </option>
            ))}
          </select>
        </label>
        <span className="dim">
          Non-identical pairs in decision order. Each side links to that order's forensics in its
          own run.
        </span>
      </div>
      {alignment.pairs_truncated ? (
        <p className="cmp-warning" role="note">
          <Badge kind="warn">truncated</Badge> The list is capped at {alignment.pairs_cap} pairs:{" "}
          {alignment.pairs.length} of {nonIdentical} non-identical pairs are listed, earliest
          first. The counts under "Order alignment" cover all of them.
        </p>
      ) : null}
      {matching.length === 0 ? (
        <Empty>No listed pair is in this class.</Empty>
      ) : (
        <div className="cmp-scroll">
          <table className="table cmp-pairs-table">
            <thead>
              <tr>
                <th scope="col" rowSpan={2}>
                  Order
                </th>
                <th scope="col" rowSpan={2} className="cmp-num">
                  Tick
                </th>
                <th scope="col" rowSpan={2}>
                  Decision time
                </th>
                <th scope="col" rowSpan={2}>
                  Class
                </th>
                <th scope="colgroup" colSpan={3} className="cmp-side-start">
                  <span className="cmp-tag">A</span> {cmp.label_a}
                </th>
                <th scope="colgroup" colSpan={3} className="cmp-side-start">
                  <span className="cmp-tag">B</span> {cmp.label_b}
                </th>
              </tr>
              <tr>
                <th scope="col" className="cmp-side-start">
                  Decision
                </th>
                <th scope="col">Outcome</th>
                <th scope="col" className="cmp-num">
                  Filled
                </th>
                <th scope="col" className="cmp-side-start">
                  Decision
                </th>
                <th scope="col">Outcome</th>
                <th scope="col" className="cmp-num">
                  Filled
                </th>
              </tr>
            </thead>
            <tbody>
              {shown.map((pair) => (
                <tr key={pair.order_id}>
                  <th scope="row" className="mono">
                    {pair.order_id}
                  </th>
                  <td className="mono cmp-num">{pair.tick_no}</td>
                  <td>
                    <ClockText ns={pair.decision_ts} />
                  </td>
                  <td>
                    <span className={`cmp-class cmp-seg-${pair.class}`}>
                      {CLASS_LABEL[pair.class]}
                    </span>
                  </td>
                  <PairSide
                    tag="A"
                    runId={cmp.run_a}
                    orderId={pair.order_id}
                    outcome={pair.a}
                    inst={instA}
                  />
                  <PairSide
                    tag="B"
                    runId={cmp.run_b}
                    orderId={pair.order_id}
                    outcome={pair.b}
                    inst={instB}
                  />
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {matching.length > shown.length ? (
        <div className="cmp-more">
          <span className="dim">
            Showing {shown.length} of {matching.length} listed pairs.
          </span>
          <button type="button" className="cmp-btn" onClick={() => setLimit(limit + PAGE_ROWS)}>
            Show {Math.min(PAGE_ROWS, matching.length - shown.length)} more
          </button>
        </div>
      ) : null}
    </div>
  );
}
