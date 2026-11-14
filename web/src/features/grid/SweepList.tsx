// Recent sweeps of this workspace (`GET /api/v1/sweeps`, newest first), each linking to its
// result matrix. Shared by the sweep form (/sim/sweep) and Experiment Comparison -> Sweeps.

import { Link } from "react-router-dom";

import { api } from "../../api/client";
import type { Sweep, SweepState } from "../../api/model";
import { DataTable, type Column } from "../../components/DataTable";
import { fmtInt } from "../../components/fmt";
import { fmtNsUtc } from "../../components/ns";
import { ErrorBox, Notice, Panel, Pill, Spinner, useAsync } from "../../components/ui";
import { paths } from "../../shell/paths";
import { AXIS_PARAMS } from "./gridModel";

/** How many recent sweeps are listed. */
const RECENT = 50;

export const SWEEP_TONE: Record<SweepState, "ok" | "warn" | "error" | "neutral" | "info"> = {
  queued: "info",
  running: "info",
  completed: "ok",
  partial: "warn",
  cancelled: "neutral",
};

/** States in which a sweep's cells can still change. */
export const LIVE_SWEEP: Record<SweepState, boolean> = {
  queued: true,
  running: true,
  completed: false,
  partial: false,
  cancelled: false,
};

/** The readable name of a served axis parameter, or the parameter itself when unknown. */
export function axisName(axis: string): string {
  return AXIS_PARAMS.find((p) => p.param === axis)?.label ?? axis;
}

/** `progress` exactly as served: completed of total, then every non-zero other count. */
export function progressText(p: Sweep["progress"]): string {
  const rest = (
    [
      ["running", p.running],
      ["queued", p.queued],
      ["failed", p.failed],
      ["cancelled", p.cancelled],
      ["expired", p.expired],
    ] as const
  )
    .filter(([, n]) => n > 0)
    .map(([what, n]) => `${fmtInt(n)} ${what}`);
  return [`${fmtInt(p.completed)} of ${fmtInt(p.cells_total)} completed`, ...rest].join(" \u00b7 ");
}

export function RecentSweeps({ fill = false }: { fill?: boolean }) {
  const page = useAsync((signal) => api.sweepsPage({ limit: RECENT }, { signal }), []);
  const columns: Column<Sweep>[] = [
    {
      id: "label",
      header: "Sweep",
      rowHeader: true,
      cell: (s) => <Link to={paths.sweep(s.id)}>{s.label === "" ? `sweep ${s.id.slice(0, 12)}` : s.label}</Link>,
      raw: (s) => s.label,
      sortValue: (s) => s.label,
    },
    {
      id: "axes",
      header: "Rows by columns",
      cell: (s) => (
        <span title={`${s.y.axis} by ${s.x.axis}`}>
          {axisName(s.y.axis)} ({s.y.labels.length}) by {axisName(s.x.axis)} ({s.x.labels.length})
        </span>
      ),
    },
    {
      id: "state",
      header: "State",
      cell: (s) => <Pill tone={SWEEP_TONE[s.state]}>{s.state}</Pill>,
      raw: (s) => s.state,
      sortValue: (s) => s.state,
    },
    { id: "progress", header: "Progress", cell: (s) => progressText(s.progress), sortValue: (s) => s.progress.completed },
    {
      id: "created",
      header: "Created",
      unit: "UTC",
      cell: (s) => <span className="mono">{fmtNsUtc(s.created_at_ns, 0)}</span>,
      sortValue: (s) => s.created_at_ns.padStart(20, "0"),
    },
  ];
  return (
    <Panel
      title="Recent sweeps"
      caption="Sweeps of this workspace, newest first. Open one to read its result matrix; a live sweep keeps updating there."
      actions={
        <button type="button" className="btn" onClick={page.reload}>
          Refresh
        </button>
      }
      flush
      fill={fill}
    >
      {page.error ? <ErrorBox error={page.error} onRetry={page.reload} /> : null}
      {page.loading && !page.data ? <Spinner label="Loading sweeps" /> : null}
      {page.data?.next_cursor != null ? (
        <Notice tag="List incomplete">First {fmtInt(page.data.items.length)} sweeps shown; older sweeps are not listed here.</Notice>
      ) : null}
      {page.data ? (
        <DataTable
          label="Recent sweeps"
          rows={page.data.items}
          rowKey={(s) => s.id}
          columns={columns}
          empty="No sweep has been submitted in this workspace yet."
          fill={fill}
          maxHeight={fill ? undefined : 360}
        />
      ) : null}
    </Panel>
  );
}
