// System / Admin (UI_DESIGN_SPEC.md §11.10). Health comes from `GET /api/v1/status` (memory-only,
// polled by the shell) and, on demand, `GET /readyz` (database and storage readiness; it wakes
// the database, so it is never polled). Admin reads (`/system/*`) need an `admin` key; without
// one their panels say so instead of calling. Every view here is read-only: key management and
// dataset administration stay on the CLI.

import { useMemo, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';

import { api, PAGE_LIMIT } from '../../api/client';
import type { ApiKeyInfo, DatasetSummary, DatasetVersion, SystemEvent, WorkerHeartbeat } from '../../api/model';
import type { Readiness } from '../../api/v1';
import { DataTable, type Column } from '../../components/DataTable';
import { DASH, fmtBytes, fmtDuration, fmtInt, fmtPct } from '../../components/fmt';
import { fmtNsUtc } from '../../components/ns';
import { Empty, ErrorBox, Notice, Panel, Pill, Spinner, useAsync } from '../../components/ui';
import { useAccess } from '../../shell/access';
import { useHealth } from '../../shell/health';
import { paths } from '../../shell/paths';
import { COMMIT_NOT_REPORTED, RESEARCH_BANNER, useNow, WORKER_MODE, workerHealth } from '../../shell/StatusBar';
import { Workspace } from '../../shell/Workspace';
import { fmtSeconds } from '../sim/JobViews';
import './system.css';

type Tab = 'health' | 'queue' | 'storage' | 'access' | 'datasets' | 'events' | 'about';

const TABS: { id: Tab; label: string }[] = [
  { id: 'health', label: 'Health' },
  { id: 'queue', label: 'Queue' },
  { id: 'storage', label: 'Storage' },
  { id: 'access', label: 'Access' },
  { id: 'datasets', label: 'Datasets' },
  { id: 'events', label: 'Events' },
  { id: 'about', label: 'About' },
];

const ATTRIBUTION =
  'Charts on run pages use TradingView Lightweight Charts\u2122, Copyright (c) 2025 TradingView, Inc. https://www.tradingview.com/';

const HOST_NOTE =
  'Hosted on a free tier: the API spins down after 15 minutes without traffic and takes about a minute to spin up (Render docs, Deploy for Free).';

const utc = (ns: string | null) => (ns === null ? DASH : fmtNsUtc(ns, 0));
const clock = (ms: number | null) => (ms === null ? DASH : new Date(ms).toISOString().slice(11, 19) + ' UTC');

interface HealthRow {
  component: string;
  state: string;
  tone: 'ok' | 'warn' | 'error' | 'neutral' | 'info';
  detail: string;
  checked: string;
}

const HEALTH_COLUMNS: Column<HealthRow>[] = [
  { id: 'component', header: 'Component', rowHeader: true, cell: (r) => r.component },
  { id: 'state', header: 'State', cell: (r) => <Pill tone={r.tone}>{r.state}</Pill>, raw: (r) => r.state },
  { id: 'detail', header: 'Detail', cell: (r) => <span className="mono">{r.detail}</span>, raw: (r) => r.detail },
  { id: 'checked', header: 'Checked', cell: (r) => <span className="mono">{r.checked}</span> },
];

function AdminOnly({ what }: { what: string }) {
  const access = useAccess();
  return (
    <Notice tag="ADMIN" tone="info">
      {what} needs a key with the admin scope.{' '}
      {access.mode !== 'research' ? (
        <button type="button" className="btn-link" onClick={access.openKeyDialog}>
          Enter API key
        </button>
      ) : (
        'This key does not have it.'
      )}
    </Notice>
  );
}

function HealthTab() {
  const h = useHealth();
  const access = useAccess();
  const now = useNow();
  const [ready, setReady] = useState<{ at: number; value: Readiness | null; error: unknown } | null>(null);
  const [checking, setChecking] = useState(false);
  const isAdmin = access.hasScope('admin');
  const workers = useAsync((signal) => (isAdmin ? api.systemWorkers({ signal }) : Promise.resolve(null)), [isAdmin]);
  const s = h.status;
  const w = s ? workerHealth(s, h.checkedAt, now.getTime()) : null;
  const checked = clock(h.checkedAt);

  const rows: HealthRow[] = [];
  rows.push({
    component: 'API',
    state: h.phase === 'ok' ? (s?.status ?? 'ok') : h.phase,
    tone: h.phase === 'ok' ? (s?.status === 'draining' ? 'warn' : 'ok') : h.phase === 'unreachable' ? 'error' : 'warn',
    detail: h.phase === 'ok' ? `${h.rttMs ?? DASH} ms round trip` : (h.lastError ?? DASH),
    checked,
  });
  if (s && w) {
    rows.push({
      component: 'Worker',
      state: w.label,
      tone: w.tone,
      detail: `${WORKER_MODE[s.worker.mode] ?? `${s.worker.mode} worker`} \u00b7 ${s.worker.busy} of ${s.worker.slots} slots busy \u00b7 ${w.heartbeatAgeS === null ? 'no heartbeat yet' : `heartbeat ${fmtSeconds(w.heartbeatAgeS)} ago`} (expected every ${Math.round(s.worker.heartbeat_interval_ms / 1000)} s; not responding after ${Math.round((3 * s.worker.heartbeat_interval_ms) / 1000)} s without one)`,
      checked,
    });
    rows.push({
      component: 'Queue',
      state: `${fmtInt(s.queue.depth)} queued`,
      tone: 'info',
      detail: `${fmtInt(s.queue.running)} running${s.queue.as_of_ns ? ` \u00b7 as of ${utc(s.queue.as_of_ns)} (worker's last poll)` : ''}`,
      checked,
    });
  }
  if (ready) {
    rows.push(
      ready.value
        ? {
            component: 'Database',
            state: ready.value.db,
            tone: ready.value.db === 'ok' ? 'ok' : 'warn',
            detail: `${ready.value.status} \u00b7 migrations ${ready.value.migrations}`,
            checked: clock(ready.at),
          }
        : {
            component: 'Database',
            state: 'not ready',
            tone: 'error',
            detail: ready.error instanceof Error ? ready.error.message : String(ready.error),
            checked: clock(ready.at),
          },
    );
    if (ready.value) {
      rows.push({ component: 'Storage', state: ready.value.blob, tone: 'info', detail: 'artefact store', checked: clock(ready.at) });
    }
  }

  const checkReady = async () => {
    setChecking(true);
    try {
      setReady({ at: Date.now(), value: await api.readyz(), error: null });
    } catch (e) {
      setReady({ at: Date.now(), value: null, error: e });
    } finally {
      setChecking(false);
    }
  };

  const workerColumns = useMemo<Column<WorkerHeartbeat>[]>(
    () => [
      { id: 'id', header: 'Worker', rowHeader: true, cell: (r) => <span className="mono">{r.worker_id}</span> },
      { id: 'mode', header: 'Mode', cell: (r) => <span title={r.mode}>{WORKER_MODE[r.mode] ?? r.mode}</span> },
      { id: 'platform', header: 'Platform', cell: (r) => <span className="mono">{r.platform}</span> },
      { id: 'version', header: 'Version', cell: (r) => <span className="mono">{r.blackbox_version}</span> },
      { id: 'slots', header: 'Slots', numeric: true, cell: (r) => fmtInt(r.slots) },
      { id: 'state', header: 'State', cell: (r) => r.state },
      { id: 'job', header: 'Current job', cell: (r) => (r.current_job_id ? <Link className="mono" to={paths.job(r.current_job_id)}>{r.current_job_id.slice(0, 13)}</Link> : DASH) },
      { id: 'hb', header: 'Heartbeat age', numeric: true, cell: (r) => fmtSeconds(Math.floor(r.last_heartbeat_age_ms / 1000)), sortValue: (r) => r.last_heartbeat_age_ms },
    ],
    [],
  );

  return (
    <div className="col system-body">
      <Panel
        title="Health"
        flush
        caption="Rows for components the API does not report are omitted, not shown as ok."
        actions={
          <>
            <button type="button" onClick={h.retry}>
              Check API now
            </button>
            <button type="button" onClick={() => void checkReady()} disabled={checking} title="GET /readyz wakes the database; it is never polled." data-testid="check-ready">
              {checking ? 'Checking\u2026' : 'Check database readiness'}
            </button>
          </>
        }
      >
        <DataTable label="Component health" columns={HEALTH_COLUMNS} rows={rows} rowKey={(r) => r.component} />
      </Panel>
      {s ? (
        <Panel title="Build and limits">
          <dl className="kv" data-testid="system-build">
            <dt>BlackBox</dt>
            <dd className="mono">{s.blackbox_version}</dd>
            <dt>Build commit</dt>
            {s.git_sha ? <dd className="mono wrap-anywhere">{s.git_sha}</dd> : <dd className="dim">{COMMIT_NOT_REPORTED}</dd>}
            <dt>API</dt>
            <dd className="mono">
              {s.api_version} {'\u00b7'} contract v{s.contract_version}
            </dd>
            <dt>Engine</dt>
            <dd className="mono">
              {s.engine.name} {s.engine.version} ({s.engine.features.join(', ')})
            </dd>
            <dt>Server time</dt>
            <dd className="mono">{fmtNsUtc(s.server_time_ns, 3)}</dd>
            <dt>Limits ({s.limits.profile})</dt>
            <dd className="mono">
              {fmtInt(s.limits.max_ticks)} ticks {'\u00b7'} {fmtInt(s.limits.max_tape_events)} tape events {'\u00b7'} {fmtBytes(s.limits.max_output_bytes)} output {'\u00b7'}{' '}
              {fmtInt(s.limits.max_sweep_cells)} sweep cells {'\u00b7'} deadline {fmtSeconds(Math.floor(s.limits.deadline_ms / 1000))}
            </dd>
          </dl>
        </Panel>
      ) : null}
      <Panel title="Workers" flush>
        {!isAdmin ? <AdminOnly what="The worker heartbeat table" /> : null}
        {workers.loading && isAdmin ? <Spinner label="Loading workers" /> : null}
        <ErrorBox error={workers.error} onRetry={workers.reload} />
        {workers.data ? <DataTable label="Workers" columns={workerColumns} rows={workers.data} rowKey={(r) => r.worker_id} empty={<Empty>No worker heartbeats recorded.</Empty>} /> : null}
      </Panel>
      <p className="note">{HOST_NOTE}</p>
    </div>
  );
}

function QueueTab() {
  const access = useAccess();
  const isAdmin = access.hasScope('admin');
  const q = useAsync((signal) => (isAdmin ? api.systemQueue({ signal }) : Promise.resolve(null)), [isAdmin]);
  return (
    <div className="col system-body">
      <p>
        The job table of this workspace is in the <Link to={paths.sim({ state: 'all' })}>Simulation Workbench</Link>.
      </p>
      <Panel title="Queue (all workspaces)">
        {!isAdmin ? <AdminOnly what="The queue summary" /> : null}
        {q.loading && isAdmin ? <Spinner label="Loading queue" /> : null}
        <ErrorBox error={q.error} onRetry={q.reload} />
        {q.data ? (
          <dl className="kv">
            {Object.entries(q.data.depth_by_platform).map(([platform, depth]) => (
              <span key={platform} className="contents">
                <dt>Queued on {platform}</dt>
                <dd className="mono">{fmtInt(depth)}</dd>
              </span>
            ))}
            <dt>Running</dt>
            <dd className="mono">{fmtInt(q.data.running)}</dd>
            <dt>Oldest queued</dt>
            <dd className="mono">{q.data.oldest_queued_age_ms === null ? DASH : fmtSeconds(Math.floor(q.data.oldest_queued_age_ms / 1000))}</dd>
            <dt>Lease reclaims (24 h)</dt>
            <dd className="mono">{fmtInt(q.data.lease_reclaims_24h)}</dd>
          </dl>
        ) : null}
      </Panel>
    </div>
  );
}

function StorageTab() {
  const isAdmin = useAccess().hasScope('admin');
  const u = useAsync((signal) => (isAdmin ? api.systemUsage({ signal }) : Promise.resolve(null)), [isAdmin]);
  return (
    <div className="col system-body">
      <Panel title="Storage and database usage">
        {!isAdmin ? <AdminOnly what="Storage usage" /> : null}
        {u.loading && isAdmin ? <Spinner label="Loading usage" /> : null}
        <ErrorBox error={u.error} onRetry={u.reload} />
        {u.data ? (
          <dl className="kv">
            <dt>Blob month</dt>
            <dd className="mono">{u.data.blob.month}</dd>
            <dt>Uploads</dt>
            <dd className="mono">
              {fmtInt(u.data.blob.uploads)} of {fmtInt(u.data.blob.uploads_limit)} ({fmtPct(u.data.blob.uploads / u.data.blob.uploads_limit)})
            </dd>
            <dt>Reads</dt>
            <dd className="mono">
              {fmtInt(u.data.blob.reads)} of {fmtInt(u.data.blob.reads_limit)}
            </dd>
            <dt>Bytes stored</dt>
            <dd className="mono">
              {fmtBytes(u.data.blob.bytes_stored)} of {fmtBytes(u.data.blob.bytes_limit)}
            </dd>
            <dt>Database size</dt>
            <dd className="mono">
              {fmtBytes(u.data.db.size_bytes)} of {fmtBytes(u.data.db.limit_bytes)}
            </dd>
          </dl>
        ) : null}
      </Panel>
    </div>
  );
}

function AccessTab() {
  const access = useAccess();
  const w = access.whoami;
  const isAdmin = access.hasScope('admin');
  const keys = useAsync((signal) => (isAdmin ? api.systemApiKeys({ signal }) : Promise.resolve(null)), [isAdmin]);
  const keyColumns = useMemo<Column<ApiKeyInfo>[]>(
    () => [
      { id: 'prefix', header: 'Key', rowHeader: true, cell: (k) => <span className="mono">{k.display_prefix}</span> },
      { id: 'label', header: 'Label', cell: (k) => k.label },
      { id: 'workspace', header: 'Workspace', cell: (k) => <span className="mono">{k.workspace_id}</span> },
      { id: 'scopes', header: 'Scopes', cell: (k) => k.scopes.join(', ') },
      { id: 'created', header: 'Created', cell: (k) => <span className="mono">{utc(k.created_at_ns)}</span> },
      { id: 'expires', header: 'Expires', cell: (k) => <span className="mono">{utc(k.expires_at_ns)}</span> },
      { id: 'revoked', header: 'Revoked', cell: (k) => <span className="mono">{utc(k.revoked_at_ns)}</span> },
      { id: 'used', header: 'Last used', cell: (k) => <span className="mono">{utc(k.last_used_at_ns)}</span> },
    ],
    [],
  );
  return (
    <div className="col system-body">
      <Panel title="Access">
        {access.loading ? <Spinner label="Checking access" /> : null}
        <ErrorBox error={access.error} onRetry={access.refresh} />
        {w ? (
          <dl className="kv" data-testid="whoami">
            <dt>Mode</dt>
            <dd>{w.mode === 'research' ? <Pill tone="ok">RESEARCH</Pill> : <Pill tone="neutral">DEMO read-only</Pill>}</dd>
            <dt>Workspace</dt>
            <dd>
              {w.workspace.name} <span className="mono">({w.workspace.slug}, {w.workspace.kind})</span>
            </dd>
            {w.key ? (
              <>
                <dt>Key</dt>
                <dd className="mono">
                  {w.key.display_prefix} {'\u00b7'} {w.key.label}
                </dd>
                <dt>Scopes</dt>
                <dd className="mono">{w.key.scopes.join(', ')}</dd>
                <dt>Expires</dt>
                <dd className="mono">{w.key.expires_at_ns ? utc(w.key.expires_at_ns) : 'never'}</dd>
              </>
            ) : null}
            {w.quotas ? (
              <>
                <dt>Experiments today</dt>
                <dd className="mono">
                  {fmtInt(w.quotas.experiments_per_day.used)} of {fmtInt(w.quotas.experiments_per_day.limit)}
                </dd>
                <dt>Sweeps today</dt>
                <dd className="mono">
                  {fmtInt(w.quotas.sweeps_per_day.used)} of {fmtInt(w.quotas.sweeps_per_day.limit)}
                </dd>
                <dt>Active jobs</dt>
                <dd className="mono">
                  {fmtInt(w.quotas.active_jobs.used)} of {fmtInt(w.quotas.active_jobs.limit)}
                </dd>
              </>
            ) : null}
          </dl>
        ) : null}
        <div className="row">
          {access.mode === 'research' ? (
            <button type="button" onClick={access.signOut}>
              Sign out
            </button>
          ) : (
            <button type="button" className="btn-primary" onClick={access.openKeyDialog}>
              Enter API key
            </button>
          )}
        </div>
        <p className="note">Keys are minted and revoked with the CLI (`blackbox keys mint`); this console never lists or stores key secrets.</p>
      </Panel>
      <Panel title="API keys" flush>
        {!isAdmin ? <AdminOnly what="The key list" /> : null}
        <ErrorBox error={keys.error} onRetry={keys.reload} />
        {keys.data ? <DataTable label="API keys" columns={keyColumns} rows={keys.data} rowKey={(k) => k.id} /> : null}
      </Panel>
    </div>
  );
}

function DatasetsTab() {
  const datasets = useAsync((signal) => api.datasets({ signal }), []);
  const [selected, setSelected] = useState<string | null>(null);
  const versions = useAsync((signal) => (selected ? api.datasetVersions(selected, { limit: PAGE_LIMIT }, { signal }) : Promise.resolve(null)), [selected]);
  const columns = useMemo<Column<DatasetSummary>[]>(
    () => [
      { id: 'id', header: 'Dataset', rowHeader: true, cell: (d) => <Link className="mono" to={paths.dataset(d.id)}>{d.id}</Link>, raw: (d) => d.id },
      { id: 'title', header: 'Title', cell: (d) => d.title },
      { id: 'kind', header: 'Kind', cell: (d) => d.kind },
      { id: 'owner', header: 'Owner', cell: (d) => d.owner },
      { id: 'version', header: 'Current version', numeric: true, cell: (d) => (d.current_version ? `v${d.current_version.version}` : DASH) },
      { id: 'rows', header: 'Rows', numeric: true, cell: (d) => fmtInt(d.current_version?.rows) },
      { id: 'span', header: 'Span', cell: (d) => (d.current_version ? fmtDuration(d.current_version.end_ns - d.current_version.start_ns) : DASH) },
      { id: 'quality', header: 'Quality', cell: (d) => { const q = d.current_version?.quality; return q ? (q.passed ? 'passed' : `${q.error_count} errors`) : DASH; } },
    ],
    [],
  );
  const versionColumns = useMemo<Column<DatasetVersion>[]>(
    () => [
      { id: 'v', header: 'Version', rowHeader: true, cell: (v) => `v${v.version}`, sortValue: (v) => v.version },
      { id: 'sha', header: 'Events SHA-256', cell: (v) => <span className="mono wrap-anywhere">{v.events_sha256}</span> },
      { id: 'norm', header: 'Normalization', numeric: true, cell: (v) => `v${v.normalization_version}` },
      { id: 'contract', header: 'Contract', numeric: true, cell: (v) => `v${v.contract_version}` },
      { id: 'bytes', header: 'Events', numeric: true, cell: (v) => fmtBytes(v.events_bytes) },
      { id: 'source', header: 'Source', cell: (v) => (v.source ? `${v.source.vendor} (${v.source.licence}${v.source.hostable ? '' : ', not hostable'})` : DASH) },
      { id: 'retired', header: 'Retired', cell: (v) => <span className="mono">{utc(v.retired_at_ns)}</span> },
    ],
    [],
  );
  return (
    <div className="col system-body">
      <Panel title="Datasets" flush caption="Read-only. Datasets are ingested and retired with the CLI.">
        {datasets.loading ? <Spinner label="Loading datasets" /> : null}
        <ErrorBox error={datasets.error} onRetry={datasets.reload} />
        {datasets.data ? (
          <DataTable
            label="Datasets"
            columns={columns}
            rows={datasets.data.items}
            rowKey={(d) => d.id}
            selectedKey={selected}
            onSelect={(d) => setSelected(d.id)}
            empty={<Empty>No datasets.</Empty>}
          />
        ) : null}
      </Panel>
      {selected ? (
        <Panel title={`Versions of ${selected}`} flush>
          {versions.loading ? <Spinner label="Loading versions" /> : null}
          <ErrorBox error={versions.error} onRetry={versions.reload} />
          {versions.data ? <DataTable label="Dataset versions" columns={versionColumns} rows={versions.data.items} rowKey={(v) => String(v.version)} /> : null}
        </Panel>
      ) : null}
    </div>
  );
}

function EventsTab() {
  const isAdmin = useAccess().hasScope('admin');
  const ev = useAsync((signal) => (isAdmin ? api.systemEvents({ limit: 200 }, { signal }) : Promise.resolve(null)), [isAdmin]);
  const columns = useMemo<Column<SystemEvent>[]>(
    () => [
      { id: 'at', header: 'At (UTC)', cell: (e) => <span className="mono">{utc(e.at_ns)}</span>, sortValue: (e) => BigInt(e.at_ns) },
      { id: 'source', header: 'Source', cell: (e) => e.source },
      { id: 'level', header: 'Level', cell: (e) => e.level },
      { id: 'kind', header: 'Kind', cell: (e) => <span className="mono">{e.kind}</span> },
      { id: 'message', header: 'Message', rowHeader: true, cell: (e) => e.message },
    ],
    [],
  );
  return (
    <div className="col system-body">
      <Panel title="System and audit events" flush caption="Newest first, as the server returns them.">
        {!isAdmin ? <AdminOnly what="The event log" /> : null}
        <ErrorBox error={ev.error} onRetry={ev.reload} />
        {ev.data ? <DataTable label="Events" columns={columns} rows={ev.data} rowKey={(e) => `${e.source}:${e.id}`} empty={<Empty>No events.</Empty>} /> : null}
      </Panel>
    </div>
  );
}

function AboutTab() {
  const s = useHealth().status;
  return (
    <div className="col system-body prose">
      <p>{RESEARCH_BANNER}</p>
      {s ? <p>{s.disclaimer}</p> : null}
      <p>
        BlackBox {s?.blackbox_version ?? DASH} {'\u00b7'} API contract v{s?.contract_version ?? DASH} {'\u00b7'} engine {s ? `${s.engine.name} ${s.engine.version}` : DASH}
      </p>
      <p>{ATTRIBUTION}</p>
    </div>
  );
}

export default function SystemPage() {
  const [params, setParams] = useSearchParams();
  const tab = (TABS.find((t) => t.id === params.get('tab'))?.id ?? 'health') as Tab;
  const toolbar = (
    <nav aria-label="System sections" className="route-tabs">
      {TABS.map((t) => (
        <button
          key={t.id}
          type="button"
          className={t.id === tab ? 'tab active' : 'tab'}
          aria-current={t.id === tab ? 'page' : undefined}
          onClick={() => setParams(t.id === 'health' ? {} : { tab: t.id })}
        >
          {t.label}
        </button>
      ))}
    </nav>
  );
  return (
    <Workspace context={tab} toolbar={toolbar} scroll>
      {tab === 'health' ? <HealthTab /> : null}
      {tab === 'queue' ? <QueueTab /> : null}
      {tab === 'storage' ? <StorageTab /> : null}
      {tab === 'access' ? <AccessTab /> : null}
      {tab === 'datasets' ? <DatasetsTab /> : null}
      {tab === 'events' ? <EventsTab /> : null}
      {tab === 'about' ? <AboutTab /> : null}
    </Workspace>
  );
}
