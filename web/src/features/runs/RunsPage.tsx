import { useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';

import { api, isStaticViewer, STATIC_VIEWER_MESSAGE } from '../../api/client';
import type { RunSummary } from '../../api/types';
import { DataTable, type Column } from '../../components/DataTable';
import { assumptionsLabel, strategyLabel } from '../../components/describe';
import { fmtDuration, fmtInt, fmtPnl, fmtUnixMs } from '../../components/fmt';
import { ToolbarDivider } from '../../components/Toolbar';
import { Badge, ErrorBox, HashText, Panel, Spinner, useAsync } from '../../components/ui';
import { useInspector } from '../../shell/inspector';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import './runs.css';

const LEAD =
  'Every stored simulation, newest first. A run is identified by the hash of its inputs, so repeating an experiment returns the same run rather than a new one.';

const PNL_NOTE =
  'Net P&L is simulated (realized + unrealized \u2212 fees at the last sample, in quote currency) under each run\u2019s own assumptions; it is not comparable across datasets and is never an exchange result. Equal result hashes mean byte-identical tapes, fills and equity samples.';

function signClass(x: number | null): string | undefined {
  return x === null ? undefined : x > 0 ? 'up' : x < 0 ? 'down' : undefined;
}

function RunSummaryDetails({ run }: { run: RunSummary }) {
  const m = run.manifest;
  return (
    <div className="col">
      <dl className="kv">
        <dt>Label</dt>
        <dd>{run.label || <span className="dim">unlabelled</span>}</dd>
        <dt>Run id</dt>
        <dd className="mono wrap-anywhere">{m.run_id}</dd>
        <dt>Strategy</dt>
        <dd className="mono">
          {strategyLabel(m.config.strategy.kind)} every {fmtDuration(m.config.strategy.interval_ns)}
        </dd>
        <dt>Dataset</dt>
        <dd className="mono wrap-anywhere">
          <Link to={paths.dataset(m.dataset.id)}>{m.dataset.id}</Link>
        </dd>
        <dt>Execution assumptions</dt>
        <dd className="mono">
          {assumptionsLabel(m.config)} <Badge kind="modeled">modeled</Badge>
        </dd>
        <dt>Orders / fills</dt>
        <dd className="mono">
          {fmtInt(run.order_count)} / {fmtInt(run.fill_count)}
        </dd>
        <dt>
          Net P&amp;L <span className="unit">quote</span>
        </dt>
        <dd className={`mono ${signClass(run.metrics.net_pnl) ?? ''}`}>{fmtPnl(run.metrics.net_pnl, 4)}</dd>
        <dt>Result hash</dt>
        <dd className="mono wrap-anywhere">{m.result_sha256}</dd>
        <dt>Created</dt>
        <dd className="mono">{fmtUnixMs(m.created_unix_ms)}</dd>
      </dl>
      <div className="row">
        <Link className="btn" to={paths.run(m.run_id)}>
          Open run
        </Link>
      </div>
      <p className="note">{PNL_NOTE}</p>
    </div>
  );
}

export default function RunsPage() {
  const navigate = useNavigate();
  const runs = useAsync(() => api.runs(), []);
  // In pick order: the first is scenario A, the second scenario B.
  const [picked, setPicked] = useState<string[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<unknown>(null);

  const list = runs.data;
  // A picked run can disappear (deleted elsewhere); only offer comparison for runs still listed.
  const live = picked.filter((id) => list?.some((r) => r.manifest.run_id === id));
  const selectedRun = list?.find((r) => r.manifest.run_id === selected) ?? null;

  const toggle = (runId: string) =>
    setPicked((p) => (p.includes(runId) ? p.filter((id) => id !== runId) : [...p, runId]));

  const confirmDelete = async (runId: string) => {
    setDeleting(true);
    setDeleteError(null);
    try {
      await api.deleteRun(runId);
      setPicked((p) => p.filter((id) => id !== runId));
      setSelected(null);
      setConfirming(false);
      runs.reload();
    } catch (e) {
      setDeleteError(e);
    } finally {
      setDeleting(false);
    }
  };

  useInspector(
    selectedRun ? `Run ${selectedRun.label || selectedRun.manifest.run_id}` : 'Stored runs',
    selectedRun ? (
      <RunSummaryDetails run={selectedRun} />
    ) : (
      <div className="col">
        <p className="note">{LEAD}</p>
        <p className="note">{PNL_NOTE}</p>
      </div>
    ),
    [selectedRun],
  );

  const columns = useMemo<Column<RunSummary>[]>(
    () => [
      {
        id: 'pick',
        header: 'Pick',
        width: 56,
        cell: (r) => {
          const id = r.manifest.run_id;
          const idx = live.indexOf(id);
          return (
            <label className="row runs-pick" onClick={(e) => e.stopPropagation()}>
              <input
                type="checkbox"
                checked={idx >= 0}
                onChange={() => toggle(id)}
                aria-label={`Pick run ${r.label || id} for comparison`}
              />
              {idx === 0 ? <Badge kind="neutral">A</Badge> : idx === 1 ? <Badge kind="neutral">B</Badge> : null}
            </label>
          );
        },
        raw: (r) => (live.includes(r.manifest.run_id) ? 'picked' : ''),
      },
      {
        id: 'run',
        header: 'Run id',
        rowHeader: true,
        cell: (r) => (
          <Link to={paths.run(r.manifest.run_id)} tabIndex={-1}>
            <HashText hash={r.manifest.run_id} />
          </Link>
        ),
        raw: (r) => r.manifest.run_id,
        sortValue: (r) => r.manifest.run_id,
      },
      {
        id: 'label',
        header: 'Label',
        cell: (r) => r.label || <span className="dim">unlabelled</span>,
        sortValue: (r) => r.label,
      },
      { id: 'dataset', header: 'Dataset', cell: (r) => <span className="mono">{r.manifest.dataset.id}</span>, sortValue: (r) => r.manifest.dataset.id },
      {
        id: 'strategy',
        header: 'Strategy',
        cell: (r) => <span className="mono">{strategyLabel(r.manifest.config.strategy.kind)}</span>,
        sortValue: (r) => r.manifest.config.strategy.kind.type,
      },
      { id: 'orders', header: 'Orders', unit: 'count', numeric: true, cell: (r) => fmtInt(r.order_count), sortValue: (r) => r.order_count },
      { id: 'fills', header: 'Fills', unit: 'count', numeric: true, cell: (r) => fmtInt(r.fill_count), sortValue: (r) => r.fill_count },
      {
        id: 'pnl',
        header: 'Net P&L',
        unit: 'quote',
        numeric: true,
        cell: (r) => fmtPnl(r.metrics.net_pnl, 4),
        className: (r) => signClass(r.metrics.net_pnl),
        sortValue: (r) => r.metrics.net_pnl,
      },
      { id: 'hash', header: 'Result hash', cell: (r) => <HashText hash={r.manifest.result_sha256} />, raw: (r) => r.manifest.result_sha256, sortValue: (r) => r.manifest.result_sha256 },
      {
        id: 'created',
        header: 'Created',
        unit: 'UTC',
        cell: (r) => <span className="mono">{fmtUnixMs(r.manifest.created_unix_ms)}</span>,
        sortValue: (r) => r.manifest.created_unix_ms,
      },
    ],
    // `toggle` only closes over the state setter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [live.join(',')],
  );

  const pickNote =
    live.length === 0
      ? 'Tick two runs to compare them.'
      : live.length === 1
        ? 'Scenario A picked; tick one more for B.'
        : live.length === 2
          ? 'A and B picked.'
          : `${live.length} runs picked.`;

  return (
    <Workspace
      context={null}
      toolbar={
        <>
          <Link className="btn btn-primary" to={paths.strategy()}>
            New run
          </Link>
          <button type="button" onClick={runs.reload} disabled={runs.loading}>
            Reload
          </button>
          <ToolbarDivider />
          <span className="dim">{pickNote}</span>
          <button type="button" disabled={live.length !== 2} onClick={() => setPicked([live[1], live[0]])}>
            Swap A and B
          </button>
          {live.length === 2 ? (
            <Link className="btn" to={paths.comparePair(live[0], live[1])}>
              Compare A with B
            </Link>
          ) : (
            <button type="button" disabled title="Pick exactly two runs">
              Compare A with B
            </button>
          )}
          {live.length >= 2 ? (
            <Link className="btn" to={paths.compareSet({ runs: live })}>
              Compare {live.length} runs as a set
            </Link>
          ) : (
            <button type="button" disabled title="Pick two or more runs">
              Compare runs as a set
            </button>
          )}
          <ToolbarDivider />
          {confirming && selectedRun ? (
            <span className="row" role="group" aria-label={`Confirm deleting ${selectedRun.manifest.run_id}`}>
              <span>Delete this run and its tape?</span>
              <button
                type="button"
                className="btn-danger"
                onClick={() => void confirmDelete(selectedRun.manifest.run_id)}
                disabled={deleting}
              >
                {deleting ? 'Deleting' : 'Confirm delete'}
              </button>
              <button type="button" onClick={() => setConfirming(false)} disabled={deleting}>
                Cancel
              </button>
            </span>
          ) : (
            <button
              type="button"
              className="btn-danger"
              onClick={() => {
                setConfirming(true);
                setDeleteError(null);
              }}
              disabled={isStaticViewer() || selectedRun === null}
              title={isStaticViewer() ? STATIC_VIEWER_MESSAGE : selectedRun === null ? 'Select a run row first' : undefined}
              aria-label={selectedRun ? `Delete run ${selectedRun.label || selectedRun.manifest.run_id}` : 'Delete run'}
            >
              Delete
            </button>
          )}
        </>
      }
    >
      {runs.error ? <ErrorBox error={runs.error} onRetry={runs.reload} /> : null}
      {deleteError !== null ? <ErrorBox error={deleteError} /> : null}
      {runs.loading && !list ? <Spinner label="Loading runs" /> : null}
      {list ? (
        <Panel
          title={
            <>
              Stored runs <Badge kind="modeled">simulated</Badge>
            </>
          }
          caption={LEAD}
          flush
          fill
        >
          <DataTable
            label="Stored runs"
            rows={list}
            rowKey={(r) => r.manifest.run_id}
            columns={columns}
            selectedKey={selected}
            onSelect={(r) => {
              setSelected(r.manifest.run_id);
              setConfirming(false);
            }}
            onActivate={(r) => navigate(paths.run(r.manifest.run_id))}
            empty={
              <>
                No runs are stored yet. Configure and simulate one in the{' '}
                <Link to={paths.strategy()}>Strategy Lab</Link>.
              </>
            }
            footer={<p className="note">{PNL_NOTE} Enter or double-click opens a run; Space selects it for the inspector and Delete.</p>}
            fill
          />
        </Panel>
      ) : null}
    </Workspace>
  );
}
