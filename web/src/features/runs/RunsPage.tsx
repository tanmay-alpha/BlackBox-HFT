import { useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';

import { api } from '../../api/client';
import type { Run } from '../../api/model';
import { DataTable, type Column } from '../../components/DataTable';
import { fmtDuration, fmtInt, fmtPnl } from '../../components/fmt';
import { fmtNsUtc } from '../../components/ns';
import { ToolbarDivider } from '../../components/Toolbar';
import { Badge, ErrorBox, HashText, Notice, Panel, Spinner, useAsync } from '../../components/ui';
import { GatedButton } from '../../shell/access';
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

function RunInspector({ run }: { run: Run }) {
  return (
    <div className="col">
      <dl className="kv">
        <dt>Label</dt>
        <dd>{run.label ?? <span className="dim">unlabelled</span>}</dd>
        <dt>Run id</dt>
        <dd className="mono wrap-anywhere">{run.run_id}</dd>
        <dt>Strategy</dt>
        <dd className="mono">
          {run.strategy_kind} every {fmtDuration(run.interval_ns)}
        </dd>
        <dt>Dataset</dt>
        <dd className="mono wrap-anywhere">
          <Link to={paths.dataset(run.dataset.id)}>{run.dataset.id}</Link> v{run.dataset.version}
        </dd>
        <dt>Config fingerprint</dt>
        <dd className="mono wrap-anywhere">
          {run.fingerprint} <Badge kind="modeled">modeled</Badge>
        </dd>
        <dt>Orders / fills</dt>
        <dd className="mono">
          {fmtInt(run.counts.orders)} / {fmtInt(run.counts.fills)}
        </dd>
        <dt>
          Net P&amp;L <span className="unit">quote</span>
        </dt>
        <dd className={`mono ${signClass(run.metrics.net_pnl) ?? ''}`}>{fmtPnl(run.metrics.net_pnl, 4)}</dd>
        <dt>Result hash</dt>
        <dd className="mono wrap-anywhere">{run.result_sha256}</dd>
        <dt>Platform</dt>
        <dd className="mono">
          {run.platform}
          {run.canonical ? ' (canonical)' : ''}
        </dd>
        <dt>Created</dt>
        <dd className="mono">{fmtNsUtc(run.created_at_ns, 0)}</dd>
      </dl>
      <div className="row">
        <Link className="btn" to={paths.run(run.run_id)}>
          Open run
        </Link>
        <Link className="btn" to={paths.riskRun(run.run_id)}>
          Risk &amp; Performance
        </Link>
        <Link className="btn" to={paths.report(run.run_id)}>
          Research Report
        </Link>
      </div>
      <p className="note">The full execution assumptions are on the run page. {PNL_NOTE}</p>
    </div>
  );
}

export default function RunsPage() {
  const navigate = useNavigate();
  const runs = useAsync((signal) => api.runs({}, { signal }), []);
  // In pick order: the first is scenario A, the second scenario B.
  const [picked, setPicked] = useState<string[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<unknown>(null);

  const list = runs.data?.items ?? null;
  // A picked run can disappear (its experiment deleted elsewhere); only offer comparison for runs still listed.
  const live = picked.filter((id) => list?.some((r) => r.run_id === id));
  const selectedRun = list?.find((r) => r.run_id === selected) ?? null;

  const toggle = (runId: string) =>
    setPicked((p) => (p.includes(runId) ? p.filter((id) => id !== runId) : [...p, runId]));

  const confirmDelete = async (run: Run, experimentId: string) => {
    setDeleting(true);
    setDeleteError(null);
    try {
      await api.deleteExperiment(experimentId);
      setPicked((p) => p.filter((id) => id !== run.run_id));
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
    selectedRun ? `Run ${selectedRun.label ?? selectedRun.run_id}` : 'Stored runs',
    selectedRun ? (
      <RunInspector run={selectedRun} />
    ) : (
      <div className="col">
        <p className="note">{LEAD}</p>
        <p className="note">{PNL_NOTE}</p>
      </div>
    ),
    [selectedRun],
  );

  const columns = useMemo<Column<Run>[]>(
    () => [
      {
        id: 'pick',
        header: 'Pick',
        width: 56,
        cell: (r) => {
          const id = r.run_id;
          const idx = live.indexOf(id);
          return (
            <label className="row runs-pick" onClick={(e) => e.stopPropagation()}>
              <input
                type="checkbox"
                checked={idx >= 0}
                onChange={() => toggle(id)}
                aria-label={`Pick run ${r.label ?? id} for comparison`}
              />
              {idx === 0 ? <Badge kind="neutral">A</Badge> : idx === 1 ? <Badge kind="neutral">B</Badge> : null}
            </label>
          );
        },
        raw: (r) => (live.includes(r.run_id) ? 'picked' : ''),
      },
      {
        id: 'run',
        header: 'Run id',
        rowHeader: true,
        cell: (r) => (
          <Link to={paths.run(r.run_id)} tabIndex={-1}>
            <HashText hash={r.run_id} />
          </Link>
        ),
        raw: (r) => r.run_id,
        sortValue: (r) => r.run_id,
      },
      {
        id: 'label',
        header: 'Label',
        cell: (r) => r.label ?? <span className="dim">unlabelled</span>,
        sortValue: (r) => r.label,
      },
      {
        id: 'dataset',
        header: 'Dataset',
        cell: (r) => (
          <span className="mono">
            {r.dataset.id} v{r.dataset.version}
          </span>
        ),
        sortValue: (r) => r.dataset.id,
      },
      {
        id: 'strategy',
        header: 'Strategy',
        cell: (r) => <span className="mono">{r.strategy_kind}</span>,
        sortValue: (r) => r.strategy_kind,
      },
      {
        id: 'interval',
        header: 'Interval',
        numeric: true,
        cell: (r) => fmtDuration(r.interval_ns),
        sortValue: (r) => r.interval_ns,
      },
      { id: 'orders', header: 'Orders', unit: 'count', numeric: true, cell: (r) => fmtInt(r.counts.orders), sortValue: (r) => r.counts.orders },
      { id: 'fills', header: 'Fills', unit: 'count', numeric: true, cell: (r) => fmtInt(r.counts.fills), sortValue: (r) => r.counts.fills },
      {
        id: 'pnl',
        header: 'Net P&L',
        unit: 'quote',
        numeric: true,
        cell: (r) => fmtPnl(r.metrics.net_pnl, 4),
        className: (r) => signClass(r.metrics.net_pnl),
        sortValue: (r) => r.metrics.net_pnl,
      },
      { id: 'hash', header: 'Result hash', cell: (r) => <HashText hash={r.result_sha256} />, raw: (r) => r.result_sha256, sortValue: (r) => r.result_sha256 },
      {
        id: 'created',
        header: 'Created',
        unit: 'UTC',
        cell: (r) => <span className="mono">{fmtNsUtc(r.created_at_ns, 0)}</span>,
        raw: (r) => fmtNsUtc(r.created_at_ns, 0),
        sortValue: (r) => BigInt(r.created_at_ns),
      },
      {
        id: 'views',
        header: 'Views',
        cell: (r) => (
          <span className="row" onClick={(e) => e.stopPropagation()}>
            <Link to={paths.riskRun(r.run_id)} tabIndex={-1}>
              Risk
            </Link>
            <Link to={paths.report(r.run_id)} tabIndex={-1}>
              Report
            </Link>
          </span>
        ),
        raw: () => '',
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
          {selectedRun?.experiment_id ? (
            confirming ? (
              <span className="row" role="group" aria-label={`Confirm deleting the experiment of ${selectedRun.run_id}`}>
                <span>Delete this run’s experiment from the workspace?</span>
                <GatedButton
                  scope="run"
                  className="btn-danger"
                  onClick={() => void confirmDelete(selectedRun, selectedRun.experiment_id as string)}
                  disabled={deleting}
                >
                  {deleting ? 'Deleting' : 'Confirm delete'}
                </GatedButton>
                <button type="button" onClick={() => setConfirming(false)} disabled={deleting}>
                  Cancel
                </button>
              </span>
            ) : (
              <GatedButton
                scope="run"
                className="btn-danger"
                onClick={() => {
                  setConfirming(true);
                  setDeleteError(null);
                }}
                aria-label={`Delete the experiment of run ${selectedRun.label ?? selectedRun.run_id}`}
              >
                Delete experiment
              </GatedButton>
            )
          ) : null}
        </>
      }
    >
      {runs.error ? <ErrorBox error={runs.error} onRetry={runs.reload} /> : null}
      {deleteError !== null ? <ErrorBox error={deleteError} /> : null}
      {runs.loading && !list ? <Spinner label="Loading runs" /> : null}
      {runs.data?.incomplete ? (
        <Notice tag="List incomplete">First {fmtInt(runs.data.items.length)} runs shown; older runs are not listed here.</Notice>
      ) : null}
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
            rowKey={(r) => r.run_id}
            columns={columns}
            selectedKey={selected}
            onSelect={(r) => {
              setSelected(r.run_id);
              setConfirming(false);
            }}
            onActivate={(r) => navigate(paths.run(r.run_id))}
            empty={
              <>
                No runs are stored yet. Configure and simulate one in the{' '}
                <Link to={paths.strategy()}>Strategy Lab</Link>.
              </>
            }
            footer={<p className="note">{PNL_NOTE} Enter or double-click opens a run; Space selects it for the inspector.</p>}
            fill
          />
        </Panel>
      ) : null}
    </Workspace>
  );
}
