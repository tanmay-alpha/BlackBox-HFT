// Status bar (UI_DESIGN_SPEC.md §5.5): API state and round trip, build, mode, the research banner
// (verbatim, never hidden) and a UTC clock. Changes of API state are announced once, through the
// shell's polite live region. There is no worker segment: the legacy API runs simulations
// synchronously and reports no worker; the segment arrives with the v1 job API.

import { useEffect, useRef, useState } from 'react';

import { isStaticViewer } from '../api/client';
import { STATIC_VIEWER_BANNER } from '../components/staticViewer';
import { Badge } from '../components/ui';
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

function useNow(): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(t);
  }, []);
  return now;
}

export function StatusBar({ announce }: { announce: (message: string) => void }) {
  const h = useHealth();
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
        : 'GET /api/health, polled every 30 s while this tab is visible. Activate to check now.';
  const build = h.health
    ? `v${h.health.blackbox_version} \u00b7 contract v${h.health.contract_version} \u00b7 ${h.health.engine.name} ${h.health.engine.version}`
    : null;
  const utc = `${pad2(now.getUTCHours())}:${pad2(now.getUTCMinutes())}:${pad2(now.getUTCSeconds())} UTC`;
  const staticViewer = isStaticViewer();

  return (
    <footer className="statusbar" aria-label="Status">
      <button type="button" className={`sb-seg sb-api sb-${tone}`} title={apiTitle} onClick={h.retry}>
        <span className="sb-dot" aria-hidden="true" />
        API {h.phase === 'unreachable' ? 'ERROR ' : ''}
        {PHASE_TEXT[h.phase]}
        {h.phase === 'ok' && h.rttMs !== null ? ` ${h.rttMs} ms` : null}
        {h.phase === 'waking' ? ` ${elapsed} s` : null}
      </button>
      {h.contractMismatch ? (
        <span className="sb-seg sb-warn" tabIndex={0} title="This console was built for another API contract version. Fields may be missing or misread.">
          WARN contract mismatch
        </span>
      ) : null}
      {build ? (
        <button
          type="button"
          className="sb-seg"
          title="Build: BlackBox version, API contract, simulation engine. Activate to copy."
          onClick={() => void navigator.clipboard?.writeText(build).then(() => setCopied(true))}
        >
          {copied ? 'copied' : build}
        </button>
      ) : null}
      <span
        className="sb-seg"
        tabIndex={0}
        title={staticViewer ? STATIC_VIEWER_BANNER : 'Local server: no authentication; every action is available.'}
      >
        {staticViewer ? 'PRECOMPUTED BUNDLE' : 'LOCAL'}
      </span>
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
