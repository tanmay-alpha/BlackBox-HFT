// Overview (UI_DESIGN_SPEC.md §11.1): the canonical experiments of the chosen dataset (their
// stored runs open directly; "Run demo" submits them as experiment jobs), the most recent runs,
// "My jobs" when a research key is present, the registered datasets and the state of the API and
// worker from `GET /api/v1/status`. The selected experiment's reading guide, controlled settings
// and CLI reproduction are in the inspector.

import { useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';

import { api } from '../../api/client';
import type { DatasetSummary, Demo, Job, Run } from '../../api/model';
import { CONTRACT_VERSION } from '../../api/types';
import { DataTable, type Column } from '../../components/DataTable';
import { DASH, fmtDuration, fmtInt, fmtPnl } from '../../components/fmt';
import { fmtNsUtc } from '../../components/ns';
import { SplitPane } from '../../components/SplitPane';
import { Badge, BasisBadge, Empty, ErrorBox, HashText, Notice, Panel, Pill, Spinner, useAsync } from '../../components/ui';
import { GatedButton, useAccess } from '../../shell/access';
import { useAnnounce } from '../../shell/announce';
import { useHealth, type ApiPhase } from '../../shell/health';
import { useInspector } from '../../shell/inspector';
import { paths } from '../../shell/paths';
import { useNow, workerHealth } from '../../shell/StatusBar';
import { Workspace } from '../../shell/Workspace';
import { heartbeatText, JobProgress, JobStatePill, queueText } from '../sim/JobViews';
import { usePolledJobs } from '../sim/jobs';
import '../sim/sim.css';
import './mission.css';

const RECENT_RUNS = 12;
const MY_JOBS = 10;

/** The demo's stored runs in demo order, or `null` while any has no visible canonical run. */
function storedDemoRuns(demo: Demo): string[] | null {
  const ids = demo.runs.map((r) => r.run_id);
  return ids.every((id): id is string => id !== null) ? ids : null;
}

// ----- canonical experiments ------------------------------------------------------------------

function ExperimentDetails({ demo }: { demo: Demo }) {
  return (
    <div className="col">
      <p>{demo.question}</p>
      <h3 className="inspector-heading">Varied</h3>
      {demo.varied.length === 0 ? (
        <p className="dim">nothing</p>
      ) : (
        <ul className="ov-list mono">
          {demo.varied.map((v) => (
            <li key={v}>{v}</li>
          ))}
        </ul>
      )}
      <h3 className="inspector-heading">Controlled ({demo.controlled.length} settings held identical)</h3>
      <ul className="ov-list">
        {demo.controlled.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ul>
      <h3 className="inspector-heading">Runs</h3>
      <ol className="ov-list">
        {demo.runs.map((r, i) => (
          <li key={i}>
            {r.label}
            {i === 0 ? <span className="dim"> (baseline)</span> : null}{' '}
            {r.run_id ? (
              <Link className="mono" to={paths.run(r.run_id)}>
                {r.run_id}
              </Link>
            ) : (
              <span className="dim">not simulated on this deployment</span>
            )}
          </li>
        ))}
      </ol>
      <h3 className="inspector-heading">How to read</h3>
      <p>{demo.how_to_read}</p>
      <p className="note">
        &quot;Run demo&quot; submits the {demo.runs.length} configurations as experiment jobs; stored runs return at once. Needs an API key.
      </p>
      <h3 className="inspector-heading">Reproduce from the command line ({demo.reproduce.length} commands)</h3>
      {demo.reproduce.map((cmd, i) => (
        <pre key={i} className="ov-cmd">
          {cmd}
        </pre>
      ))}
    </div>
  );
}

function Experiments({ datasetId }: { datasetId: string }) {
  const navigate = useNavigate();
  const announce = useAnnounce();
  const demos = useAsync((signal) => api.demos(datasetId || undefined, { signal }), [datasetId]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [runError, setRunError] = useState<unknown>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const list = demos.data ?? [];
  const selected = list.find((d) => d.id === selectedId) ?? list[0] ?? null;

  useInspector(
    selected ? selected.title : 'Canonical experiments',
    selected ? <ExperimentDetails demo={selected} /> : <p className="note">No experiment is loaded.</p>,
    [selected],
  );

  const runDemo = async (demo: Demo) => {
    setRunError(null);
    setNote(null);
    setSelectedId(demo.id);
    setPending(demo.id);
    try {
      const results = [];
      for (const r of demo.runs) {
        results.push(await api.submitExperiment(r.config, `${demo.id}: ${r.label}`));
      }
      const stored = results.every((r) => r.run !== null);
      const ids = results.map((r) => r.experiment.run_id);
      if (stored) {
        announce(`All ${ids.length} runs of ${demo.title} are stored; opening the comparison.`);
        navigate(paths.compareSet({ runs: ids, demo: demo.id }));
      } else {
        const queued = results.filter((r) => r.job !== null).length;
        const text = `${queued} of ${ids.length} runs of ${demo.title} were submitted as jobs; the rest are stored. Follow them in the Simulation Workbench, then open the results.`;
        setNote(text);
        announce(text);
        demos.reload();
      }
    } catch (e) {
      setRunError(e);
    } finally {
      setPending(null);
    }
  };

  const columns: Column<Demo>[] = [
    { id: 'title', header: 'Experiment', rowHeader: true, cell: (d) => d.title, sortValue: (d) => d.title },
    { id: 'question', header: 'Question', cell: (d) => <span title={d.question}>{d.question}</span> },
    { id: 'varied', header: 'Varied', cell: (d) => <span className="mono">{d.varied.length === 0 ? 'nothing' : d.varied.join(', ')}</span> },
    {
      id: 'runs',
      header: 'Runs stored',
      numeric: true,
      cell: (d) => `${fmtInt(d.runs.filter((r) => r.run_id !== null).length)} / ${fmtInt(d.runs.length)}`,
      sortValue: (d) => d.runs.length,
    },
    {
      id: 'actions',
      header: 'Actions',
      cell: (d) => {
        const stored = storedDemoRuns(d);
        return (
          <span className="row ov-actions">
            {stored ? (
              <Link to={paths.compareSet({ runs: stored, demo: d.id })} title="Every run of this experiment is stored; open them without simulating.">
                Open results
              </Link>
            ) : null}
            <GatedButton onClick={() => void runDemo(d)} blockedReason={pending !== null ? 'Submitting\u2026' : null} title="Submit every configuration as an experiment job.">
              Run demo
            </GatedButton>
          </span>
        );
      },
      raw: () => '',
    },
  ];

  return (
    <Panel
      title="Canonical experiments"
      caption="Each experiment is a set of run configurations on one dataset that differ only in the listed variables. Runs are content-addressed: submitting a demo again returns the same runs. Select a row for its reading guide, controlled settings and command-line reproduction in the inspector (i)."
      flush
      actions={
        <button type="button" onClick={demos.reload} disabled={demos.loading}>
          Reload
        </button>
      }
    >
      {pending !== null || runError || note || demos.error || (demos.loading && !demos.data) ? (
        <div className="ov-status col">
          {pending !== null ? <Spinner label="Submitting the experiment's runs" /> : null}
          {note ? (
            <p className="note">
              {note} <Link to={paths.sim({ mine: 1 })}>Open my jobs</Link>
            </p>
          ) : null}
          <ErrorBox error={runError} />
          {demos.loading && !demos.data ? <Spinner label="Loading experiments" /> : null}
          <ErrorBox error={demos.error} onRetry={demos.reload} />
        </div>
      ) : null}
      {demos.data ? (
        <DataTable
          label="Canonical experiments"
          rows={demos.data}
          rowKey={(d) => d.id}
          columns={columns}
          selectedKey={selected?.id ?? null}
          onSelect={(d) => setSelectedId(d.id)}
          empty="The server offers no experiments for this dataset."
          fill
        />
      ) : null}
    </Panel>
  );
}

// ----- recent runs ----------------------------------------------------------------------------

const RUN_COLUMNS: Column<Run>[] = [
  {
    id: 'run',
    header: 'Run',
    rowHeader: true,
    cell: (r) => (
      <Link to={paths.run(r.run_id)}>
        <HashText hash={r.run_id} />
      </Link>
    ),
    raw: (r) => r.run_id,
  },
  {
    id: 'label',
    header: 'Label',
    cell: (r) => (r.label ? <span title={r.label}>{r.label}</span> : <span className="dim">unlabelled</span>),
    sortValue: (r) => r.label ?? '',
  },
  { id: 'dataset', header: 'Dataset', cell: (r) => <span className="mono">{r.dataset.id}</span>, sortValue: (r) => r.dataset.id },
  { id: 'strategy', header: 'Strategy', cell: (r) => <span className="mono">{r.strategy_kind} every {fmtDuration(r.interval_ns)}</span> },
  { id: 'orders', header: 'Orders', unit: 'count', numeric: true, cell: (r) => fmtInt(r.counts.orders), sortValue: (r) => r.counts.orders },
  { id: 'fills', header: 'Fills', unit: 'count', numeric: true, cell: (r) => fmtInt(r.counts.fills), sortValue: (r) => r.counts.fills },
  {
    id: 'net',
    header: 'Net P&L',
    unit: 'quote',
    numeric: true,
    cell: (r) => fmtPnl(r.metrics.net_pnl, 4),
    sortValue: (r) => r.metrics.net_pnl,
    className: (r) => (r.metrics.net_pnl > 0 ? 'up' : r.metrics.net_pnl < 0 ? 'down' : undefined),
  },
  { id: 'created', header: 'Created (UTC)', cell: (r) => <span className="mono">{fmtNsUtc(r.created_at_ns, 0)}</span>, sortValue: (r) => BigInt(r.created_at_ns) },
  {
    id: 'open',
    header: 'Open',
    cell: (r) => (
      <span className="row ov-actions">
        <Link to={paths.run(r.run_id)}>Forensics</Link>
        <Link to={paths.riskRun(r.run_id)}>Risk</Link>
        <Link to={paths.report(r.run_id)}>Report</Link>
      </span>
    ),
    raw: () => '',
  },
];

function RecentRuns() {
  const navigate = useNavigate();
  const runs = useAsync((signal) => api.runsPage({ limit: RECENT_RUNS }, { signal }), []);
  return (
    <Panel
      title="Recent runs"
      flush
      actions={
        <>
          <Link className="btn" to={paths.runs()}>
            All runs
          </Link>
          <button type="button" onClick={runs.reload} disabled={runs.loading}>
            Reload
          </button>
        </>
      }
    >
      {(runs.loading && !runs.data) || runs.error ? (
        <div className="ov-status col">
          {runs.loading && !runs.data ? <Spinner label="Loading runs" /> : null}
          <ErrorBox error={runs.error} onRetry={runs.reload} />
        </div>
      ) : null}
      {runs.data ? (
        <DataTable
          label="Recent runs"
          rows={runs.data.items}
          rowKey={(r) => r.run_id}
          columns={RUN_COLUMNS}
          onActivate={(r) => navigate(paths.run(r.run_id))}
          empty={
            <>
              No runs yet. Run one of the experiments above or configure a run in the <Link to={paths.strategy()}>Strategy Lab</Link>.
            </>
          }
          footer={<span>Newest first, the {fmtInt(runs.data.items.length)} most recent runs. All values are simulated.</span>}
          fill
        />
      ) : null}
    </Panel>
  );
}

// ----- my jobs --------------------------------------------------------------------------------

function MyJobs() {
  const access = useAccess();
  const now = useNow();
  const research = access.mode === 'research';
  const jobs = usePolledJobs(
    async (signal) => (research ? (await api.jobsPage({ mine: true, limit: MY_JOBS }, { signal })).items : []),
    `mine|${research ? access.whoami?.key?.id ?? '' : 'demo'}`,
  );
  const nowMs = now.getTime();
  const columns = useMemo<Column<Job>[]>(
    () => [
      { id: 'job', header: 'Job', rowHeader: true, cell: (j) => <Link className="mono" to={paths.job(j.id)}>{j.id.slice(0, 13)}</Link>, raw: (j) => j.id },
      { id: 'state', header: 'State', cell: (j) => <JobStatePill job={j} /> },
      { id: 'where', header: 'Queue / progress', cell: (j) => (j.state === 'queued' ? <span className="mono">{queueText(j)}</span> : <JobProgress job={j} />) },
      { id: 'hb', header: 'Heartbeat', cell: (j) => <span className="mono">{heartbeatText(j, nowMs)}</span> },
    ],
    [nowMs],
  );
  return (
    <Panel
      title="My jobs"
      flush
      actions={
        <Link className="btn" to={paths.sim({ mine: 1, state: 'all' })}>
          Workbench
        </Link>
      }
    >
      {!research ? (
        <Empty>
          Shown with an API key: jobs submitted by your key.{' '}
          <button type="button" className="btn-link" onClick={access.openKeyDialog}>
            Enter API key
          </button>
        </Empty>
      ) : (
        <>
          <ErrorBox error={jobs.error} onRetry={jobs.reload} />
          {jobs.loading && jobs.updatedAt === null ? <Spinner label="Loading my jobs" /> : null}
          {jobs.updatedAt !== null ? (
            <DataTable label="My jobs" rows={jobs.jobs} rowKey={(j) => j.id} columns={columns} empty={<Empty>No jobs submitted by this key.</Empty>} fill />
          ) : null}
        </>
      )}
    </Panel>
  );
}

// ----- datasets -------------------------------------------------------------------------------

const DATASET_COLUMNS: Column<DatasetSummary>[] = [
  { id: 'title', header: 'Dataset', rowHeader: true, cell: (d) => <span title={d.title}>{d.title}</span>, sortValue: (d) => d.title },
  { id: 'id', header: 'Id', cell: (d) => <span className="mono">{d.id}</span>, sortValue: (d) => d.id },
  {
    id: 'kind',
    header: 'Kind',
    cell: (d) =>
      d.kind === 'synthetic' ? (
        <Badge kind="warn" title="Generated by BlackBox from a seed. This is not market data.">
          synthetic
        </Badge>
      ) : (
        <Badge kind="observed" title="Recorded historical market data.">
          recorded
        </Badge>
      ),
  },
  { id: 'version', header: 'Version', numeric: true, cell: (d) => (d.current_version ? `v${d.current_version.version}` : DASH) },
  {
    id: 'source',
    header: 'Source',
    cell: (d) => (d.current_version ? <span className="mono">{d.current_version.source?.vendor ?? DASH} · {d.current_version.instrument.exchange} · {d.current_version.instrument.symbol}</span> : DASH),
  },
  {
    id: 'span',
    header: 'Span',
    numeric: true,
    cell: (d) => (d.current_version ? fmtDuration(d.current_version.end_ns - d.current_version.start_ns) : DASH),
    sortValue: (d) => (d.current_version ? d.current_version.end_ns - d.current_version.start_ns : null),
  },
  { id: 'rows', header: 'Rows', unit: 'count', numeric: true, cell: (d) => fmtInt(d.current_version?.rows), sortValue: (d) => d.current_version?.rows },
  {
    id: 'quality',
    header: 'Quality',
    cell: (d) => {
      const q = d.current_version?.quality;
      if (!q) return DASH;
      return q.passed ? (
        <Badge kind="ok">passed{q.warning_count > 0 ? `, ${q.warning_count} warnings` : ''}</Badge>
      ) : (
        <Badge kind="error">
          {q.error_count} errors, {q.warning_count} warnings
        </Badge>
      );
    },
  },
  {
    id: 'open',
    header: 'Open',
    cell: (d) => (
      <span className="row ov-actions">
        <Link to={paths.dataset(d.id)}>Market data</Link>
        <Link to={paths.bookDataset(d.id)}>Order book</Link>
        <Link to={paths.strategy({ dataset: d.id })}>Strategy Lab</Link>
      </span>
    ),
    raw: () => '',
  },
];

function Datasets({ datasets, error, loading, reload }: { datasets: DatasetSummary[] | null; error: Error | null; loading: boolean; reload: () => void }) {
  return (
    <Panel
      title="Datasets"
      flush
      actions={
        <button type="button" onClick={reload} disabled={loading}>
          Reload
        </button>
      }
    >
      {(loading && !datasets) || error ? (
        <div className="ov-status col">
          {loading && !datasets ? <Spinner label="Loading datasets" /> : null}
          <ErrorBox error={error} onRetry={reload} />
        </div>
      ) : null}
      {datasets ? (
        <DataTable
          label="Datasets"
          rows={datasets}
          rowKey={(d) => d.id}
          columns={DATASET_COLUMNS}
          empty={<>No datasets are registered on the server. Ingest one with the <span className="mono">blackbox</span> CLI, then reload.</>}
          fill
        />
      ) : null}
    </Panel>
  );
}

// ----- system ---------------------------------------------------------------------------------

const PHASE_TONE: Record<ApiPhase, 'ok' | 'warn' | 'error' | 'neutral'> = {
  connecting: 'neutral',
  ok: 'ok',
  waking: 'warn',
  unreachable: 'error',
};

function System() {
  const h = useHealth();
  const access = useAccess();
  const now = useNow();
  const s = h.status;
  const w = s ? workerHealth(s, h.checkedAt, now.getTime()) : null;
  return (
    <Panel
      title="System"
      actions={
        <>
          <Link className="btn" to={paths.system()}>
            Details
          </Link>
          <button type="button" onClick={h.retry} title="Request GET /api/v1/status now">
            Check now
          </button>
        </>
      }
    >
      <dl className="kv" data-testid="ov-system">
        <dt>API</dt>
        <dd className="row">
          <Pill tone={PHASE_TONE[h.phase]}>
            {h.phase === 'unreachable' ? 'ERROR ' : ''}
            {h.phase}
          </Pill>
          {h.phase === 'ok' && h.rttMs !== null ? <span>round trip {fmtInt(h.rttMs)} ms</span> : null}
          {s?.status === 'draining' ? <Pill tone="warn">draining</Pill> : null}
        </dd>
        {h.lastError ? (
          <>
            <dt>Last error</dt>
            <dd className="wrap-anywhere">{h.lastError}</dd>
          </>
        ) : null}
        <dt>Worker</dt>
        <dd className="row">
          {s && w ? (
            <>
              <Pill tone={w.stale ? 'warn' : 'ok'}>{s.worker.state}</Pill>
              <span className="mono">
                busy {s.worker.busy}/{s.worker.slots} {'\u00b7'} queued {fmtInt(w.queueDepth)} {'\u00b7'} running {fmtInt(w.running)}
              </span>
            </>
          ) : (
            DASH
          )}
        </dd>
        <dt>Heartbeat</dt>
        <dd className="mono">
          {w && s ? (w.heartbeatAgeS === null ? 'none reported' : `${w.heartbeatAgeS} s ago (every ${Math.round(s.worker.heartbeat_interval_ms / 1000)} s)${w.stale ? ' STALE' : ''}`) : DASH}
        </dd>
        <dt>Contract</dt>
        <dd className="row">
          <span>
            {s ? `v${s.contract_version}` : DASH} <span className="dim">(console v{CONTRACT_VERSION})</span>
          </span>
          {h.contractMismatch ? (
            <Pill tone="warn" title="This console was built for another API contract version. Fields may be missing or misread.">
              WARN mismatch
            </Pill>
          ) : null}
        </dd>
        <dt>Build</dt>
        <dd className="mono">{s ? `${s.blackbox_version} \u00b7 ${s.git_sha?.slice(0, 12) ?? 'commit not reported'}` : DASH}</dd>
        <dt>Engine</dt>
        <dd>{s ? `${s.engine.name} ${s.engine.version}` : DASH}</dd>
        <dt>Access</dt>
        <dd className="row">
          {access.mode === 'research' && access.whoami ? (
            <span>
              RESEARCH {'\u00b7'} workspace <span className="mono">{access.whoami.workspace.slug}</span>
            </span>
          ) : (
            <>
              <span>DEMO read-only</span>
              <button type="button" className="btn-link" onClick={access.openKeyDialog}>
                Enter key
              </button>
            </>
          )}
        </dd>
      </dl>
      {s ? <p className="prose ov-disclaimer">{s.disclaimer}</p> : null}
      <section className="col ov-about" aria-labelledby="ov-about-title">
        <h3 id="ov-about-title" className="inspector-heading">
          What this laboratory can and cannot tell you
        </h3>
        <p className="prose">
          Replay historical Level-2 market data, simulate a strategy&apos;s orders through a pinned matching engine, and take every simulated order
          apart on two clocks. Research only: nothing here is a live or real execution.
        </p>
        <dl className="ov-bases">
          <dt>
            <BasisBadge basis="observed" />
          </dt>
          <dd>
            Historical Level-2 book updates and trade prints, with the timestamps the data records. Prices and displayed quantity per price level
            come from the dataset, not from us.
          </dd>
          <dt>
            <BasisBadge basis="modeled" />
          </dt>
          <dd>
            Everything about your orders: latency, queue position, fills and fees are outputs of the configured execution model on a historical
            replay. A simulated fill is not an execution.
          </dd>
          <dt>
            <BasisBadge basis="unobservable" />
          </dt>
          <dd>
            How the market would have reacted to your orders, the individual orders and hidden liquidity behind each price level, and your true
            place in the queue. Replay cannot show market impact.
          </dd>
        </dl>
      </section>
    </Panel>
  );
}

// ----- workspace ------------------------------------------------------------------------------

export default function MissionControlPage() {
  const access = useAccess();
  const datasets = useAsync((signal) => api.datasets({ signal }), []);
  const [datasetId, setDatasetId] = useState('');
  const list = datasets.data?.items ?? [];

  const toolbar = (
    <label className="row ov-dataset">
      <span>Dataset</span>
      <select value={datasetId} onChange={(e) => setDatasetId(e.target.value)} disabled={list.length === 0}>
        <option value="">{list.length > 0 ? `server default (${list[0].id})` : 'server default'}</option>
        {list.map((d) => (
          <option key={d.id} value={d.id}>
            {d.id}
          </option>
        ))}
      </select>
    </label>
  );

  return (
    <Workspace context={datasetId || null} toolbar={toolbar} layout>
      <div className="ov-body">
        {access.mode === 'demo' ? (
          <Notice tag="DEMO" tone="info">
            Public demo workspace, read-only: every stored result can be opened; submitting experiments, sweeps and comparisons needs an API key.
          </Notice>
        ) : null}
        <SplitPane
          id="overview.main"
          panes={[
            {
              id: 'work',
              label: 'Experiments and runs',
              defaultSize: '64%',
              minSize: '35%',
              content: (
                <SplitPane
                  id="overview.left"
                  orientation="vertical"
                  panes={[
                    { id: 'experiments', label: 'Canonical experiments', defaultSize: '40%', minSize: '20%', content: <Experiments datasetId={datasetId} /> },
                    { id: 'runs', label: 'Recent runs', defaultSize: '60%', minSize: '20%', content: <RecentRuns /> },
                  ]}
                />
              ),
            },
            {
              id: 'side',
              label: 'System, jobs and datasets',
              defaultSize: '36%',
              minSize: '20%',
              content: (
                <SplitPane
                  id="overview.side2"
                  orientation="vertical"
                  panes={[
                    { id: 'system', label: 'System', defaultSize: '45%', minSize: '20%', content: <System /> },
                    { id: 'jobs', label: 'My jobs', defaultSize: '25%', minSize: '10%', content: <MyJobs /> },
                    {
                      id: 'datasets',
                      label: 'Datasets',
                      defaultSize: '30%',
                      minSize: '10%',
                      content: <Datasets datasets={datasets.data?.items ?? null} error={datasets.error} loading={datasets.loading} reload={datasets.reload} />,
                    },
                  ]}
                />
              ),
            },
          ]}
        />
      </div>
    </Workspace>
  );
}
