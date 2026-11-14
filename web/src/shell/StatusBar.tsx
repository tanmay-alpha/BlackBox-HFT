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
  /** Heartbeat age now, seconds, extrapolated from the status response's age and when it arrived. */
  heartbeatAgeS: number | null;
  /** Heartbeat older than 3x the declared interval (§10.4). */
  stale: boolean;
  queueDepth: number;
  running: number;
}

/** The worker facts of a status response, aged to `nowMs`. Nothing is inferred beyond them. */
export function workerHealth(status: Status, checkedAt: number | null, nowMs: number): WorkerHealth {
  const w = status.worker;
  const since = checkedAt === null ? 0 : Math.max(0, nowMs - checkedAt);
  const ageMs = w.last_heartbeat_age_ms === null ? null : w.last_heartbeat_age_ms + since;
  return {
    text: `worker ${w.state} ${w.busy}/${w.slots}`,
    heartbeatAgeS: ageMs === null ? null : Math.floor(ageMs / 1000),
    stale: ageMs !== null && ageMs > 3 * w.heartbeat_interval_ms,
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
          className={`sb-seg${worker.stale ? ' sb-warn' : ''}`}
          tabIndex={0}
          data-testid="sb-worker"
          title={`Worker and queue as reported by GET /api/v1/status (heartbeat every ${Math.round((s?.worker.heartbeat_interval_ms ?? 0) / 1000)} s).`}
        >
          {worker.text} {'\u00b7'} queued {worker.queueDepth} {'\u00b7'} running {worker.running}
          {worker.heartbeatAgeS !== null ? ` \u00b7 hb ${worker.heartbeatAgeS} s` : ' \u00b7 no heartbeat'}
          {worker.stale ? ' STALE' : null}
        </span>
      ) : null}
      {build ? (
        <button
          type="button"
          className="sb-seg"
          title="Build: BlackBox version, git commit, API contract, simulation engine. Activate to copy."
          onClick={() => void navigator.clipboard?.writeText(build).then(() => setCopied(true))}
        >
          {copied ? 'copied' : build}
        </button>
      ) : null}
      <button
        type="button"
        className="sb-seg"
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
