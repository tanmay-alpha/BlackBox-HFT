import { describe, expect, it, vi } from 'vitest';

import { configCellText, configDiffRows, variedPaths } from './configValue';
import { fmtDuration, fmtDurationExact } from './fmt';

describe('configCellText', () => {
  it('lists a heavy-tail latency leg as key/value text, min first', () => {
    expect(
      configCellText('execution.order_latency.entry', { clip: 0.1, jitter: 0.5, min_ns: '768920', unit: 10 }),
    ).toBe('min 768.92 \u00b5s, jitter 0.5, clip 0.1, unit 10');
  });

  it('reads a log-normal leg with its median as a duration', () => {
    expect(configCellText('execution.order_latency.entry', { median_ns: '20000000', sigma: 0 })).toBe('median 20 ms, sigma 0');
  });

  it('shows null as none and scalars as read', () => {
    expect(configCellText('execution.order_latency.entry', null)).toBe('none');
    expect(configCellText('execution.order_latency.entry_ns', 1_000_000)).toBe('1 ms');
    expect(configCellText('execution.fees.maker', 0.0002)).toBe('0.0002 (2 bp)');
    expect(configCellText('risk.flatten', true)).toBe('yes');
    expect(configCellText('x', {})).toBe('none');
  });

  it('nests objects in parentheses', () => {
    expect(configCellText('a', { inner: { min_ns: 1000 }, k: 2 })).toBe('inner (min 1 \u00b5s), k 2');
  });
});

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
