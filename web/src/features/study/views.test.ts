import { describe, expect, it } from 'vitest';

import publishedText from '../../../public/study-results/exec-validation/study-result.json?raw';
import { parseStudyResult, type StudyResult } from './contract';
import { makeStudy } from './study.fixtures';
import { cancelDelayViews, feedDelayViews, inventoryView, seedStrip, seedStrips, spreads } from './views';

const published = parseStudyResult(publishedText);
const jitter = (s: StudyResult) => s.aggregates[1];

function reason<T>(v: { ok: true; data: T } | { ok: false; reason: string }): string {
  if (v.ok) throw new Error('expected the view to be omitted');
  return v.reason;
}

describe('seed strip validation', () => {
  it('draws every seed value when they agree with the published bootstrap', () => {
    const s = makeStudy();
    const v = seedStrip(s, jitter(s), 'fill_rate');
    expect(v.ok && v.data.points.map((p) => [p.seed, p.value])).toEqual([
      [1, 0.6],
      [2, 0.62],
    ]);
    expect(v.ok && v.data.key).toBe('group.normal_liquidity.power_prob3.jitter.0');
  });

  it.each([
    ['a mean that is not the mean of the seeds', (s: StudyResult) => mutateBootstrap(s, { mean: 0.7 }), /published mean/],
    ['a seed count that disagrees with the bootstrap', (s: StudyResult) => mutateBootstrap(s, { n: 5 }), /counts 5 seeds/],
    ['an interval outside the seed values', (s: StudyResult) => mutateBootstrap(s, { lower_95: 0.5 }), /interval/],
    ['a missing member run', (s: StudyResult) => void s.cells.pop(), /missing/],
    ['a member without the measure', (s: StudyResult) => void delete s.cells[2].measures.fill_rate, /has no fill_rate/],
    ['two members on one seed', (s: StudyResult) => void (s.cells[2].seed = 1), /share a seed/],
    ['a seed outside the study seeds', (s: StudyResult) => void (s.cells[2].seed = 99), /not one of the study latency seeds/],
  ])('omits the strip for %s', (_, mutate, why) => {
    const s = makeStudy();
    mutate(s);
    expect(reason(seedStrip(s, jitter(s), 'fill_rate'))).toMatch(why);
  });

  it('never draws a deterministic group as a strip', () => {
    const s = makeStudy();
    expect(reason(seedStrip(s, s.aggregates[0], 'fill_rate'))).toMatch(/no seed mean/);
  });

  it('validates every stochastic group of the published study', () => {
    const v = seedStrips(published, 'net_pnl');
    expect(v.ok && v.data.omitted).toEqual([]);
    expect(v.ok && v.data.strips.length).toBe(published.aggregates.filter((a) => a.members.length > 1).length);
    expect(v.ok && v.data.strips.every((s) => s.points.length === published.design.latency_seeds.length)).toBe(true);
  });
});

function mutateBootstrap(s: StudyResult, patch: Partial<{ mean: number; n: number; lower_95: number }>): void {
  const m = jitter(s).metrics.fill_rate;
  if (m.kind === 'stochastic_seed_mean' && m.bootstrap) Object.assign(m.bootstrap, patch);
}

describe('inventory view', () => {
  it('is omitted when the runs lack the inventory measures', () => {
    expect(reason(inventoryView(makeStudy()))).toMatch(/lacks mean_abs_position/);
  });

  it('is omitted when a run reports a mean above its maximum', () => {
    const s = makeStudy();
    for (const c of s.cells) Object.assign(c.measures, { mean_abs_position: 0.02, max_abs_position: 0.05 });
    for (const a of s.aggregates) a.metrics.mean_abs_position = { kind: 'deterministic_single', value: 0.02 };
    expect(inventoryView(s).ok).toBe(true);
    s.cells[0].measures.mean_abs_position = 0.06;
    expect(reason(inventoryView(s))).toMatch(/above its maximum/);
  });

  it('draws the published baseline-regime groups with the risk limit', () => {
    const v = inventoryView(published);
    expect(v.ok && v.data.limit).toBe(0.1);
    expect(v.ok && v.data.rows.filter((r) => r.reference).length).toBe(1);
    expect(v.ok && v.data.rows.every((r) => r.mean.value <= r.max)).toBe(true);
  });
});

describe('cancel and feed delay views', () => {
  it('are omitted when the study has no such level or group', () => {
    expect(reason(cancelDelayViews(makeStudy()))).toMatch(/no latency level/);
    expect(reason(feedDelayViews(makeStudy()))).toMatch(/no group changes the feed latency offset/);
  });

  it('pair the published cancel-delay and feed-offset groups with the baseline', () => {
    const cancel = cancelDelayViews(published);
    expect(cancel.ok && cancel.data.map((d) => [d.key, d.referenceKey, d.label.kinds])).toEqual([
      ['group.normal_liquidity.power_prob3.cancel_delay.0', 'group.normal_liquidity.power_prob3.constant.0', ['paired']],
    ]);
    const feed = feedDelayViews(published);
    expect(feed.ok && feed.data.map((d) => d.key)).toEqual(['group.normal_liquidity.power_prob3.constant.20000000']);
    expect(feed.ok && feed.data[0].rows.find((r) => r.metric === 'net_pnl')?.delta).toBeCloseTo(-15.066288800000267 - -19.508749399999573, 9);
  });

  it('refuse a feed comparison whose configuration diff changes anything else', () => {
    const s = structuredClone(published);
    const feedRun = s.cells.find((c) => c.feed_offset_ns !== 0)!;
    feedRun.comparison!.config_diffs.push({ path: 'execution.queue_model.type', a: 'power_prob3', b: 'log_prob' });
    expect(reason(feedDelayViews(s))).toMatch(/also changes execution.queue_model.type/);
  });

  it('refuse a comparison that is not aligned like-for-like', () => {
    const s = structuredClone(published);
    s.cells.find((c) => c.latency === 'cancel_delay')!.comparison!.aligned = false;
    expect(reason(cancelDelayViews(s))).toMatch(/not paired like-for-like/);
  });
});

describe('uncertainty spreads', () => {
  it('separates paired parameter sensitivity from cross-dataset variation', () => {
    const rows = spreads(published, 'net_pnl');
    expect(rows.map((r) => [r.factor, r.across, r.levels])).toEqual([
      ['Queue-position model', 'parameter', 6],
      ['Order-latency model (incl. cancel delay)', 'parameter', 7],
      ['Feed latency offset', 'parameter', 2],
      ['Synthetic market regime (dataset)', 'dataset', 6],
    ]);
    for (const r of rows) expect(r.range).toBeCloseTo(r.max - r.min, 12);
  });
});
