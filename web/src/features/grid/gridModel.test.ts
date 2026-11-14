import { describe, expect, it } from 'vitest';

import type { RunConfig } from '../../api/types';
import { buildSweep, newDraft, parseAxis, splitSpan } from './gridModel';
import type { SweepFormDraft } from './gridModel';

const base = { dataset_id: 'd', seed: 1 } as unknown as RunConfig;
const span = { start: 0, end: 19_999_999_999 };

function draft(over: Partial<SweepFormDraft> = {}): SweepFormDraft {
  return {
    kind: 'grid',
    x: parseAxis({ ...newDraft('order_latency_ns'), text: '1, 5' }, 'X'),
    y: parseAxis({ ...newDraft('maker_fee'), text: '0, 1' }, 'Y'),
    random: { samples: '3', seed: '7' },
    seeds: { mode: 'count', count: '3', baseSeed: '', list: '' },
    inSamplePercent: null,
    objective: { metric: 'net_pnl', direction: 'maximize' },
    cscvBlocks: null,
    ...over,
  };
}

describe('splitSpan', () => {
  it('splits the inclusive span exactly, out of sample after in sample', () => {
    expect(splitSpan(span, 60)).toEqual({
      in_sample: { start: 0, end: 12_000_000_000 },
      out_of_sample: { start: 12_000_000_000, end: 20_000_000_000 },
    });
  });
});

describe('buildSweep', () => {
  it('builds a partitioned grid with CSCV', () => {
    const b = buildSweep(base, draft({ inSamplePercent: '60', cscvBlocks: 4 }), span, 25);
    expect(b.problems).toEqual([]);
    expect(b.configurations).toBe(4);
    expect(b.runs).toBe(8);
    expect(b.spec?.design).toEqual({
      type: 'grid',
      axes: [
        { type: 'order_latency_ns', values: [1_000_000, 5_000_000] },
        { type: 'maker_fee', values: [0, 0.0001] },
      ],
    });
    expect(b.spec?.partition?.out_of_sample.end).toBe(20_000_000_000);
    expect(b.spec?.cscv_blocks).toBe(4);
  });

  it('builds a random design over the axis values as discrete sets', () => {
    const b = buildSweep(base, draft({ kind: 'random' }), span, 25);
    expect(b.problems).toEqual([]);
    expect(b.spec?.design).toEqual({
      type: 'random',
      samples: 3,
      seed: '7',
      params: [
        { type: 'order_latency_ns', domain: { kind: 'set', values: [1_000_000, 5_000_000] } },
        { type: 'maker_fee', domain: { kind: 'set', values: [0, 0.0001] } },
      ],
    });
    expect(buildSweep(base, draft({ kind: 'random', random: { samples: '5', seed: '7' } }), span, 25).problems[0]).toMatch(/only 4 combinations/);
  });

  it('builds seed ensembles from a count or a list and refuses bad seeds', () => {
    expect(buildSweep(base, draft({ kind: 'seeds' }), span, 25).spec?.design).toEqual({ type: 'seeds', count: 3 });
    expect(
      buildSweep(base, draft({ kind: 'seeds', seeds: { mode: 'count', count: '3', baseSeed: '100', list: '' } }), span, 25).spec?.design,
    ).toEqual({ type: 'seeds', count: 3, base_seed: '100' });
    const listed = buildSweep(base, draft({ kind: 'seeds', seeds: { mode: 'list', count: '', baseSeed: '', list: '4, 9 12' } }), span, 25);
    expect(listed.spec?.design).toEqual({ type: 'seeds', seeds: ['4', '9', '12'] });
    expect(listed.configurations).toBe(3);
    const bad = (list: string) => buildSweep(base, draft({ kind: 'seeds', seeds: { mode: 'list', count: '', baseSeed: '', list } }), span, 25).problems.join('; ');
    expect(bad('1, 1')).toMatch(/twice/);
    expect(bad('1')).toMatch(/at least 2/);
    expect(bad('1, 9007199254740992')).toMatch(/2\^53/);
    expect(bad('1, x')).toMatch(/not a non-negative integer/);
    expect(buildSweep(base, draft({ kind: 'seeds', cscvBlocks: 4 }), span, 25).problems.join()).toMatch(/nothing is selected/);
  });

  it('enforces the server limit, a known span and an additive CSCV objective', () => {
    expect(buildSweep(base, draft({ kind: 'seeds', seeds: { mode: 'count', count: '30', baseSeed: '', list: '' } }), span, 25).problems.join()).toMatch(/limit is 25/);
    expect(buildSweep(base, draft({ inSamplePercent: '60' }), null, 25).problems.join()).toMatch(/span is not known/);
    expect(
      buildSweep(base, draft({ cscvBlocks: 4, objective: { metric: 'fill_count', direction: 'maximize' } }), span, 25).problems.join(),
    ).toMatch(/time-additive/);
    expect(buildSweep(null, draft(), span, 25).spec).toBeNull();
  });
});
