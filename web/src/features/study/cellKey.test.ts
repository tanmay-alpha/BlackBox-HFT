import { describe, expect, it } from 'vitest';

import { groupCellKey, parseCellKey, runCellKey } from './cellKey';

describe('cell keys', () => {
  it('round-trips a run key and a group key', () => {
    expect(parseCellKey(runCellKey('ccc243055c1a7e59'))).toEqual({ kind: 'run', runId: 'ccc243055c1a7e59' });
    const g = { regime: 'normal_liquidity', queue_model: 'power_prob3', latency: 'constant', feed_offset_ns: 20_000_000 };
    expect(groupCellKey(g)).toBe('group.normal_liquidity.power_prob3.constant.20000000');
    expect(parseCellKey(groupCellKey(g))).toEqual({
      kind: 'group',
      regime: 'normal_liquidity',
      queueModel: 'power_prob3',
      latency: 'constant',
      feedOffsetNs: 20_000_000,
    });
  });

  it('survives URLSearchParams encoding unchanged', () => {
    const key = 'group.thin_book.risk-adverse.heavy_tail.0';
    const q = new URLSearchParams({ cell: key });
    expect(q.toString()).toBe(`cell=${key}`);
    expect(parseCellKey(new URLSearchParams(q.toString()).get('cell'))).not.toBeNull();
  });

  it.each([
    null,
    '',
    'run',
    'run.',
    'run.CCC243055C1A7E59',
    'run.ccc243055c1a7e5',
    'run.ccc243055c1a7e59.x',
    'group.a.b.c',
    'group.a.b.c.0.0',
    'group.a.b.c.-1',
    'group.a.b.c.01',
    'group.a.b.c.1e6',
    'group.a b.c.d.0',
    'group..b.c.0',
    'group.a.b.c.99999999999999999999',
    'cell.normal_liquidity',
  ])('rejects %j', (raw) => {
    expect(parseCellKey(raw)).toBeNull();
  });
});
