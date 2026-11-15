// Simulation Workbench -> Sweep (`/sim/sweep?base=<runId>`): declares a `SweepSpec` over one
// stored run's configuration (a grid or a seeded random sample of two parameters, or a seed
// ensemble of the configuration itself), optionally split in time into in-sample and
// out-of-sample windows, with the objective that will pick the best configuration, and submits
// it (`POST /api/v1/sweeps`, one job per run). The result and the report are read on the
// sweep's page, which follows the jobs as they finish.

import { useMemo, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";

import { api } from "../../api/client";
import { MAX_GRID_CELLS, type ObjectiveMetric } from "../../api/types";
import { strategyName } from "../../components/describe";
import { fmtDurationExact, fmtInt } from "../../components/fmt";
import { SplitPane } from "../../components/SplitPane";
import { BasisLegend, Empty, ErrorBox, Notice, Panel, Spinner, useAsync } from "../../components/ui";
import { GatedButton } from "../../shell/access";
import { useHealth } from "../../shell/health";
import { paths } from "../../shell/paths";
import { Workspace } from "../../shell/Workspace";
import { AxisEditor, BaseSummary } from "./AxisEditor";
import { CSCV_BLOCKS, DESIGNS, OBJECTIVES, buildSweep, newDraft, parseAxis, splitSpan } from "./gridModel";
import type { AxisDraft, DesignKind, RandomDraft, SeedsDraft } from "./gridModel";
import { RecentSweeps } from "./SweepList";
import "./grid.css";

/** Router state handed to the sweep page: the server returned an existing sweep (`created: false`). */
export interface SweepNavState {
  existing: boolean;
}

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

export default function SweepFormPage() {
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const baseId = params.get("base") ?? "";
  const serverLimit = useHealth().status?.limits.max_sweep_cells ?? null;
  const maxCells = serverLimit ?? MAX_GRID_CELLS;

  const runs = useAsync((signal) => api.runs({}, { signal }), []);
  // List items carry no configuration; the base run's own resource does.
  const base = useAsync((signal) => (baseId === "" ? Promise.resolve(null) : api.run(baseId, { signal })), [baseId]);
  const baseRun = base.data;
  const datasetId = baseRun?.config.dataset_id ?? "";
  // The sweep binds the dataset's current version; its span bounds the partition.
  const dataset = useAsync((signal) => (datasetId === "" ? Promise.resolve(null) : api.dataset(datasetId, { signal })), [datasetId]);
  const current = dataset.data?.current_version ?? null;
  const span = current ? { start: current.start_ns, end: current.end_ns } : null;

  const [kind, setKind] = useState<DesignKind>("grid");
  const [xDraft, setXDraft] = useState<AxisDraft>(initialX);
  const [yDraft, setYDraft] = useState<AxisDraft>(initialY);
  const [random, setRandom] = useState<RandomDraft>({ samples: "6", seed: "1" });
  const [seeds, setSeeds] = useState<SeedsDraft>({ mode: "count", count: "5", baseSeed: "", list: "" });
  const [partitioned, setPartitioned] = useState(false);
  const [inSamplePercent, setInSamplePercent] = useState("60");
  const [metric, setMetric] = useState<ObjectiveMetric>("net_pnl");
  const [direction, setDirection] = useState<"maximize" | "minimize">("maximize");
  const [cscvBlocks, setCscvBlocks] = useState<number | null>(null);
  const [label, setLabel] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<unknown>(null);

  const parsedX = useMemo(() => parseAxis(xDraft, "X"), [xDraft]);
  const parsedY = useMemo(() => parseAxis(yDraft, "Y"), [yDraft]);
  const built = useMemo(
    () =>
      buildSweep(
        baseRun ? baseRun.config : null,
        {
          kind,
          x: parsedX,
          y: parsedY,
          random,
          seeds,
          inSamplePercent: partitioned ? inSamplePercent : null,
          objective: { metric, direction },
          cscvBlocks: kind === "seeds" ? null : cscvBlocks,
        },
        span,
        maxCells,
      ),
    [baseRun, kind, parsedX, parsedY, random, seeds, partitioned, inSamplePercent, metric, direction, cscvBlocks, span, maxCells],
  );
  const problems = built.problems;
  const preview = span && partitioned && /^\d+$/.test(inSamplePercent) ? splitSpan(span, Number(inSamplePercent)) : null;

  const blockedReason = submitting
    ? "Submitting the sweep\u2026"
    : !baseRun
      ? "Choose a base run first."
      : problems.length > 0
        ? `Fix the sweep first: ${problems[0]}.`
        : null;

  const submit = async () => {
    if (built.spec === null || submitting) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      const res = await api.createSweep(built.spec, label.trim() === "" ? null : label.trim());
      const state: SweepNavState = { existing: !res.created };
      navigate(paths.sweep(res.sweep.id), { state });
    } catch (e) {
      setSubmitError(e);
      setSubmitting(false);
    }
  };

  const list = runs.data?.items ?? [];
  const toolbar = (
    <>
      <label className="row grd-toolbar-field" htmlFor="grd-base">
        <span>Base run</span>
        <select
          id="grd-base"
          value={baseId}
          onChange={(e) => setParams(e.target.value === "" ? {} : { base: e.target.value })}
          disabled={!runs.data}
        >
          <option value="">{runs.loading ? "Loading runs\u2026" : "Select a run"}</option>
          {baseId !== "" && !list.some((r) => r.run_id === baseId) ? <option value={baseId}>{baseId} (not among the listed runs)</option> : null}
          {list.map((r) => (
            <option key={r.run_id} value={r.run_id}>
              {r.label === null || r.label === "" ? "(no label)" : r.label} | {strategyName(r.strategy_kind)} | {r.dataset.id} | {r.run_id}
            </option>
          ))}
        </select>
      </label>
      <GatedButton scope="run" className="btn-primary" onClick={() => void submit()} blockedReason={blockedReason}>
        {submitting ? "Submitting\u2026" : `Submit sweep (${fmtInt(built.runs)} simulation${built.runs === 1 ? "" : "s"})`}
      </GatedButton>
      <Link to={paths.sweeps()}>All sweeps</Link>
    </>
  );

  const objectiveInfo = OBJECTIVES.find((o) => o.metric === metric);
  const definition = (
    <Panel
      title="Sweep definition"
      basis="modeled"
      caption="Re-runs one stored configuration under declared variations. Every configuration is a complete simulation on historical replay, queued as its own job (two with an in-sample / out-of-sample split); the sweep page shows each run as it finishes and the report once all have."
    >
      {runs.error ? <ErrorBox error={runs.error} onRetry={runs.reload} /> : null}
      {runs.data?.incomplete ? (
        <Notice tag="List incomplete">First {fmtInt(list.length)} runs offered as base; older runs are not listed.</Notice>
      ) : null}
      {runs.data && list.length === 0 && baseId === "" ? (
        <Empty>
          A sweep starts from a stored run, and none exists yet. <Link to={paths.strategy()}>Run a simulation in the Strategy Lab</Link>
        </Empty>
      ) : (
        // Submission happens only through the gated toolbar button; Enter in a field does nothing.
        <form className="col grd-form" onSubmit={(e) => e.preventDefault()}>
          <fieldset className="grd-fieldset">
            <legend>Base run</legend>
            <p className="note">Its configuration is the base of every run. Choose it in the toolbar.</p>
            {base.loading && baseId !== "" ? <Spinner label="Loading the base run" /> : null}
            {base.error ? <ErrorBox error={base.error} onRetry={base.reload} /> : null}
            {baseRun ? <BaseSummary run={baseRun} /> : null}
          </fieldset>
          <fieldset className="grd-fieldset">
            <legend>Design</legend>
            <div className="row" role="radiogroup" aria-label="Sweep design">
              {DESIGNS.map((d) => (
                <label key={d.kind} className="grd-check" title={d.help}>
                  <input type="radio" name="grd-design" value={d.kind} checked={kind === d.kind} onChange={() => setKind(d.kind)} />
                  {d.label}
                </label>
              ))}
            </div>
            <p className="grd-help">{DESIGNS.find((d) => d.kind === kind)?.help}</p>
          </fieldset>
          {kind === "seeds" ? (
            <fieldset className="grd-fieldset">
              <legend>Seeds</legend>
              <div className="row" role="radiogroup" aria-label="How the seeds are given">
                <label className="grd-check">
                  <input type="radio" name="grd-seed-mode" checked={seeds.mode === "count"} onChange={() => setSeeds({ ...seeds, mode: "count" })} />
                  Count from a base seed
                </label>
                <label className="grd-check">
                  <input type="radio" name="grd-seed-mode" checked={seeds.mode === "list"} onChange={() => setSeeds({ ...seeds, mode: "list" })} />
                  Explicit list
                </label>
              </div>
              {seeds.mode === "count" ? (
                <div className="row">
                  <label className="field" htmlFor="grd-seed-count">
                    <span className="field-label">Seeds</span>
                    <span className="field-control">
                      <input id="grd-seed-count" type="text" inputMode="numeric" value={seeds.count} onChange={(e) => setSeeds({ ...seeds, count: e.target.value.trim() })} />
                    </span>
                  </label>
                  <label className="field" htmlFor="grd-seed-base">
                    <span className="field-label">Base seed</span>
                    <span className="field-control">
                      <input
                        id="grd-seed-base"
                        type="text"
                        inputMode="numeric"
                        value={seeds.baseSeed}
                        placeholder={baseRun ? `${baseRun.config.seed} (the base run's)` : "the base run's seed"}
                        onChange={(e) => setSeeds({ ...seeds, baseSeed: e.target.value })}
                      />
                    </span>
                  </label>
                </div>
              ) : (
                <label className="field" htmlFor="grd-seed-list">
                  <span className="field-label">Seeds (comma or space separated)</span>
                  <span className="field-control">
                    <input id="grd-seed-list" type="text" value={seeds.list} onChange={(e) => setSeeds({ ...seeds, list: e.target.value })} placeholder="e.g. 1, 2, 3" />
                  </span>
                </label>
              )}
              {baseRun && (baseRun.config.execution.order_latency.type === "constant" ||
                (baseRun.config.execution.order_latency.type === "jitter" && baseRun.config.execution.order_latency.jitter_ns === 0)) ? (
                <Notice tag="Collapses" tone="warn">
                  The base run's order latency uses no randomness, and the seed drives nothing else: every seed will replay the
                  same simulation, and the report will say the ensemble collapses to one result. Choose a base run with jitter,
                  log-normal or heavy-tail latency to measure seed dispersion.
                </Notice>
              ) : null}
            </fieldset>
          ) : (
            <>
              <AxisEditor name="X" draft={xDraft} parsed={parsedX} base={baseRun ? baseRun.config : null} onChange={setXDraft} />
              <AxisEditor name="Y" draft={yDraft} parsed={parsedY} base={baseRun ? baseRun.config : null} onChange={setYDraft} />
              {kind === "random" ? (
                <fieldset className="grd-fieldset">
                  <legend>Random sample</legend>
                  <p className="grd-help">Combinations of the X and Y values are drawn uniformly without replacement with this seed; the same seed always draws the same combinations.</p>
                  <div className="row">
                    <label className="field" htmlFor="grd-samples">
                      <span className="field-label">Samples</span>
                      <span className="field-control">
                        <input id="grd-samples" type="text" inputMode="numeric" value={random.samples} onChange={(e) => setRandom({ ...random, samples: e.target.value.trim() })} />
                      </span>
                    </label>
                    <label className="field" htmlFor="grd-random-seed">
                      <span className="field-label">Seed</span>
                      <span className="field-control">
                        <input id="grd-random-seed" type="text" inputMode="numeric" value={random.seed} onChange={(e) => setRandom({ ...random, seed: e.target.value.trim() })} />
                      </span>
                    </label>
                  </div>
                </fieldset>
              ) : null}
            </>
          )}
          <fieldset className="grd-fieldset">
            <legend>In-sample / out-of-sample</legend>
            <label className="grd-check">
              <input type="checkbox" checked={partitioned} onChange={(e) => setPartitioned(e.target.checked)} />
              Split the dataset in time: select on the first part, then test on the rest
            </label>
            {partitioned ? (
              <>
                <label className="field" htmlFor="grd-is-share">
                  <span className="field-label">In-sample share (%)</span>
                  <span className="field-control">
                    <input id="grd-is-share" type="text" inputMode="numeric" value={inSamplePercent} onChange={(e) => setInSamplePercent(e.target.value.trim())} />
                  </span>
                </label>
                {dataset.loading && datasetId !== "" ? <Spinner label="Loading the dataset span" /> : null}
                {dataset.error ? <ErrorBox error={dataset.error} onRetry={dataset.reload} /> : null}
                {preview ? (
                  <p className="grd-help mono">
                    in sample [{fmtInt(preview.in_sample.start)}, {fmtInt(preview.in_sample.end)}) ns ={" "}
                    {fmtDurationExact(preview.in_sample.end - preview.in_sample.start)} {"\u00b7"} out of sample [{fmtInt(preview.out_of_sample.start)},{" "}
                    {fmtInt(preview.out_of_sample.end)}) ns = {fmtDurationExact(preview.out_of_sample.end - preview.out_of_sample.start)}
                  </p>
                ) : null}
                <p className="grd-help">
                  Each configuration runs twice, on derived datasets holding only its window (with the order book at the window start).
                  The out-of-sample window plays no part in the selection.
                </p>
              </>
            ) : null}
          </fieldset>
          {kind !== "seeds" ? (
            <fieldset className="grd-fieldset">
              <legend>Objective</legend>
              <p className="grd-help">Declared before anything runs: the one quantity that selects the best configuration (on the in-sample window when split).</p>
              <div className="row">
                <select aria-label="Objective metric" value={metric} onChange={(e) => setMetric(e.target.value as ObjectiveMetric)}>
                  {OBJECTIVES.map((o) => (
                    <option key={o.metric} value={o.metric}>
                      {o.label}
                    </option>
                  ))}
                </select>
                <select aria-label="Objective direction" value={direction} onChange={(e) => setDirection(e.target.value as "maximize" | "minimize")}>
                  <option value="maximize">maximise</option>
                  <option value="minimize">minimise</option>
                </select>
                <select
                  aria-label="CSCV blocks"
                  value={cscvBlocks === null ? "" : String(cscvBlocks)}
                  onChange={(e) => setCscvBlocks(e.target.value === "" ? null : Number(e.target.value))}
                >
                  <option value="">no CSCV (no PBO)</option>
                  {CSCV_BLOCKS.map((b) => (
                    <option key={b} value={b} disabled={!objectiveInfo?.additive}>
                      CSCV with {b} time blocks
                    </option>
                  ))}
                </select>
              </div>
              {!objectiveInfo?.additive ? <p className="grd-help">CSCV (the probability of backtest overfitting) needs a time-additive objective: net P&L, realized P&L or fees.</p> : null}
            </fieldset>
          ) : null}
          <label className="field" htmlFor="grd-label">
            <span className="field-label">Label (optional)</span>
            <span className="field-control">
              <input id="grd-label" type="text" value={label} maxLength={200} onChange={(e) => setLabel(e.target.value)} placeholder="e.g. latency by queue model" />
            </span>
          </label>
          <div className="col grd-submit">
            <p className={built.configurations > maxCells ? "grd-count is-over" : "grd-count"}>
              <span className="mono">{fmtInt(built.configurations)}</span> configuration{built.configurations === 1 ? "" : "s"}
              {partitioned ? (
                <>
                  {" "}
                  {"\u00d7"} 2 windows = <span className="mono">{fmtInt(built.runs)}</span> runs
                </>
              ) : null}{" "}
              of at most <span className="mono">{fmtInt(maxCells)}</span> configurations
              {serverLimit === null
                ? ` (the server's limit is not known yet; the engine bound of ${MAX_GRID_CELLS} applies until /status answers)`
                : " (this server's limit)"}
              . Each run is one full simulation and counts against your quotas.
            </p>
            <ul className="grd-problems" aria-live="polite" aria-label="Problems with this sweep">
              {problems.map((problem, i) => (
                <li key={i}>{problem}</li>
              ))}
            </ul>
            {submitError ? <ErrorBox error={submitError} /> : null}
          </div>
        </form>
      )}
      <h3 className="grd-subhead">Evidence basis</h3>
      <BasisLegend />
    </Panel>
  );

  return (
    <Workspace context={baseRun ? `sweep of ${baseRun.run_id.slice(0, 12)}` : "new sweep"} toolbar={toolbar} layout>
      <SplitPane
        id="sim.sweep.main"
        panes={[
          { id: "definition", label: "Sweep definition", defaultSize: "50%", minSize: "30%", content: definition },
          { id: "recent", label: "Recent sweeps", defaultSize: "50%", minSize: "25%", content: <RecentSweeps fill /> },
        ]}
      />
    </Workspace>
  );
}
