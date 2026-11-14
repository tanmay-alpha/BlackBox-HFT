// Research Reports (UI_DESIGN_SPEC.md §11.9). `/reports` lists saved report snapshots and the
// runs whose deterministic report can be generated; `/reports/:runId` renders a run's report
// (`GET /runs/{id}/report`, text/markdown) next to its reproducibility manifest; and
// `/reports/saved/:reportId` renders an immutable saved snapshot whose body is checked against
// its recorded SHA-256 in the browser. Markdown is rendered without raw HTML.

import { useMemo, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';

import { api, saveDownload, saveText } from '../../api/client';
import type { Report, Run, RunDetail } from '../../api/model';
import { DataTable, type Column } from '../../components/DataTable';
import { RouteTabs } from '../../components/Tabs';
import { Markdown, MarkdownBlocks, sectionOf } from '../../components/Markdown';
import { DASH, fmtBytes, fmtDuration, fmtNum, fmtPnl } from '../../components/fmt';
import { fmtNsUtc } from '../../components/ns';
import { Empty, ErrorBox, HashText, Notice, Panel, Pill, Spinner, useAsync } from '../../components/ui';
import { GatedButton } from '../../shell/access';
import { useAnnounce } from '../../shell/announce';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import './reports.css';

function ReportsTabs() {
  return <RouteTabs label="Research Reports" items={[{ to: paths.reports(), label: 'Reports', end: true }]} />;
}

async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

// ----- index -----------------------------------------------------------------------------------

function ReportsIndex() {
  const navigate = useNavigate();
  const saved = useAsync((signal) => api.reportsPage({ limit: 100 }, { signal }), []);
  const runs = useAsync((signal) => api.runs({}, { signal }), []);

  const savedColumns = useMemo<Column<Report>[]>(
    () => [
      { id: 'title', header: 'Title', rowHeader: true, cell: (r) => <Link to={paths.savedReport(r.id)}>{r.title}</Link>, raw: (r) => r.title },
      { id: 'kind', header: 'Kind', cell: (r) => r.kind },
      { id: 'subject', header: 'Subject', cell: (r) => <span className="mono">{r.subject_id}</span> },
      { id: 'created', header: 'Created (UTC)', cell: (r) => <span className="mono">{fmtNsUtc(r.created_at_ns, 0)}</span>, sortValue: (r) => BigInt(r.created_at_ns) },
      { id: 'size', header: 'Size', numeric: true, cell: (r) => fmtBytes(r.size_bytes), sortValue: (r) => r.size_bytes },
      { id: 'sha', header: 'Body SHA-256', cell: (r) => <HashText hash={r.body_sha256} /> },
    ],
    [],
  );
  const runColumns = useMemo<Column<Run>[]>(
    () => [
      { id: 'run', header: 'Run', rowHeader: true, cell: (r) => <Link className="mono" to={paths.report(r.run_id)}>{r.run_id}</Link>, raw: (r) => r.run_id },
      { id: 'label', header: 'Label', cell: (r) => r.label || <span className="dim">unlabelled</span>, sortValue: (r) => r.label ?? '' },
      { id: 'strategy', header: 'Strategy', cell: (r) => <span className="mono">{r.strategy_kind} every {fmtDuration(r.interval_ns)}</span> },
      { id: 'dataset', header: 'Dataset', cell: (r) => <span className="mono">{r.dataset.id} v{r.dataset.version}</span> },
      { id: 'created', header: 'Created (UTC)', cell: (r) => <span className="mono">{fmtNsUtc(r.created_at_ns, 0)}</span>, sortValue: (r) => BigInt(r.created_at_ns) },
      { id: 'net', header: 'Net P&L', unit: 'quote', numeric: true, cell: (r) => fmtPnl(r.metrics.net_pnl), sortValue: (r) => r.metrics.net_pnl },
    ],
    [],
  );

  return (
    <Workspace toolbar={<ReportsTabs />} layout>
      <div className="col reports-index">
        <Panel
          title="Saved reports"
          flush
          caption={saved.data?.next_cursor ? 'First 100 saved reports shown.' : 'Immutable snapshots; each body is stored with its SHA-256.'}
        >
          {saved.loading ? <Spinner label="Loading saved reports" /> : null}
          <ErrorBox error={saved.error} onRetry={saved.reload} />
          {saved.data ? (
            <DataTable
              label="Saved reports"
              columns={savedColumns}
              rows={saved.data.items}
              rowKey={(r) => r.id}
              onActivate={(r) => navigate(paths.savedReport(r.id))}
              empty={<Empty>No saved reports in this workspace. Open a run report and use "Save snapshot".</Empty>}
            />
          ) : null}
        </Panel>
        <Panel title="Run reports" flush caption={runs.data?.incomplete ? `List incomplete: first ${runs.data.items.length} runs shown.` : 'Generated on demand; deterministic for a run.'}>
          {runs.loading ? <Spinner label="Loading runs" /> : null}
          <ErrorBox error={runs.error} onRetry={runs.reload} />
          {runs.data ? (
            <DataTable
              label="Runs"
              columns={runColumns}
              rows={runs.data.items}
              rowKey={(r) => r.run_id}
              onActivate={(r) => navigate(paths.report(r.run_id))}
              empty={<Empty>No completed runs are visible in this workspace.</Empty>}
            />
          ) : null}
        </Panel>
      </div>
    </Workspace>
  );
}

// ----- run report ------------------------------------------------------------------------------

function ManifestPanel({ run }: { run: RunDetail }) {
  return (
    <Panel title="Reproducibility manifest" basis="observed">
      <dl className="kv" data-testid="report-manifest">
        <dt>Run</dt>
        <dd className="mono">{run.run_id}</dd>
        <dt>Config fingerprint</dt>
        <dd className="mono wrap-anywhere">{run.fingerprint}</dd>
        <dt>Dataset</dt>
        <dd className="mono">
          {run.dataset.id} version {run.dataset.version}
        </dd>
        <dt>Dataset content hash</dt>
        <dd className="mono wrap-anywhere">{run.dataset.events_sha256}</dd>
        <dt>Dataset origin</dt>
        <dd className="mono">{fmtNsUtc(run.dataset.origin_ns, 3)}</dd>
        <dt>Platform class</dt>
        <dd>
          <span className="mono">{run.platform}</span>{' '}
          {run.canonical ? <Pill tone="ok">canonical</Pill> : <Pill tone="warn" title="Computed on a platform other than the deployment's canonical result platform.">non-canonical</Pill>}
        </dd>
        <dt>Result hash</dt>
        <dd className="mono wrap-anywhere">{run.result_sha256}</dd>
        <dt>Summary hash</dt>
        <dd className="mono wrap-anywhere">{run.summary_sha256}</dd>
        <dt>Bundle</dt>
        <dd className="mono wrap-anywhere">
          format v{run.bundle.format_version} {'\u00b7'} {fmtBytes(run.bundle.size_bytes)} {'\u00b7'} {run.bundle.sha256}
        </dd>
        <dt>Engine</dt>
        <dd className="mono">
          {run.engine.name} {run.engine.version}
        </dd>
        <dt>BlackBox</dt>
        <dd className="mono">
          {run.blackbox_version} ({run.build_profile}) {'\u00b7'} contract v{run.contract_version} {'\u00b7'} simulation revision {run.simulation_revision}
        </dd>
        <dt>Seed</dt>
        <dd className="mono">{run.config.seed}</dd>
        <dt>Wall time</dt>
        <dd className="mono">{fmtNum(run.wall_time_ms, 0)} ms (informational; not hashed)</dd>
        <dt>Created</dt>
        <dd className="mono">{fmtNsUtc(run.created_at_ns, 3)}</dd>
      </dl>
    </Panel>
  );
}

function RunReport({ runId }: { runId: string }) {
  const announce = useAnnounce();
  const navigate = useNavigate();
  const run = useAsync((signal) => api.run(runId, { signal }), [runId]);
  const report = useAsync((signal) => api.runReport(runId, { signal }), [runId]);
  const [actionError, setActionError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const limitations = useMemo(() => (report.data ? sectionOf(report.data, /limitation/i) : null), [report.data]);

  const act = async (fn: () => Promise<void>) => {
    setBusy(true);
    setActionError(null);
    try {
      await fn();
    } catch (e) {
      setActionError(e);
    } finally {
      setBusy(false);
    }
  };

  const toolbar = (
    <>
      <ReportsTabs />
      <button type="button" disabled={!report.data} onClick={() => report.data && saveText(report.data, `run-${runId}-report.md`)} data-testid="download-report">
        Download .md
      </button>
      <button
        type="button"
        disabled={busy}
        onClick={() => void act(async () => saveDownload(await api.runExport(runId)))}
        title="The verified run bundle (gzip NDJSON, format v1); `blackbox verify-bundle` checks it offline."
        data-testid="download-bundle"
      >
        Download bundle (.bbz)
      </button>
      <GatedButton
        blockedReason={busy ? 'Working\u2026' : null}
        data-testid="save-report"
        onClick={() =>
          void act(async () => {
            const title = `Run ${runId}${run.data?.label ? `: ${run.data.label}` : ''}`;
            const saved = await api.createReport('run', runId, title);
            announce(`Report snapshot saved (${saved.id}).`);
            navigate(paths.savedReport(saved.id));
          })
        }
      >
        Save snapshot
      </GatedButton>
      <Link to={paths.run(runId)}>Forensics</Link>
      <Link to={paths.riskRun(runId)}>Risk</Link>
    </>
  );

  return (
    <Workspace context={`report ${runId}`} toolbar={toolbar} scroll>
      <div className="reports-run">
        <div className="col reports-main">
          <ErrorBox error={actionError} />
          {report.loading ? <Spinner label="Generating report" /> : null}
          <ErrorBox error={report.error} onRetry={report.reload} />
          {report.data ? (
            <article className="report-doc" aria-label={`Research report of run ${runId}`}>
              <Markdown source={report.data} />
            </article>
          ) : null}
        </div>
        <aside className="col reports-side">
          {run.loading ? <Spinner label="Loading run" /> : null}
          <ErrorBox error={run.error} onRetry={run.reload} />
          {run.data ? <ManifestPanel run={run.data} /> : null}
          <Panel title="Limitations" caption="Verbatim from the report.">
            {limitations && limitations.length > 0 ? <MarkdownBlocks blocks={limitations} /> : report.data ? <Empty>The report has no limitations section.</Empty> : null}
          </Panel>
        </aside>
      </div>
    </Workspace>
  );
}

// ----- saved report ----------------------------------------------------------------------------

function SavedReport({ reportId }: { reportId: string }) {
  const meta = useAsync((signal) => api.report(reportId, { signal }), [reportId]);
  const body = useAsync(async (signal) => {
    const text = await api.reportContent(reportId, { signal });
    return { text, sha: await sha256Hex(text) };
  }, [reportId]);
  const m = meta.data;
  const verified = m && body.data ? body.data.sha === m.body_sha256 : null;

  const toolbar = (
    <>
      <ReportsTabs />
      <button type="button" disabled={!body.data} onClick={() => body.data && saveText(body.data.text, `report-${reportId}.md`)}>
        Download .md
      </button>
      {m?.kind === 'run' ? <Link to={paths.report(m.subject_id)}>Live run report</Link> : null}
    </>
  );

  return (
    <Workspace context={`saved report ${reportId.slice(0, 12)}`} toolbar={toolbar} scroll>
      <div className="reports-run">
        <div className="col reports-main">
          {verified === false ? (
            <Notice tag="INTEGRITY" tone="error">
              The body's SHA-256 ({body.data?.sha}) does not match the recorded {m?.body_sha256}. The content is shown but must not be trusted.
            </Notice>
          ) : null}
          {body.loading ? <Spinner label="Loading report" /> : null}
          <ErrorBox error={body.error} onRetry={body.reload} />
          {body.data ? (
            <article className="report-doc" aria-label="Saved research report">
              <Markdown source={body.data.text} />
            </article>
          ) : null}
        </div>
        <aside className="col reports-side">
          <ErrorBox error={meta.error} onRetry={meta.reload} />
          {m ? (
            <Panel title="Snapshot">
              <dl className="kv">
                <dt>Title</dt>
                <dd>{m.title}</dd>
                <dt>Kind</dt>
                <dd>{m.kind}</dd>
                <dt>Subject</dt>
                <dd className="mono">{m.subject_id}</dd>
                <dt>Generator</dt>
                <dd className="mono">v{m.generator_version} ({m.format})</dd>
                <dt>Created</dt>
                <dd className="mono">{fmtNsUtc(m.created_at_ns, 3)}</dd>
                <dt>Body SHA-256</dt>
                <dd className="mono wrap-anywhere">{m.body_sha256}</dd>
                <dt>Checked here</dt>
                <dd>{verified === null ? DASH : verified ? <Pill tone="ok">matches</Pill> : <Pill tone="error">mismatch</Pill>}</dd>
              </dl>
            </Panel>
          ) : null}
        </aside>
      </div>
    </Workspace>
  );
}

export default function ReportsPage() {
  const { runId, reportId } = useParams();
  if (reportId) return <SavedReport reportId={reportId} />;
  if (runId) return <RunReport runId={runId} />;
  return <ReportsIndex />;
}
