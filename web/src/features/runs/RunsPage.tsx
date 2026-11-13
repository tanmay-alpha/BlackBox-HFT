import { useState } from 'react';
import { Link } from 'react-router-dom';

import { api, isStaticViewer, STATIC_VIEWER_MESSAGE } from '../../api/client';
import type { RunSummary } from '../../api/types';
import { assumptionsLabel, strategyLabel } from '../../components/describe';
import { fmtDuration, fmtInt, fmtUnixMs } from '../../components/fmt';
import { PageHeader, usePageTitle } from '../../components/page';
import { Badge, Empty, ErrorBox, HashText, Panel, Spinner, useAsync } from '../../components/ui';
import { fmtQuoteSigned } from './format';

/** Creation time with the date over the time, so the table fits a 1440 px viewport with its actions. */
function CreatedCell({ ms }: { ms: number | null }) {
  const [date, ...rest] = fmtUnixMs(ms).split(' ');
  return (
    <td className="mono">
      {date}
      {rest.length > 0 ? (
        <>
          <br />
          <span className="dim">{rest.join(' ')}</span>
        </>
      ) : null}
    </td>
  );
}

function RunRow({
  run,
  slot,
  canPick,
  onToggle,
  confirming,
  deleting,
  onAskDelete,
  onCancelDelete,
  onConfirmDelete,
}: {
  run: RunSummary;
  /** `A`/`B` when the run is picked for comparison. */
  slot: 'A' | 'B' | null;
  canPick: boolean;
  onToggle: () => void;
  confirming: boolean;
  deleting: boolean;
  onAskDelete: () => void;
  onCancelDelete: () => void;
  onConfirmDelete: () => void;
}) {
  const m = run.manifest;
  const to = `/runs/${encodeURIComponent(m.run_id)}`;
  return (
    <tr className={slot ? 'selected' : undefined}>
      <td>
        <label className="row run-pick">
          <input
            type="checkbox"
            checked={slot !== null}
            disabled={slot === null && !canPick}
            onChange={onToggle}
            aria-label={`Pick run ${run.label || m.run_id} for comparison`}
          />
          {slot ? <Badge kind="neutral">{slot}</Badge> : null}
        </label>
      </td>
      <th scope="row">
        <Link to={to}>{run.label || <span className="dim">unlabelled</span>}</Link>
        <br />
        <span className="mono dim">{m.run_id}</span>
      </th>
      <td className="mono">
        {strategyLabel(m.config.strategy.kind)}
        <br />
        <span className="dim">every {fmtDuration(m.config.strategy.interval_ns)}</span>
      </td>
      <td className="mono wrap wrap-anywhere">{m.dataset.id}</td>
      <td className="mono wrap">{assumptionsLabel(m.config)}</td>
      <td className="num">{fmtInt(run.order_count)}</td>
      <td className="num">{fmtInt(run.fill_count)}</td>
      <td className="num">{fmtQuoteSigned(run.metrics.net_pnl)}</td>
      <td>
        <HashText hash={m.result_sha256} chars={12} />
      </td>
      <CreatedCell ms={m.created_unix_ms} />
      <td>
        {confirming ? (
          <span className="row run-actions" role="group" aria-label={`Confirm deleting ${m.run_id}`}>
            <span>Delete this run and its tape?</span>
            <button type="button" className="btn-danger" onClick={onConfirmDelete} disabled={deleting}>
              {deleting ? 'Deleting' : 'Confirm delete'}
            </button>
            <button type="button" onClick={onCancelDelete} disabled={deleting}>
              Cancel
            </button>
          </span>
        ) : (
          <span className="row run-actions">
            <Link className="btn" to={to}>
              Open
            </Link>
            <button
              type="button"
              className="btn-danger"
              onClick={onAskDelete}
              disabled={isStaticViewer()}
              title={isStaticViewer() ? STATIC_VIEWER_MESSAGE : undefined}
              aria-label={`Delete run ${run.label || m.run_id}`}
            >
              Delete
            </button>
          </span>
        )}
      </td>
    </tr>
  );
}

export default function RunsPage() {
  usePageTitle('Runs');
  const runs = useAsync(() => api.runs(), []);
  // In pick order: the first is scenario A, the second scenario B.
  const [picked, setPicked] = useState<string[]>([]);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<unknown>(null);

  const toggle = (runId: string) => {
    setPicked((p) => {
      if (p.includes(runId)) return p.filter((id) => id !== runId);
      return p.length < 2 ? [...p, runId] : p;
    });
  };

  const confirmDelete = async (runId: string) => {
    setDeleting(runId);
    setDeleteError(null);
    try {
      await api.deleteRun(runId);
      setPicked((p) => p.filter((id) => id !== runId));
      setConfirming(null);
      runs.reload();
    } catch (e) {
      setDeleteError(e);
    } finally {
      setDeleting(null);
    }
  };

  const list = runs.data;
  // A picked run can disappear (deleted elsewhere); only offer comparison for runs still listed.
  const live = picked.filter((id) => list?.some((r) => r.manifest.run_id === id));
  const compareTo =
    live.length === 2
      ? `/compare?a=${encodeURIComponent(live[0])}&b=${encodeURIComponent(live[1])}`
      : null;

  return (
    <div className="col">
      <PageHeader
        title="Runs"
        lead="Every stored simulation, newest first. A run is identified by the hash of its inputs, so repeating an experiment returns the same run rather than a new one."
        actions={
          <>
            <button type="button" onClick={runs.reload} disabled={runs.loading}>
              Reload
            </button>
            <Link className="btn btn-primary" to="/lab">
              New run
            </Link>
          </>
        }
      />

      {runs.error ? <ErrorBox error={runs.error} /> : null}
      {deleteError !== null ? <ErrorBox error={deleteError} /> : null}
      {runs.loading && !list ? <Spinner label="Loading runs" /> : null}

      {list && list.length === 0 ? (
        <Empty>
          <p>No runs are stored yet.</p>
          <p>
            Configure and simulate one in the <Link to="/lab">Strategy Laboratory</Link>.
          </p>
        </Empty>
      ) : null}

      {list && list.length > 0 ? (
        <Panel
          title={
            <>
              Stored runs <Badge kind="modeled">simulated</Badge>
            </>
          }
          actions={
            <>
              <span className="note">
                {live.length === 0
                  ? 'Tick two runs to compare them.'
                  : live.length === 1
                    ? 'Scenario A picked; tick one more for B.'
                    : 'A and B picked.'}
              </span>
              <button
                type="button"
                disabled={live.length !== 2}
                onClick={() => setPicked([live[1], live[0]])}
              >
                Swap A and B
              </button>
              {compareTo ? (
                <Link className="btn btn-primary" to={compareTo}>
                  Compare A with B
                </Link>
              ) : (
                <button type="button" className="btn-primary" disabled>
                  Compare A with B
                </button>
              )}
            </>
          }
        >
          <div className="table-scroll">
            <table className="table">
              <thead>
                <tr>
                  <th scope="col">Pick</th>
                  <th scope="col">Label / run id</th>
                  <th scope="col">Strategy</th>
                  <th scope="col">Dataset</th>
                  <th scope="col">Execution assumptions (modeled)</th>
                  <th scope="col" className="num">
                    Orders
                  </th>
                  <th scope="col" className="num">
                    Fills
                  </th>
                  <th scope="col" className="num">
                    Net P&amp;L
                  </th>
                  <th scope="col">Result hash</th>
                  <th scope="col">Created</th>
                  <th scope="col">Actions</th>
                </tr>
              </thead>
              <tbody>
                {list.map((r) => {
                  const id = r.manifest.run_id;
                  const idx = live.indexOf(id);
                  return (
                    <RunRow
                      key={id}
                      run={r}
                      slot={idx === 0 ? 'A' : idx === 1 ? 'B' : null}
                      canPick={live.length < 2}
                      onToggle={() => toggle(id)}
                      confirming={confirming === id}
                      deleting={deleting === id}
                      onAskDelete={() => {
                        setConfirming(id);
                        setDeleteError(null);
                      }}
                      onCancelDelete={() => setConfirming(null)}
                      onConfirmDelete={() => void confirmDelete(id)}
                    />
                  );
                })}
              </tbody>
            </table>
          </div>
          <p className="note runs-footnote">
            Net P&amp;L is simulated (realized + unrealized {'\u2212'} fees at the last sample, in
            quote currency) under each run&apos;s own assumptions; it is not comparable across
            datasets and is never an exchange result. Equal result hashes mean byte-identical
            tapes, fills and equity samples.
          </p>
        </Panel>
      ) : null}
    </div>
  );
}
