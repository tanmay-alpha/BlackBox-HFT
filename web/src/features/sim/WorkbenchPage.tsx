// Simulation Workbench (UI_DESIGN_SPEC.md §11.5): the job queue of this workspace with state,
// queue position, progress, heartbeat age and attempts exactly as `GET /api/v1/jobs` reports
// them, and the selected job's detail with cancel / re-submit. `/sim` lists, `/sim/jobs/:jobId`
// selects. Polling follows API_CONTRACT §8.5 (2 s; 5 s when waking or hidden).

import { useMemo, useRef } from 'react';
import { useLocation, useNavigate, useParams, useSearchParams } from 'react-router-dom';

import { api, PAGE_LIMIT } from '../../api/client';
import { LIVE_JOB_STATES, type Job, type JobState } from '../../api/model';
import { DataTable, type Column } from '../../components/DataTable';
import { SplitPane } from '../../components/SplitPane';
import { RouteTabs } from '../../components/Tabs';
import { DASH, fmtInt } from '../../components/fmt';
import { Empty, ErrorBox, Notice, Panel, Spinner } from '../../components/ui';
import { useAccess } from '../../shell/access';
import { useHealth } from '../../shell/health';
import { paths } from '../../shell/paths';
import { useNow, workerHealth } from '../../shell/StatusBar';
import { Workspace } from '../../shell/Workspace';
import { heartbeatText, JobDetail, JobProgress, JobStatePill, queueText, utc } from './JobViews';
import { usePolledJob, usePolledJobs } from './jobs';
import './sim.css';

const ALL_STATES: JobState[] = ['created', 'queued', 'running', 'cancelling', 'completed', 'failed', 'cancelled', 'expired'];

const FILTERS: { id: string; label: string; states: readonly JobState[] | undefined }[] = [
  { id: 'live', label: 'Non-terminal', states: LIVE_JOB_STATES },
  { id: 'all', label: 'All states', states: undefined },
  ...ALL_STATES.map((s) => ({ id: s, label: s, states: [s] as const })),
];

const PAGE = PAGE_LIMIT;

export function SimTabs() {
  return (
    <RouteTabs
      label="Simulation Workbench"
      items={[
        { to: paths.sim(), label: 'Jobs', end: true },
        { to: paths.sweepForm(), label: 'Sweep' },
        { to: paths.runs(), label: 'Runs' },
      ]}
    />
  );
}

export default function WorkbenchPage() {
  const { jobId } = useParams();
  const location = useLocation();
  const submitted = typeof location.state?.submitted === 'string' ? (location.state.submitted as string) : null;
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const access = useAccess();
  const health = useHealth();
  const now = useNow();
  const filterId = FILTERS.some((f) => f.id === params.get('state')) ? (params.get('state') as string) : 'live';
  const filter = FILTERS.find((f) => f.id === filterId) ?? FILTERS[0];
  const mine = params.get('mine') === '1' && access.mode === 'research';

  const truncated = useRef(false);
  const list = usePolledJobs(
    async (signal) => {
      const page = await api.jobsPage({ state: filter.states, mine: mine || undefined, limit: PAGE }, { signal });
      truncated.current = page.next_cursor !== null;
      return page.items;
    },
    `${filterId}|${mine ? 1 : 0}|${access.whoami?.workspace.slug ?? ''}`,
  );
  const selected = usePolledJob(jobId ?? null);
  const job = selected.jobs[0] ?? null;
  const worker = health.status ? workerHealth(health.status, health.checkedAt, now.getTime()) : null;
  const nowMs = now.getTime();

  const columns = useMemo<Column<Job>[]>(
    () => [
      { id: 'state', header: 'State', cell: (j) => <JobStatePill job={j} />, sortValue: (j) => j.state },
      { id: 'job', header: 'Job', rowHeader: true, cell: (j) => <span className="mono">{j.id.slice(0, 13)}</span>, raw: (j) => j.id },
      { id: 'kind', header: 'Kind', cell: (j) => j.kind },
      {
        id: 'experiment',
        header: 'Experiment',
        cell: (j) => <span className="mono" title={j.experiment_fingerprint}>{j.experiment_fingerprint.slice(0, 12)}</span>,
      },
      { id: 'created', header: 'Created (UTC)', cell: (j) => <span className="mono">{utc(j.created_at_ns)}</span>, sortValue: (j) => BigInt(j.created_at_ns) },
      { id: 'pos', header: 'Queue', cell: (j) => <span className="mono">{queueText(j)}</span>, sortValue: (j) => j.queue_position },
      { id: 'progress', header: 'Progress', cell: (j) => <JobProgress job={j} /> },
      { id: 'hb', header: 'Heartbeat', cell: (j) => <span className="mono">{heartbeatText(j, nowMs)}</span> },
      { id: 'attempt', header: 'Attempt', numeric: true, cell: (j) => `${j.attempt}/${j.max_attempts}`, sortValue: (j) => j.attempt },
      { id: 'error', header: 'Failure', cell: (j) => (j.error ? `${j.error.category} \u00b7 ${j.error.code}` : j.expired_reason ?? DASH) },
    ],
    [nowMs],
  );

  const setFilter = (id: string) => {
    const next = new URLSearchParams(params);
    if (id === 'live') next.delete('state');
    else next.set('state', id);
    setParams(next);
  };

  const toolbar = (
    <>
      <SimTabs />
      <label className="row">
        Filter state{' '}
        <select value={filterId} onChange={(e) => setFilter(e.target.value)} data-testid="job-filter">
          {FILTERS.map((f) => (
            <option key={f.id} value={f.id}>
              {f.label}
            </option>
          ))}
        </select>
      </label>
      <label className="row" title={access.mode === 'research' ? 'Only jobs created by this key' : 'Needs an API key: in the demo there is no "mine".'}>
        <input
          type="checkbox"
          checked={mine}
          aria-disabled={access.mode !== 'research' || undefined}
          onChange={(e) => {
            if (access.mode !== 'research') return access.openKeyDialog();
            const next = new URLSearchParams(params);
            if (e.target.checked) next.set('mine', '1');
            else next.delete('mine');
            setParams(next);
          }}
        />
        Mine only
      </label>
    </>
  );

  const stale = list.error !== null && list.updatedAt !== null;

  return (
    <Workspace context={jobId ? `job ${jobId.slice(0, 12)}` : null} toolbar={toolbar} layout>
      {submitted ? (
        <Notice tag="SUBMITTED" tone="info">
          {submitted}
        </Notice>
      ) : null}
      {health.phase === 'waking' ? (
        <Notice tag="WAKING">The API is waking up (free host; first response can take about a minute). The table keeps its last data.</Notice>
      ) : null}
      <SplitPane
        id="sim"
        panes={[
          {
            id: 'queue',
            label: 'Job queue',
            defaultSize: '62%',
            minSize: '30%',
            content: (
              <Panel
                title="Job queue"
                fill
                flush
                caption={
                  <>
                    {worker
                      ? `${worker.text} \u00b7 queued ${fmtInt(worker.queueDepth)} \u00b7 running ${fmtInt(worker.running)}${worker.heartbeatAgeS !== null ? ` \u00b7 hb ${worker.heartbeatAgeS} s` : ''}`
                      : 'worker status not reported yet'}
                    {stale ? ' \u00b7 STALE (last refresh failed)' : ''}
                    {truncated.current ? ` \u00b7 first ${PAGE} jobs shown` : ''}
                  </>
                }
                actions={
                  <button type="button" onClick={list.reload}>
                    Refresh
                  </button>
                }
              >
                {list.error ? <ErrorBox error={list.error} onRetry={list.reload} /> : null}
                {list.loading && list.updatedAt === null ? <Spinner label="Loading jobs" /> : null}
                {list.updatedAt !== null ? (
                  <DataTable
                    label="Jobs"
                    columns={columns}
                    rows={list.jobs}
                    rowKey={(j) => j.id}
                    selectedKey={jobId ?? null}
                    onSelect={(j) => navigate({ pathname: paths.job(j.id), search: params.toString() })}
                    onActivate={(j) => navigate({ pathname: paths.job(j.id), search: params.toString() })}
                    fill
                    empty={
                      <Empty>
                        No {filter.id === 'all' ? '' : `${filter.label.toLowerCase()} `}jobs in workspace {access.whoami?.workspace.slug ?? '?'}
                        {mine ? ' created by this key' : ''}.
                      </Empty>
                    }
                  />
                ) : null}
              </Panel>
            ),
          },
          {
            id: 'detail',
            label: 'Job detail',
            minSize: '25%',
            content: (
              <Panel title={job ? `Job ${job.id.slice(0, 13)}` : 'Job'} fill>
                {!jobId ? <Empty>Select a job to see its state, progress, heartbeat and result.</Empty> : null}
                {jobId && selected.error ? <ErrorBox error={selected.error} onRetry={selected.reload} /> : null}
                {jobId && !job && selected.loading ? <Spinner label="Loading job" /> : null}
                {job ? <JobDetail job={job} nowMs={nowMs} onChanged={() => { selected.reload(); list.reload(); }} /> : null}
              </Panel>
            ),
          },
        ]}
      />
    </Workspace>
  );
}
