// Overview (UI_DESIGN_SPEC.md §11.1): the canonical experiments of the chosen dataset (run on
// demand through the idempotent run endpoint), the most recent runs, the registered datasets and
// the state of the API. The selected experiment's reading guide, controlled settings and CLI
// reproduction are in the inspector.

import { useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';

import { api, isStaticViewer } from '../../api/client';
import { CONTRACT_VERSION } from '../../api/types';
import type { DatasetManifest, Demo, QualityReport, RunSummary, Severity } from '../../api/types';
import { DataTable, type Column } from '../../components/DataTable';
import { assumptionsLabel, severityBadge, strategyLabel } from '../../components/describe';
import { DASH, fmtDuration, fmtInt, fmtPnl, fmtUnixMs } from '../../components/fmt';
import { SplitPane } from '../../components/SplitPane';
import { StaticViewerNotice } from '../../components/staticViewer';
import { Badge, BasisBadge, ErrorBox, HashText, Panel, Pill, Spinner, useAsync } from '../../components/ui';
import { useHealth, type ApiPhase } from '../../shell/health';
import { useInspector } from '../../shell/inspector';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import './mission.css';

const RECENT_RUNS = 12;

const SEVERITY_RANK: Record<Severity, number> = { info: 0, warn: 1, error: 2 };

/** Highest severity among failed quality checks, or `null` when every check passed. */
function worstFailed(q: QualityReport): Severity | null {
  let worst: Severity | null = null;
  for (const c of q.checks) {
    if (c.passed) continue;
    if (worst === null || SEVERITY_RANK[c.severity] > SEVERITY_RANK[worst]) worst = c.severity;
  }
  return worst;
}

interface DemoProgress {
  demoId: string;
  done: number;
  total: number;
}

/**
 * Stored runs with exactly the configurations of the demo, in demo order, or `null` when any run
 * has not been simulated yet. Runs are content-addressed, so equal configurations are the same run.
 */
function storedDemoRuns(demo: Demo, runs: RunSummary[] | null): string[] | null {
  if (!runs) return null;
  const byConfig = new Map(runs.map((r) => [JSON.stringify(r.manifest.config), r.manifest.run_id]));
  const ids: string[] = [];
  for (const r of demo.runs) {
    const id = byConfig.get(JSON.stringify(r.config));
    if (id === undefined) return null;
    ids.push(id);
  }
  return ids;
}

function runHint(demo: Demo): string {
  return isStaticViewer()
    ? `Opens the ${demo.runs.length} precomputed runs in Experiment Comparison (Set). This hosted viewer simulates nothing; run BlackBox locally to simulate (see README).`
    : `Runs ${demo.runs.length} simulations, or returns the stored ones, then opens Experiment Comparison (Set).`;
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
            {i === 0 ? <span className="dim"> (baseline)</span> : null}
          </li>
        ))}
      </ol>
      <h3 className="inspector-heading">How to read</h3>
      <p>{demo.how_to_read}</p>
      <p className="note">{runHint(demo)}</p>
      <h3 className="inspector-heading">Reproduce from the command line ({demo.reproduce.length} commands)</h3>
      {demo.reproduce.map((cmd, i) => (
        <pre key={i} className="ov-cmd">
          {cmd}
        </pre>
      ))}
    </div>
  );
}

function Experiments({
  datasetId,
  runs,
  progress,
  setProgress,
}: {
  datasetId: string;
  runs: RunSummary[] | null;
  progress: DemoProgress | null;
  setProgress: (p: DemoProgress | null) => void;
}) {
  const navigate = useNavigate();
  const demos = useAsync(() => api.demos(datasetId || undefined), [datasetId]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [runError, setRunError] = useState<unknown>(null);
  const list = demos.data ?? [];
  const selected = list.find((d) => d.id === selectedId) ?? list[0] ?? null;

  useInspector(
    selected ? selected.title : 'Canonical experiments',
    selected ? <ExperimentDetails demo={selected} /> : <p className="note">No experiment is loaded.</p>,
    [selected],
  );

  const runDemo = async (demo: Demo) => {
    setRunError(null);
    setSelectedId(demo.id);
    setProgress({ demoId: demo.id, done: 0, total: demo.runs.length });
    const ids: string[] = [];
    try {
      // Sequential: the server runs each simulation synchronously; one at a time keeps the
      // progress honest and avoids competing simulations.
      for (const [i, r] of demo.runs.entries()) {
        const summary = await api.createRun({ config: r.config, label: `${demo.title}: ${r.label}` });
        ids.push(summary.manifest.run_id);
        setProgress({ demoId: demo.id, done: i + 1, total: demo.runs.length });
      }
    } catch (e) {
      setRunError(e);
      setProgress(null);
      return;
    }
    setProgress(null);
    navigate(paths.compareSet({ runs: ids, demo: demo.id }));
  };

  const columns: Column<Demo>[] = [
    { id: 'title', header: 'Experiment', rowHeader: true, cell: (d) => d.title, sortValue: (d) => d.title },
    { id: 'question', header: 'Question', cell: (d) => <span title={d.question}>{d.question}</span> },
    {
      id: 'varied',
      header: 'Varied',
      cell: (d) => <span className="mono">{d.varied.length === 0 ? 'nothing' : d.varied.join(', ')}</span>,
    },
    { id: 'runs', header: 'Runs', unit: 'count', numeric: true, cell: (d) => fmtInt(d.runs.length), sortValue: (d) => d.runs.length },
    {
      id: 'actions',
      header: 'Actions',
      cell: (d) => {
        const stored = storedDemoRuns(d, runs);
        return (
          <span className="row ov-actions">
            <button type="button" className="btn-primary" onClick={() => void runDemo(d)} disabled={progress !== null} title={runHint(d)}>
              {isStaticViewer() ? 'Open precomputed demo' : 'Run demo'}
            </button>
            {stored ? (
              <Link to={paths.compareSet({ runs: stored, demo: d.id })} title="All runs of this experiment are stored; open them without simulating.">
                Open results
              </Link>
            ) : null}
          </span>
        );
      },
      raw: () => '',
    },
  ];

  return (
    <Panel
      title="Canonical experiments"
      caption="Each experiment is a set of run configurations on one dataset that differ only in the listed variables. Runs are content-addressed: running a demo again returns the same run ids and results. Select a row for its reading guide, controlled settings and command-line reproduction in the inspector (i)."
      flush
      actions={
        <button type="button" onClick={demos.reload} disabled={demos.loading}>
          Reload
        </button>
      }
    >
      {progress !== null || runError || demos.error || (demos.loading && !demos.data) ? (
        <div className="ov-status col">
          {progress !== null ? (
            <Spinner
              label={
                isStaticViewer()
                  ? 'Opening the precomputed runs'
                  : `Simulating run ${Math.min(progress.done + 1, progress.total)} of ${progress.total} (stored runs return immediately)`
              }
            />
          ) : null}
          {runError ? <ErrorBox error={runError} /> : null}
          {demos.loading && !demos.data ? <Spinner label="Loading experiments" /> : null}
          {demos.error ? <ErrorBox error={demos.error} onRetry={demos.reload} /> : null}
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

const RUN_COLUMNS: Column<RunSummary>[] = [
  {
    id: 'run',
    header: 'Run',
    rowHeader: true,
    cell: (r) => (
      <Link to={paths.run(r.manifest.run_id)}>
        <HashText hash={r.manifest.run_id} />
      </Link>
    ),
    raw: (r) => r.manifest.run_id,
  },
  {
    id: 'label',
    header: 'Label',
    cell: (r) => (r.label ? <span title={r.label}>{r.label}</span> : <span className="dim">unlabelled</span>),
    sortValue: (r) => r.label,
  },
  { id: 'dataset', header: 'Dataset', cell: (r) => <span className="mono">{r.manifest.dataset.id}</span>, sortValue: (r) => r.manifest.dataset.id },
  { id: 'strategy', header: 'Strategy', cell: (r) => <span className="mono">{strategyLabel(r.manifest.config.strategy.kind)}</span> },
  {
    id: 'assumptions',
    header: 'Execution assumptions',
    cell: (r) => {
      const text = assumptionsLabel(r.manifest.config);
      return <span title={text}>{text}</span>;
    },
  },
  { id: 'orders', header: 'Orders', unit: 'count', numeric: true, cell: (r) => fmtInt(r.order_count), sortValue: (r) => r.order_count },
  { id: 'fills', header: 'Fills', unit: 'count', numeric: true, cell: (r) => fmtInt(r.fill_count), sortValue: (r) => r.fill_count },
  {
    id: 'net',
    header: 'Net P&L',
    unit: 'quote',
    numeric: true,
    cell: (r) => fmtPnl(r.metrics.net_pnl, 4),
    sortValue: (r) => r.metrics.net_pnl,
    className: (r) => (r.metrics.net_pnl > 0 ? 'up' : r.metrics.net_pnl < 0 ? 'down' : undefined),
  },
  {
    id: 'created',
    header: 'Created',
    cell: (r) => <span className="mono">{fmtUnixMs(r.manifest.created_unix_ms)}</span>,
    sortValue: (r) => r.manifest.created_unix_ms,
  },
  {
    id: 'open',
    header: 'Open',
    cell: (r) => (
      <span className="row ov-actions">
        <Link to={paths.run(r.manifest.run_id)}>Forensics</Link>
        <Link to={paths.bookRun(r.manifest.run_id)}>Replay Terminal</Link>
      </span>
    ),
    raw: () => '',
  },
];

function RecentRuns({ runs, error, loading, reload }: { runs: RunSummary[] | null; error: Error | null; loading: boolean; reload: () => void }) {
  const navigate = useNavigate();
  const list = runs ? runs.slice(0, RECENT_RUNS) : null;
  return (
    <Panel
      title="Recent runs"
      flush
      actions={
        <>
          <Link className="btn" to={paths.runs()}>
            All runs
          </Link>
          <button type="button" onClick={reload} disabled={loading}>
            Reload
          </button>
        </>
      }
    >
      {(loading && !runs) || error ? (
        <div className="ov-status col">
          {loading && !runs ? <Spinner label="Loading runs" /> : null}
          {error ? <ErrorBox error={error} onRetry={reload} /> : null}
        </div>
      ) : null}
      {list ? (
        <DataTable
          label="Recent runs"
          rows={list}
          rowKey={(r) => r.manifest.run_id}
          columns={RUN_COLUMNS}
          onActivate={(r) => navigate(paths.run(r.manifest.run_id))}
          empty={
            <>
              No runs yet. Run one of the experiments above or configure a run in the{' '}
              <Link to={paths.strategy()}>Strategy Lab</Link>.
            </>
          }
          footer={
            <span>
              Newest first, {fmtInt(list.length)} of {fmtInt(runs?.length ?? 0)} runs. All values are simulated.
            </span>
          }
          fill
        />
      ) : null}
    </Panel>
  );
}

// ----- datasets -------------------------------------------------------------------------------

const DATASET_COLUMNS: Column<DatasetManifest>[] = [
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
  {
    id: 'source',
    header: 'Source',
    cell: (d) => (
      <span className="mono">
        {d.source.vendor} · {d.source.exchange} · {d.source.symbol}
      </span>
    ),
  },
  { id: 'span', header: 'Span', numeric: true, cell: (d) => fmtDuration(d.end - d.start), sortValue: (d) => d.end - d.start },
  { id: 'rows', header: 'Rows', unit: 'count', numeric: true, cell: (d) => fmtInt(d.counts.rows), sortValue: (d) => d.counts.rows },
  {
    id: 'quality',
    header: 'Quality',
    cell: (d) => {
      const worst = worstFailed(d.quality);
      const failed = d.quality.checks.filter((c) => !c.passed).length;
      return worst === null ? (
        <Badge kind="ok">{d.quality.checks.length} checks passed</Badge>
      ) : (
        <Badge kind={severityBadge(worst)}>
          worst failed: {worst} ({failed} of {d.quality.checks.length})
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

function Datasets({ datasets, error, loading, reload }: { datasets: DatasetManifest[] | null; error: Error | null; loading: boolean; reload: () => void }) {
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
          {error ? <ErrorBox error={error} onRetry={reload} /> : null}
        </div>
      ) : null}
      {datasets ? (
        <DataTable
          label="Datasets"
          rows={datasets}
          rowKey={(d) => d.id}
          columns={DATASET_COLUMNS}
          empty={
            <>
              No datasets are registered on the server. Ingest one with the <span className="mono">blackbox</span> CLI,
              then reload.
            </>
          }
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
  const health = h.health;
  return (
    <Panel
      title="System"
      actions={
        <button type="button" onClick={h.retry} title="Request GET /api/health now">
          Check now
        </button>
      }
    >
      <dl className="kv">
        <dt>API</dt>
        <dd className="row">
          <Pill tone={PHASE_TONE[h.phase]}>
            {h.phase === 'unreachable' ? 'ERROR ' : ''}
            {h.phase}
          </Pill>
          {h.phase === 'ok' && h.rttMs !== null ? <span>round trip {fmtInt(h.rttMs)} ms</span> : null}
        </dd>
        {h.lastError ? (
          <>
            <dt>Last error</dt>
            <dd className="wrap-anywhere">{h.lastError}</dd>
          </>
        ) : null}
        <dt>Server status</dt>
        <dd>{health ? health.status : DASH}</dd>
        <dt>BlackBox version</dt>
        <dd>{health ? health.blackbox_version : DASH}</dd>
        <dt>Contract</dt>
        <dd className="row">
          <span>
            {health ? `v${health.contract_version}` : DASH} <span className="dim">(console v{CONTRACT_VERSION})</span>
          </span>
          {h.contractMismatch ? (
            <Pill tone="warn" title="This console was built for another API contract version. Fields may be missing or misread.">
              WARN mismatch
            </Pill>
          ) : null}
        </dd>
        <dt>Engine</dt>
        <dd>{health ? `${health.engine.name} ${health.engine.version}` : DASH}</dd>
        {health && health.engine.features.length > 0 ? (
          <>
            <dt>Engine features</dt>
            <dd>{health.engine.features.join(', ')}</dd>
          </>
        ) : null}
        <dt>Mode</dt>
        <dd>{isStaticViewer() ? 'PRECOMPUTED BUNDLE' : 'LOCAL'}</dd>
      </dl>
      {health ? <p className="prose ov-disclaimer">{health.disclaimer}</p> : null}
      <section className="col ov-about" aria-labelledby="ov-about-title">
        <h3 id="ov-about-title" className="inspector-heading">
          What this laboratory can and cannot tell you
        </h3>
        <p className="prose">
          Replay historical Level-2 market data, simulate a strategy's orders through a pinned matching engine, and
          take every simulated order apart on two clocks. Research only: nothing here is a live or real execution.
        </p>
        <dl className="ov-bases">
          <dt>
            <BasisBadge basis="observed" />
          </dt>
          <dd>
            Historical Level-2 book updates and trade prints, with the timestamps the data records. Prices and
            displayed quantity per price level come from the dataset, not from us.
          </dd>
          <dt>
            <BasisBadge basis="modeled" />
          </dt>
          <dd>
            Everything about your orders: latency, queue position, fills and fees are outputs of the configured
            execution model on a historical replay. A simulated fill is not an execution.
          </dd>
          <dt>
            <BasisBadge basis="unobservable" />
          </dt>
          <dd>
            How the market would have reacted to your orders, the individual orders and hidden liquidity behind each
            price level, and your true place in the queue. Replay cannot show market impact.
          </dd>
        </dl>
      </section>
    </Panel>
  );
}

// ----- workspace ------------------------------------------------------------------------------

export default function MissionControlPage() {
  const datasets = useAsync(() => api.datasets(), []);
  const runs = useAsync(() => api.runs(), []);
  const [datasetId, setDatasetId] = useState('');
  const [progress, setProgress] = useState<DemoProgress | null>(null);
  const list = datasets.data ?? [];

  const toolbar = (
    <label className="row ov-dataset">
      <span>Dataset</span>
      <select value={datasetId} onChange={(e) => setDatasetId(e.target.value)} disabled={progress !== null || list.length === 0}>
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
        <StaticViewerNotice />
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
                    {
                      id: 'experiments',
                      label: 'Canonical experiments',
                      defaultSize: '40%',
                      minSize: '20%',
                      content: (
                        <Experiments
                          datasetId={datasetId}
                          runs={runs.data}
                          progress={progress}
                          setProgress={setProgress}
                        />
                      ),
                    },
                    {
                      id: 'runs',
                      label: 'Recent runs',
                      defaultSize: '60%',
                      minSize: '20%',
                      content: <RecentRuns runs={runs.data} error={runs.error} loading={runs.loading} reload={runs.reload} />,
                    },
                  ]}
                />
              ),
            },
            {
              id: 'side',
              label: 'System and datasets',
              defaultSize: '36%',
              minSize: '20%',
              content: (
                <SplitPane
                  id="overview.side"
                  orientation="vertical"
                  panes={[
                    { id: 'system', label: 'System', defaultSize: '60%', minSize: '20%', content: <System /> },
                    {
                      id: 'datasets',
                      label: 'Datasets',
                      defaultSize: '40%',
                      minSize: '15%',
                      content: (
                        <Datasets datasets={datasets.data} error={datasets.error} loading={datasets.loading} reload={datasets.reload} />
                      ),
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
