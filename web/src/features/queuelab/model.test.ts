import { describe, expect, it } from 'vitest';

import publishedText from '../../../public/study-results/queue-shadow/study-result.json?raw';
import { parseQueueShadow } from './contract';
import {
  bounds,
  cellAt,
  checkpointAt,
  fmtDiff,
  fmtProb,
  fmtSpan,
  hbPart2Rows,
  hypothesisRows,
  intervalText,
  labQuery,
  modelLabel,
  num,
  outcomeSentence,
  outcomeSummary,
  parseLabState,
  volumeAt,
} from './model';

const study = parseQueueShadow(publishedText);
const MINUS = '\u2212';
/** The document's rendering of a probability (three decimals, U+2212). */
const doc3 = (x: number) => x.toFixed(3).replace('-', MINUS);

describe('displayed intervals are the artefact values, formatted only', () => {
  it('every Wilson and Newcombe bound is passed through unchanged and printed at three decimals', () => {
    let checked = 0;
    for (const c of study.results.cells) {
      const lo = c.fill_fraction_95.lower;
      const hi = c.fill_fraction_95.upper;
      if (lo.status === 'available' && hi.status === 'available') {
        expect(bounds(c.fill_fraction_95)).toEqual([lo.value, hi.value]);
        expect(intervalText(c.fill_fraction_95, fmtProb)).toBe(`[${doc3(lo.value)}, ${doc3(hi.value)}]`);
        checked++;
      } else {
        expect(intervalText(c.fill_fraction_95, fmtProb)).toBeNull();
      }
      const d = c.vs_l3?.difference_95;
      if (d && d.lower.status === 'available' && d.upper.status === 'available') {
        expect(bounds(d)).toEqual([d.lower.value, d.upper.value]);
        const signed = (x: number) => (x > 0 && Number(x.toFixed(3)) !== 0 ? `+${doc3(x)}` : doc3(x));
        expect(intervalText(d, fmtDiff)).toBe(`[${signed(d.lower.value)}, ${signed(d.upper.value)}]`);
      }
    }
    expect(checked).toBe(study.results.cells.filter((c) => c.fill_fraction_95.lower.status === 'available' && c.fill_fraction_95.upper.status === 'available').length);
  });

  it('formats the selected values directly from the published artefact', () => {
    const symbol = study.results.symbols[0].symbol;
    const latency = study.design.latencies_ns[0];
    const tau = study.design.horizons_ns[0];
    const checkpoint = study.design.checkpoints_ns[0];
    const selected = cellAt(study, symbol, latency, tau, 'l3_shadow');
    expect(selected).toBe(study.results.cells.find((c) => c.symbol === symbol && c.latency_ns === latency && c.horizon_ns === tau && c.model === 'l3_shadow'));
    expect(fmtProb(num(selected.fill_fraction))).toBe(selected.fill_fraction.status === 'available' ? doc3(selected.fill_fraction.value) : '\u2014');
    expect(volumeAt(study, symbol, latency, tau)).toBe(study.results.cancelled_volume.find((c) => c.symbol === symbol && c.latency_ns === latency && c.horizon_ns === tau));
    expect(checkpointAt(study, symbol, latency, checkpoint, 'risk_adverse')).toBe(
      study.results.checkpoints.find((c) => c.symbol === symbol && c.latency_ns === latency && c.checkpoint_ns === checkpoint && c.model === 'risk_adverse'),
    );
  });

  it('the nominal artefact interval is the Wilson score interval of its counts (independent check)', () => {
    const l3 = cellAt(study, 'SYNTH', 0, 1e9, 'l3_shadow');
    const [n, p, z] = [l3.eligible, l3.filled / l3.eligible, 1.959963984540054];
    const centre = (p + (z * z) / (2 * n)) / (1 + (z * z) / n);
    const half = (z / (1 + (z * z) / n)) * Math.sqrt((p * (1 - p)) / n + (z * z) / (4 * n * n));
    const [lo, hi] = bounds(l3.fill_fraction_95)!;
    expect(lo).toBeCloseTo(centre - half, 9);
    expect(hi).toBeCloseTo(centre + half, 9);
  });

  it('an unavailable value stays unavailable with the artefact reason', () => {
    const c = cellAt(study, 'SYNTH', 0, 30e9, 'l3_shadow');
    expect(num(c.fill_fraction)).toBeNull();
    expect(intervalText(c.fill_fraction_95, fmtProb)).toBeNull();
    expect(c.fill_fraction.status === 'unavailable' && c.fill_fraction.reason).toBe('no eligible shadow orders');
  });
});

describe('labels', () => {
  it('formats design spans exactly', () => {
    expect([0, 50_000, 500_000, 1e8, 1e9, 30e9, 7].map(fmtSpan)).toEqual(['0', '50 \u00b5s', '500 \u00b5s', '100 ms', '1 s', '30 s', '7 ns']);
  });

  it('names models with the hftbacktest families', () => {
    expect(study.models.map(modelLabel)).toEqual([
      'L3 shadow (reconstructed)',
      'RiskAdverse',
      'LogProb',
      'LogProb2',
      'PowerProb n=2',
      'PowerProb n=3',
      'PowerProb2 n=2',
      'PowerProb2 n=3',
      'PowerProb3 n=2',
      'PowerProb3 n=3',
    ]);
  });
});

describe('URL state', () => {
  it('defaults to the first design value of every parameter', () => {
    const { state, ignored } = parseLabState(new URLSearchParams(), study);
    expect(state).toEqual({ symbol: 'SYNTH', model: 'risk_adverse', side: 'pooled', latency: 0, tau: 1e9, checkpoint: 1e8, stage: 'overview' });
    expect(ignored).toEqual([]);
  });

  it('reads valid parameters and round-trips through labQuery', () => {
    const q = new URLSearchParams('model=power_prob2_n3&latency=500000&tau=5000000000&checkpoint=1000000000&tab=queue&side=pooled');
    const { state, ignored } = parseLabState(q, study);
    expect(state).toMatchObject({ model: 'power_prob2_n3', latency: 500_000, tau: 5e9, checkpoint: 1e9, stage: 'queue', side: 'pooled' });
    expect(ignored).toEqual([]);
    expect(parseLabState(new URLSearchParams(labQuery(state)), study).state).toEqual(state);
  });

  it('reports and replaces values outside the design, never guesses', () => {
    const { state, ignored } = parseLabState(new URLSearchParams('model=l3_shadow&latency=60000&tau=1e9&checkpoint=-1&tab=nope'), study);
    expect(state).toMatchObject({ model: 'risk_adverse', latency: 0, tau: 1e9, checkpoint: 1e8, stage: 'overview' });
    expect(ignored).toHaveLength(5);
    expect(ignored[0]).toBe('model=l3_shadow is not an L2 model of this study; showing risk_adverse.');
    expect(ignored[1]).toMatch(/^latency=60000 is not a design latency \(0, 50000, 500000 ns\)/);
  });

  it('explains that contract v1 has no per-side values', () => {
    const { state, ignored } = parseLabState(new URLSearchParams('side=buy'), study);
    expect(state.side).toBe('pooled');
    expect(ignored[0]).toMatch(/^side=buy: contract v1 pools buy and sell shadows/);
  });
});

describe('hypotheses', () => {
  it('derives outcomes from the artefact flags; cells without eligible shadows are not testable', () => {
    const rows = hypothesisRows(study);
    expect(rows).toHaveLength(study.hypotheses.cells.length);
    for (const r of rows) {
      const eligible = cellAt(study, r.cell.symbol, r.cell.latency_ns, r.cell.horizon_ns, 'l3_shadow').eligible;
      const testable = eligible > 0 && r.cell.h_a_supported !== null;
      expect(r.testable).toBe(testable);
      expect(r.ha).toBe(!testable ? 'not_testable' : r.cell.h_a_supported ? 'supported' : 'not_supported');
      expect(r.h0Rejected).toEqual(testable ? study.models.slice(1).map((m) => m.id).filter((m) => !r.cell.h0_not_rejected.includes(m)) : []);
    }
    for (const r of hbPart2Rows(study, rows)) {
      const testable = rows.some((h) => h.testable && h.cell.latency_ns === r.latency && h.cell.horizon_ns === r.tau);
      expect(r.testable).toBe(testable);
      expect(r.outcome).toBe(!testable ? 'not_testable' : r.models.length === 0 ? 'supported' : 'not_supported');
    }
  });

  it('states the synthetic outcome with counts from the artefact', () => {
    const summary = outcomeSummary(study);
    const testable = hypothesisRows(study).filter((r) => r.testable);
    expect(summary.testable).toBe(testable.length);
    expect(summary.haSupported).toBe(testable.filter((r) => r.cell.h_a_supported).length);
    expect(summary.h0Rejected).toBe(testable.reduce((n, r) => n + r.h0Rejected.length, 0));
    const text = outcomeSentence(summary);
    expect(text).toContain(`H-A is nominally supported in ${summary.haSupported} of ${summary.testable}.`);
    expect(text).toContain(`H0 is nominally rejected in ${summary.h0Rejected} of ${summary.h0Tested}`);
    expect(text).toContain('exploratory; unadjusted for multiplicity');
    expect(text).toContain(`H-B part 2 nominally holds in ${summary.hb2Holds} of ${summary.hb2Testable}`);
  });
});
