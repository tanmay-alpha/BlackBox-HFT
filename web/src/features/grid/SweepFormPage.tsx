// Simulation Workbench -> Sweep (`/sim/sweep?base=<runId>`): defines a two-axis sweep over one
// stored run's configuration and submits it as asynchronous jobs (`POST /api/v1/sweeps`, one job
// per cell). The result is read on the sweep's page, which follows the jobs as they finish.

import { useMemo, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";

import { api } from "../../api/client";
import { MAX_GRID_CELLS } from "../../api/types";
import { fmtInt } from "../../components/fmt";
import { SplitPane } from "../../components/SplitPane";
import { BasisLegend, Empty, ErrorBox, Notice, Panel, Spinner, useAsync } from "../../components/ui";
import { GatedButton } from "../../shell/access";
import { useHealth } from "../../shell/health";
import { paths } from "../../shell/paths";
import { Workspace } from "../../shell/Workspace";
import { AxisEditor, BaseSummary } from "./AxisEditor";
import { newDraft, parseAxis, validateGrid } from "./gridModel";
import type { AxisDraft } from "./gridModel";
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
  const [xDraft, setXDraft] = useState<AxisDraft>(initialX);
  const [yDraft, setYDraft] = useState<AxisDraft>(initialY);
  const [label, setLabel] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<unknown>(null);

  const parsedX = useMemo(() => parseAxis(xDraft, "X"), [xDraft]);
  const parsedY = useMemo(() => parseAxis(yDraft, "Y"), [yDraft]);
  const problems = useMemo(() => validateGrid(parsedX, parsedY, maxCells), [parsedX, parsedY, maxCells]);
  const baseRun = base.data;
  const cells = parsedX.axis.values.length * parsedY.axis.values.length;

  const blockedReason = submitting
    ? "Submitting the sweep\u2026"
    : !baseRun
      ? "Choose a base run first."
      : problems.length > 0
        ? `Fix the sweep first: ${problems[0]}.`
        : null;

  const submit = async () => {
    if (!baseRun || problems.length > 0 || submitting) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      const res = await api.createSweep(
        { base: baseRun.config, x: parsedX.axis, y: parsedY.axis },
        label.trim() === "" ? null : label.trim(),
      );
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
              {r.label === null || r.label === "" ? "(no label)" : r.label} | {r.strategy_kind} | {r.dataset.id} | {r.run_id}
            </option>
          ))}
        </select>
      </label>
      <GatedButton scope="run" className="btn-primary" onClick={() => void submit()} blockedReason={blockedReason}>
        {submitting ? "Submitting\u2026" : `Submit sweep (${fmtInt(cells)} simulation${cells === 1 ? "" : "s"})`}
      </GatedButton>
      <Link to={paths.sweeps()}>All sweeps</Link>
    </>
  );

  const definition = (
    <Panel
      title="Sweep definition"
      basis="modeled"
      caption="Re-runs one stored configuration while two execution assumptions are varied, one along each axis. Every cell is a complete simulation on historical replay, queued as its own job; the sweep page shows each cell as it finishes."
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
            <p className="note">Its configuration is the base of every cell. Choose it in the toolbar.</p>
            {base.loading && baseId !== "" ? <Spinner label="Loading the base run" /> : null}
            {base.error ? <ErrorBox error={base.error} onRetry={base.reload} /> : null}
            {baseRun ? <BaseSummary run={baseRun} /> : null}
          </fieldset>
          <AxisEditor name="X" draft={xDraft} parsed={parsedX} base={baseRun ? baseRun.config : null} onChange={setXDraft} />
          <AxisEditor name="Y" draft={yDraft} parsed={parsedY} base={baseRun ? baseRun.config : null} onChange={setYDraft} />
          <label className="field" htmlFor="grd-label">
            <span className="field-label">Label (optional)</span>
            <span className="field-control">
              <input id="grd-label" type="text" value={label} maxLength={200} onChange={(e) => setLabel(e.target.value)} placeholder="e.g. latency by queue model" />
            </span>
          </label>
          <div className="col grd-submit">
            <p className={cells > maxCells ? "grd-count is-over" : "grd-count"}>
              <span className="mono">
                {parsedX.axis.values.length} {"\u00d7"} {parsedY.axis.values.length} = {cells}
              </span>{" "}
              of at most <span className="mono">{fmtInt(maxCells)}</span> cells
              {serverLimit === null
                ? ` (the server's limit is not known yet; the engine bound of ${MAX_GRID_CELLS} applies until /status answers)`
                : " (this server's limit)"}
              . Each cell is one full simulation and counts against your quotas.
            </p>
            <ul className="grd-problems" aria-live="polite" aria-label="Problems with this sweep">
              {!baseRun && baseId === "" ? <li>choose a base run</li> : null}
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
