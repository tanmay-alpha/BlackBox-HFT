import { describe, expect, it, vi } from 'vitest';

import { configDiffRows, variedPaths } from './configValue';
import { fmtDuration, fmtDurationExact } from './fmt';

describe('configDiffRows', () => {
  it('merges the two keys of one latency setting into one row', () => {
    const rows = configDiffRows([
      { path: 'execution.order_latency.entry', a: null, b: { min_ns: '1000', jitter: 0.5, clip: 0.1, unit: 10 } },
      { path: 'execution.order_latency.entry_ns', a: 1_000_000, b: null },
      { path: 'execution.order_latency.type', a: 'constant', b: 'heavy_tail' },
    ] as never);
    expect(rows.map((r) => r.path)).toEqual(['execution.order_latency.entry_ns', 'execution.order_latency.type']);
    expect(rows[0].a).toEqual({ path: 'execution.order_latency.entry_ns', value: 1_000_000 });
    expect(rows[0].b.path).toBe('execution.order_latency.entry');
  });

  it('keeps both rows when one run holds both keys', () => {
    const rows = configDiffRows([
      { path: 'p.entry', a: 1, b: 2 },
      { path: 'p.entry_ns', a: 3, b: null },
    ] as never);
    expect(rows).toHaveLength(2);
  });

  it('drops a difference that is unset on both sides', () => {
    expect(configDiffRows([{ path: 'e.cancel', a: null, b: null }] as never)).toEqual([]);
  });

  it('lists a varied setting once', () => {
    expect(variedPaths(['e.entry', 'e.entry_ns', 'e.type'])).toEqual(['e.entry_ns', 'e.type']);
  });
});

describe('draft numbers', () => {
  it('format as a dash without a console error', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(fmtDuration(Number.NaN)).toBe('\u2014');
    expect(fmtDurationExact(Number.POSITIVE_INFINITY)).toBe('\u2014');
    expect(fmtDuration(1e105)).toBe('\u2014');
    expect(spy).not.toHaveBeenCalled();
    spy.mockRestore();
  });
});
