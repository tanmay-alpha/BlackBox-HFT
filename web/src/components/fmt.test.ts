import { describe, expect, it } from 'vitest';

import {
  DASH,
  fmtBp,
  fmtClock,
  fmtDuration,
  fmtInt,
  fmtNum,
  fmtOriginUtc,
  fmtPct,
  fmtPnl,
  fmtPp,
  fmtPrice,
  fmtQty,
  fmtSigned,
  fmtStat,
  fmtTicks,
  fmtUnixMs,
  humanizeNsText,
  rawText,
} from './fmt';

const M = '\u2212';
const inst = { tick_size: 0.1, lot_size: 0.001, contract_size: 1 };

describe('numbers (locale-independent, U+2212 minus)', () => {
  it('groups thousands with commas and fixes decimals', () => {
    expect(fmtNum(372305.266, 4)).toBe('372,305.2660');
    expect(fmtNum(-74.70961, 4)).toBe(`${M}74.7096`);
    expect(fmtInt(12591)).toBe('12,591');
    expect(fmtNum(-0.00001, 2)).toBe('0.00');
  });

  it('signs deltas and P&L explicitly; zero unsigned', () => {
    expect(fmtSigned(29.8734, 4)).toBe('+29.8734');
    expect(fmtSigned(-496, 0)).toBe(`${M}496`);
    expect(fmtSigned(0.00001, 2)).toBe('0.00');
    expect(fmtPnl(-74.70961, 2)).toBe(`${M}74.71`);
  });

  it('formats each quantity class with its precision and unit', () => {
    expect(fmtPct(0.6024)).toBe('60.24%');
    expect(fmtPp(-0.1789)).toBe(`${M}17.89 pp`);
    expect(fmtBp(0.5)).toBe('0.50 bp');
    expect(fmtTicks(-0.1294)).toBe(`${M}0.129`);
    expect(fmtStat(0.62)).toBe('0.62');
    expect(fmtStat(0.0612)).toBe('0.061');
    expect(fmtPrice(299989, inst)).toBe('29998.9');
    expect(fmtPrice(299990.5, inst)).toBe('29999.05');
    expect(fmtQty(0.01, inst)).toBe('0.010');
  });

  it('never prints NaN, Infinity, null or undefined', () => {
    for (const v of [Number.NaN, Number.POSITIVE_INFINITY, null, undefined]) {
      expect(fmtNum(v as number)).toBe(DASH);
      expect(fmtSigned(v as number)).toBe(DASH);
      expect(fmtPct(v as number)).toBe(DASH);
      expect(fmtDuration(v as number)).toBe(DASH);
    }
  });

  it('gives raw text for copies', () => {
    expect(rawText(`${M}372,305.2660`)).toBe('-372305.2660');
    expect(rawText('+00:20.100\u2009000')).toBe('+00:20.100000');
  });
});

describe('time', () => {
  it('formats durations from numbers, strings and fractional statistics', () => {
    expect(fmtDuration(50_000_000)).toBe('50.0 ms');
    expect(fmtDuration('2780000')).toBe('2.78 ms');
    expect(fmtDuration(1234.5)).toBe('1.23 \u00b5s');
  });

  it('shows an em dash instead of a rounded value for unsafe numbers', () => {
    expect(fmtDuration(2 ** 60)).toBe(DASH);
    expect(fmtClock(2 ** 60)).toBe(DASH);
  });

  it('formats relative clock with the dataset precision', () => {
    expect(fmtClock(20_100_000_000)).toBe('+00:20.100\u2009000');
    expect(fmtClock(20_100_000_000, 3)).toBe('+00:20.100');
  });

  it('formats absolute UTC from a decimal-string origin without float conversion', () => {
    expect(fmtOriginUtc('1700000000000000000')).toBe('2023-11-14 22:13:20.000 UTC');
    expect(fmtOriginUtc('1700000000000000000', 1_123_456_789, 9)).toBe('2023-11-14 22:13:21.123456789 UTC');
    expect(fmtUnixMs(1_763_000_000_000)).toBe('2025-11-13 02:13:20 UTC');
  });

  it('humanizes raw nanosecond counts in server prose', () => {
    expect(humanizeNsText('gap of 5000000000 ns')).toBe('gap of 5 s');
    expect(humanizeNsText('within [0, 2500000] ns')).toBe('within [0 ns, 2.5 ms]');
  });
});
