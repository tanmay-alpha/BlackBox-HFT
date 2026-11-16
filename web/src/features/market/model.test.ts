import { describe, expect, it } from 'vitest';

import {
  checkCounts,
  checkStatus,
  fmtPrice4,
  fmtSessionNs,
  fmtShare,
  fmtShares,
  listedOf,
  nsBetween,
  parseWorkbenchQuery,
  ratio,
  sampleAtOrBefore,
  shareImbalance,
  sortChecks,
  stepWithin,
  topOfBook,
  typeCountRows,
} from './model';

const ev = (...idx: number[]) => idx.map((event_index) => ({ event_index }));

describe('parseWorkbenchQuery', () => {
  it('reads valid values and ignores malformed ones', () => {
    expect(parseWorkbenchQuery(new URLSearchParams('tab=book&event=42&type=A&from=40'))).toEqual({ tab: 'book', event: 42, type: 'A', from: 40 });
    expect(parseWorkbenchQuery(new URLSearchParams('tab=nope&event=-1&type=AB&from=100000'))).toEqual({
      tab: 'dataset',
      event: null,
      type: null,
      from: null,
    });
    expect(parseWorkbenchQuery(new URLSearchParams('event=1.5'))).toMatchObject({ event: null });
  });
});

describe('fmtPrice4', () => {
  it('prints exact four-decimal prices', () => {
    expect(fmtPrice4(1_234_500)).toBe('123.4500');
    expect(fmtPrice4(1)).toBe('0.0001');
    expect(fmtPrice4(123_456_789_0)).toBe('123,456.7890');
    expect(fmtPrice4(-25)).toBe('\u22120.0025');
    expect(fmtPrice4(null)).toBe('\u2014');
  });
});

describe('fmtShares', () => {
  it('groups thousands with a true minus', () => {
    expect(fmtShares(1200)).toBe('1,200');
    expect(fmtShares(-300)).toBe('\u2212300');
    expect(fmtShares(null)).toBe('\u2014');
  });
});

describe('fmtSessionNs', () => {
  it('prints time since midnight to the nanosecond', () => {
    // 09:30:00.000 000 001
    expect(fmtSessionNs('34200000000001')).toBe('09:30:00.000\u2009000\u2009001');
    expect(fmtSessionNs('0')).toBe('00:00:00.000\u2009000\u2009000');
    // 23:59:59.999 999 999
    expect(fmtSessionNs('86399999999999')).toBe('23:59:59.999\u2009999\u2009999');
    expect(fmtSessionNs(null)).toBe('\u2014');
    expect(fmtSessionNs('12.5')).toBe('\u2014');
  });
});

describe('nsBetween', () => {
  it('subtracts exactly beyond 2^53', () => {
    expect(nsBetween('9007199254740993', '9007199254740995')).toBe('2');
    expect(nsBetween('100', '40')).toBe('-60');
    expect(nsBetween(null, '1')).toBeNull();
  });
});

describe('typeCountRows', () => {
  it('sorts by count then letter and computes shares', () => {
    const rows = typeCountRows({ D: 10, A: 30, X: 10, S: 0 });
    expect(rows.map((r) => r.type)).toEqual(['A', 'D', 'X', 'S']);
    expect(rows[0]).toEqual({ type: 'A', name: 'Add order', count: 30, share: 0.6 });
    expect(rows[3].share).toBe(0);
    expect(typeCountRows({ Z: 1 })[0].name).toBe('Not defined by ITCH 5.0');
  });
});

describe('quality checks', () => {
  const checks = [
    { passed: true, severity: 'error' as const },
    { passed: false, severity: 'warn' as const },
    { passed: false, severity: 'error' as const },
    { passed: false, severity: 'info' as const },
    { passed: true, severity: 'warn' as const },
  ];
  it('maps outcome and severity to a status', () => {
    expect(checks.map(checkStatus)).toEqual(['pass', 'warn', 'fail', 'info', 'pass']);
    expect(checkCounts(checks)).toEqual({ pass: 2, warn: 1, fail: 1, info: 1 });
  });
  it('orders failures first and keeps source order within a status', () => {
    expect(sortChecks(checks).map((c) => checks.indexOf(c))).toEqual([2, 1, 3, 0, 4]);
  });
});

describe('topOfBook', () => {
  it('classifies the touch', () => {
    expect(topOfBook(1000, 1003)).toEqual({ bid: 1000, ask: 1003, spread4: 3, mid4: 1001.5, state: 'two-sided' });
    expect(topOfBook(1000, 1000).state).toBe('locked');
    expect(topOfBook(1001, 1000)).toMatchObject({ spread4: -1, state: 'crossed' });
    expect(topOfBook(null, 1000)).toEqual({ bid: null, ask: 1000, spread4: null, mid4: null, state: 'one-sided' });
    expect(topOfBook(null, null).state).toBe('empty');
  });
});

describe('shareImbalance and ratio', () => {
  it('is bounded and undefined without depth', () => {
    expect(shareImbalance(300, 100)).toBe(0.5);
    expect(shareImbalance(0, 100)).toBe(-1);
    expect(shareImbalance(0, 0)).toBeNull();
    expect(ratio(1, 4)).toBe(0.25);
    expect(ratio(1, 0)).toBeNull();
    expect(fmtShare(0.25)).toBe('25.0%');
    expect(fmtShare(null)).toBe('\u2014');
  });
});

describe('listedOf', () => {
  it('sums the listed orders only', () => {
    expect(listedOf({ orders: [{ shares: 100 }, { shares: 50 }] as never })).toEqual({ orders: 2, shares: 150 });
  });
});

describe('stepWithin', () => {
  const page = { from: 0, complete: false };
  it('steps inside the page', () => {
    expect(stepWithin(ev(2, 5, 9), 5, 1, page)).toEqual({ kind: 'select', index: 9 });
    expect(stepWithin(ev(2, 5, 9), 5, -1, page)).toEqual({ kind: 'select', index: 2 });
  });
  it('steps from an event that is not on the page when the page proves the neighbour', () => {
    // Filtered page from 0: 4 is not an add, 5 is the first add after it.
    expect(stepWithin(ev(2, 5, 9), 4, 1, page)).toEqual({ kind: 'select', index: 5 });
    expect(stepWithin(ev(2, 5, 9), 4, -1, page)).toEqual({ kind: 'select', index: 2 });
  });
  it('asks the server at the page edges', () => {
    expect(stepWithin(ev(2, 5, 9), 9, 1, page)).toEqual({ kind: 'fetch' });
    expect(stepWithin(ev(2, 5, 9), 12, -1, page)).toEqual({ kind: 'fetch' });
    expect(stepWithin(ev(2, 5, 9), 12, -1, { from: 0, complete: true })).toEqual({ kind: 'select', index: 9 });
    expect(stepWithin(ev(2, 5, 9), 2, -1, page)).toEqual({ kind: 'fetch' });
    // A page that starts after the selection cannot prove nothing lies in between.
    expect(stepWithin(ev(20, 25), 10, 1, { from: 20, complete: false })).toEqual({ kind: 'fetch' });
    expect(stepWithin(ev(11, 25), 10, 1, { from: 11, complete: false })).toEqual({ kind: 'select', index: 11 });
    expect(stepWithin([], 3, 1, page)).toEqual({ kind: 'fetch' });
  });
});

describe('sampleAtOrBefore', () => {
  it('never looks ahead', () => {
    const s = ev(0, 10, 20);
    expect(sampleAtOrBefore(s, 15)).toBe(1);
    expect(sampleAtOrBefore(s, 20)).toBe(2);
    expect(sampleAtOrBefore(s, 99)).toBe(2);
    expect(sampleAtOrBefore(ev(5, 10), 4)).toBe(-1);
    expect(sampleAtOrBefore([], 4)).toBe(-1);
  });
});
