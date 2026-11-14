import { describe, expect, it } from 'vitest';

import type { Job } from '../../api/model';
import { acceptUpdate, ageSeconds, compareJobs, heartbeatStale, mergeJobs } from './jobs';

const job = (patch: Partial<Job>): Job =>
  ({
    id: 'j1',
    kind: 'run',
    state: 'queued',
    version: 1,
    created_at_ns: '1763095313707237000',
    queue_position: null,
    last_heartbeat_at_ns: null,
    heartbeat_interval_ms: 15000,
    ...patch,
  }) as Job;

describe('job transitions', () => {
  it('applies forward edges, including running -> queued (re-queued after a lost lease)', () => {
    expect(acceptUpdate(job({ state: 'queued', version: 1 }), job({ state: 'running', version: 2 }))).toBe(true);
    expect(acceptUpdate(job({ state: 'running', version: 2 }), job({ state: 'queued', version: 3 }))).toBe(true);
    expect(acceptUpdate(job({ state: 'running', version: 2 }), job({ state: 'cancelling', version: 3 }))).toBe(true);
    expect(acceptUpdate(job({ state: 'cancelling', version: 3 }), job({ state: 'cancelled', version: 4 }))).toBe(true);
  });

  it('ignores stale versions and edges out of terminal states', () => {
    expect(acceptUpdate(job({ state: 'running', version: 3 }), job({ state: 'queued', version: 2 }))).toBe(false);
    expect(acceptUpdate(job({ state: 'completed', version: 5 }), job({ state: 'running', version: 6 }))).toBe(false);
    expect(acceptUpdate(job({ state: 'queued', version: 1 }), job({ state: 'completed', version: 2 }))).toBe(false);
  });

  it('merges a fresh list without moving a job backwards', () => {
    const known = new Map([['j1', job({ state: 'running', version: 4 })]]);
    const merged = mergeJobs(known, [job({ state: 'queued', version: 3 }), job({ id: 'j2', state: 'created' })]);
    expect(merged.get('j1')?.state).toBe('running');
    expect(merged.get('j2')?.state).toBe('created');
  });

  it('sorts running, cancelling, queued (by position), created, then terminal', () => {
    const list = [
      job({ id: 'done', state: 'completed' }),
      job({ id: 'q2', state: 'queued', queue_position: 2 }),
      job({ id: 'run', state: 'running' }),
      job({ id: 'q1', state: 'queued', queue_position: 1 }),
      job({ id: 'new', state: 'created' }),
      job({ id: 'stop', state: 'cancelling' }),
    ];
    expect(list.sort(compareJobs).map((j) => j.id)).toEqual(['run', 'stop', 'q1', 'q2', 'new', 'done']);
  });
});

describe('heartbeat', () => {
  const at = 1_763_095_313_000;
  it('ages an absolute ns instant in whole seconds, never negative', () => {
    expect(ageSeconds('1763095300000000000', at)).toBe(13);
    expect(ageSeconds('1763095400000000000', at)).toBe(0);
    expect(ageSeconds(null, at)).toBeNull();
  });

  it('is stale beyond three server-declared intervals', () => {
    const running = (hbNs: string) => job({ state: 'running', last_heartbeat_at_ns: hbNs, heartbeat_interval_ms: 5000 });
    expect(heartbeatStale(running('1763095300000000000'), at)).toBe(false);
    expect(heartbeatStale(running('1763095290000000000'), at)).toBe(true);
    expect(heartbeatStale(job({ state: 'queued', last_heartbeat_at_ns: '0' }), at)).toBe(false);
  });
});
