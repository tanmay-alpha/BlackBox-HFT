// Experiment Comparison -> Sweep result (`/compare/sweeps/:sweepId`): one sweep as the server holds
// it (`GET /api/v1/sweeps/{id}`), re-read while any cell can still change (every 2 s; 5 s while
// the API wakes or the tab is hidden). Each cell is one job; its value is the metric the server
// serves for that cell (net P&L, fills, fees, max drawdown), and a cell that has not completed is
// hatched and labelled with its state. Nothing is estimated or interpolated.

import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";

import { api, isAbort } from "../../api/client";
import type { ExperimentStatus, Sweep, SweepCell } from "../../api/model";
import { DataTable, type Column } from "../../components/DataTable";
import { DASH, fmtInt } from "../../components/fmt";
import { fmtNsUtc } from "../../components/ns";
import { SplitPane } from "../../components/SplitPane";
import { ToolbarDivider } from "../../components/Toolbar";
import { Term } from "../../components/Tooltip";
import { BasisBadge, Empty, ErrorBox, HashText, Notice, Panel, Pill, Spinner } from "../../components/ui";
import { GatedButton } from "../../shell/access";
import { isWakingError, useApiPhase } from "../../shell/health";
import { paths } from "../../shell/paths";
import { Workspace } from "../../shell/Workspace";
import { CompareTabs } from "../experiments/CompareTabs";
import { STATE_NOTE, STATE_TONE, pollInterval } from "../sim/jobs";
import { METRICS, axisValueLabels, buildScale, legendValues, scaleColor } from "./gridModel";
import type { MetricDef, Scale } from "./gridModel";
import type { SweepNavState } from "./SweepFormPage";
import { LIVE_SWEEP, SWEEP_TONE, axisName, progressText } from "./SweepList";
import "./grid.css";

const CELL_TONE: Record<ExperimentStatus, "ok" | "warn" | "error" | "neutral" | "info"> = { ...STATE_TONE, detached: "neutral" };
const CELL_NOTE: Record<ExperimentStatus, string> = {
  ...STATE_NOTE,
  detached: "This cell's experiment was detached from its job.",
};

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
        <dt>Columns (X)</dt>
        <dd>
          {axisName(sweep.x.axis)} <span className="dim">({sweep.x.labels.length} values)</span>
        </dd>
        <dt>Rows (Y)</dt>
        <dd>
          {axisName(sweep.y.axis)} <span className="dim">({sweep.y.labels.length} values)</span>
        </dd>
        {sweep.spec ? (
          <>
            <dt>Base configuration</dt>
            <dd>
              strategy <span className="mono">{sweep.spec.base.strategy.kind.type}</span> on dataset{" "}
              <span className="mono">{sweep.spec.base.dataset_id}</span>
            </dd>
          </>
        ) : null}
        <dt>Sweep</dt>
        <dd>
          <HashText hash={sweep.id} /> <span className="dim">version {sweep.version}</span>
        </dd>
        <dt>Created</dt>
        <dd className="mono">{fmtNsUtc(sweep.created_at_ns, 0)} UTC</dd>
      </dl>
    </Panel>
  );

  return (
    <Workspace context={sweep.label === "" ? sweep.id.slice(0, 12) : sweep.label} toolbar={toolbar} layout>
      <SplitPane
        id="compare.sweep.result"
        orientation="vertical"
        panes={[
          { id: "summary", label: "Sweep", defaultSize: "24%", minSize: "10%", content: summary },
          {
            id: "matrix",
            label: "Result matrix",
            defaultSize: "46%",
            minSize: "20%",
            content: <MatrixPanel sweep={sweep} def={def} onMetric={setMetricKey} />,
          },
          { id: "cells", label: "Cells", defaultSize: "30%", minSize: "12%", content: <CellsPanel sweep={sweep} def={def} /> },
        ]}
      />
    </Workspace>
  );
}

/** Readable labels when the sweep returned its spec, falling back to the labels the server sent. */
function axisLabels(sweep: Sweep, which: "x" | "y"): string[] {
  const friendly = sweep.spec ? axisValueLabels(sweep.spec[which]) : [];
  return sweep[which].labels.map((label, i) => friendly[i] ?? label);
}

function cellValue(cell: SweepCell, def: MetricDef): number | null {
  const v = cell.metrics === null ? null : cell.metrics[def.key];
  return v !== null && Number.isFinite(v) ? v : null;
}

function CellLinks({ cell }: { cell: SweepCell }) {
  return (
    <span className="grd-cell-run mono">
      {cell.run_id !== null ? <Link to={paths.run(cell.run_id)}>run {cell.run_id.slice(0, 12)}</Link> : null}
      {cell.run_id !== null && cell.job_id !== null ? " \u00b7 " : null}
      {cell.job_id !== null ? <Link to={paths.job(cell.job_id)}>job</Link> : null}
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
        <span className="grd-swatch grd-swatch-missing" aria-hidden="true" /> no value: the cell has not completed (its state is printed)
      </span>
    </div>
  );
}

function HeatCell({ cell, def, scale }: { cell: SweepCell | undefined; def: MetricDef; scale: Scale | null }) {
  if (!cell) {
    return (
      <td className="grd-cell grd-missing">
        <span className="grd-cell-state">absent</span>
        <span className="grd-cell-error">the sweep lists no cell for this position</span>
      </td>
    );
  }
  const value = cellValue(cell, def);
  if (cell.state !== "completed" || value === null || scale === null) {
    return (
      <td className="grd-cell grd-missing" title={CELL_NOTE[cell.state]}>
        <span className="grd-cell-state">
          <Pill tone={CELL_TONE[cell.state]}>{cell.state}</Pill>
        </span>
        <span className="grd-cell-error">
          {cell.state === "completed" ? "completed, but the server served no value for this metric" : CELL_NOTE[cell.state]}
        </span>
        <CellLinks cell={cell} />
      </td>
    );
  }
  return (
    <td className="grd-cell" style={{ background: scaleColor(scale, value) }} title={`run ${cell.run_id ?? "unknown"}`}>
      {cell.run_id === null ? (
        <span className="grd-cell-value mono">{def.format(value)}</span>
      ) : (
        <Link className="grd-cell-value mono" to={paths.run(cell.run_id)} aria-label={`${def.format(value)}, open run ${cell.run_id}`}>
          {def.format(value)}
        </Link>
      )}
      <CellLinks cell={cell} />
    </td>
  );
}

function MatrixPanel({ sweep, def, onMetric }: { sweep: Sweep; def: MetricDef; onMetric: (key: MetricDef["key"]) => void }) {
  const xLabels = axisLabels(sweep, "x");
  const yLabels = axisLabels(sweep, "y");
  const cells = sweep.cells ?? [];

  // Keyed by each cell's own coordinates, so a short or reordered list cannot shift values into
  // the wrong position.
  const byPosition = useMemo(() => {
    const map = new Map<string, SweepCell>();
    for (const cell of cells) map.set(`${cell.x},${cell.y}`, cell);
    return map;
  }, [cells]);

  const scale = useMemo(() => {
    const values: number[] = [];
    for (const cell of cells) {
      const v = cell.state === "completed" ? cellValue(cell, def) : null;
      if (v !== null) values.push(v);
    }
    return buildScale(def, values);
  }, [cells, def]);

  const p = sweep.progress;
  const live = LIVE_SWEEP[sweep.state];
  const notDone = p.cells_total - p.completed;

  return (
    <Panel
      title="Result matrix"
      basis="modeled"
      caption="Each completed cell is one simulation; its value links to the run behind it, and every cell links to its job."
      actions={
        <label className="row grd-metric" htmlFor="grd-metric">
          <span>Metric</span>
          <select
            id="grd-metric"
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
      }
    >
      <p className="grd-summary">
        <BasisBadge basis="modeled" /> <strong>{def.term ? <Term term={def.term}>{def.label}</Term> : def.label}</strong>{" "}
        <span className="unit">{def.unit}</span> for {fmtInt(p.completed)} completed of {fmtInt(p.cells_total)} cells.
      </p>
      {live ? (
        <Notice tag="Live" tone="info">
          {fmtInt(notDone)} cell{notDone === 1 ? " has" : "s have"} not completed yet; each fills in when its job completes.
          Nothing is estimated in the meantime.
        </Notice>
      ) : notDone > 0 ? (
        <Notice tag="Missing" tone="error">
          {fmtInt(notDone)} cell{notDone === 1 ? " did" : "s did"} not complete ({progressText(p)}) and{" "}
          {notDone === 1 ? "is" : "are"} shown as such.
        </Notice>
      ) : null}
      {sweep.cells === undefined ? (
        <Empty>The server returned this sweep without its cells.</Empty>
      ) : (
        <div className="table-scroll">
          <table className="grd-heat">
            <caption className="grd-caption">
              {def.label} per cell ({def.unit}). Each value links to the run behind it.
            </caption>
            <thead>
              <tr>
                <th scope="col" className="grd-corner">
                  <span>{axisName(sweep.y.axis)}</span>
                  <span className="dim"> by </span>
                  <span>{axisName(sweep.x.axis)}</span>
                </th>
                {xLabels.map((label, xi) => (
                  <th key={xi} scope="col" className="mono" title={sweep.x.labels[xi]}>
                    {label}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {yLabels.map((label, yi) => (
                <tr key={yi}>
                  <th scope="row" className="mono" title={sweep.y.labels[yi]}>
                    {label}
                  </th>
                  {xLabels.map((_, xi) => (
                    <HeatCell key={xi} cell={byPosition.get(`${xi},${yi}`)} def={def} scale={scale} />
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {scale ? <Legend scale={scale} def={def} /> : <p className="dim">No completed cell has a value for this metric yet, so there is no scale.</p>}
      <Notice tag="Modeled" tone="modeled">
        Each completed cell is one simulation, identified by its run id (listed in Cells). The matrix shows how sensitive
        the simulation model's output is to these two assumptions on this dataset. It is not a prediction of live trading
        results, and cells that did not complete are never filled in from their neighbours.
      </Notice>
    </Panel>
  );
}

interface CellRow {
  cell: SweepCell;
  x: string;
  y: string;
  value: number | null;
}

function CellsPanel({ sweep, def }: { sweep: Sweep; def: MetricDef }) {
  const navigate = useNavigate();
  const xLabels = axisLabels(sweep, "x");
  const yLabels = axisLabels(sweep, "y");
  const rows: CellRow[] = (sweep.cells ?? []).map((cell) => ({
    cell,
    x: xLabels[cell.x] ?? `x ${cell.x}`,
    y: yLabels[cell.y] ?? `y ${cell.y}`,
    value: cell.state === "completed" ? cellValue(cell, def) : null,
  }));
  const columns: Column<CellRow>[] = [
    { id: "x", header: axisName(sweep.x.axis), cell: (r) => <span className="mono">{r.x}</span>, sortValue: (r) => r.cell.x },
    { id: "y", header: axisName(sweep.y.axis), cell: (r) => <span className="mono">{r.y}</span>, sortValue: (r) => r.cell.y },
    {
      id: "state",
      header: "State",
      cell: (r) => (
        <span title={CELL_NOTE[r.cell.state]}>
          <Pill tone={CELL_TONE[r.cell.state]}>{r.cell.state}</Pill>
        </span>
      ),
      raw: (r) => r.cell.state,
      sortValue: (r) => r.cell.state,
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
        r.cell.run_id === null ? (
          DASH
        ) : (
          <Link to={paths.run(r.cell.run_id)}>
            <HashText hash={r.cell.run_id} />
          </Link>
        ),
      raw: (r) => r.cell.run_id ?? "",
    },
    {
      id: "job",
      header: "Job",
      cell: (r) =>
        r.cell.job_id === null ? (
          DASH
        ) : (
          <Link to={paths.job(r.cell.job_id)}>
            <HashText hash={r.cell.job_id} />
          </Link>
        ),
      raw: (r) => r.cell.job_id ?? "",
    },
    { id: "experiment", header: "Experiment", cell: (r) => <HashText hash={r.cell.experiment_id} />, raw: (r) => r.cell.experiment_id },
  ];
  return (
    <Panel title="Cells" flush caption="One row per cell: its state, its value when completed, the run behind it and its job.">
      <DataTable
        label="Sweep cells"
        rows={rows}
        rowKey={(r) => `${r.cell.x},${r.cell.y}`}
        columns={columns}
        onActivate={(r) => {
          if (r.cell.run_id !== null) navigate(paths.run(r.cell.run_id));
          else if (r.cell.job_id !== null) navigate(paths.job(r.cell.job_id));
        }}
        empty="The sweep lists no cells."
        fill
      />
    </Panel>
  );
}
