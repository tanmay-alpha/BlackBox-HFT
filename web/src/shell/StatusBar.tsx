// Status bar (UI_DESIGN_SPEC.md §5.5): API state and round trip, worker health and queue depth
// (from `GET /api/v1/status`), build, access mode, the research banner (verbatim, never hidden)
// and a UTC clock. Changes of API state are announced once, through the shell's polite live
// region.

import { useEffect, useRef, useState } from 'react';

import type { Status } from '../api/model';
import { Badge } from '../components/ui';
import { useAccess } from './access';
import { useHealth, type ApiPhase } from './health';

/** Shown on every view. The wording is fixed by the product's honesty rules. */
export const RESEARCH_BANNER =
  'Simulated on historical replay \u2014 research only. Simulated orders do not affect historical prices.';

const PHASE_TEXT: Record<ApiPhase, string> = {
  connecting: 'connecting',
  ok: 'ok',
  waking: 'waking',
  unreachable: 'unreachable',
};

const pad2 = (n: number) => String(n).padStart(2, '0');

export function useNow(intervalMs = 1000): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = window.setInterval(() => setNow(new Date()), intervalMs);
    return () => window.clearInterval(t);
  }, [intervalMs]);
  return now;
}

export interface WorkerHealth {
  /** `worker busy 1/1`. */
  text: string;
  /** Plain-language state: `Idle — waiting for jobs (in-process worker)`, `Busy …`, `Not responding …`. */
  label: string;
  tone: 'ok' | 'info' | 'warn' | 'error' | 'neutral';
  /** Heartbeat age as reported at the last check, seconds; aged to now only once the worker is not responding. */
  heartbeatAgeS: number | null;
  /** Heartbeat older than 3x the declared interval (§10.4). */
  stale: boolean;
  queueDepth: number;
  running: number;
}

export const WORKER_MODE: Record<string, string> = { in_process: 'in-process worker', standalone: 'standalone worker' };

/** The worker facts of a status response, aged to `nowMs`. Nothing is inferred beyond them. */
export function workerHealth(status: Status, checkedAt: number | null, nowMs: number): WorkerHealth {
  const w = status.worker;
  const since = checkedAt === null ? 0 : Math.max(0, nowMs - checkedAt);
  const reported = w.last_heartbeat_age_ms;
  // Judged on the age the API reported: an idle worker keeps beating between our 30 s polls, so
  // extrapolating would flag (and age) a healthy worker. Only a missing heartbeat keeps aging.
  const stale = w.state !== 'stopped' && reported !== null && reported > 3 * w.heartbeat_interval_ms;
  const ageMs = reported === null ? null : stale ? reported + since : reported;
  const heartbeatAgeS = ageMs === null ? null : Math.floor(ageMs / 1000);
  const mode = WORKER_MODE[w.mode] ?? `${w.mode} worker`;
  let label: string;
  let tone: WorkerHealth['tone'];
  if (stale) {
    label = `Not responding (no heartbeat for ${heartbeatAgeS} s)`;
    tone = 'warn';
  } else if (w.state === 'idle') {
    label = `Idle \u2014 waiting for jobs (${mode})`;
    tone = 'ok';
  } else if (w.state === 'busy') {
    label = `Busy \u2014 ${w.busy} of ${w.slots} job slots in use (${mode})`;
    tone = 'info';
  } else if (w.state === 'starting') {
    label = `Starting (${mode})`;
    tone = 'neutral';
  } else if (w.state === 'draining') {
    label = `Draining \u2014 finishing current jobs, taking no new ones (${mode})`;
    tone = 'warn';
  } else {
    label = `Stopped (${mode})`;
    tone = 'error';
  }
  return {
    text: `worker ${w.state} ${w.busy}/${w.slots}`,
    label,
    tone,
    heartbeatAgeS,
    stale,
    queueDepth: status.queue.depth,
    running: status.queue.running,
  };
}

export function StatusBar({ announce }: { announce: (message: string) => void }) {
  const h = useHealth();
  const access = useAccess();
  const now = useNow();
  const [copied, setCopied] = useState(false);
  const lastPhase = useRef<ApiPhase | null>(null);
  const lastMismatch = useRef(false);

  useEffect(() => {
    if (lastPhase.current !== null && lastPhase.current !== h.phase) {
      announce(`API ${PHASE_TEXT[h.phase]}${h.phase === 'unreachable' && h.lastError ? `: ${h.lastError}` : ''}`);
    }
    lastPhase.current = h.phase;
  }, [h.phase, h.lastError, announce]);

  useEffect(() => {
    if (h.contractMismatch && !lastMismatch.current) announce('API contract mismatch');
    lastMismatch.current = h.contractMismatch;
  }, [h.contractMismatch, announce]);

  const elapsed = Math.max(0, Math.floor((now.getTime() - h.since) / 1000));
  const tone = h.phase === 'ok' ? 'ok' : h.phase === 'unreachable' ? 'error' : h.phase === 'waking' ? 'warn' : 'neutral';
  const apiTitle =
    h.phase === 'unreachable'
      ? `ERROR: ${h.lastError ?? 'no response'}. Activate to retry now.`
      : h.phase === 'waking'
        ? 'The API is waking up (a free host can take about a minute to start). Activate to retry now.'
        : 'GET /api/v1/status, polled every 30 s while this tab is visible. Activate to check now.';
  const s = h.status;
  const build = s
    ? `v${s.blackbox_version} \u00b7 ${s.git_sha?.slice(0, 7) ?? 'no sha'} \u00b7 contract v${s.contract_version} \u00b7 ${s.engine.name} ${s.engine.version}`
    : null;
  const worker = s ? workerHealth(s, h.checkedAt, now.getTime()) : null;
  const utc = `${pad2(now.getUTCHours())}:${pad2(now.getUTCMinutes())}:${pad2(now.getUTCSeconds())} UTC`;
  const w = access.whoami;
  const mode =
    access.mode === 'research' && w
      ? `RESEARCH \u00b7 ${w.key?.display_prefix ?? ''} \u00b7 ${w.workspace.slug}`
      : access.mode === 'demo'
        ? 'DEMO read-only'
        : access.loading
          ? 'ACCESS checking'
          : 'ACCESS unknown';

  return (
    <footer className="statusbar" aria-label="Status">
      <button type="button" className={`sb-seg sb-api sb-${tone}`} title={apiTitle} onClick={h.retry}>
        <span className="sb-dot" aria-hidden="true" />
        API {h.phase === 'unreachable' ? 'ERROR ' : ''}
        {PHASE_TEXT[h.phase]}
        {h.phase === 'ok' && h.rttMs !== null ? ` ${h.rttMs} ms` : null}
        {h.phase === 'waking' ? ` ${elapsed} s` : null}
        {s?.status === 'draining' ? ' \u00b7 draining' : null}
      </button>
      {h.contractMismatch ? (
        <span className="sb-seg sb-warn" tabIndex={0} title="This console was built for another API contract version. Fields may be missing or misread.">
          WARN contract mismatch
        </span>
      ) : null}
      {worker ? (
        <span
          className={`sb-seg sb-worker sb-${worker.tone === 'info' ? 'neutral' : worker.tone}`}
          tabIndex={0}
          data-testid="sb-worker"
          title={`Worker: ${worker.label}; queued ${worker.queueDepth}; running ${worker.running}. Reported by GET /api/v1/status: ${worker.text}, heartbeat ${worker.heartbeatAgeS === null ? 'not reported yet' : `${worker.heartbeatAgeS} s ago`} (expected every ${Math.round((s?.worker.heartbeat_interval_ms ?? 0) / 1000)} s; flagged as not responding after 3 missed).`}
        >
          <span className="sb-dot" aria-hidden="true" />
          <span className="sb-worker-text">Worker: {worker.label} {'\u00b7'} queued {worker.queueDepth} {'\u00b7'} running {worker.running}</span>
        </span>
      ) : null}
      {build ? (
        <button
          type="button"
          className="sb-seg sb-build"
          title={`Build: ${build}. Activate to copy.`}
          onClick={() => void navigator.clipboard?.writeText(build).then(() => setCopied(true))}
        >
          <span className="sb-build-text">{copied ? 'copied' : build}</span>
        </button>
      ) : null}
      <button
        type="button"
        className="sb-seg sb-mode"
        data-testid="sb-mode"
        title={access.mode === 'research' ? 'Research access. Activate to change the key.' : 'Public demo workspace, read-only. Activate to enter an API key.'}
        onClick={access.openKeyDialog}
      >
        {mode}
      </button>
      <span className="sb-seg sb-banner" tabIndex={0} title={RESEARCH_BANNER} role="note">
        <Badge kind="modeled">simulated</Badge>
        <span className="sb-banner-text">{RESEARCH_BANNER}</span>
      </span>
      <span className="sb-seg sb-clock" tabIndex={0} title={`Local time ${now.toString()}`}>
        {utc}
      </span>
    </footer>
  );
}
