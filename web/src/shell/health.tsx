// API connection state (UI_DESIGN_SPEC.md §10.1) from the existing `GET /api/health`.
//
//   connecting --200--> ok;  connecting/ok --(>3 s, 502/503/504, or a non-JSON body)--> waking
//   waking --200--> ok;  waking for >120 s --> unreachable;  3 failures in a row --> unreachable
//   unreachable --200--> ok (retry button or backoff)
//
// Health is polled every 30 s while the tab is visible and paused while it is hidden. While
// waking or failing it retries after 2, 4, 8, 15, 15... s. Nothing pings the API just to keep it
// warm. Data already on screen is never cleared by a connection change.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';

import { ApiError, api } from '../api/client';
import { CONTRACT_VERSION, type Health } from '../api/types';

export type ApiPhase = 'connecting' | 'ok' | 'waking' | 'unreachable';

export interface HealthState {
  phase: ApiPhase;
  /** Last successful response; kept through later failures. */
  health: Health | null;
  /** Round trip of the last successful request, ms. */
  rttMs: number | null;
  /** Verbatim message of the last failure. */
  lastError: string | null;
  /** When the current phase began (ms since epoch), for "waking 37 s". */
  since: number;
  /** Health reports a different contract version than this console was built for. */
  contractMismatch: boolean;
}

interface HealthApi extends HealthState {
  retry: () => void;
}

const POLL_OK_MS = 30_000;
const SLOW_MS = 3_000;
const WAKING_LIMIT_MS = 120_000;
const BACKOFF_S = [2, 4, 8, 15];
const WAKING_STATUS = new Set([502, 503, 504]);

const HealthContext = createContext<HealthApi | null>(null);

function isWakingError(e: unknown): boolean {
  // The free host answers with its own HTML loading page while the service spins up.
  return e instanceof ApiError && e.code === 'bad_response' && (WAKING_STATUS.has(e.status) || e.status === 200);
}

export function HealthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<HealthState>({
    phase: 'connecting',
    health: null,
    rttMs: null,
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
      const health = await api.health();
      failures.current = 0;
      attempt.current = 0;
      setPhase('ok', {
        health,
        rttMs: Math.round(performance.now() - start),
        lastError: null,
        contractMismatch: health.contract_version !== CONTRACT_VERSION,
      });
      next = POLL_OK_MS;
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      failures.current += 1;
      const s = stateRef.current;
      const wakingTooLong = s.phase === 'waking' && Date.now() - s.since > WAKING_LIMIT_MS;
      if (isWakingError(e) && !wakingTooLong) setPhase('waking', { lastError: message });
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
