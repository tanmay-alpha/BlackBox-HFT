// Experiment Comparison -> Sweep result (`/compare/sweeps/:sweepId`): one sweep as the server holds
// it (`GET /api/v1/sweeps/{id}`), re-read while any run can still change (every 2 s; 5 s while
// the API wakes or the tab is hidden). Each configuration runs once, or twice (in sample and out
// of sample) with a partition; a run's value is the metric the server serves for it, and a run
// that has not completed is shown with its state. A grid is drawn as a matrix. Once every run is
// terminal the report (`GET /api/v1/sweeps/{id}/report`) is shown: the configurations tried, the
// in-sample selection and its out-of-sample degradation, Spearman, PBO or why it is unavailable,
// and the caveats; for a seed ensemble the dispersion of every metric over the seeds. Nothing is
// estimated or interpolated.

import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";

import { api, isAbort } from "../../api/client";
import type { Sweep, SweepCell, SweepCellRun, SweepReport, SweepRunState } from "../../api/model";
import type { Segment } from "../../api/v1";
import { DataTable, type Column } from "../../components/DataTable";
import { DASH, fmtDurationExact, fmtInt, fmtStat } from "../../components/fmt";
import { strategyLabel } from "../../components/describe";
import { fmtNsUtc } from "../../components/ns";
import { SplitPane } from "../../components/SplitPane";
import { ToolbarDivider } from "../../components/Toolbar";
import { Term } from "../../components/Tooltip";
import { BasisBadge, Empty, ErrorBox, HashText, Notice, Panel, Pill, Spinner, useAsync } from "../../components/ui";
import { GatedButton } from "../../shell/access";
import { isWakingError, useApiPhase } from "../../shell/health";
import { paths } from "../../shell/paths";
import { Workspace } from "../../shell/Workspace";
import { CompareTabs } from "../experiments/CompareTabs";
import { STATE_NOTE, STATE_TONE, pollInterval } from "../sim/jobs";
import { METRICS, OBJECTIVES, axisValueLabels, buildScale, legendValues, scaleColor } from "./gridModel";
import type { MetricDef, Scale } from "./gridModel";
import type { SweepNavState } from "./SweepFormPage";
import { LIVE_SWEEP, SWEEP_TONE, axisName, designText, progressText } from "./SweepList";
import "./grid.css";

const RUN_TONE: Record<SweepRunState, "ok" | "warn" | "error" | "neutral" | "info"> = {
  ...STATE_TONE,
  detached: "neutral",
  not_submitted: "warn",
};
const RUN_NOTE: Record<SweepRunState, string> = {
  ...STATE_NOTE,
  detached: "This run's experiment was detached from its job.",
  not_submitted: "This run was never queued (the submission was interrupted); submitting the same sweep again queues it.",
};

const SEGMENT_LABEL: Record<Segment, string> = { full: "whole dataset", in_sample: "in sample", out_of_sample: "out of sample" };

export default function SweepPage() {
  const { sweepId = "" } = useParams();
  return <SweepView key={sweepId} id={sweepId} />;
}

/** The sweep, re-read while it is live. Older versions than the one shown are ignored. */
function usePolledSweep(id: string) {
  const phase = useApiPhase();
  const phaseRef = useRef(phase);
  useEffect(() => {
    phaseRef.current = phase;
  }, [phase]);
  const latest = useRef<Sweep | null>(null);
  const [sweep, setSweep] = useState<Sweep | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [round, setRound] = useState(0);

  useEffect(() => {
    const ctl = new AbortController();
    let timer: number | undefined;
    const tick = async () => {
      try {
        const fresh = await api.sweep(id, { signal: ctl.signal });
        if (ctl.signal.aborted) return;
        const shown = latest.current !== null && fresh.version < latest.current.version ? latest.current : fresh;
        latest.current = shown;
        setSweep(shown);
        setError(null);
        if (LIVE_SWEEP[shown.state]) timer = window.setTimeout(() => void tick(), pollInterval(phaseRef.current));
      } catch (e) {
        if (ctl.signal.aborted || isAbort(e)) return;
        setError(e);
        // A waking API is retried at the slow interval; any other failure waits for Retry.
        if (isWakingError(e)) timer = window.setTimeout(() => void tick(), pollInterval("waking"));
      }
    };
    void tick();
    return () => {
      ctl.abort();
      window.clearTimeout(timer);
    };
  }, [id, round]);

  const accept = (next: Sweep) => {
    if (latest.current === null || next.version >= latest.current.version) {
      latest.current = next;
      setSweep(next);
    }
  };
  return { sweep, error, restart: () => setRound((n) => n + 1), accept };
}

/** `param = label, …` of a configuration. */
function paramsText(params: SweepCell["params"]): string {
  return params.map((p) => `${axisName(p.param)} = ${p.label}`).join(", ");
}

function windowText(w: { start: number; end: number }): string {
  return `[${fmtInt(w.start)}, ${fmtInt(w.end)}) ns, ${fmtDurationExact(w.end - w.start)}`;
}

function SweepView({ id }: { id: string }) {
  const { sweep, error, restart, accept } = usePolledSweep(id);
  const location = useLocation();
  const opened = (location.state as SweepNavState | null) ?? null;
  const [metricKey, setMetricKey] = useState<MetricDef["key"]>("net_pnl");
  const def = METRICS.find((m) => m.key === metricKey) ?? (METRICS[0] as MetricDef);
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState<unknown>(null);

  const cancel = async () => {
    setCancelling(true);
    setCancelError(null);
    try {
      accept(await api.cancelSweep(id));
      restart();
    } catch (e) {
      setCancelError(e);
    } finally {
      setCancelling(false);
    }
  };

  const live = sweep !== null && LIVE_SWEEP[sweep.state];
  const toolbar = (
    <>
      <CompareTabs />
      <ToolbarDivider />
      {live ? (
        <GatedButton scope="run" className="btn-danger" onClick={() => void cancel()} blockedReason={cancelling ? "Cancelling\u2026" : null}>
          {cancelling ? "Cancelling\u2026" : "Cancel sweep"}
        </GatedButton>
      ) : null}
      <Link to={paths.sweepForm()}>New sweep</Link>
    </>
  );

  if (sweep === null) {
    return (
      <Workspace context={id.slice(0, 12)} toolbar={toolbar} scroll>
        <div className="pad-3">{error ? <ErrorBox error={error} onRetry={restart} /> : <Spinner label="Loading the sweep" />}</div>
      </Workspace>
    );
  }

  const objective = OBJECTIVES.find((o) => o.metric === sweep.objective.metric)?.label ?? sweep.objective.metric;
  const summary = (
    <Panel title={sweep.label === "" ? `Sweep ${sweep.id.slice(0, 12)}` : sweep.label} basis="modeled">
      {opened?.existing ? (
        <Notice tag="Existing" tone="info">
          Same sweep already exists in this workspace; the server returned it instead of starting another, and it is
          opened here.
        </Notice>
      ) : null}
      {error ? <ErrorBox error={error} onRetry={restart} /> : null}
      {cancelError ? <ErrorBox error={cancelError} /> : null}
      <dl className="kv">
        <dt>State</dt>
        <dd>
          <Pill tone={SWEEP_TONE[sweep.state]}>{sweep.state}</Pill>
          {live ? <span className="dim"> re-read every 2 s while live (5 s while the API wakes or this tab is hidden)</span> : null}
        </dd>
        <dt>Progress</dt>
        <dd className="mono">{progressText(sweep.progress)}</dd>
        <dt>Design</dt>
        <dd>{designText(sweep)}</dd>
        {sweep.partition ? (
          <>
            <dt>In sample</dt>
            <dd className="mono">{windowText(sweep.partition.in_sample)}</dd>
            <dt>Out of sample</dt>
            <dd className="mono">{windowText(sweep.partition.out_of_sample)}</dd>
          </>
        ) : null}
        {sweep.design !== "seeds" ? (
          <>
            <dt>Objective</dt>
            <dd>
              {sweep.objective.direction === "maximize" ? "maximise" : "minimise"} {objective}
              {sweep.spec?.cscv_blocks ? <span className="dim"> {"\u00b7"} CSCV with {sweep.spec.cscv_blocks} blocks</span> : null}
            </dd>
          </>
        ) : null}
        {sweep.spec ? (
          <>
            <dt>Base configuration</dt>
            <dd>
              strategy <span>{strategyLabel(sweep.spec.base.strategy.kind)}</span> on dataset{" "}
              <span className="mono">{sweep.spec.base.dataset_id}</span>
            </dd>
          </>
        ) : null}
        <dt>Sweep</dt>
        <dd>
          <HashText hash={sweep.id} /> <span className="dim">version {sweep.version} {"\u00b7"} spec <HashText hash={sweep.fingerprint} /></span>
        </dd>
        <dt>Created</dt>
        <dd className="mono">{fmtNsUtc(sweep.created_at_ns, 0)}</dd>
      </dl>
    </Panel>
  );

  const showMatrix = sweep.design === "grid" && sweep.axes.length >= 1 && sweep.axes.length <= 2;
  return (
    <Workspace context={sweep.label === "" ? sweep.id.slice(0, 12) : sweep.label} toolbar={toolbar} layout>
      <SplitPane
        id="compare.sweep.result"
        orientation="vertical"
        panes={[
          { id: "summary", label: "Sweep", defaultSize: "22%", minSize: "10%", content: summary },
          { id: "report", label: "Report", defaultSize: "30%", minSize: "10%", content: <ReportPanel sweep={sweep} /> },
          ...(showMatrix
            ? [
                {
                  id: "matrix",
                  label: "Result matrix",
                  defaultSize: "26%",
                  minSize: "12%",
                  content: <MatrixPanel sweep={sweep} def={def} onMetric={setMetricKey} />,
                },
              ]
            : []),
          {
            id: "runs",
            label: "Runs",
            defaultSize: showMatrix ? "22%" : "48%",
            minSize: "12%",
            content: <RunsPanel sweep={sweep} def={def} onMetric={showMatrix ? null : setMetricKey} />,
          },
        ]}
      />
    </Workspace>
  );
}

// ---------------------------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------------------------

function ReportPanel({ sweep }: { sweep: Sweep }) {
  const ready = sweep.report_ready;
  const report = useAsync((signal) => (ready ? api.sweepReport(sweep.id, { signal }) : Promise.resolve(null)), [sweep.id, ready]);
  const p = sweep.progress;
  return (
    <Panel
      title={sweep.design === "seeds" ? "Seed ensemble report" : "Sweep report"}
      basis="modeled"
      caption="Built by the server once every run is terminal. Simulated results only; nothing here is an exchange execution."
    >
      {!ready ? (
        <Notice tag="Pending" tone="info">
          The report is built once every run has completed, failed, expired or been cancelled ({progressText(p)}).
        </Notice>
      ) : report.error ? (
        <ErrorBox error={report.error} onRetry={report.reload} />
      ) : report.data === null ? (
        <Spinner label="Building the report" />
      ) : report.data.ensemble ? (
        <EnsembleView report={report.data} />
      ) : report.data.analysis ? (
        <AnalysisView report={report.data} />
      ) : (
        <Empty>The server returned no analysis for this sweep.</Empty>
      )}
    </Panel>
  );
}

function Caveats({ caveats }: { caveats: { code: string; text: string }[] }) {
  return (
    <>
      <h3 className="grd-subhead">Caveats</h3>
      <ul className="grd-caveats">
        {caveats.map((c) => (
          <li key={c.code} data-code={c.code}>
            {c.text}
          </li>
        ))}
      </ul>
    </>
  );
}

function AnalysisView({ report }: { report: SweepReport }) {
  const a = report.analysis;
  if (!a) return null;
  const k = a.configurations;
  const s = a.selected ?? null;
  const selectedCell = s === null ? null : report.cells.find((c) => c.index === s.cell) ?? null;
  const unit = a.unit;
  const rc = a.rank_correlation;
  const pbo = a.pbo;
  return (
    <>
      <dl className="kv">
        <dt>Configurations tried</dt>
        <dd>
          <span className="mono">{fmtInt(k.tried)}</span>{" "}
          <span className="dim">
            ({fmtInt(k.completed_in_sample)} completed {report.partition ? "in sample" : ""}
            {report.partition ? `, ${fmtInt(k.completed_out_of_sample)} out of sample` : ""}, {fmtInt(k.failed)} failed, {fmtInt(k.missing)} missing). Every declared
            configuration counts as a trial.
          </span>
        </dd>
        <dt>Selected</dt>
        <dd>
          {s === null ? (
            "nothing: no configuration has an in-sample value"
          ) : (
            <>
              cell <span className="mono">{s.cell}</span>
              {selectedCell ? <> ({paramsText(selectedCell.params)})</> : null}
              {s.tied_with > 0 ? <span className="dim"> {"\u00b7"} tied with {s.tied_with} other(s); the lowest index is selected</span> : null}
            </>
          )}
        </dd>
        {s !== null ? (
          <>
            <dt>{report.partition ? "In sample" : "Value"}</dt>
            <dd className="mono">
              {fmtStat(s.in_sample)} {unit}
              {s.in_sample_per_second != null ? <span className="dim"> ({fmtStat(s.in_sample_per_second)} {unit}/s)</span> : null}
            </dd>
            {report.partition ? (
              <>
                <dt>Out of sample</dt>
                <dd className="mono">
                  {fmtStat(s.out_of_sample)} {unit}
                  {s.out_of_sample_per_second != null ? <span className="dim"> ({fmtStat(s.out_of_sample_per_second)} {unit}/s)</span> : null}
                  {s.out_of_sample_rank != null ? (
                    <span className="dim">
                      {" "}
                      {"\u00b7"} rank {s.out_of_sample_rank} of {s.out_of_sample_of}
                    </span>
                  ) : null}
                </dd>
                <dt>IS {"\u2192"} OOS degradation</dt>
                <dd className="mono">
                  {fmtStat(s.degradation)} {unit}
                  {s.relative_degradation != null ? <span className="dim"> ({fmtStat(s.relative_degradation * 100)}% of the in-sample value)</span> : null}
                  {s.degradation_per_second != null ? <span className="dim"> {"\u00b7"} {fmtStat(s.degradation_per_second)} {unit}/s</span> : null}
                </dd>
              </>
            ) : null}
          </>
        ) : null}
        <dt>Spearman (IS vs OOS)</dt>
        <dd>
          {rc.status === "available" ? (
            <span className="mono">
              {"\u03c1"} = {fmtStat(rc.spearman_rho)} over {fmtInt(rc.cells)} configurations
            </span>
          ) : (
            <span className="dim">unavailable: {rc.reason}</span>
          )}
        </dd>
        <dt>
          PBO <span className="dim">(probability of backtest overfitting)</span>
        </dt>
        <dd>
          {pbo.status === "available" ? (
            <span className="mono">
              {fmtStat(pbo.pbo)} <span className="dim">(CSCV, {pbo.blocks} blocks, {fmtInt(pbo.combinations)} splits, {pbo.cells} configurations)</span>
            </span>
          ) : (
            <span className="dim">unavailable: {pbo.reason}</span>
          )}
        </dd>
        <dt>Selection bias</dt>
        <dd className="mono">
          z = {fmtStat(a.selection_bias.selected_z)}{" "}
          <span className="dim">vs {fmtStat(a.selection_bias.expected_max_z_under_noise)} expected for the best of {a.selection_bias.trials} noise trials</span>
        </dd>
      </dl>
      <Caveats caveats={a.caveats} />
      <h3 className="grd-subhead">Configurations</h3>
      <div className="table-scroll">
        <table className="grd-report-table">
          <thead>
            <tr>
              <th scope="col">Cell</th>
              <th scope="col">Parameters</th>
              <th scope="col">{report.partition ? "In sample" : "Value"} ({unit})</th>
              {report.partition ? <th scope="col">Out of sample ({unit})</th> : null}
              <th scope="col">Runs</th>
            </tr>
          </thead>
          <tbody>
            {report.cells.map((c) => (
              <tr key={c.index} className={s !== null && c.index === s.cell ? "is-selected" : undefined}>
                <td className="mono">{c.index}</td>
                <td>{paramsText(c.params)}</td>
                <td className="mono num">{fmtStat(c.in_sample)}</td>
                {report.partition ? <td className="mono num">{fmtStat(c.out_of_sample)}</td> : null}
                <td className="mono">
                  {c.error ? <span className="dim">{c.error}</span> : null}
                  {[c.in_sample_run_id, c.out_of_sample_run_id].map((rid, i) =>
                    rid ? (
                      <span key={i}>
                        {i > 0 ? " \u00b7 " : ""}
                        <Link to={paths.run(rid)}>{rid.slice(0, 12)}</Link>
                      </span>
                    ) : null,
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}

function EnsembleView({ report }: { report: SweepReport }) {
  const e = report.ensemble;
  if (!e) return null;
  return (
    <>
      {e.collapse_reason ? (
        <Notice tag="Collapsed" tone="warn">
          {e.collapse_reason}
        </Notice>
      ) : (
        <p className="grd-summary">
          <BasisBadge basis="modeled" /> {fmtInt(e.seed_count)} seeds of one configuration; the order-latency model draws from the seed, and the runs differ.
        </p>
      )}
      {e.segments.map((seg) => (
        <section key={seg.segment}>
          <h3 className="grd-subhead">
            {SEGMENT_LABEL[seg.segment]} {"\u00b7"} {fmtInt(seg.completed)} of {fmtInt(seg.members.length)} seeds completed
          </h3>
          <div className="table-scroll">
            <table className="grd-report-table">
              <thead>
                <tr>
                  <th scope="col">Metric</th>
                  <th scope="col">Mean</th>
                  <th scope="col">Std dev</th>
                  <th scope="col">Min</th>
                  <th scope="col">Max</th>
                  <th scope="col">n</th>
                </tr>
              </thead>
              <tbody>
                {seg.metrics.map((m) => (
                  <tr key={m.metric}>
                    <th scope="row">
                      {OBJECTIVES.find((o) => o.metric === m.metric)?.label ?? m.metric} <span className="dim">({m.unit})</span>
                    </th>
                    <td className="mono num">{fmtStat(m.mean)}</td>
                    <td className="mono num">{m.std_dev == null ? DASH : fmtStat(m.std_dev)}</td>
                    <td className="mono num">{fmtStat(m.min)}</td>
                    <td className="mono num">{fmtStat(m.max)}</td>
                    <td className="mono num">{fmtInt(m.n)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="grd-help">
            Runs by seed:{" "}
            {seg.members.map((m, i) => (
              <span key={m.cell} className="mono">
                {i > 0 ? " \u00b7 " : ""}
                {m.seed}: {m.run_id ? <Link to={paths.run(m.run_id)}>{m.run_id.slice(0, 12)}</Link> : (m.error ?? "no run")}
              </span>
            ))}
          </p>
        </section>
      ))}
      <Caveats caveats={e.caveats} />
    </>
  );
}

// ---------------------------------------------------------------------------------------------
// Grid matrix
// ---------------------------------------------------------------------------------------------

function runValue(run: SweepCellRun | undefined, def: MetricDef): number | null {
  const v = !run || run.metrics === null ? null : run.metrics[def.key];
  return v !== null && v !== undefined && Number.isFinite(v) ? v : null;
}

function RunLinks({ run }: { run: SweepCellRun }) {
  return (
    <span className="grd-cell-run mono">
      {run.run_id !== null ? <Link to={paths.run(run.run_id)}>run {run.run_id.slice(0, 12)}</Link> : null}
      {run.run_id !== null && run.job_id !== null ? " \u00b7 " : null}
      {run.job_id !== null ? <Link to={paths.job(run.job_id)}>job</Link> : null}
    </span>
  );
}

function Legend({ scale, def }: { scale: Scale; def: MetricDef }) {
  const stops = legendValues(scale);
  const middle = (stops.length - 1) / 2;
  return (
    <div className="grd-legend">
      <span className="dim">
        {scale.kind === "diverging"
          ? "Diverging scale centred on zero, symmetric to the largest magnitude."
          : "Sequential scale from the smallest to the largest cell value."}
      </span>
      <ol className="grd-stops" aria-label="Colour scale">
        {stops.map((value, i) => (
          <li key={i}>
            <span className="grd-swatch" style={{ background: scaleColor(scale, value) }} aria-hidden="true" />
            <span className="mono">
              {i === 0 || i === stops.length - 1 || (scale.kind === "diverging" && i === middle) ? def.format(value) : ""}
            </span>
          </li>
        ))}
      </ol>
      <span className="grd-legend-note">
        <span className="grd-swatch grd-swatch-missing" aria-hidden="true" /> no value: the run has not completed (its state is printed)
      </span>
    </div>
  );
}

function HeatCell({ run, def, scale }: { run: SweepCellRun | undefined; def: MetricDef; scale: Scale | null }) {
  if (!run) {
    return (
      <td className="grd-cell grd-missing">
        <span className="grd-cell-state">absent</span>
        <span className="grd-cell-error">the sweep lists no run for this position</span>
      </td>
    );
  }
  const value = runValue(run, def);
  if (run.state !== "completed" || value === null || scale === null) {
    return (
      <td className="grd-cell grd-missing" title={RUN_NOTE[run.state]}>
        <span className="grd-cell-state">
          <Pill tone={RUN_TONE[run.state]}>{run.state}</Pill>
        </span>
        <span className="grd-cell-error">
          {run.state === "completed" ? "completed, but the server served no value for this metric" : (run.error ?? RUN_NOTE[run.state])}
        </span>
        <RunLinks run={run} />
      </td>
    );
  }
  return (
    <td className="grd-cell" style={{ background: scaleColor(scale, value) }} title={`run ${run.run_id ?? "unknown"}`}>
      {run.run_id === null ? (
        <span className="grd-cell-value mono">{def.format(value)}</span>
      ) : (
        <Link className="grd-cell-value mono" to={paths.run(run.run_id)} aria-label={`${def.format(value)}, open run ${run.run_id}`}>
          {def.format(value)}
        </Link>
      )}
      <RunLinks run={run} />
    </td>
  );
}

/** Readable value labels of grid axis `i`, falling back to the labels the server sent. */
function axisLabels(sweep: Sweep, i: number): string[] {
  const design = sweep.spec?.design;
  const friendly = design?.type === "grid" && design.axes[i] ? axisValueLabels(design.axes[i]) : [];
  return (sweep.axes[i]?.labels ?? [""]).map((label, j) => friendly[j] ?? label);
}

function MetricSelect({ id, def, onMetric }: { id: string; def: MetricDef; onMetric: (key: MetricDef["key"]) => void }) {
  return (
    <label className="row grd-metric" htmlFor={id}>
      <span>Metric</span>
      <select
        id={id}
        value={def.key}
        onChange={(e) => {
          const next = METRICS.find((m) => m.key === e.target.value);
          if (next) onMetric(next.key);
        }}
      >
        {METRICS.map((m) => (
          <option key={m.key} value={m.key}>
            {m.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function MatrixPanel({ sweep, def, onMetric }: { sweep: Sweep; def: MetricDef; onMetric: (key: MetricDef["key"]) => void }) {
  const segments: Segment[] = sweep.partition ? ["in_sample", "out_of_sample"] : ["full"];
  const [segment, setSegment] = useState<Segment>(segments[0] as Segment);
  const xLabels = axisLabels(sweep, 0);
  const yLabels = sweep.axes.length > 1 ? axisLabels(sweep, 1) : [""];
  const cells = useMemo(() => sweep.cells ?? [], [sweep.cells]);

  // Keyed by each cell's own coordinates, so a short or reordered list cannot shift values into
  // the wrong position.
  const byPosition = useMemo(() => {
    const map = new Map<string, SweepCellRun>();
    for (const cell of cells) {
      const run = cell.runs.find((r) => r.segment === segment);
      if (run) map.set(`${cell.coords[0] ?? 0},${cell.coords[1] ?? 0}`, run);
    }
    return map;
  }, [cells, segment]);

  const scale = useMemo(() => {
    const values: number[] = [];
    for (const run of byPosition.values()) {
      const v = run.state === "completed" ? runValue(run, def) : null;
      if (v !== null) values.push(v);
    }
    return buildScale(def, values);
  }, [byPosition, def]);

  const runsHere = [...byPosition.values()];
  const done = runsHere.filter((r) => r.state === "completed").length;
  const notDone = runsHere.length - done;
  const live = LIVE_SWEEP[sweep.state];
  const xName = axisName(sweep.axes[0]?.axis ?? "");
  const yName = sweep.axes.length > 1 ? axisName(sweep.axes[1]?.axis ?? "") : "";

  return (
    <Panel
      title="Result matrix"
      basis="modeled"
      caption="Each completed cell is one simulation; its value links to the run behind it, and every cell links to its job."
      actions={
        <span className="row">
          {segments.length > 1 ? (
            <select aria-label="Window" value={segment} onChange={(e) => setSegment(e.target.value as Segment)}>
              {segments.map((s) => (
                <option key={s} value={s}>
                  {SEGMENT_LABEL[s]}
                </option>
              ))}
            </select>
          ) : null}
          <MetricSelect id="grd-metric" def={def} onMetric={onMetric} />
        </span>
      }
    >
      <p className="grd-summary">
        <BasisBadge basis="modeled" /> <strong>{def.term ? <Term term={def.term}>{def.label}</Term> : def.label}</strong>{" "}
        <span className="unit">{def.unit}</span> {SEGMENT_LABEL[segment]} for {fmtInt(done)} completed of {fmtInt(runsHere.length)} cells.
      </p>
      {live && notDone > 0 ? (
        <Notice tag="Live" tone="info">
          {fmtInt(notDone)} cell{notDone === 1 ? " has" : "s have"} not completed yet; each fills in when its job completes.
          Nothing is estimated in the meantime.
        </Notice>
      ) : notDone > 0 ? (
        <Notice tag="Missing" tone="error">
          {fmtInt(notDone)} cell{notDone === 1 ? " did" : "s did"} not complete and {notDone === 1 ? "is" : "are"} shown as such.
        </Notice>
      ) : null}
      <div className="table-scroll">
        <table className="grd-heat">
          <caption className="grd-caption">
            {def.label} per cell ({def.unit}), {SEGMENT_LABEL[segment]}. Each value links to the run behind it.
          </caption>
          <thead>
            <tr>
              <th scope="col" className="grd-corner">
                {yName !== "" ? (
                  <>
                    <span>{yName}</span>
                    <span className="dim"> by </span>
                  </>
                ) : null}
                <span>{xName}</span>
              </th>
              {xLabels.map((label, xi) => (
                <th key={xi} scope="col" className="mono" title={sweep.axes[0]?.labels[xi]}>
                  {label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {yLabels.map((label, yi) => (
              <tr key={yi}>
                <th scope="row" className="mono" title={sweep.axes[1]?.labels[yi]}>
                  {label}
                </th>
                {xLabels.map((_, xi) => (
                  <HeatCell key={xi} run={byPosition.get(`${xi},${yi}`)} def={def} scale={scale} />
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {scale ? <Legend scale={scale} def={def} /> : <p className="dim">No completed cell has a value for this metric yet, so there is no scale.</p>}
      <Notice tag="Modeled" tone="modeled">
        Each completed cell is one simulation, identified by its run id (listed in Runs). The matrix shows how sensitive the
        simulation model's output is to these assumptions on this dataset. It is not a prediction of live trading results,
        and cells that did not complete are never filled in from their neighbours.
      </Notice>
    </Panel>
  );
}

// ---------------------------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------------------------

interface RunRow {
  cell: SweepCell;
  run: SweepCellRun;
  params: string;
  value: number | null;
}

function RunsPanel({ sweep, def, onMetric }: { sweep: Sweep; def: MetricDef; onMetric: ((key: MetricDef["key"]) => void) | null }) {
  const navigate = useNavigate();
  const rows: RunRow[] = (sweep.cells ?? []).flatMap((cell) =>
    cell.runs.map((run) => ({
      cell,
      run,
      params: paramsText(cell.params),
      value: run.state === "completed" ? runValue(run, def) : null,
    })),
  );
  const columns: Column<RunRow>[] = [
    { id: "cell", header: "Cell", numeric: true, cell: (r) => <span className="mono">{r.cell.index}</span>, sortValue: (r) => r.cell.index },
    { id: "params", header: "Parameters", cell: (r) => <span>{r.params}</span>, raw: (r) => r.params, sortValue: (r) => r.params },
    {
      id: "segment",
      header: "Window",
      cell: (r) => SEGMENT_LABEL[r.run.segment],
      raw: (r) => r.run.segment,
      sortValue: (r) => r.run.segment,
    },
    {
      id: "state",
      header: "State",
      cell: (r) => (
        <span title={r.run.error ?? RUN_NOTE[r.run.state]}>
          <Pill tone={RUN_TONE[r.run.state]}>{r.run.state}</Pill>
        </span>
      ),
      raw: (r) => r.run.state,
      sortValue: (r) => r.run.state,
    },
    {
      id: "value",
      header: def.label,
      unit: def.unit,
      term: def.term,
      numeric: true,
      cell: (r) => (r.value === null ? DASH : def.format(r.value)),
      sortValue: (r) => r.value,
      className: (r) => (!def.signed || r.value === null || r.value === 0 ? undefined : r.value > 0 ? "up" : "down"),
    },
    {
      id: "run",
      header: "Run",
      rowHeader: true,
      cell: (r) =>
        r.run.run_id === null ? (
          DASH
        ) : (
          <Link to={paths.run(r.run.run_id)}>
            <HashText hash={r.run.run_id} />
          </Link>
        ),
      raw: (r) => r.run.run_id ?? "",
    },
    {
      id: "job",
      header: "Job",
      cell: (r) =>
        r.run.job_id === null ? (
          DASH
        ) : (
          <Link to={paths.job(r.run.job_id)}>
            <HashText hash={r.run.job_id} />
          </Link>
        ),
      raw: (r) => r.run.job_id ?? "",
    },
  ];
  return (
    <Panel
      title="Runs"
      flush
      caption="One row per run: its configuration, window, state, value when completed, the run behind it and its job."
      actions={onMetric ? <MetricSelect id="grd-runs-metric" def={def} onMetric={onMetric} /> : undefined}
    >
      {sweep.cells === null ? (
        <Empty>The server returned this sweep without its cells.</Empty>
      ) : (
        <DataTable
          label="Sweep runs"
          rows={rows}
          rowKey={(r) => `${r.cell.index},${r.run.segment}`}
          columns={columns}
          onActivate={(r) => {
            if (r.run.run_id !== null) navigate(paths.run(r.run.run_id));
            else if (r.run.job_id !== null) navigate(paths.job(r.run.job_id));
          }}
          empty="The sweep lists no runs."
          fill
        />
      )}
    </Panel>
  );
}
