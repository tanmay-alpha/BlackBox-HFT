// The workspaces that show one run, linked from each other with the run id in the address, so a
// reader moves along Strategy Lab → job → run → replay → order forensics → comparison → report
// without copying the id (UI_AUDIT "context links"). The workspace in view is left out.

import { Link } from 'react-router-dom';

import { paths } from '../../shell/paths';

export type RunView = 'overview' | 'replay' | 'risk' | 'report' | 'compare';

const ORDER: RunView[] = ['overview', 'replay', 'risk', 'report', 'compare'];

const LABEL: Record<RunView, string> = {
  overview: 'Run overview',
  replay: 'Replay',
  risk: 'Risk & P&L',
  report: 'Report',
  compare: 'Compare with…',
};

/** Where each view of `runId` lives; `order` opens the replay at that order. */
export function runViewPath(view: RunView, runId: string, order?: number | null): string {
  switch (view) {
    case 'overview':
      return paths.run(runId);
    case 'replay':
      return paths.bookRun(runId, { order: order ?? null });
    case 'risk':
      return paths.riskRun(runId);
    case 'report':
      return paths.report(runId);
    case 'compare':
      return paths.comparePair(runId);
  }
}

/** The views of a run not already on the page (`omit`), in journey order. */
export function runViews(omit: readonly RunView[]): RunView[] {
  return ORDER.filter((v) => !omit.includes(v));
}

/** `disabled`: the run could not be read (e.g. 404), so the views are shown but not offered. */
export function RunContextLinks({ runId, omit, order, disabled }: { runId: string; omit: readonly RunView[]; order?: number | null; disabled?: boolean }) {
  return (
    <>
      {runViews(omit).map((v) =>
        disabled ? (
          <button key={v} type="button" className="btn" disabled data-run-view={v}>
            {LABEL[v]}
          </button>
        ) : (
          <Link key={v} className="btn" to={runViewPath(v, runId, order)} data-run-view={v}>
            {LABEL[v]}
          </Link>
        ),
      )}
    </>
  );
}
