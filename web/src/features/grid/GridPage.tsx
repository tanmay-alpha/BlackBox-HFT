import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { ApiError, api } from "../../api/client";
import type { GridCell, GridResult, GridSpec, RunConfig, RunSummary } from "../../api/types";
import { MAX_GRID_CELLS } from "../../api/types";
import { fmtDuration } from "../../components/fmt";
import { Badge, Empty, ErrorBox, Panel, Spinner, useAsync } from "../../components/ui";
import { EvidenceLegend } from "../forensics/evidence";
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
  const [baseId, setBaseId] = useState(params.get("base") ?? "");
  const [xDraft, setXDraft] = useState<AxisDraft>(initialX);
  const [yDraft, setYDraft] = useState<AxisDraft>(initialY);
  const [job, setJob] = useState<Job>({ status: "idle" });
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

  const baseRun: RunSummary | null =
    runs.data?.find((r) => r.manifest.run_id === baseId) ?? null;
  const cells = parsedX.axis.values.length * parsedY.axis.values.length;
  const pending = job.status === "pending";
  const canRun = baseRun !== null && problems.length === 0 && !pending;

  const submit = async () => {
    if (baseRun === null || problems.length > 0 || pending) return;
    const spec: GridSpec = { base: baseRun.manifest.config, x: parsedX.axis, y: parsedY.axis };
    ticket.current += 1;
    const mine = ticket.current;
    setJob({ status: "pending", cells });
    try {
      const result = await api.grid(spec);
      if (ticket.current === mine) setJob({ status: "done", result });
    } catch (error) {
      if (ticket.current === mine) setJob({ status: "error", error });
    }
  };

  return (
    <div className="grd-page">
      <header className="grd-title">
        <h1>Sensitivity grid</h1>
        <p className="dim">
          Re-runs one stored configuration while two execution assumptions are varied, one along
          each axis. Every cell is a complete simulation on historical replay, limited to{" "}
          {MAX_GRID_CELLS} cells per grid.
        </p>
      </header>

      <EvidenceLegend />

      <Panel title="Define the grid">
        {runs.error ? (
          <ErrorBox error={runs.error} />
        ) : !runs.data ? (
          <Spinner />
        ) : runs.data.length === 0 ? (
          <Empty>
            A grid starts from a stored run, and none exists yet.{" "}
            <Link to="/lab">Run a simulation in the strategy laboratory</Link>
          </Empty>
        ) : (
          <form
            className="grd-form"
            onSubmit={(e) => {
              e.preventDefault();
              void submit();
            }}
          >
            <fieldset className="grd-fieldset">
              <legend>Base run</legend>
              <label className="grd-field" htmlFor="grd-base">
                <span className="grd-field-label">Its configuration is the base of every cell</span>
                <select id="grd-base" value={baseId} onChange={(e) => setBaseId(e.target.value)}>
                  <option value="">Select a run</option>
                  {runs.data.map((r) => (
                    <option key={r.manifest.run_id} value={r.manifest.run_id}>
                      {r.label === "" ? "(no label)" : r.label} | {r.manifest.config.strategy.kind.type}{" "}
                      | {r.manifest.dataset.id} | {r.manifest.run_id}
                    </option>
                  ))}
                </select>
              </label>
              {baseId !== "" && baseRun === null ? (
                <p className="grd-problem">
                  Run <span className="mono">{baseId}</span> is not among the stored runs.
                </p>
              ) : null}
              {baseRun ? <BaseSummary run={baseRun} /> : null}
            </fieldset>

            <div className="grd-axes">
              <AxisEditor
                name="X"
                draft={xDraft}
                parsed={parsedX}
                base={baseRun ? baseRun.manifest.config : null}
                onChange={setXDraft}
              />
              <AxisEditor
                name="Y"
                draft={yDraft}
                parsed={parsedY}
                base={baseRun ? baseRun.manifest.config : null}
                onChange={setYDraft}
              />
            </div>

            <div className="grd-submit">
              <p className={cells > MAX_GRID_CELLS ? "grd-count is-over" : "grd-count"}>
                <span className="mono">
                  {parsedX.axis.values.length} x {parsedY.axis.values.length} = {cells}
                </span>{" "}
                of at most <span className="mono">{MAX_GRID_CELLS}</span> cells. Each cell is one
                full simulation.
              </p>
              <ul className="grd-problems" aria-live="polite" aria-label="Problems with this grid">
                {baseRun === null && baseId === "" ? <li>choose a base run</li> : null}
                {problems.map((problem, i) => (
                  <li key={i}>{problem}</li>
                ))}
              </ul>
              <button type="submit" className="grd-btn grd-btn-primary" disabled={!canRun}>
                {pending ? "Running" : `Run ${cells} simulation${cells === 1 ? "" : "s"}`}
              </button>
            </div>
          </form>
        )}
      </Panel>

      {job.status === "pending" ? (
        <Panel title="Running the grid">
          <div className="grd-pending" role="status">
            <Spinner />
            <p>
              Running {job.cells} simulation{job.cells === 1 ? "" : "s"}. Every cell is a full
              simulation of the base configuration with two parameters replaced, so this takes
              about as long as that many separate runs. The request returns when all cells have
              finished; nothing is estimated or interpolated in the meantime.
            </p>
          </div>
        </Panel>
      ) : null}

      {job.status === "error" ? (
        <Panel title="The grid did not run">
          {job.error instanceof ApiError && job.error.code === "invalid_config" ? (
            <p className="dim">
              The server rejected the grid configuration. Its message lists every problem:
            </p>
          ) : null}
          <ErrorBox error={job.error} />
        </Panel>
      ) : null}

      {job.status === "done" ? <GridResultView result={job.result} /> : null}
    </div>
  );
}

function BaseSummary({ run }: { run: RunSummary }) {
  const { config } = run.manifest;
  const latency = config.execution.order_latency;
  return (
    <dl className="grd-base">
      <div>
        <dt>Strategy</dt>
        <dd className="mono">
          {config.strategy.kind.type}, every {fmtDuration(config.strategy.interval_ns)}
        </dd>
      </div>
      <div>
        <dt>Dataset</dt>
        <dd className="mono">{run.manifest.dataset.id}</dd>
      </div>
      <div>
        <dt>Exchange model</dt>
        <dd className="mono">{config.execution.exchange_model}</dd>
      </div>
      <div>
        <dt>Order latency</dt>
        <dd className="mono">
          entry {fmtDuration(latency.entry_ns)}, response {fmtDuration(latency.response_ns)}
          {latency.type === "jitter" ? `, jitter up to ${fmtDuration(latency.jitter_ns)}` : ""}
        </dd>
      </div>
      <div>
        <dt>Seed</dt>
        <dd className="mono">{config.seed}</dd>
      </div>
    </dl>
  );
}

/** The base run's own value of a parameter: what each grid value replaces. */
function baseValueText(param: AxisParam, config: RunConfig): string {
  const execution = config.execution;
  const bps = (fraction: number) => `${Number((fraction * 10_000).toFixed(6))} bps`;
  switch (param) {
    case "order_latency_ns":
      return `entry ${fmtDuration(execution.order_latency.entry_ns)}, response ${fmtDuration(execution.order_latency.response_ns)}`;
    case "entry_latency_ns":
      return fmtDuration(execution.order_latency.entry_ns);
    case "feed_latency_offset_ns":
      return fmtDuration(execution.feed_latency_offset_ns);
    case "queue_model":
      return queueModelLabel(execution.queue_model);
    case "maker_fee":
      return bps(execution.fees.maker);
    case "taker_fee":
      return bps(execution.fees.taker);
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
      <label className="grd-field" htmlFor={`${id}-param`}>
        <span className="grd-field-label">Parameter</span>
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
      </label>
      <p className="dim grd-help">
        <Badge kind="modeled">modeled</Badge> {info.help}
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
                  onChange={(e) =>
                    onChange({ ...draft, queue: { ...draft.queue, [variant]: e.target.checked } })
                  }
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
          <p className="dim grd-help">
            Several exponents for one variant give one model, and one row or column, per exponent.
          </p>
        </div>
      ) : (
        <label className="grd-field" htmlFor={`${id}-values`}>
          <span className="grd-field-label">
            {info.input === "ms"
              ? "Values in milliseconds, comma separated"
              : "Values in basis points, comma separated"}
          </span>
          <input
            id={`${id}-values`}
            type="text"
            inputMode="decimal"
            className="mono"
            value={draft.text}
            placeholder={info.input === "ms" ? "e.g. 0, 0.5, 2, 10" : "e.g. -1, 0, 2"}
            onChange={(e) => onChange({ ...draft, text: e.target.value })}
          />
        </label>
      )}

      <div className="grd-converted">
        <span className="dim">
          {info.input === "ms"
            ? "Sent as nanoseconds (1 ms = 1,000,000 ns):"
            : info.input === "bps"
              ? "Sent as a fraction of traded value (1 bps = 0.0001):"
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
      <ol className="grd-stops">
        {stops.map((value, i) => (
          <li key={i}>
            <span
              className="grd-swatch"
              style={{ background: scaleColor(scale, value) }}
              aria-hidden="true"
            />
            <span className="mono">
              {i === 0 || i === stops.length - 1 || (scale.kind === "diverging" && i === middle)
                ? def.format(value)
                : ""}
            </span>
          </li>
        ))}
      </ol>
      <span className="grd-legend-note">
        <span className="grd-swatch grd-swatch-missing" aria-hidden="true" /> missing: the
        simulation did not complete
      </span>
      <span className="grd-legend-note">
        <span className="grd-swatch grd-swatch-na" aria-hidden="true" /> metric not defined for
        that run
      </span>
    </div>
  );
}

function HeatCell({
  cell,
  def,
  scale,
}: {
  cell: GridCell | undefined;
  def: MetricDef;
  scale: Scale | null;
}) {
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
      <Link className="grd-cell-run mono" to={`/runs/${cell.run_id}`}>
        run {cell.run_id}
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
        <span className="grd-cell-value mono">{"\u2014"}</span>
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
      <span className="grd-cell-value mono">{def.format(value)}</span>
      {runLink}
    </td>
  );
}

function GridResultView({ result }: { result: GridResult }) {
  const [metricKey, setMetricKey] = useState("net_pnl");
  const def = METRICS.find((m) => m.key === metricKey) ?? (METRICS[0] as MetricDef);

  const xLabels = displayLabels(axisValueLabels(result.spec.x), result.x_labels);
  const yLabels = displayLabels(axisValueLabels(result.spec.y), result.y_labels);
  const xInfo = axisInfo(result.spec.x.type);
  const yInfo = axisInfo(result.spec.y.type);

  // Keyed by each cell's own coordinates, so a short or reordered response cannot shift values
  // into the wrong position.
  const byPosition = useMemo(() => {
    const map = new Map<string, GridCell>();
    for (const cell of result.cells) map.set(`${cell.xi},${cell.yi}`, cell);
    return map;
  }, [result]);

  const scale = useMemo(() => {
    const values: number[] = [];
    for (const cell of result.cells) {
      if (cell.metrics === null) continue;
      const v = def.get(cell.metrics);
      if (v !== null && Number.isFinite(v)) values.push(v);
    }
    return buildScale(def, values);
  }, [result, def]);

  const expected = xLabels.length * yLabels.length;
  const completed = result.cells.filter((c) => c.metrics !== null).length;

  return (
    <>
      <Panel
        title="Result"
        actions={
          <label className="grd-metric" htmlFor="grd-metric">
            <span className="dim">Metric</span>
            <select id="grd-metric" value={def.key} onChange={(e) => setMetricKey(e.target.value)}>
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
          <Badge kind="modeled">modeled</Badge> <strong>{def.label}</strong>{" "}
          <span className="dim">({def.unit})</span> for {completed} completed of {expected} cells,
          computed in {fmtDuration(result.wall_time_ms * 1_000_000)}.{" "}
          {completed < expected ? (
            <span className="grd-problem">
              {expected - completed} cell{expected - completed === 1 ? " is" : "s are"} missing
              and shown as such.
            </span>
          ) : null}
        </p>
        <p className="dim grd-axes-note">
          Columns (X): {xInfo.label}. Rows (Y): {yInfo.label}. Base run configuration: strategy{" "}
          <span className="mono">{result.spec.base.strategy.kind.type}</span> on dataset{" "}
          <span className="mono">{result.spec.base.dataset_id}</span>.
        </p>
        <div className="grd-scroll">
          <table className="grd-heat">
            <caption className="grd-caption">
              {def.label} per cell. Each value links to the run behind it.
            </caption>
            <thead>
              <tr>
                <th scope="col" className="grd-corner">
                  <span>{yInfo.label}</span>
                  <span className="dim"> by </span>
                  <span>{xInfo.label}</span>
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
                    <HeatCell
                      key={xi}
                      cell={byPosition.get(`${xi},${yi}`)}
                      def={def}
                      scale={scale}
                    />
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        {scale ? (
          <Legend scale={scale} def={def} />
        ) : (
          <p className="dim">No completed cell has a value for this metric, so there is no scale.</p>
        )}
        <p className="grd-statement">
          Each cell is one completed simulation, identified by its run id and result hash (listed
          below). The grid shows how sensitive the simulation model's output is to these two
          assumptions on this dataset. It is not a prediction of live trading results, and missing
          cells are never filled in from their neighbours.
        </p>
      </Panel>

      <Panel title="Cells">
        <div className="grd-scroll">
          <table className="table grd-cells">
            <caption className="grd-caption">
              One row per cell: the simulation that backs it and the hash of its result.
            </caption>
            <thead>
              <tr>
                <th scope="col">{xInfo.label}</th>
                <th scope="col">{yInfo.label}</th>
                <th scope="col">Run</th>
                <th scope="col">Result sha256</th>
                <th scope="col" className="grd-num">
                  {def.label}
                </th>
                <th scope="col">State</th>
              </tr>
            </thead>
            <tbody>
              {result.cells.map((cell) => {
                const value = cell.metrics === null ? null : def.get(cell.metrics);
                return (
                  <tr key={`${cell.xi},${cell.yi}`}>
                    <td className="mono">{xLabels[cell.xi] ?? `x ${cell.xi}`}</td>
                    <td className="mono">{yLabels[cell.yi] ?? `y ${cell.yi}`}</td>
                    <td className="mono">
                      {cell.run_id === null ? (
                        "\u2014"
                      ) : (
                        <Link to={`/runs/${cell.run_id}`}>{cell.run_id}</Link>
                      )}
                    </td>
                    <td className="mono grd-hash">{cell.result_sha256 ?? "\u2014"}</td>
                    <td className="mono grd-num">{value === null ? "\u2014" : def.format(value)}</td>
                    <td>
                      {cell.metrics === null ? (
                        <>
                          <Badge kind="error">missing</Badge>{" "}
                          <span>{cell.error ?? "no error text was returned"}</span>
                        </>
                      ) : (
                        <Badge kind="ok">completed</Badge>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Panel>
    </>
  );
}
