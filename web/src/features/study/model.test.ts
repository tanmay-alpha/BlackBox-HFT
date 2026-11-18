import { describe, expect, it } from 'vitest';

import { decomposition, fmtPoint, latencyComparison, metricPoint, regimeComparison } from './model';
import { makeStudy } from './study.fixtures';

const study = makeStudy();

describe('metricPoint', () => {
  it('gives deterministic values no interval and seed means their bootstrap interval', () => {
    expect(metricPoint({ kind: 'deterministic_single', value: 0.5 })).toEqual({ value: 0.5, interval: null, n: 1, dataClass: 'deterministic' });
    expect(metricPoint(study.aggregates[1].metrics.fill_rate)).toEqual({ value: 0.61, interval: [0.6, 0.62], n: 2, dataClass: 'seed_variation' });
    expect(metricPoint({ kind: 'stochastic_seed_mean', bootstrap: null })).toBeNull();
  });
});

describe('comparisons', () => {
  it('orders latency levels as the study does and marks the baseline as reference', () => {
    const rows = latencyComparison(study, 'net_pnl');
    expect(rows.map((r) => [r.label, r.reference])).toEqual([
      ['Constant 1 ms / 1 ms (baseline)', true],
      ['Uniform jitter, median 1 ms', false],
    ]);
    expect(fmtPoint(rows[1].point, (v) => v.toFixed(1))).toBe('-2.5 [-3.0, -2.0]');
  });

  it('compares regimes through their baseline groups only', () => {
    expect(regimeComparison(study, 'fill_rate').map((r) => r.label)).toEqual(['Normal liquidity']);
  });
});

describe('decomposition', () => {
  it('averages gross, fees and net over the group members, with net = gross - fees', () => {
    const [base, jitter] = decomposition(study);
    expect(base).toMatchObject({ gross: -0.5, fees: 0.5, net: -1, n: 1 });
    expect(jitter.n).toBe(2);
    expect(jitter.gross - jitter.fees).toBeCloseTo(jitter.net, 12);
  });
});
