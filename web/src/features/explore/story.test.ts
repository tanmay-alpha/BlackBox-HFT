import { beforeEach, describe, expect, it, vi } from 'vitest';

import { loadLatencyStory, StoryUnavailable } from './story';

// `loadLatencyStory` reads only these four client calls; each test scripts their answers.
const calls: string[] = [];
const pending = new Map<string, { resolve: (v: unknown) => void; reject: (e: unknown) => void }>();
// The tests settle each request by hand (the lib target, ES2022, has no `Promise.withResolvers`).
function deferred(name: string): Promise<never> {
  calls.push(name);
  return new Promise((resolve, reject) => pending.set(name, { resolve: resolve as (v: unknown) => void, reject }));
}

vi.mock('../../api/client', () => ({
  DEMO_WORKSPACE: 'demo',
  api: {
    demos: () => deferred('demos'),
    run: (id: string) => deferred(`run ${id}`),
    comparisons: () => deferred('comparisons'),
    comparison: (id: string) => deferred(`comparison ${id}`),
  },
}));

const latency = (ns: number) => ({ type: 'constant', entry_ns: ns, response_ns: ns });
const DEMOS = [
  {
    id: 'cost_of_latency',
    runs: [
      { run_id: 'slow', label: '50 ms', config: { execution: { order_latency: latency(50_000_000) } } },
      { run_id: 'fast', label: '1 ms', config: { execution: { order_latency: latency(1_000_000) } } },
    ],
  },
];

/** Runs every callback of the promises settled so far (one macrotask turn; no wall-clock wait). */
const flush = () => new Promise((r) => setTimeout(r, 0));

async function answer(name: string, value: unknown, fail = false) {
  await flush();
  const p = pending.get(name);
  if (!p) throw new Error(`${name} was not requested (requested: ${calls.join(', ')})`);
  if (fail) p.reject(value);
  else p.resolve(value);
  await flush();
}

beforeEach(() => {
  calls.length = 0;
  pending.clear();
});

describe('loadLatencyStory', () => {
  it('loads the list with /demos and the stored comparison with the runs, with the same result', async () => {
    const story = loadLatencyStory(new AbortController().signal);
    await flush();
    expect(calls).toEqual(['comparisons', 'demos']);
    // The stored pair is found whichever way round it was stored.
    await answer('comparisons', { items: [{ id: 'other', run_a: 'x', run_b: 'fast' }, { id: 'c1', run_a: 'fast', run_b: 'slow' }] });
    await answer('demos', DEMOS);
    // Both runs and the stored comparison are in flight together (their order is not significant).
    expect([...calls].sort()).toEqual(['comparison c1', 'comparisons', 'demos', 'run fast', 'run slow']);
    await answer('comparison c1', { run_a: 'fast', run_b: 'slow', metrics: [] });
    await answer('run slow', { id: 'slow' });
    await answer('run fast', { id: 'fast' });
    const s = await story;
    expect(s.fast.runId).toBe('fast');
    expect(s.slow.runId).toBe('slow');
    expect(s.comparisonId).toBe('c1');
    expect(s.comparison).toEqual({ run_a: 'fast', run_b: 'slow', metrics: [] });
    expect(s.oriented).toBe(true);
  });

  it('has no comparison when none is stored for the pair', async () => {
    const story = loadLatencyStory(new AbortController().signal);
    await answer('demos', DEMOS);
    await answer('comparisons', { items: [{ id: 'c2', run_a: 'slow', run_b: 'other' }] });
    await answer('run slow', { id: 'slow' });
    await answer('run fast', { id: 'fast' });
    const s = await story;
    expect(calls.some((c) => c.startsWith('comparison '))).toBe(false);
    expect(s.comparison).toBeNull();
    expect(s.comparisonId).toBeNull();
    expect(s.oriented).toBe(true);
  });

  it('reports a run failure before a list failure, as the sequential loader did', async () => {
    const story = loadLatencyStory(new AbortController().signal);
    const runError = new Error('run failed');
    const outcome = expect(story).rejects.toBe(runError);
    await answer('comparisons', new Error('list failed'), true);
    await answer('demos', DEMOS);
    await answer('run slow', runError, true);
    await outcome;
  });

  it('reports the list failure once the runs loaded', async () => {
    const story = loadLatencyStory(new AbortController().signal);
    const listError = new Error('list failed');
    const outcome = expect(story).rejects.toBe(listError);
    await answer('demos', DEMOS);
    await answer('run slow', { id: 'slow' });
    await answer('run fast', { id: 'fast' });
    await answer('comparisons', listError, true);
    await outcome;
  });

  it('says the experiment is unavailable when the demo is not published, whatever the list did', async () => {
    const story = loadLatencyStory(new AbortController().signal);
    const outcome = expect(story).rejects.toBeInstanceOf(StoryUnavailable);
    await answer('comparisons', new Error('list failed'), true);
    await answer('demos', []);
    await outcome;
  });
});
