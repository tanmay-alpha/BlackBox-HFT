// Job states (UI_DESIGN_SPEC.md §10.4, API_CONTRACT.md §8.5, §9.3, TARGET_ARCHITECTURE §4.1).
//
//   created ──► queued ──► running ──► completed | failed
//   created/queued/running ──► cancelling ──► cancelled;  created/queued ──► cancelled
//   created/queued/running ──► expired;  running ──► queued (re-queued after a drain or lost lease)
//
// Updates are applied only forward: a job resource carries a monotonically increasing `version`,
// and an update whose version is lower, or whose transition is not an edge above, is ignored and
// logged once. Polling: every 2 s while any shown job is live, 5 s when the API is waking or the
// tab is hidden. No client-side ETA and no extrapolated progress.

import { useEffect, useRef, useState } from 'react';

import { api, isAbort } from '../../api/client';
import { TERMINAL_JOB_STATES, type Job, type JobState } from '../../api/model';
import { isWakingError, useApiPhase } from '../../shell/health';

const EDGES: Record<JobState, readonly JobState[]> = {
  created: ['queued', 'running', 'cancelling', 'cancelled', 'expired', 'failed'],
  queued: ['running', 'cancelling', 'cancelled', 'expired', 'failed'],
  running: ['completed', 'failed', 'cancelling', 'expired', 'queued'],
  cancelling: ['cancelled', 'completed', 'failed', 'expired'],
  completed: [],
  failed: [],
  cancelled: [],
  expired: [],
};

/** Whether `next` may replace `prev` (same job). */
export function acceptUpdate(prev: Job, next: Job): boolean {
  if (next.version < prev.version) return false;
  if (next.state === prev.state) return true;
  return EDGES[prev.state].includes(next.state);
}

const logged = new Set<string>();

/** Merges a fresh list into the known jobs, keeping known jobs that the fresh list left out. */
export function mergeJobs(known: ReadonlyMap<string, Job>, fresh: readonly Job[]): Map<string, Job> {
  const out = new Map(known);
  for (const j of fresh) {
    const prev = out.get(j.id);
    if (!prev || acceptUpdate(prev, j)) {
      out.set(j.id, j);
    } else {
      const key = `${j.id}:${prev.state}->${j.state}@${j.version}`;
      if (!logged.has(key)) {
        logged.add(key);
        console.warn(`Ignored out-of-order job update ${j.id}: ${prev.state} (v${prev.version}) -> ${j.state} (v${j.version})`);
      }
    }
  }
  return out;
}

/** Sort key: running → cancelling → queued → created → terminal, then newest first. */
const GROUP: Record<JobState, number> = {
  running: 0,
  cancelling: 1,
  queued: 2,
  created: 3,
  completed: 4,
  failed: 4,
  cancelled: 4,
  expired: 4,
};

export function compareJobs(a: Job, b: Job): number {
  const g = GROUP[a.state] - GROUP[b.state];
  if (g !== 0) return g;
  if (a.state === 'queued' && b.state === 'queued' && a.queue_position !== null && b.queue_position !== null) {
    return a.queue_position - b.queue_position;
  }
  return a.created_at_ns === b.created_at_ns ? 0 : BigInt(a.created_at_ns) > BigInt(b.created_at_ns) ? -1 : 1;
}

export const isTerminal = (s: JobState) => TERMINAL_JOB_STATES[s];

export function pollInterval(phase: string | null): number {
  return phase === 'waking' || (typeof document !== 'undefined' && document.visibilityState === 'hidden') ? 5000 : 2000;
}

export interface PolledJobs {
  jobs: Job[];
  error: unknown;
  loading: boolean;
  /** Last successful response, ms since epoch. Data stays on screen (STALE) after failures. */
  updatedAt: number | null;
  reload: () => void;
}

/**
 * The job list for `filterKey` (a stable string for the query), re-polled while any listed job is
 * live. `load` receives an AbortSignal and returns the fresh jobs.
 */
export function usePolledJobs(load: (signal: AbortSignal) => Promise<Job[]>, filterKey: string): PolledJobs {
  const phase = useApiPhase();
  const [known, setKnown] = useState<Map<string, Job>>(new Map());
  const [error, setError] = useState<unknown>(null);
  const [loading, setLoading] = useState(true);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [tick, setTick] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  const phaseRef = useRef(phase);
  phaseRef.current = phase;
  const knownRef = useRef(known);
  knownRef.current = known;

  useEffect(() => {
    setKnown(new Map());
    setUpdatedAt(null);
  }, [filterKey]);

  useEffect(() => {
    const ctl = new AbortController();
    let timer: number | undefined;
    setLoading(true);
    loadRef.current(ctl.signal).then(
      (fresh) => {
        if (ctl.signal.aborted) return;
        // Jobs the filter no longer returns are dropped; the rest only move forward.
        setKnown((k) => mergeJobs(new Map([...k].filter(([id]) => fresh.some((j) => j.id === id))), fresh));
        setError(null);
        setLoading(false);
        setUpdatedAt(Date.now());
        if (fresh.some((j) => !isTerminal(j.state))) {
          timer = window.setTimeout(() => setTick((t) => t + 1), pollInterval(phaseRef.current));
        }
      },
      (e: unknown) => {
        if (ctl.signal.aborted || isAbort(e)) return;
        setError(e);
        setLoading(false);
        // Keep polling through a waking API or while live jobs are on screen; otherwise stop.
        if (isWakingError(e) || [...knownRef.current.values()].some((j) => !isTerminal(j.state))) {
          timer = window.setTimeout(() => setTick((t) => t + 1), 5000);
        }
      },
    );
    return () => {
      ctl.abort();
      window.clearTimeout(timer);
    };
  }, [filterKey, tick]);

  const jobs = [...known.values()].sort(compareJobs);
  return { jobs, error, loading, updatedAt, reload: () => setTick((t) => t + 1) };
}

/** One job, polled until terminal. */
export function usePolledJob(jobId: string | null): PolledJobs {
  return usePolledJobs(
    async (signal) => (jobId === null ? [] : [await api.job(jobId, { signal })]),
    `job:${jobId ?? ''}`,
  );
}

/** Seconds since an absolute epoch-ns instant, by `nowMs`; `null` if unknown. Never negative. */
export function ageSeconds(atNs: string | null, nowMs: number): number | null {
  if (atNs === null) return null;
  const ms = Number(BigInt(atNs) / 1_000_000n);
  return Math.max(0, Math.floor((nowMs - ms) / 1000));
}

/** Heartbeat older than 3x the server-declared interval (§10.4). */
export function heartbeatStale(job: Job, nowMs: number): boolean {
  if (job.state !== 'running' || job.last_heartbeat_at_ns === null || job.heartbeat_interval_ms === null) return false;
  const age = ageSeconds(job.last_heartbeat_at_ns, nowMs);
  return age !== null && age * 1000 > 3 * job.heartbeat_interval_ms;
}

export const STATE_TONE: Record<JobState, 'ok' | 'warn' | 'error' | 'neutral' | 'info'> = {
  created: 'neutral',
  queued: 'info',
  running: 'info',
  cancelling: 'warn',
  completed: 'ok',
  failed: 'error',
  cancelled: 'neutral',
  expired: 'warn',
};

/** Copy rule per state (§10.4). */
export const STATE_NOTE: Record<JobState, string> = {
  created: 'Accepted, not yet queued.',
  queued: 'Waiting for a worker.',
  running: 'A worker holds the lease and is simulating.',
  cancelling: 'Cancellation requested; the worker stops at its next checkpoint.',
  completed: 'Finished; the run is stored.',
  failed: 'The job failed; the error is shown verbatim.',
  cancelled: 'Cancelled.',
  expired: 'Expired by the server.',
};
