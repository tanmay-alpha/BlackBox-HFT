import { describe, expect, it } from 'vitest';

import type { SubmitExperimentResult } from '../../api/model';
import { currentJob, jobStages, jobView, stepStates } from './progress';
import type { Job } from './progress';

/** A job with only the fields the Lab reads; the rest of the wire shape is not built here. */
function job(id: string, state: Job['state'], extra: Partial<Job> = {}): Job {
  return {
    id,
    state,
    queue_position: null,
    queue_depth: null,
    progress: null,
    error: null,
    expired_reason: null,
    started_at_ns: null,
    run_id: null,
    ...extra,
  } as unknown as Job;
}

function result(j: Job | null, opts: { created?: boolean; storedRun?: string } = {}): SubmitExperimentResult {
  return {
    created: opts.created ?? true,
    experiment: { id: 'exp-1', run_id: 'run-new', status: 'pending' },
    job: j,
    run: opts.storedRun ? { run_id: opts.storedRun } : null,
  } as unknown as SubmitExperimentResult;
}

describe('jobView (PHASE3_REVIEW D1: a queued job is never described as already simulated)', () => {
  it('shows the submitted job as queued before the first poll lands', () => {
    const v = jobView(result(job('j1', 'queued', { queue_position: 1, queue_depth: 3 })), null);
    expect(v.phase).toBe('queued');
    expect(v.ready).toBe(false);
    expect(v.job?.id).toBe('j1');
    expect(`${v.headline} ${v.detail}`).not.toMatch(/already simulated|not needed/i);
    expect(v.detail).toContain('Nothing has been simulated yet');
    expect(v.detail).toContain('position 1 of 3');
  });

  it('ignores a polled job from an earlier submission', () => {
    const stale = job('j0', 'completed', { run_id: 'run-old' });
    const v = jobView(result(job('j1', 'queued')), stale);
    expect(v.job?.id).toBe('j1');
    expect(v.phase).toBe('queued');
    expect(currentJob(job('j1', 'queued'), stale)?.id).toBe('j1');
  });

  it('follows the poll through running to completed', () => {
    const r = result(job('j1', 'queued'));
    const running = jobView(r, job('j1', 'running', { started_at_ns: '1', progress: { done: 300, total: 1199, unit: 'ticks', tape_events: 0 } }));
    expect(running.phase).toBe('running');
    expect(running.ready).toBe(false);
    expect(running.detail).toContain('300 of 1,199 ticks done');
    const done = jobView(r, job('j1', 'completed', { run_id: 'run-new' }));
    expect(done.phase).toBe('completed');
    expect(done.ready).toBe(true);
    expect(jobStages(done).map((s) => s.state)).toEqual(['done', 'done', 'done']);
  });

  it('says "already simulated" only when the server returned a stored run and no job', () => {
    const v = jobView(result(null, { storedRun: 'run-old' }), null);
    expect(v.phase).toBe('stored');
    expect(v.ready).toBe(true);
    expect(v.headline).toBe('Already simulated');
    expect(jobStages(v)).toEqual([]);
  });

  it('reports a shared job and a failure honestly', () => {
    const shared = jobView(result(job('j1', 'running'), { created: false }), null);
    expect(shared.detail).toMatch(/already submitted/);
    const failed = jobView(result(job('j1', 'queued')), job('j1', 'failed', { error: { category: 'internal', code: 'x', message: 'Worker crashed.' } as Job['error'] }));
    expect(failed.phase).toBe('failed');
    expect(failed.ready).toBe(false);
    expect(failed.detail).toContain('Worker crashed.');
    expect(jobStages(failed).map((s) => s.state)).toEqual(['done', 'skipped', 'error']);
  });
});

describe('stepStates (PHASE3_REVIEW D2: no "Run · ready" while invalid)', () => {
  it('blocks the Run step while the configuration has problems', () => {
    const s = stepStates({ origin: 'defaults', edits: 1, problems: 1, submitted: null });
    expect(s['lab-step-review']).toEqual({ status: '1 to fix', state: 'error' });
    expect(s['lab-step-run'].status).toBe('blocked');
    expect(s['lab-step-run'].status).not.toBe('ready');
  });

  it('blocks even when an earlier submission completed', () => {
    const s = stepStates({ origin: 'preset', edits: 2, problems: 2, submitted: 'completed' });
    expect(s['lab-step-run'].status).toBe('blocked');
    expect(s['lab-step-configure'].state).toBe('error');
  });

  it('is ready when valid and reports the job phase after submission', () => {
    expect(stepStates({ origin: 'preset', edits: 0, problems: 0, submitted: null })['lab-step-run']).toEqual({ status: 'ready', state: 'open' });
    expect(stepStates({ origin: 'preset', edits: 0, problems: 0, submitted: 'queued' })['lab-step-run'].status).toBe('queued');
    expect(stepStates({ origin: 'run', edits: 0, problems: 0, submitted: 'completed' })['lab-step-run']).toEqual({ status: 'completed', state: 'done' });
    expect(stepStates({ origin: 'run', edits: 0, problems: 0, submitted: null })['lab-step-choose'].status).toBe('from run');
  });
});
