import { describe, expect, it } from 'vitest';

import type { Status } from '../api/model';
import { workerHealth } from './StatusBar';

const status = (worker: Partial<Status['worker']>): Status =>
  ({
    worker: { busy: 0, slots: 1, mode: 'in_process', state: 'idle', heartbeat_interval_ms: 15_000, last_heartbeat_age_ms: 4_000, ...worker },
    queue: { depth: 0, running: 0, as_of_ns: null },
  }) as unknown as Status;

describe('workerHealth', () => {
  it('reports a healthy idle worker without a warning', () => {
    const w = workerHealth(status({}), 1_000, 1_000);
    expect(w.label).toBe('Idle \u2014 waiting for jobs (in-process worker)');
    expect(w.stale).toBe(false);
    expect(w.tone).toBe('ok');
  });

  it('does not flag a healthy worker between polls', () => {
    // Reported 4 s ago; the next poll is due, but the worker kept beating in the meantime.
    const w = workerHealth(status({}), 0, 60_000);
    expect(w.stale).toBe(false);
    expect(w.heartbeatAgeS).toBe(4);
  });

  it('reports busy workers', () => {
    expect(workerHealth(status({ state: 'busy', busy: 1 }), 0, 0).label).toBe('Busy \u2014 1 of 1 job slots in use (in-process worker)');
  });

  it('reports not responding after 3 missed heartbeats', () => {
    const w = workerHealth(status({ last_heartbeat_age_ms: 46_000 }), 0, 0);
    expect(w.stale).toBe(true);
    expect(w.label).toBe('Not responding (no heartbeat for 46 s)');
    expect(w.tone).toBe('warn');
  });

  it('never flags a worker that has not reported its first heartbeat', () => {
    const w = workerHealth(status({ state: 'starting', last_heartbeat_age_ms: null }), 0, 600_000);
    expect(w.stale).toBe(false);
    expect(w.label).toBe('Starting (in-process worker)');
  });
});
