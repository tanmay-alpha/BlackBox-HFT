// Experiment Comparison -> Pair (`/compare/pair?a=&b=`). Every number on this page is a field of a
// comparison the server stored (`/api/v1/comparisons`) or of the two runs; nothing is compared in
// the browser. Without a stored comparison for the pair the page offers to create one (scope
// `run`), and lists the comparisons that already exist.

import { Fragment, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ApiError, api } from "../../api/client";
import type { Comparison, CursorPage, RunDetail, Run } from "../../api/model";
import type {
  Alignment,
  DecisionOutcome,
  DeltaStats,
  InstrumentMeta,
  MetricRow,
  OrderPair,
  PairClass,
} from "../../api/types";
import { ConfigValue, configDiffRows, metricLabel } from "../../components/configValue";
import { exchangeModelLabel, feeLabel, latencyLabel, queueModelLabel, strategyName } from "../../components/describe";
import { DataTable, type Column } from "../../components/DataTable";
import { fmtDuration, fmtInt, fmtNum, fmtPrice, fmtQty, fmtSigned } from "../../components/fmt";
import { pathLabel, runLabelText, unitLabel, useDemoTitles, valueLabel } from "../../components/labels";
import { fmtNsUtc } from "../../components/ns";
import { Badge, BasisBadge, Empty, ErrorBox, HashText, Notice, Panel, Pill, Spinner, useAsync } from "../../components/ui";
import { GatedButton } from "../../shell/access";
import { paths } from "../../shell/paths";
import { ClockText, EvidenceLegend } from "../forensics/evidence";
import {
  EM_DASH,
  fmtSignedDuration,
  outcomeKind,
  words,
} from "../forensics/labels";
import { CLASS_LABEL, CLASS_MEANING, EligibilityNotice, PAIR_CLASSES } from "./classes";
import { EquityOverlay } from "./EquityOverlay";
import { differenceWords } from "../explore/data";
import "./compare.css";

/** `GET /comparisons/{id}/pairs` page size (the contract's maximum). */
const PAIRS_PAGE = 1000;

/** Equity points per run requested for the overlay; the server downsamples above it. */
const EQUITY_POINTS = 2000;

export default function ComparePage() {
  const [params, setParams] = useSearchParams();
  const a = params.get("a") ?? "";
  const b = params.get("b") ?? "";

  return (
    <div className="cmp-page">
      <header className="cmp-title">
        <h1>Scenario comparison</h1>
        <p className="dim">
          Two simulations side by side, as compared by the server: which configuration fields differ, which measured
          outcomes differ, and where the two tapes first part. Both are simulations on replayed market data (recorded or synthetic, as each run&apos;s dataset declares).
        </p>
      </header>
      {a === "" || b === "" ? (
        <>
          <RunPicker a={a} b={b} onPick={(nextA, nextB) => setParams({ a: nextA, b: nextB })} />
          <StoredComparisons />
        </>
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

function runOptionText(run: Run, titles: ReadonlyMap<string, string>): string {
  const label = run.label === null || run.label === "" ? "(no label)" : runLabelText(run.label, titles);
  return `${label} | ${strategyName(run.strategy_kind)} | ${run.dataset.id} | ${run.run_id}`;
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
  const runs = useAsync((signal) => api.runs({}, { signal }), []);
  const titles = useDemoTitles();
  const [pickA, setPickA] = useState(a);
  const [pickB, setPickB] = useState(b);

  if (runs.error) return <ErrorBox error={runs.error} onRetry={runs.reload} />;
  if (!runs.data) return <Spinner label="Loading runs" />;
  const list = runs.data.items;

  // A combobox, not a select: the list holds this workspace's runs, but any readable run (a public
  // demo run, for example) can be compared by typing or pasting its id.
  const select = (id: string, value: string, onChange: (v: string) => void, legend: string) => (
    <label className="cmp-pick" htmlFor={id}>
      <span className="cmp-pick-label">{legend}</span>
      <input
        id={id}
        type="text"
        list="cmp-run-options"
        value={value}
        placeholder="Select a run or paste a run id"
        autoComplete="off"
        spellCheck={false}
        onChange={(e) => onChange(e.target.value.trim())}
      />
    </label>
  );
  const same = pickA !== "" && pickA === pickB;
  const ready = /^[0-9a-f]{16}$/.test(pickA) && /^[0-9a-f]{16}$/.test(pickB) && !same;

  return (
    <Panel title="Choose two runs">
      {runs.data.incomplete ? (
        <Notice tag="List incomplete">First {fmtInt(list.length)} runs shown; older runs are not listed here.</Notice>
      ) : null}
      {list.length === 0 ? (
        <p className="dim">
          This workspace has no stored runs yet. <Link to={paths.strategy()}>Run a simulation in the Strategy Lab</Link>, or
          paste the ids of two runs you can open.
        </p>
      ) : null}
      <datalist id="cmp-run-options">
        {list.map((r) => (
          <option key={r.run_id} value={r.run_id}>
            {runOptionText(r, titles)}
          </option>
        ))}
      </datalist>
      <form
        className="cmp-picker"
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) onPick(pickA, pickB);
        }}
      >
        {select("cmp-pick-a", pickA, setPickA, "Scenario A")}
        {select("cmp-pick-b", pickB, setPickB, "Scenario B")}
        <div className="cmp-picker-go">
          <button type="submit" disabled={!ready}>
            Compare
          </button>
          {same ? (
            <span className="dim">Choose two different runs.</span>
          ) : !ready && (pickA !== "" || pickB !== "") ? (
            <span className="dim">A run id is 16 hexadecimal characters.</span>
          ) : null}
        </div>
      </form>
    </Panel>
  );
}

/** The comparisons the server already holds for this workspace (the demo lists its curated ones). */
function StoredComparisons() {
  const list = useAsync((signal) => api.comparisons({ signal }), []);
  const titles = useDemoTitles();
  const columns: Column<Comparison>[] = [
    {
      id: "label",
      header: "Comparison",
      rowHeader: true,
      cell: (c) => (
        <Link to={paths.comparePair(c.run_a, c.run_b)} title={c.label === "" ? undefined : c.label}>
          {c.label === "" ? `comparison ${c.id.slice(0, 12)}` : runLabelText(c.label, titles)}
        </Link>
      ),
      raw: (c) => c.label,
      sortValue: (c) => c.label,
    },
    { id: "a", header: "A", cell: (c) => <span title={`${c.label_a} (${c.run_a})`}>{c.label_a === "" ? c.run_a.slice(0, 12) : runLabelText(c.label_a, titles)}</span> },
    { id: "b", header: "B", cell: (c) => <span title={`${c.label_b} (${c.run_b})`}>{c.label_b === "" ? c.run_b.slice(0, 12) : runLabelText(c.label_b, titles)}</span> },
    {
      id: "status",
      header: "Status",
      cell: (c) => <Pill tone={c.status === "aligned" ? "ok" : "warn"} title={`Comparison status as returned by the server: ${c.status}`}>{c.status.replace(/_/g, " ")}</Pill>,
      raw: (c) => c.status,
      sortValue: (c) => c.status,
    },
    {
      id: "created",
      header: "Created",
      unit: "UTC",
      cell: (c) => <span className="mono">{fmtNsUtc(c.created_at_ns, 0)}</span>,
      sortValue: (c) => c.created_at_ns.padStart(20, "0"),
    },
  ];
  return (
    <Panel
      title="Stored comparisons"
      caption="Comparisons the server has already computed in this workspace; the public demo lists its curated ones. Open one to read it."
      flush
    >
      {list.error ? <ErrorBox error={list.error} onRetry={list.reload} /> : null}
      {list.loading && !list.data ? <Spinner label="Loading comparisons" /> : null}
      {list.data?.incomplete ? (
        <Notice tag="List incomplete">First {fmtInt(list.data.items.length)} comparisons shown; older ones are not listed here.</Notice>
      ) : null}
      {list.data ? (
        <DataTable
          label="Stored comparisons"
          rows={list.data.items}
          rowKey={(c) => c.id}
          columns={columns}
          empty="No comparison is stored in this workspace yet."
          maxHeight={360}
        />
      ) : null}
    </Panel>
  );
}

interface Lookup {
  hit: Comparison | null;
  /** The stored comparison has `run_a = b` and `run_b = a`. */
  swapped: boolean;
  incomplete: boolean;
  listed: number;
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
  const runs = useAsync((signal) => Promise.all([api.run(a, { signal }), api.run(b, { signal })]), [a, b]);
  const lookup = useAsync(async (signal): Promise<Lookup> => {
    const list = await api.readableComparisons({ signal });
    const direct = list.items.find((c) => c.run_a === a && c.run_b === b);
    const reverse = direct ? undefined : list.items.find((c) => c.run_a === b && c.run_b === a);
    return { hit: direct ?? reverse ?? null, swapped: !direct && reverse !== undefined, incomplete: list.incomplete, listed: list.items.length };
  }, [a, b]);
  const [created, setCreated] = useState<Comparison | null>(null);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<unknown>(null);

  const current: { id: string; swapped: boolean } | null = created
    ? { id: created.id, swapped: false }
    : lookup.data?.hit
      ? { id: lookup.data.hit.id, swapped: lookup.data.swapped }
      : null;
  // The list omits `alignment`; the resource itself carries it (with `pairs` empty: they are paged).
  const detail = useAsync(
    (signal) => (current === null ? Promise.resolve(null) : api.comparison(current.id, { signal })),
    [current?.id ?? null],
  );

  const create = async () => {
    setCreating(true);
    setCreateError(null);
    try {
      setCreated(await api.createComparison(a, b));
    } catch (e) {
      setCreateError(e);
    } finally {
      setCreating(false);
    }
  };
  const createButton = (text: string) => (
    <GatedButton scope="run" onClick={() => void create()} blockedReason={creating ? "Creating the comparison\u2026" : null}>
      {creating ? "Creating\u2026" : text}
    </GatedButton>
  );

  const actions = (
    <div className="cmp-actions">
      <button type="button" onClick={onSwap}>
        Swap A and B
      </button>
      <button type="button" onClick={onChange}>
        Choose other runs
      </button>
    </div>
  );

  if (runs.error) {
    const missing = runs.error instanceof ApiError && runs.error.status === 404;
    return (
      <div className="col">
        {missing ? (
          <Empty>
            At least one of the runs <span className="mono">{a}</span> and <span className="mono">{b}</span> is not
            visible in this workspace. {runs.error.message}
          </Empty>
        ) : (
          <ErrorBox error={runs.error} />
        )}
        <div className="cmp-actions">
          <button type="button" onClick={runs.reload}>
            Retry
          </button>
          <button type="button" onClick={onChange}>
            Choose other runs
          </button>
        </div>
      </div>
    );
  }
  if (!runs.data) return <Spinner label="Loading both runs" />;
  const [runA, runB] = runs.data;

  let body: ReactNode;
  if (lookup.error && current === null) {
    body = <ErrorBox error={lookup.error} onRetry={lookup.reload} />;
  } else if (current === null && lookup.loading) {
    body = <Spinner label="Looking for a stored comparison of these runs" />;
  } else if (current === null) {
    body = (
      <>
        <Panel title="No stored comparison">
          <div className="col">
            <p className="prose">
              The server holds no comparison of A <span className="mono">{a}</span> with B <span className="mono">{b}</span>
              {lookup.data?.incomplete ? (
                <>
                  {" "}
                  among the first {fmtInt(lookup.data.listed)} comparisons listed (the list is incomplete; an older one may
                  exist)
                </>
              ) : null}
              . A comparison is computed by the server from both stored runs; this page does not compare runs itself.
            </p>
            <div className="cmp-actions">{createButton("Create comparison")}</div>
            {createError ? <ErrorBox error={createError} /> : null}
          </div>
        </Panel>
        <StoredComparisons />
      </>
    );
  } else if (detail.error) {
    body = <ErrorBox error={detail.error} onRetry={detail.reload} />;
  } else if (!detail.data) {
    body = <Spinner label="Loading the comparison" />;
  } else {
    const cmp = detail.data;
    body = (
      <>
        {current.swapped ? (
          <Notice tag="A/B swapped" tone="info">
            The stored comparison has A = <span className="mono">{cmp.run_a}</span> and B ={" "}
            <span className="mono">{cmp.run_b}</span>, the reverse of this link. It is shown as stored: every delta is its B
            minus its A.{" "}
            <button type="button" onClick={onSwap}>
              Open in stored order
            </button>{" "}
            {createButton("Create comparison in this order")}
          </Notice>
        ) : null}
        {createError ? <ErrorBox error={createError} /> : null}
        <ComparisonBody cmp={cmp} runs={[runA, runB]} />
      </>
    );
  }

  return (
    <>
      {actions}
      {/* The run cards show on their own only when no stored comparison is coming; while one loads
          the spinner holds the place, so the comparison body (with its own cards) doesn't push
          earlier cards down when it arrives (layout shift 0.27 at 1440×900). */}
      {current === null && !lookup.loading ? (
        <section className="cmp-scenarios" aria-label="Scenarios">
          <ScenarioCard tag="A" label={runA.label ?? ""} run={runA} />
          <ScenarioCard tag="B" label={runB.label ?? ""} run={runB} />
        </section>
      ) : null}
      {body}
    </>
  );
}

/** The instrument of a run's dataset, when the dataset's current version is the run's version. */
function useInstrument(run: RunDetail | undefined): InstrumentMeta | null {
  const id = run?.dataset.id ?? null;
  const version = run?.dataset.version ?? null;
  const data = useAsync(async (signal) => {
    if (id === null) return null;
    const d = await api.dataset(id, { signal });
    // Ticks print as prices only with the instrument of the very version the run replayed.
    return d.current_version !== null && d.current_version.version === version ? d.current_version.manifest.instrument : null;
  }, [id, version]);
  return data.data ?? null;
}

function ComparisonBody({ cmp, runs }: { cmp: Comparison; runs: [RunDetail, RunDetail] }) {
  const runA = runs.find((r) => r.run_id === cmp.run_a);
  const runB = runs.find((r) => r.run_id === cmp.run_b);
  const instA = useInstrument(runA);
  const instB = useInstrument(runB);
  const titles = useDemoTitles();
  const equity = useAsync(
    (signal) =>
      Promise.all([
        api.equity(cmp.run_a, { max_points: EQUITY_POINTS }, { signal }),
        api.equity(cmp.run_b, { max_points: EQUITY_POINTS }, { signal }),
      ]),
    [cmp.run_a, cmp.run_b],
  );

  const alignment = cmp.status === "aligned" ? cmp.alignment : null;
  const first = alignment === null ? null : alignment.first_divergence;
  const noAlignment = cmp.eligibility.reasons.filter((r) => r.effect === "no_alignment").map((r) => r.field);
  const notAligned = (
    <Notice tag="Metrics only" tone="info">
      This comparison's status is <span title={cmp.status}>{cmp.status.replace(/_/g, " ")}</span>
      {noAlignment.length > 0 ? (
        <>
          : the runs differ in <FieldList fields={noAlignment} />
        </>
      ) : null}
      , so the same order id does not name the same decision in both runs and the server holds no per-order alignment.
    </Notice>
  );
  const sameDataset =
    runA !== undefined && runB !== undefined && runA.dataset.id === runB.dataset.id && runA.dataset.origin_ns === runB.dataset.origin_ns;

  return (
    <>
      <EvidenceLegend />
      <p className="dim small">
        Comparison <HashText hash={cmp.id} /> {cmp.label === "" ? null : <strong title={cmp.label}>{runLabelText(cmp.label, titles)}</strong>} · version{" "}
        <span className="mono">{cmp.version}</span> · comparator version <span className="mono">{cmp.comparator_version}</span>{" "}
        · created <span className="mono">{fmtNsUtc(cmp.created_at_ns, 0)}</span>
      </p>

      <section className="cmp-scenarios" aria-label="Scenarios">
        {runA ? <ScenarioCard tag="A" label={cmp.label_a} run={runA} /> : null}
        {runB ? <ScenarioCard tag="B" label={cmp.label_b} run={runB} /> : null}
      </section>

      <Panel title="Held constant">
        {runA ? <HeldConstant cmp={cmp} run={runA} /> : <Spinner label="Loading scenario A" />}
      </Panel>

      <Panel title="Changed">
        <div className="col">
          <ConfigDiffTable cmp={cmp} />
          <EligibilityNotice cmp={cmp} />
        </div>
      </Panel>

      <Panel title="First divergence">
        {alignment === null ? (
          notAligned
        ) : first === null ? (
          <Empty>
            No non-identical pair: every aligned decision is identical in both runs, timing, fees and recorded lifecycle
            included.
          </Empty>
        ) : (
          <FirstDivergence pair={first} outcomePair={alignment.first_outcome_divergence} cmp={cmp} instA={instA} instB={instB} />
        )}
      </Panel>

      <Panel title="Afterwards">
        <div className="col cmp-afterwards">
          <section className="col" aria-labelledby="cmp-outcomes">
            <h3 id="cmp-outcomes" className="cmp-subhead">Measured outcomes</h3>
            <MetricsTable rows={cmp.metrics} likeForLike={cmp.eligibility.like_for_like} />
          </section>
          <section className="col" aria-labelledby="cmp-equity">
            <h3 id="cmp-equity" className="cmp-subhead">Equity, overlaid</h3>
            {equity.error ? (
              <ErrorBox error={equity.error} onRetry={equity.reload} />
            ) : equity.data ? (
              <EquityOverlay a={equity.data[0]} b={equity.data[1]} divergenceTs={first === null ? null : first.decision_ts} sameDataset={sameDataset} />
            ) : (
              <Spinner label="Loading both equity curves" />
            )}
          </section>
          <section className="col" aria-labelledby="cmp-alignment">
            <h3 id="cmp-alignment" className="cmp-subhead">Order alignment</h3>
            {alignment === null ? notAligned : <AlignmentSummary alignment={alignment} />}
          </section>
          <section className="col" aria-labelledby="cmp-pairs">
            <h3 id="cmp-pairs" className="cmp-subhead">Order pairs</h3>
            {alignment === null ? notAligned : <PairsTable cmp={cmp} alignment={alignment} instA={instA} instB={instB} />}
          </section>
        </div>
      </Panel>

      <Panel title="What we can conclude">
        <div className="col">
          <p className="prose cmp-conclusion">
            {!cmp.eligibility.aligned ? (
              <>
                <Badge kind="warn">aggregates only</Badge> The runs differ in{" "}
                <FieldList fields={cmp.eligibility.reasons.map((r) => r.field)} />, so decisions cannot be paired and the
                metric deltas mix the configuration change with those differences.
              </>
            ) : !cmp.eligibility.like_for_like ? (
              <>
                <Badge kind="warn">not like-for-like</Badge> Decisions are paired, but the runs also differ in{" "}
                <FieldList fields={cmp.eligibility.reasons.map((r) => r.field)} />; the deltas do not isolate the
                configuration change.
              </>
            ) : (
              <>
                <Badge kind="ok">like-for-like</Badge> The runs share every eligibility field; the deltas reflect the{" "}
                {cmp.config_diffs.length === 1 ? "one configuration difference" : `${fmtInt(cmp.config_diffs.length)} configuration differences`}{" "}
                listed under Changed, on this one simulated path.
              </>
            )}
          </p>
          {cmp.interpretation.length === 0 ? (
            <Empty>The comparison returned no interpretation notes.</Empty>
          ) : (
            <ul className="cmp-notes" aria-label="Interpretation">
              {cmp.interpretation.map((note, i) => (
                <li key={i}>
                  <BasisBadge basis={note.basis} />
                  <span>{readableNote(note.statement, instA !== null && instB !== null && instA.tick_size === instB.tick_size ? instA : null)}</span>
                </li>
              ))}
            </ul>
          )}
          <p className="note">
            Not shown, because one simulated path per scenario cannot support it: statistical significance, market impact or how the strategy would
            behave live.
          </p>
        </div>
      </Panel>
    </>
  );
}

/**
 * The main assumptions of the run, each listed only when the server reported no configuration
 * difference and no eligibility reason under its path: held constant by the server's own diff.
 */
function HeldConstant({ cmp, run }: { cmp: Comparison; run: RunDetail }) {
  const c = run.config;
  const e = c.execution;
  const rows: [string, string[], ReactNode][] = [
    ["Dataset and instrument", ["dataset", "instrument"], <span className="mono">{run.dataset.id} v{run.dataset.version}</span>],
    ["Strategy", ["strategy"], <>{strategyName(run.strategy_kind)}, every {fmtDuration(run.interval_ns)}</>],
    ["Exchange model", ["execution.exchange_model"], exchangeModelLabel(e.exchange_model)],
    ["Queue model", ["execution.queue_model"], queueModelLabel(e.queue_model)],
    ["Order latency (entry / response)", ["execution.order_latency"], <span className="mono">{latencyLabel(e.order_latency)}</span>],
    ["Feed latency offset", ["execution.feed_latency_offset_ns"], <span className="mono">{fmtDuration(e.feed_latency_offset_ns)}</span>],
    ["Fees (maker / taker)", ["execution.fees"], <span className="mono">{feeLabel(e.fees)}</span>],
    ["Risk limits", ["risk"], <span className="mono">max position {fmtNum(c.risk.max_position, 4)}, max order {fmtNum(c.risk.max_order_qty, 4)}</span>],
    ["Seed", ["seed"], <span className="mono">{String(c.seed)}</span>],
    ["Simulator revision", ["simulation_revision", "engine"], <span className="mono">{run.simulation_revision}</span>],
  ];
  const touched = [...cmp.config_diffs.map((d) => d.path), ...cmp.eligibility.reasons.map((r) => r.field)];
  const held = rows.filter(([, prefixes]) => !touched.some((p) => prefixes.some((x) => p === x || p.startsWith(`${x}.`) || p.startsWith(`${x}_`))));
  if (held.length === 0) return <Empty>None of the main assumptions is the same in both runs.</Empty>;
  return (
    <dl className="kv cmp-held">
      {held.map(([label, , value]) => (
        <Fragment key={label}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </Fragment>
      ))}
    </dl>
  );
}

/** Eligibility or configuration paths by name; each raw path is its title. */
function FieldList({ fields }: { fields: string[] }) {
  return (
    <>
      {fields.map((f, i) => (
        <Fragment key={f}>
          {i > 0 ? ", " : null}
          <span title={f}>{pathLabel(f)}</span>
        </Fragment>
      ))}
    </>
  );
}

function ScenarioCard({ tag, label, run }: { tag: "A" | "B"; label: string; run: RunDetail }) {
  const { config } = run;
  const execution = config.execution;
  const latency = execution.order_latency;
  const titles = useDemoTitles();
  return (
    <article className={`cmp-scenario cmp-scenario-${tag.toLowerCase()}`}>
      <h2 title={label === "" ? undefined : label}>
        <span className="cmp-tag">{tag}</span> {label === "" ? "(no label)" : runLabelText(label, titles)}
      </h2>
      <dl>
        <div>
          <dt>Run</dt>
          <dd>
            <Link className="mono" to={paths.run(run.run_id)}>
              {run.run_id}
            </Link>
          </dd>
        </div>
        <div>
          <dt>Strategy</dt>
          <dd className="mono">
            {strategyName(run.strategy_kind)}, every {fmtDuration(run.interval_ns)}
          </dd>
        </div>
        <div>
          <dt>Dataset</dt>
          <dd className="mono" title={run.dataset.events_sha256}>
            {run.dataset.id} v{run.dataset.version} <span className="dim">events sha256 {run.dataset.events_sha256.slice(0, 12)}</span>
          </dd>
        </div>
        <div>
          <dt>Execution assumptions</dt>
          <dd className="mono">
            {exchangeModelLabel(execution.exchange_model)}, {queueModelLabel(execution.queue_model)},{" "}
            {latency.type === "constant" || latency.type === "jitter"
              ? `entry ${fmtDuration(latency.entry_ns)} / response ${fmtDuration(latency.response_ns)}${
                  latency.type === "jitter" ? ` / jitter ${fmtDuration(latency.jitter_ns)}` : ""
                }`
              : `${valueLabel(latency.type)}: ${latencyLabel(latency).replace(/cubic\(/g, "(")}`}
          </dd>
        </div>
        <div>
          <dt>Result hash</dt>
          <dd className="mono" title={run.result_sha256}>
            {run.result_sha256.slice(0, 16)}
          </dd>
        </div>
        <div>
          <dt>Orders / simulated fills</dt>
          <dd className="mono">
            {fmtInt(run.counts.orders)} / {fmtInt(run.counts.fills)}
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
      <caption className="cmp-caption">Every field of the serialized run configuration whose value differs.</caption>
      <thead>
        <tr>
          <th scope="col">Setting</th>
          <th scope="col">A</th>
          <th scope="col">B</th>
        </tr>
      </thead>
      <tbody>
        {configDiffRows(cmp.config_diffs).map((row) => (
          <tr key={row.path}>
            <th scope="row" title={row.path}>
              {pathLabel(row.path)}
            </th>
            <ConfigValue path={row.a.path} value={row.a.value} />
            <ConfigValue path={row.b.path} value={row.b.value} />
          </tr>
        ))}
      </tbody>
    </table>
  );
}

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

/** Rows exactly as served: A, B and the server's delta (B - A); each flagged like-for-like or not. */
function MetricsTable({ rows, likeForLike }: { rows: MetricRow[]; likeForLike: boolean }) {
  if (rows.length === 0) return <Empty>The comparison returned no metric rows.</Empty>;
  // Net P&L is the only row coloured, and only by its sign; no other difference is marked as
  // better or worse.
  const signClass = (row: MetricRow, value: number | null) =>
    row.key !== "net_pnl" || value === null || value === 0 ? "" : value > 0 ? " cmp-pos" : " cmp-neg";
  return (
    <table className="table cmp-metrics">
      <caption className="cmp-caption">
        {likeForLike ? null : (
          <>
            <Badge kind="warn">not like-for-like</Badge> The runs differ in the eligibility fields listed above, so these
            deltas do not isolate the configuration change.{" "}
          </>
        )}
        Values and deltas as computed by the server; delta is B minus A. A dash means the value is undefined for that
        run. Only net P&amp;L values are coloured, by sign; deltas are not, because a smaller loss is still a loss.
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
          <th scope="col">Like-for-like</th>
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
            <td className="mono cmp-num">{fmtMetricDelta(row.delta, row.unit)}</td>
            <td className="dim">{unitLabel(row.unit)}</td>
            <td>{likeForLike ? <span className="dim">yes</span> : <Badge kind="warn">no</Badge>}</td>
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
        Orders are aligned by order id, which encodes the strategy tick and slot of the decision;
        the runs share dataset, instrument, strategy kind, decision grid and simulator revision.{" "}
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
      <TimingTable alignment={alignment} />
    </div>
  );
}

const TIMING_ROWS: readonly [keyof Alignment["timing"], string][] = [
  ["decision", "Decision (local)"],
  ["exch_arrival", "Exchange arrival"],
  ["first_response", "First response (local)"],
  ["first_fill", "First fill"],
  ["completion", "Completion (terminal)"],
];

function fmtStat(ns: number | null): string {
  return ns === null ? EM_DASH : fmtSignedDuration(Math.round(ns));
}

function TimingTable({ alignment }: { alignment: Alignment }) {
  return (
    <table className="table cmp-timing">
      <caption className="cmp-caption">
        Timing deltas, B minus A, over pairs present in both runs with the same decision. A pair
        counts when both orders have the timestamp.
      </caption>
      <thead>
        <tr>
          <th scope="col">Timestamp</th>
          <th scope="col" className="cmp-num">
            Pairs
          </th>
          <th scope="col" className="cmp-num">
            Non-zero
          </th>
          <th scope="col" className="cmp-num">
            Min
          </th>
          <th scope="col" className="cmp-num">
            Median
          </th>
          <th scope="col" className="cmp-num">
            Max
          </th>
        </tr>
      </thead>
      <tbody>
        {TIMING_ROWS.map(([key, label]) => {
          const s: DeltaStats = alignment.timing[key];
          return (
            <tr key={key}>
              <th scope="row">{label}</th>
              <td className="mono cmp-num">{s.count}</td>
              <td className="mono cmp-num">{s.nonzero}</td>
              <td className="mono cmp-num">{fmtStat(s.min)}</td>
              <td className="mono cmp-num">{fmtStat(s.median)}</td>
              <td className="mono cmp-num">{fmtStat(s.max)}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
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
  const titles = useDemoTitles();
  return (
    <article className={`cmp-outcome cmp-scenario-${tag.toLowerCase()}`}>
      <h3 title={label === "" ? undefined : label}>
        <span className="cmp-tag">{tag}</span> {label === "" ? "(no label)" : runLabelText(label, titles)}
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
              <dt>Decided at (local)</dt>
              <dd>
                <ClockText ns={outcome.decision_ts} />
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
            <div>
              <dt>Completion (terminal)</dt>
              <dd>
                <ClockText ns={outcome.terminal_ts} />
              </dd>
            </div>
          </dl>
          <Link className="btn" to={paths.order(runId, orderId)}>
            Open this order's forensics in {tag}
          </Link>
        </>
      )}
    </article>
  );
}

function FirstDivergence({
  pair,
  outcomePair,
  cmp,
  instA,
  instB,
}: {
  pair: OrderPair;
  outcomePair: OrderPair | null;
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
        identical in both runs, timing, fees and recorded lifecycle included.{" "}
        {CLASS_MEANING[pair.class]}
        {pair.differences.length > 0 ? (
          <>
            {" "}Differs in:{" "}
            <span title={`Fields: ${pair.differences.join(", ")}`}>{pair.differences.map(differenceWords).join(", ")}</span>.
          </>
        ) : null}
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
      {outcomePair === null ? (
        <p className="dim">
          No aligned decision or fill differs in price, quantity or maker/taker side; the
          non-identical pairs differ only in timing, fees or recorded history.
        </p>
      ) : outcomePair.order_id === pair.order_id ? null : (
        <p className="dim">
          First difference in a decision or its fills: order{" "}
          <span className="mono">{outcomePair.order_id}</span> (strategy tick{" "}
          <span className="mono">{outcomePair.tick_no}</span>,{" "}
          <span className={`cmp-class cmp-seg-${outcomePair.class}`}>
            {CLASS_LABEL[outcomePair.class]}
          </span>
          ). The pairs before it may differ in timing, fees or recorded history only.
        </p>
      )}
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
          to={paths.order(runId, orderId)}
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
  alignment,
  instA,
  instB,
}: {
  cmp: Comparison;
  alignment: Alignment;
  instA: InstrumentMeta | null;
  instB: InstrumentMeta | null;
}) {
  const [filter, setFilter] = useState<PairClass | "all">("all");
  const total = PAIR_CLASSES.reduce((sum, cls) => sum + alignment[cls], 0);
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
            }}
          >
            <option value="all">All classes, identical included ({fmtInt(total)})</option>
            {PAIR_CLASSES.map((cls) => (
              <option key={cls} value={cls}>
                {CLASS_LABEL[cls]} ({fmtInt(alignment[cls])})
              </option>
            ))}
          </select>
        </label>
        <span className="dim">
          Pairs in decision order, read from the server {fmtInt(PAIRS_PAGE)} at a time; the counts are the comparison's
          own. Each side links to that order's forensics in its own run.
        </span>
      </div>
      <PairPages
        key={`${cmp.id}|${filter}`}
        cmp={cmp}
        cls={filter}
        expected={filter === "all" ? total : alignment[filter]}
        instA={instA}
        instB={instB}
      />
    </div>
  );
}

/** `GET /comparisons/{id}/pairs` for one class filter, page by page as the user asks for more. */
function PairPages({
  cmp,
  cls,
  expected,
  instA,
  instB,
}: {
  cmp: Comparison;
  cls: PairClass | "all";
  expected: number;
  instA: InstrumentMeta | null;
  instB: InstrumentMeta | null;
}) {
  const pairClass = cls === "all" ? undefined : cls;
  const titles = useDemoTitles();
  const first = useAsync(
    (signal) => api.comparisonPairs(cmp.id, { class: pairClass, limit: PAIRS_PAGE }, { signal }),
    [cmp.id, pairClass],
  );
  const [more, setMore] = useState<CursorPage<OrderPair>[]>([]);
  const [loadingMore, setLoadingMore] = useState(false);
  const [moreError, setMoreError] = useState<unknown>(null);
  const pending = useRef<AbortController | null>(null);
  useEffect(() => () => pending.current?.abort(), []);

  if (first.error) return <ErrorBox error={first.error} onRetry={first.reload} />;
  if (!first.data) return <Spinner label="Loading pairs" />;
  const pages = [first.data, ...more];
  const last = pages[pages.length - 1] as CursorPage<OrderPair>;
  const rows = pages.flatMap((p) => p.items);

  const loadMore = async () => {
    if (last.next_cursor === null || loadingMore) return;
    const ctl = new AbortController();
    pending.current = ctl;
    setLoadingMore(true);
    setMoreError(null);
    try {
      const page = await api.comparisonPairs(cmp.id, { class: pairClass, cursor: last.next_cursor, limit: PAIRS_PAGE }, { signal: ctl.signal });
      if (!ctl.signal.aborted) setMore((m) => [...m, page]);
    } catch (e) {
      if (!ctl.signal.aborted) setMoreError(e);
    } finally {
      if (!ctl.signal.aborted) setLoadingMore(false);
    }
  };

  if (rows.length === 0 && last.next_cursor === null) {
    return <Empty>{expected === 0 ? "No pair is in this class." : "The server returned no pair rows for this class."}</Empty>;
  }

  return (
    <>
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
              <th scope="col" rowSpan={2}>
                Differs in
              </th>
              <th scope="colgroup" colSpan={3} className="cmp-side-start" title={cmp.label_a}>
                <span className="cmp-tag">A</span> {runLabelText(cmp.label_a, titles)}
              </th>
              <th scope="colgroup" colSpan={3} className="cmp-side-start" title={cmp.label_b}>
                <span className="cmp-tag">B</span> {runLabelText(cmp.label_b, titles)}
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
            {rows.map((pair) => (
              <tr key={`${pair.order_id}|${pair.class}`}>
                <th scope="row" className="mono">
                  {pair.order_id}
                </th>
                <td className="mono cmp-num">{pair.tick_no}</td>
                <td>
                  <ClockText ns={pair.decision_ts} />
                </td>
                <td>
                  <span className={`cmp-class cmp-seg-${pair.class}`}>{CLASS_LABEL[pair.class]}</span>
                </td>
                <td className="small" title={pair.differences.join(", ")}>
                  {pair.differences.map(differenceWords).join(", ")}
                </td>
                <PairSide tag="A" runId={cmp.run_a} orderId={pair.order_id} outcome={pair.a} inst={instA} />
                <PairSide tag="B" runId={cmp.run_b} orderId={pair.order_id} outcome={pair.b} inst={instB} />
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="cmp-more">
        <span className="dim">
          {fmtInt(rows.length)} of {fmtInt(expected)} pairs loaded in {pages.length} page{pages.length === 1 ? "" : "s"}.
          {last.truncated_by === "bytes"
            ? " The last page ended early at the server's response size cap; Load more continues exactly where it stopped."
            : ""}
        </span>
        {last.next_cursor !== null ? (
          <button type="button" onClick={() => void loadMore()} disabled={loadingMore}>
            {loadingMore ? "Loading\u2026" : `Load more (up to ${fmtInt(PAIRS_PAGE)})`}
          </button>
        ) : null}
      </div>
      {moreError ? <ErrorBox error={moreError} onRetry={() => void loadMore()} /> : null}
    </>
  );
}
