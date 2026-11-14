import { describe, expect, it } from 'vitest';

import {
  MINUS,
  NsError,
  THIN_SPACE,
  cmpNs,
  digitsForUnit,
  fmtNsDuration,
  fmtNsDurationExact,
  fmtNsRel,
  fmtNsUtc,
  nsFromDecimal,
  nsToSafeNumber,
  parseNs,
  toNs,
} from './ns';

const T = THIN_SPACE;
const M = MINUS;

describe('parseNs / toNs', () => {
  it('parses decimal strings exactly, above 2^53', () => {
    expect(parseNs('9007199254740993')).toBe(9007199254740993n);
    expect(parseNs('1700000000123456789')).toBe(1700000000123456789n);
    expect(toNs('9223372036854775807')).toBe(2n ** 63n - 1n);
    expect(toNs('-9223372036854775808')).toBe(-(2n ** 63n));
  });

  it('parses negative values', () => {
    expect(parseNs('-1')).toBe(-1n);
    expect(toNs('-1500000000')).toBe(-1_500_000_000n);
    expect(toNs(-42)).toBe(-42n);
  });

  it('accepts safe integer numbers and bigints', () => {
    expect(toNs(20_100_000_000)).toBe(20_100_000_000n);
    expect(toNs(Number.MAX_SAFE_INTEGER)).toBe(9007199254740991n);
    expect(toNs(-Number.MAX_SAFE_INTEGER)).toBe(-9007199254740991n);
    expect(toNs(5n)).toBe(5n);
  });

  it('rejects numbers above 2^53 - 1 with an explicit error', () => {
    expect(() => toNs(2 ** 53)).toThrow(NsError);
    expect(() => toNs(9007199254740993)).toThrow(/safe JS integer range/);
    expect(() => toNs(-(2 ** 53))).toThrow(NsError);
    expect(() => toNs(1.7e18)).toThrow(/decimal string/);
  });

  it('rejects non-integers and malformed strings', () => {
    expect(() => toNs(1.5)).toThrow(/not an integer/);
    expect(() => toNs(Number.NaN)).toThrow(NsError);
    expect(() => toNs(Number.POSITIVE_INFINITY)).toThrow(NsError);
    for (const bad of ['', ' 1', '1e9', '1.0', '+1', '0x10', '12345678901234567890', '--1']) {
      expect(() => parseNs(bad), bad).toThrow(NsError);
    }
    expect(() => toNs('9223372036854775808')).toThrow(/64-bit/);
    expect(() => toNs(2n ** 63n)).toThrow(/64-bit/);
  });

  it('converts to a number only when exact', () => {
    expect(nsToSafeNumber('9007199254740991')).toBe(Number.MAX_SAFE_INTEGER);
    expect(nsToSafeNumber(-3)).toBe(-3);
    expect(() => nsToSafeNumber('9007199254740992')).toThrow(/safe JS integer range/);
    expect(() => nsToSafeNumber('-9007199254740992')).toThrow(NsError);
  });

  it('compares without conversion, beyond 2^53', () => {
    expect(cmpNs('9007199254740993', '9007199254740992')).toBe(1);
    expect(cmpNs('9007199254740992', 9007199254740991)).toBe(1);
    expect(cmpNs(-5, '-5')).toBe(0);
    expect(cmpNs('-6', -5)).toBe(-1);
  });
});

describe('digitsForUnit', () => {
  it('follows the declared source resolution', () => {
    expect(digitsForUnit('seconds')).toBe(0);
    expect(digitsForUnit('millis')).toBe(3);
    expect(digitsForUnit('micros')).toBe(6);
    expect(digitsForUnit('nanos')).toBe(9);
  });
});

describe('fmtNsUtc', () => {
  const origin = '1700000000123456789';

  it('formats absolute epoch ns exactly, above 2^53', () => {
    expect(fmtNsUtc(origin, 9)).toBe('2023-11-14 22:13:20.123456789 UTC');
    expect(fmtNsUtc(1700000000123456789n + 1n, 9)).toBe('2023-11-14 22:13:20.123456790 UTC');
  });

  it('truncates to the requested precision and never rounds up', () => {
    expect(fmtNsUtc(origin, 6)).toBe('2023-11-14 22:13:20.123456 UTC');
    expect(fmtNsUtc(origin, 3)).toBe('2023-11-14 22:13:20.123 UTC');
    expect(fmtNsUtc(origin, 0)).toBe('2023-11-14 22:13:20 UTC');
    expect(fmtNsUtc('1700000000999999999', 3)).toBe('2023-11-14 22:13:20.999 UTC');
  });

  it('floors negative values toward the earlier second', () => {
    expect(fmtNsUtc('-1', 9)).toBe('1969-12-31 23:59:59.999999999 UTC');
    expect(fmtNsUtc(-1_500_000_000, 3)).toBe('1969-12-31 23:59:58.500 UTC');
    expect(fmtNsUtc(0, 0)).toBe('1970-01-01 00:00:00 UTC');
  });

  it('accepts numbers only when safe', () => {
    expect(fmtNsUtc(1_000_000_000, 3)).toBe('1970-01-01 00:00:01.000 UTC');
    expect(() => fmtNsUtc(1.7e18, 9)).toThrow(NsError);
  });
});

describe('fmtNsRel', () => {
  it('formats relative times with sign, unwrapped minutes and grouped fraction digits', () => {
    expect(fmtNsRel(20_100_000_000, 6)).toBe(`+00:20.100${T}000`);
    expect(fmtNsRel('20100000123', 9)).toBe(`+00:20.100${T}000${T}123`);
    expect(fmtNsRel(20_100_000_000, 3)).toBe('+00:20.100');
    expect(fmtNsRel(7_503_120_045_000, 6)).toBe(`+125:03.120${T}045`);
    expect(fmtNsRel(0, 3)).toBe('+00:00.000');
    expect(fmtNsRel(59_999_999_999, 0)).toBe('+00:59');
  });

  it('shows negative relative times with U+2212 and truncates the magnitude', () => {
    expect(fmtNsRel(-1_234_567, 6)).toBe(`${M}00:00.001${T}234`);
    expect(fmtNsRel('-90000000000', 3)).toBe(`${M}01:30.000`);
    expect(fmtNsRel(-1, 3)).toBe(`${M}00:00.000`);
  });

  it('handles values beyond 2^53 given as strings', () => {
    // 2^53 + 1 ns = 104 days 5 h 59 min 59.254740993 s
    expect(fmtNsRel('9007199254740993', 9)).toBe(`+150119:59.254${T}740${T}993`);
  });
});

describe('fmtNsDuration', () => {
  it('uses three significant digits and the right unit', () => {
    expect(fmtNsDuration(0)).toBe('0 ns');
    expect(fmtNsDuration(999)).toBe('999 ns');
    expect(fmtNsDuration(1_000)).toBe('1.00 \u00b5s');
    expect(fmtNsDuration(12_345)).toBe('12.3 \u00b5s');
    expect(fmtNsDuration(2_780_000)).toBe('2.78 ms');
    expect(fmtNsDuration(50_000_000)).toBe('50.0 ms');
    expect(fmtNsDuration(1_000_000_000)).toBe('1.00 s');
    expect(fmtNsDuration('5000000000000')).toBe('5,000 s');
  });

  it('rounds half up at unit and digit boundaries', () => {
    expect(fmtNsDuration(999_499)).toBe('999 \u00b5s');
    expect(fmtNsDuration(999_500)).toBe('1.00 ms');
    expect(fmtNsDuration(9_994_999)).toBe('9.99 ms');
    expect(fmtNsDuration(9_995_000)).toBe('10.0 ms');
    expect(fmtNsDuration(99_950_000)).toBe('100 ms');
    expect(fmtNsDuration(1_005)).toBe('1.01 \u00b5s');
  });

  it('signs negative durations with U+2212', () => {
    expect(fmtNsDuration(-2_780_000)).toBe(`${M}2.78 ms`);
    expect(fmtNsDuration('-7')).toBe(`${M}7 ns`);
  });

  it('formats values beyond 2^53 exactly', () => {
    expect(fmtNsDuration('9007199254740993')).toBe('9,007,199 s');
    expect(fmtNsDurationExact('9007199254740993')).toBe('9,007,199.254740993 s');
  });

  it('gives exact durations with trailing zeros trimmed', () => {
    expect(fmtNsDurationExact(1_227_000_000)).toBe('1.227 s');
    expect(fmtNsDurationExact(1_790_156_000)).toBe('1.790156 s');
    expect(fmtNsDurationExact(1_000_000)).toBe('1 ms');
    expect(fmtNsDurationExact(750)).toBe('750 ns');
    expect(fmtNsDurationExact(-2_500)).toBe(`${M}2.5 \u00b5s`);
  });

  it('rejects unsafe numbers', () => {
    expect(() => fmtNsDuration(2 ** 60)).toThrow(NsError);
  });
});

describe('nsFromDecimal', () => {
  it('converts typed decimals with integer arithmetic', () => {
    expect(nsFromDecimal('0.5', 'ms')).toBe(500_000n);
    expect(nsFromDecimal('20.1', 's')).toBe(20_100_000_000n);
    expect(nsFromDecimal('0.1', 's')).toBe(100_000_000n);
    expect(nsFromDecimal('3', 'us')).toBe(3_000n);
    expect(nsFromDecimal('-1.25', 'ms')).toBe(-1_250_000n);
    expect(nsFromDecimal('9007199.254740993', 's')).toBe(9007199254740993n);
  });

  it('refuses sub-nanosecond digits and non-numbers', () => {
    expect(() => nsFromDecimal('0.0000000001', 's')).toThrow(/below one nanosecond/);
    expect(() => nsFromDecimal('1.5', 'ns')).toThrow(NsError);
    expect(() => nsFromDecimal('abc', 'ms')).toThrow(NsError);
  });
});
