// API connection state (UI_DESIGN_SPEC.md §10.1) from `GET /api/v1/status` (memory-only on the
// server, safe to poll every 30 s; API_CONTRACT.md §8.1, §12).
//
//   connecting --200--> ok;  connecting/ok --(>3 s, 502/503/504, or a non-JSON body)--> waking
//   waking --200--> ok;  waking for >120 s --> unreachable;  3 failures in a row --> unreachable
//   unreachable --200--> ok (retry button or backoff)
//
// A cross-origin API that is asleep answers with the host's loading page, which carries no CORS
// headers: the browser reports a network error, which also counts as waking until 120 s.
// Status is polled every 30 s while the tab is visible and paused while it is hidden. While
// waking or failing it retries after 2, 4, 8, 15, 15... s. Nothing pings the API just to keep it
// warm. Data already on screen is never cleared by a connection change.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';

import { ApiError, api } from '../api/client';
import { isCrossOrigin } from '../api/http';
import type { Status } from '../api/model';
import { CONTRACT_VERSION } from '../api/types';

export type ApiPhase = 'connecting' | 'ok' | 'waking' | 'unreachable';

export interface HealthState {
  phase: ApiPhase;
  /** Last successful response; kept through later failures. */
  status: Status | null;
  /** Round trip of the last successful request, ms. */
  rttMs: number | null;
  /** When the last successful response arrived (ms since epoch). */
  checkedAt: number | null;
  /** Verbatim message of the last failure. */
  lastError: string | null;
  /** When the current phase began (ms since epoch), for "waking 37 s". */
  since: number;
  /** Status reports a different contract version than this console was built for. */
  contractMismatch: boolean;
}

interface HealthApi extends HealthState {
  retry: () => void;
}

const POLL_OK_MS = 30_000;
const SLOW_MS = 3_000;
const WAKING_LIMIT_MS = 120_000;
const BACKOFF_S = [2, 4, 8, 15];

const HealthContext = createContext<HealthApi | null>(null);

export function isWakingError(e: unknown): boolean {
  return e instanceof ApiError && (e.code === 'waking' || (e.code === 'network' && isCrossOrigin()));
}

export function HealthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<HealthState>({
    phase: 'connecting',
    status: null,
    rttMs: null,
    checkedAt: null,
    lastError: null,
    since: Date.now(),
    contractMismatch: false,
  });
  const failures = useRef(0);
  const attempt = useRef(0);
  const timer = useRef<number | undefined>(undefined);
  const inFlight = useRef(false);
  const stateRef = useRef(state);
  stateRef.current = state;

  const setPhase = (phase: ApiPhase, patch: Partial<HealthState>) =>
    setState((s) => ({ ...s, ...patch, phase, since: s.phase === phase ? s.since : Date.now() }));

  const poll = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    window.clearTimeout(timer.current);
    const start = performance.now();
    const slow = window.setTimeout(() => {
      if (stateRef.current.phase === 'connecting') setPhase('waking', {});
    }, SLOW_MS);
    let next: number;
    try {
      const status = await api.status();
      failures.current = 0;
      attempt.current = 0;
      setPhase('ok', {
        status,
        rttMs: Math.round(performance.now() - start),
        checkedAt: Date.now(),
        lastError: null,
        contractMismatch: status.contract_version !== CONTRACT_VERSION,
      });
      next = POLL_OK_MS;
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      failures.current += 1;
      const s = stateRef.current;
      const wakingTooLong = s.phase === 'waking' && Date.now() - s.since > WAKING_LIMIT_MS;
      if (isWakingError(e) && !wakingTooLong && s.phase !== 'unreachable') setPhase('waking', { lastError: message });
      else if (failures.current >= 3 || wakingTooLong) setPhase('unreachable', { lastError: message });
      else setPhase(s.phase === 'ok' ? 'connecting' : s.phase, { lastError: message });
      next = BACKOFF_S[Math.min(attempt.current, BACKOFF_S.length - 1)] * 1000;
      attempt.current += 1;
    } finally {
      window.clearTimeout(slow);
      inFlight.current = false;
    }
    if (document.visibilityState === 'visible') timer.current = window.setTimeout(() => void poll(), next);
  }, []);

  useEffect(() => {
    void poll();
    const onVisible = () => {
      if (document.visibilityState === 'visible') void poll();
      else window.clearTimeout(timer.current);
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      document.removeEventListener('visibilitychange', onVisible);
      window.clearTimeout(timer.current);
    };
  }, [poll]);

  const retry = useCallback(() => {
    attempt.current = 0;
    void poll();
  }, [poll]);

  const value = useMemo(() => ({ ...state, retry }), [state, retry]);
  return <HealthContext.Provider value={value}>{children}</HealthContext.Provider>;
}

export function useHealth(): HealthApi {
  const h = useContext(HealthContext);
  if (!h) throw new Error('useHealth outside HealthProvider');
  return h;
}

/** The connection phase, or `null` outside the shell (tests, isolated components). */
export function useApiPhase(): ApiPhase | null {
  return useContext(HealthContext)?.phase ?? null;
}
