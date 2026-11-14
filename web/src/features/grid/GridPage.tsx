// Experiment Comparison -> Sweep (UI_DESIGN_SPEC.md §11.8): re-runs one stored configuration while
// two execution assumptions are varied, one along each axis, and reads the result as a matrix
// whose cells print their value and link to their run, plus a table of every cell.

import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";

import { ApiError, api, isStaticViewer, precomputedGrids } from "../../api/client";
import type { GridCell, GridResult, GridSpec, RunConfig, RunSummary } from "../../api/types";
import { MAX_GRID_CELLS } from "../../api/types";
import { DataTable, type Column } from "../../components/DataTable";
import { fractionToBps } from "../../components/describe";
import { DASH, fmtDuration, fmtDurationExact, fmtInt } from "../../components/fmt";
import { SplitPane } from "../../components/SplitPane";
import { StaticViewerNote } from "../../components/staticViewer";
import { ToolbarDivider } from "../../components/Toolbar";
import { Term } from "../../components/Tooltip";
import { BasisBadge, BasisLegend, Empty, ErrorBox, HashText, Notice, Panel, Pill, Spinner, useAsync } from "../../components/ui";
import { paths } from "../../shell/paths";
import { Workspace } from "../../shell/Workspace";
import { CompareTabs } from "../experiments/CompareTabs";
import { queueModelLabel } from "../forensics/labels";
import {
  AXIS_PARAMS,
  METRICS,
  QUEUE_VARIANTS,
  axisInfo,
  axisValueLabels,
  buildScale,
  isPowerVariant,
  legendValues,
  newDraft,
  parseAxis,
  scaleColor,
  validateGrid,
} from "./gridModel";
import type { AxisDraft, AxisParam, MetricDef, ParsedAxis, Scale } from "./gridModel";
import "./grid.css";

type Job =
  | { status: "idle" }
  | { status: "pending"; cells: number }
  | { status: "error"; error: unknown }
  | { status: "done"; result: GridResult };

const FORM_ID = "sweep-form";

function initialX(): AxisDraft {
  return { ...newDraft("order_latency_ns"), text: "0, 1, 5, 20" };
}

function initialY(): AxisDraft {
  const draft = newDraft("queue_model");
  return {
    ...draft,
    queue: { ...draft.queue, risk_adverse: true, log_prob: true, power_prob: true },
    exponents: { ...draft.exponents, power_prob: "3" },
  };
}

export default function GridPage() {
  const [params] = useSearchParams();
  const runs = useAsync(() => api.runs(), []);
  const staticGrids = useAsync(() => precomputedGrids(), []);
  const [baseId, setBaseId] = useState(params.get("base") ?? "");
  const [xDraft, setXDraft] = useState<AxisDraft>(initialX);
  const [yDraft, setYDraft] = useState<AxisDraft>(initialY);
  const [job, setJob] = useState<Job>({ status: "idle" });
  const [resultIndex, setResultIndex] = useState(0);
  const [metricKey, setMetricKey] = useState("net_pnl");
  const def = METRICS.find((m) => m.key === metricKey) ?? (METRICS[0] as MetricDef);
  // Identifies the latest submission so a slow, superseded or orphaned response is ignored.
  const ticket = useRef(0);
  useEffect(
    () => () => {
      ticket.current += 1;
    },
    [],
  );

  const parsedX = useMemo(() => parseAxis(xDraft, "X"), [xDraft]);
  const parsedY = useMemo(() => parseAxis(yDraft, "Y"), [yDraft]);
  const problems = useMemo(() => validateGrid(parsedX, parsedY), [parsedX, parsedY]);

  const baseRun: RunSummary | null = runs.data?.find((r) => r.manifest.run_id === baseId) ?? null;
  const cells = parsedX.axis.values.length * parsedY.axis.values.length;
  const pending = job.status === "pending";
  // The hosted read-only viewer cannot simulate; it shows the precomputed grids instead.
  const canRun = baseRun !== null && problems.length === 0 && !pending && !isStaticViewer();

  const submit = async () => {
    if (baseRun === null || problems.length > 0 || pending || isStaticViewer()) return;
    const spec: GridSpec = { base: baseRun.manifest.config, x: parsedX.axis, y: parsedY.axis };
    ticket.current += 1;
    const mine = ticket.current;
    setJob({ status: "pending", cells });
    try {
      const result = await api.grid(spec);
      if (ticket.current === mine) {
        setJob({ status: "done", result });
        setResultIndex(0);
      }
    } catch (error) {
      if (ticket.current === mine) setJob({ status: "error", error });
    }
  };

  const results: GridResult[] = [...(job.status === "done" ? [job.result] : []), ...(staticGrids.data ?? [])];
  const result = results[Math.min(resultIndex, results.length - 1)] ?? null;

  const toolbar = (
    <>
      <CompareTabs />
      <ToolbarDivider />
      <label className="row grd-toolbar-field" htmlFor="grd-base">
        <span>Base run</span>
        <select id="grd-base" value={baseId} onChange={(e) => setBaseId(e.target.value)} disabled={!runs.data || runs.data.length === 0}>
          <option value="">{runs.loading ? "Loading runs\u2026" : "Select a run"}</option>
          {(runs.data ?? []).map((r) => (
            <option key={r.manifest.run_id} value={r.manifest.run_id}>
              {r.label === "" ? "(no label)" : r.label} | {r.manifest.config.strategy.kind.type} | {r.manifest.dataset.id} |{" "}
              {r.manifest.run_id}
            </option>
          ))}
        </select>
      </label>
      <button type="submit" form={FORM_ID} className="btn-primary" disabled={!canRun} title={isStaticViewer() ? "This hosted viewer simulates nothing." : undefined}>
        {pending ? "Running" : `Run ${cells} simulation${cells === 1 ? "" : "s"}`}
      </button>
    </>
  );

  const definition = (
    <Panel
      title="Grid definition"
      basis="modeled"
      caption={`Re-runs one stored configuration while two execution assumptions are varied, one along each axis. Every cell is a complete simulation on historical replay, limited to ${MAX_GRID_CELLS} cells per grid.`}
    >
      {runs.error ? (
        <ErrorBox error={runs.error} onRetry={runs.reload} />
      ) : !runs.data ? (
        <Spinner label="Loading runs" />
      ) : runs.data.length === 0 ? (
        <Empty>
          A grid starts from a stored run, and none exists yet.{" "}
          <Link to={paths.strategy()}>Run a simulation in the Strategy Lab</Link>
        </Empty>
      ) : (
        <form
          id={FORM_ID}
          className="col grd-form"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <fieldset className="grd-fieldset">
            <legend>Base run</legend>
            <p className="note">Its configuration is the base of every cell. Choose it in the toolbar.</p>
            {baseId !== "" && baseRun === null ? (
              <p className="grd-problem">
                Run <span className="mono">{baseId}</span> is not among the stored runs.
              </p>
            ) : null}
            {baseRun ? <BaseSummary run={baseRun} /> : null}
          </fieldset>
          <AxisEditor name="X" draft={xDraft} parsed={parsedX} base={baseRun ? baseRun.manifest.config : null} onChange={setXDraft} />
          <AxisEditor name="Y" draft={yDraft} parsed={parsedY} base={baseRun ? baseRun.manifest.config : null} onChange={setYDraft} />
          <div className="col grd-submit">
            <p className={cells > MAX_GRID_CELLS ? "grd-count is-over" : "grd-count"}>
              <span className="mono">
                {parsedX.axis.values.length} {"\u00d7"} {parsedY.axis.values.length} = {cells}
              </span>{" "}
              of at most <span className="mono">{MAX_GRID_CELLS}</span> cells. Each cell is one full simulation.
            </p>
            <ul className="grd-problems" aria-live="polite" aria-label="Problems with this grid">
              {baseRun === null && baseId === "" ? <li>choose a base run</li> : null}
              {problems.map((problem, i) => (
                <li key={i}>{problem}</li>
              ))}
            </ul>
            <StaticViewerNote />
          </div>
        </form>
      )}
      <h3 className="grd-subhead">Evidence basis</h3>
      <BasisLegend />
    </Panel>
  );

  return (
    <Workspace context={baseRun ? baseRun.manifest.run_id.slice(0, 12) : null} toolbar={toolbar} layout>
      <SplitPane
        id="compare.sweep.main"
        panes={[
          { id: "definition", label: "Grid definition", defaultSize: "34%", minSize: "22%", content: definition },
          {
            id: "results",
            label: "Result",
            defaultSize: "66%",
            minSize: "30%",
            content: (
              <SplitPane
                id="compare.sweep.results"
                orientation="vertical"
                panes={[
                  {
                    id: "matrix",
                    label: "Result matrix",
                    defaultSize: "58%",
                    minSize: "20%",
                    content: (
                      <MatrixPanel
                        job={job}
                        results={results}
                        result={result}
                        resultIndex={resultIndex}
                        onResult={setResultIndex}
                        staticLoading={staticGrids.loading && !staticGrids.data}
                        staticError={staticGrids.error}
                        def={def}
                        onMetric={setMetricKey}
                      />
                    ),
                  },
                  {
                    id: "cells",
                    label: "Cells",
                    defaultSize: "42%",
                    minSize: "15%",
                    content: <CellsPanel result={result} def={def} />,
                  },
                ]}
              />
            ),
          },
        ]}
      />
    </Workspace>
  );
}

function BaseSummary({ run }: { run: RunSummary }) {
  const { config } = run.manifest;
  const latency = config.execution.order_latency;
  return (
    <dl className="kv">
      <dt>Run</dt>
      <dd>
        <Link to={paths.run(run.manifest.run_id)}>
          <HashText hash={run.manifest.run_id} />
        </Link>
      </dd>
      <dt>Strategy</dt>
      <dd>
        {config.strategy.kind.type}, every {fmtDurationExact(config.strategy.interval_ns)}
      </dd>
      <dt>Dataset</dt>
      <dd>{run.manifest.dataset.id}</dd>
      <dt>Exchange model</dt>
      <dd>{config.execution.exchange_model}</dd>
      <dt>
        <Term term="latency">Order latency</Term>
      </dt>
      <dd>
        entry {fmtDurationExact(latency.entry_ns)}, response {fmtDurationExact(latency.response_ns)}
        {latency.type === "jitter" ? `, jitter up to ${fmtDurationExact(latency.jitter_ns)}` : ""}
      </dd>
      <dt>Seed</dt>
      <dd>{config.seed}</dd>
    </dl>
  );
}

/** The base run's own value of a parameter: what each grid value replaces. */
function baseValueText(param: AxisParam, config: RunConfig): string {
  const execution = config.execution;
  switch (param) {
    case "order_latency_ns":
      return `entry ${fmtDurationExact(execution.order_latency.entry_ns)}, response ${fmtDurationExact(execution.order_latency.response_ns)}`;
    case "entry_latency_ns":
      return fmtDurationExact(execution.order_latency.entry_ns);
    case "feed_latency_offset_ns":
      return fmtDurationExact(execution.feed_latency_offset_ns);
    case "queue_model":
      return queueModelLabel(execution.queue_model);
    case "maker_fee":
      return `${fractionToBps(execution.fees.maker)} bp`;
    case "taker_fee":
      return `${fractionToBps(execution.fees.taker)} bp`;
    default:
      return "";
  }
}

function AxisEditor({
  name,
  draft,
  parsed,
  base,
  onChange,
}: {
  name: "X" | "Y";
  draft: AxisDraft;
  parsed: ParsedAxis;
  base: RunConfig | null;
  onChange: (draft: AxisDraft) => void;
}) {
  const info = axisInfo(draft.param);
  const id = `grd-${name.toLowerCase()}`;
  return (
    <fieldset className="grd-fieldset">
      <legend>
        {name} axis ({name === "X" ? "columns" : "rows"})
      </legend>
      <label className="field" htmlFor={`${id}-param`}>
        <span className="field-label">Parameter</span>
        <span className="field-control">
          <select
            id={`${id}-param`}
            value={draft.param}
            onChange={(e) => {
              const next = AXIS_PARAMS.find((p) => p.param === e.target.value);
              // Units differ between parameters, so typed values are not carried across.
              if (next) onChange({ ...draft, param: next.param, text: "" });
            }}
          >
            {AXIS_PARAMS.map((p) => (
              <option key={p.param} value={p.param}>
                {p.label}
              </option>
            ))}
          </select>
        </span>
      </label>
      <p className="grd-help">
        <BasisBadge basis="modeled" /> {info.help}
        {base ? (
          <>
            {" "}
            In the base run: <span className="mono">{baseValueText(draft.param, base)}</span>.
          </>
        ) : null}
      </p>

      {info.input === "queue" ? (
        <div className="grd-queue" role="group" aria-label={`${name} axis queue models`}>
          {QUEUE_VARIANTS.map((variant) => (
            <div key={variant} className="grd-queue-row">
              <label className="grd-check">
                <input
                  type="checkbox"
                  checked={draft.queue[variant]}
                  onChange={(e) => onChange({ ...draft, queue: { ...draft.queue, [variant]: e.target.checked } })}
                />
                <span className="mono">{variant}</span>
              </label>
              {isPowerVariant(variant) ? (
                <label className="grd-exponent">
                  <span className="dim">exponent n</span>
                  <input
                    type="text"
                    inputMode="decimal"
                    className="mono"
                    value={draft.exponents[variant]}
                    disabled={!draft.queue[variant]}
                    placeholder="e.g. 2, 3"
                    aria-label={`${variant} exponent n, comma separated for several`}
                    onChange={(e) =>
                      onChange({
                        ...draft,
                        exponents: { ...draft.exponents, [variant]: e.target.value },
                      })
                    }
                  />
                </label>
              ) : null}
            </div>
          ))}
          <p className="grd-help">Several exponents for one variant give one model, and one row or column, per exponent.</p>
        </div>
      ) : (
        <label className="field" htmlFor={`${id}-values`}>
          <span className="field-label">
            {info.input === "ms" ? "Values in milliseconds, comma separated" : "Values in basis points, comma separated"}
          </span>
          <span className="field-control">
            <input
              id={`${id}-values`}
              type="text"
              inputMode="decimal"
              className="mono"
              value={draft.text}
              placeholder={info.input === "ms" ? "e.g. 0, 0.5, 2, 10" : "e.g. -1, 0, 2"}
              onChange={(e) => onChange({ ...draft, text: e.target.value })}
            />
            <span className="field-unit">{info.input}</span>
          </span>
        </label>
      )}

      <div className="grd-converted">
        <span className="dim">
          {info.input === "ms"
            ? "Sent as nanoseconds (1 ms = 1,000,000 ns):"
            : info.input === "bp"
              ? "Sent as a fraction of traded value (1 bp = 0.0001):"
              : "Sent as these queue models:"}
        </span>
        {parsed.conversions.length === 0 ? (
          <span className="dim"> no value yet</span>
        ) : (
          <ul>
            {parsed.conversions.map((line, i) => (
              <li key={i} className="mono">
                {line}
              </li>
            ))}
          </ul>
        )}
      </div>
    </fieldset>
  );
}

// ---------------------------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------------------------

/** Readable labels where the spec allows, falling back to the label the server returned. */
function displayLabels(friendly: string[], raw: string[]): string[] {
  return raw.map((label, i) => friendly[i] ?? label);
}

function resultName(result: GridResult): string {
  return `${axisInfo(result.spec.y.type).label} by ${axisInfo(result.spec.x.type).label}`;
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
        <span className="grd-swatch grd-swatch-missing" aria-hidden="true" /> missing: the simulation did not complete
      </span>
      <span className="grd-legend-note">
        <span className="grd-swatch grd-swatch-na" aria-hidden="true" /> metric not defined for that run
      </span>
    </div>
  );
}

function HeatCell({ cell, def, scale }: { cell: GridCell | undefined; def: MetricDef; scale: Scale | null }) {
  if (!cell) {
    return (
      <td className="grd-cell grd-missing">
        <span className="grd-cell-state">missing</span>
        <span className="grd-cell-error">the response holds no cell for this position</span>
      </td>
    );
  }
  const runLink =
    cell.run_id === null ? null : (
      <Link className="grd-cell-run mono" to={paths.run(cell.run_id)}>
        run {cell.run_id.slice(0, 12)}
      </Link>
    );
  if (cell.metrics === null) {
    return (
      <td className="grd-cell grd-missing">
        <span className="grd-cell-state">missing</span>
        <span className="grd-cell-error">{cell.error ?? "no error text was returned"}</span>
        {runLink}
      </td>
    );
  }
  const value = def.get(cell.metrics);
  if (value === null || !Number.isFinite(value) || scale === null) {
    return (
      <td className="grd-cell grd-na">
        <span className="grd-cell-value mono">{DASH}</span>
        <span className="grd-cell-error">not defined for this run</span>
        {runLink}
      </td>
    );
  }
  return (
    <td
      className="grd-cell"
      style={{ background: scaleColor(scale, value) }}
      title={`run ${cell.run_id ?? "unknown"}\nresult sha256 ${cell.result_sha256 ?? "unknown"}`}
    >
      {cell.run_id === null ? (
        <span className="grd-cell-value mono">{def.format(value)}</span>
      ) : (
        <Link className="grd-cell-value mono" to={paths.run(cell.run_id)} aria-label={`${def.format(value)}, open run ${cell.run_id}`}>
          {def.format(value)}
        </Link>
      )}
      {cell.run_id !== null ? <span className="grd-cell-run mono">{cell.run_id.slice(0, 12)}</span> : null}
    </td>
  );
}

function MatrixPanel({
  job,
  results,
  result,
  resultIndex,
  onResult,
  staticLoading,
  staticError,
  def,
  onMetric,
}: {
  job: Job;
  results: GridResult[];
  result: GridResult | null;
  resultIndex: number;
  onResult: (i: number) => void;
  staticLoading: boolean;
  staticError: Error | null;
  def: MetricDef;
  onMetric: (key: string) => void;
}) {
  const xLabels = result ? displayLabels(axisValueLabels(result.spec.x), result.x_labels) : [];
  const yLabels = result ? displayLabels(axisValueLabels(result.spec.y), result.y_labels) : [];

  // Keyed by each cell's own coordinates, so a short or reordered response cannot shift values
  // into the wrong position.
  const byPosition = useMemo(() => {
    const map = new Map<string, GridCell>();
    for (const cell of result?.cells ?? []) map.set(`${cell.xi},${cell.yi}`, cell);
    return map;
  }, [result]);

  const scale = useMemo(() => {
    const values: number[] = [];
    for (const cell of result?.cells ?? []) {
      if (cell.metrics === null) continue;
      const v = def.get(cell.metrics);
      if (v !== null && Number.isFinite(v)) values.push(v);
    }
    return buildScale(def, values);
  }, [result, def]);

  const expected = xLabels.length * yLabels.length;
  const completed = result ? result.cells.filter((c) => c.metrics !== null).length : 0;

  return (
    <Panel
      title="Result matrix"
      basis="modeled"
      caption={
        isStaticViewer()
          ? "Precomputed grid. Computed ahead of time by blackbox export-static on the synthetic dataset: every cell is a stored simulation you can open."
          : "Each cell is one completed simulation; its value links to the run behind it."
      }
      actions={
        <>
          {results.length > 1 ? (
            <label className="row grd-metric" htmlFor="grd-result">
              <span>Grid</span>
              <select id="grd-result" value={Math.min(resultIndex, results.length - 1)} onChange={(e) => onResult(Number(e.target.value))}>
                {results.map((r, i) => (
                  <option key={i} value={i}>
                    {i + 1}: {resultName(r)}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          <label className="row grd-metric" htmlFor="grd-metric">
            <span>Metric</span>
            <select id="grd-metric" value={def.key} onChange={(e) => onMetric(e.target.value)}>
              {METRICS.map((m) => (
                <option key={m.key} value={m.key}>
                  {m.label}
                </option>
              ))}
            </select>
          </label>
        </>
      }
    >
      {job.status === "pending" ? (
        <div className="col grd-pending" role="status">
          <Spinner label="Running the grid" />
          <p className="prose">
            Running {job.cells} simulation{job.cells === 1 ? "" : "s"}. Every cell is a full simulation of the base
            configuration with two parameters replaced, so this takes about as long as that many separate runs. The
            request returns when all cells have finished; nothing is estimated or interpolated in the meantime.
          </p>
        </div>
      ) : null}
      {job.status === "error" ? (
        <div className="col">
          <p className="grd-problem">The grid did not run.</p>
          {job.error instanceof ApiError && job.error.code === "invalid_config" ? (
            <p className="dim">The server rejected the grid configuration. Its message lists every problem:</p>
          ) : null}
          <ErrorBox error={job.error} />
        </div>
      ) : null}
      {staticError ? <ErrorBox error={staticError} /> : null}
      {staticLoading ? <Spinner label="Loading the precomputed grids" /> : null}
      {result === null && job.status !== "pending" && !staticLoading ? (
        <Empty>No grid has run yet. Define the axes, choose a base run and run the grid.</Empty>
      ) : null}
      {result ? (
        <>
          <p className="grd-summary">
            <BasisBadge basis="modeled" /> <strong>{def.term ? <Term term={def.term}>{def.label}</Term> : def.label}</strong>{" "}
            <span className="unit">{def.unit}</span> for {fmtInt(completed)} completed of {fmtInt(expected)} cells, computed in{" "}
            {fmtDuration(result.wall_time_ms * 1_000_000)}.
          </p>
          {completed < expected ? (
            <Notice tag="Missing" tone="error">
              {expected - completed} cell{expected - completed === 1 ? " is" : "s are"} missing and shown as such.
            </Notice>
          ) : null}
          <p className="note">
            Columns (X): {axisInfo(result.spec.x.type).label}. Rows (Y): {axisInfo(result.spec.y.type).label}. Base run
            configuration: strategy <span className="mono">{result.spec.base.strategy.kind.type}</span> on dataset{" "}
            <span className="mono">{result.spec.base.dataset_id}</span>.
          </p>
          <div className="table-scroll">
            <table className="grd-heat">
              <caption className="grd-caption">
                {def.label} per cell ({def.unit}). Each value links to the run behind it.
              </caption>
              <thead>
                <tr>
                  <th scope="col" className="grd-corner">
                    <span>{axisInfo(result.spec.y.type).label}</span>
                    <span className="dim"> by </span>
                    <span>{axisInfo(result.spec.x.type).label}</span>
                  </th>
                  {xLabels.map((label, xi) => (
                    <th key={xi} scope="col" className="mono" title={result.x_labels[xi]}>
                      {label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {yLabels.map((label, yi) => (
                  <tr key={yi}>
                    <th scope="row" className="mono" title={result.y_labels[yi]}>
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
          {scale ? <Legend scale={scale} def={def} /> : <p className="dim">No completed cell has a value for this metric, so there is no scale.</p>}
          <Notice tag="Modeled" tone="modeled">
            Each cell is one completed simulation, identified by its run id and result hash (listed in Cells). The grid
            shows how sensitive the simulation model's output is to these two assumptions on this dataset. It is not a
            prediction of live trading results, and missing cells are never filled in from their neighbours.
          </Notice>
        </>
      ) : null}
    </Panel>
  );
}

interface CellRow {
  cell: GridCell;
  x: string;
  y: string;
  value: number | null;
}

function CellsPanel({ result, def }: { result: GridResult | null; def: MetricDef }) {
  const navigate = useNavigate();
  if (!result) {
    return (
      <Panel title="Cells">
        <Empty>The cells of a grid are listed here once it has run.</Empty>
      </Panel>
    );
  }
  const xLabels = displayLabels(axisValueLabels(result.spec.x), result.x_labels);
  const yLabels = displayLabels(axisValueLabels(result.spec.y), result.y_labels);
  const rows: CellRow[] = result.cells.map((cell) => {
    const v = cell.metrics === null ? null : def.get(cell.metrics);
    return {
      cell,
      x: xLabels[cell.xi] ?? `x ${cell.xi}`,
      y: yLabels[cell.yi] ?? `y ${cell.yi}`,
      value: v !== null && Number.isFinite(v) ? v : null,
    };
  });
  const columns: Column<CellRow>[] = [
    { id: "x", header: axisInfo(result.spec.x.type).label, cell: (r) => <span className="mono">{r.x}</span>, sortValue: (r) => r.cell.xi },
    { id: "y", header: axisInfo(result.spec.y.type).label, cell: (r) => <span className="mono">{r.y}</span>, sortValue: (r) => r.cell.yi },
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
      id: "sha",
      header: "Result sha256",
      cell: (r) => (r.cell.result_sha256 === null ? DASH : <HashText hash={r.cell.result_sha256} />),
      raw: (r) => r.cell.result_sha256 ?? "",
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
      id: "state",
      header: "State",
      cell: (r) =>
        r.cell.metrics === null ? (
          <span className="row grd-state" title={r.cell.error ?? "no error text was returned"}>
            <Pill tone="error">failed</Pill>
            <span>{r.cell.error ?? "no error text was returned"}</span>
          </span>
        ) : (
          <Pill tone="ok">completed</Pill>
        ),
      sortValue: (r) => (r.cell.metrics === null ? 1 : 0),
    },
  ];
  return (
    <Panel
      title="Cells"
      flush
      caption="One row per cell: the simulation that backs it and the hash of its result."
    >
      <DataTable
        label="Grid cells"
        rows={rows}
        rowKey={(r) => `${r.cell.xi},${r.cell.yi}`}
        columns={columns}
        onActivate={(r) => {
          if (r.cell.run_id !== null) navigate(paths.run(r.cell.run_id));
        }}
        empty="The response holds no cells."
        fill
      />
    </Panel>
  );
}
