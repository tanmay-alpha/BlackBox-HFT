import { describe, expect, it } from 'vitest';

import publishedText from '../../../public/study-results/exec-validation/study-result.json?raw';
import { groupCellKey, runCellKey } from './cellKey';
import { parseStudyResult, type StudyCell, type StudyResult } from './contract';
import {
  classifyComparison,
  controlledDifferences,
  findGroup,
  labelGroup,
  labelRun,
  pathLabel,
  resolveSelection,
  type ComparisonKind,
  type PairFacts,
  type Selection,
} from './explore';
import { ANCHOR_IDS, BASE_ID, JITTER_IDS, makeStudy } from './study.fixtures';

const published = parseStudyResult(publishedText);

function sel(study: StudyResult, key: string): Selection {
  const s = resolveSelection(study, key);
  if (!s) throw new Error(`no selection for ${key}`);
  return s;
}

/** The fixture plus a thin-book regime: its baseline run and one queue-model cell paired with it. */
function withThinBook(): StudyResult {
  const study = makeStudy();
  const base = study.cells[0];
  const dataset = { id: 'study-thin_book', sha256: '9'.repeat(64), rows: 50 };
  const thinBase: StudyCell = {
    ...base,
    run_id: '4444444444444444',
    config_sha256: `4444444444444444${'a'.repeat(48)}`,
    regime: 'thin_book',
    dataset,
    comparison: {
      reference_run_id: '4444444444444444',
      aligned: true,
      like_for_like: true,
      config_diffs: [],
      first_divergence: null,
      first_outcome_divergence: null,
      lifecycle_counts: null,
    },
  };
  const thinRisk: StudyCell = {
    ...base,
    run_id: '5555555555555555',
    config_sha256: `5555555555555555${'a'.repeat(48)}`,
    regime: 'thin_book',
    queue_model: 'risk_adverse',
    design: 'factorial',
    dataset,
    comparison: {
      reference_run_id: '4444444444444444',
      aligned: true,
      like_for_like: true,
      config_diffs: [{ path: 'execution.queue_model.type', a: 'power_prob3', b: 'risk_adverse' }],
      first_divergence: null,
      first_outcome_divergence: null,
      lifecycle_counts: null,
    },
  };
  study.cells.push(thinBase, thinRisk);
  const agg = study.aggregates[0];
  study.aggregates.push(
    { ...agg, regime: 'thin_book', members: [thinBase.run_id] },
    { ...agg, regime: 'thin_book', queue_model: 'risk_adverse', design: 'factorial', members: [thinRisk.run_id] },
  );
  return study;
}

describe('resolveSelection', () => {
  const study = makeStudy();

  it('resolves a group key to its member runs and a run key to the run and its group', () => {
    const g = sel(study, 'group.normal_liquidity.power_prob3.jitter.0');
    expect(g.kind).toBe('group');
    if (g.kind === 'group') expect(g.members.map((m) => m.run_id)).toEqual(JITTER_IDS);
    const r = sel(study, runCellKey(JITTER_IDS[1]));
    expect(r.kind === 'run' && r.group?.latency).toBe('jitter');
  });

  it('resolves a demo anchor with its reference run', () => {
    const a = sel(study, runCellKey(ANCHOR_IDS[1]));
    expect(a.kind === 'anchor' && a.reference?.run_id).toBe(ANCHOR_IDS[0]);
  });

  it('selects nothing for a well-formed key that names nothing', () => {
    expect(resolveSelection(study, 'group.thin_book.power_prob3.constant.0')).toBeNull();
    expect(resolveSelection(study, runCellKey('abcdefabcdefabcd'))).toBeNull();
    expect(resolveSelection(study, 'not-a-key')).toBeNull();
  });
});

describe('controlledDifferences', () => {
  it('lists only the fields that differ, with readable names and raw paths', () => {
    const study = makeStudy();
    expect(controlledDifferences(study, sel(study, runCellKey(BASE_ID)))).toMatchObject({ diffs: [], problem: null });
    const d = controlledDifferences(study, sel(study, runCellKey(JITTER_IDS[0])));
    expect(d.problem).toBeNull();
    expect(d.diffs).toEqual([{ path: 'execution.order_latency.type', label: 'Order-latency model', baseline: 'constant', value: 'jitter' }]);
  });

  it('gives a stochastic group its shared differences plus the seeds it averages over', () => {
    const study = makeStudy();
    const d = controlledDifferences(study, sel(study, 'group.normal_liquidity.power_prob3.jitter.0'));
    expect(d.diffs.map((x) => [x.path, x.value])).toEqual([
      ['execution.order_latency.type', 'jitter'],
      ['seed', '2 seeds: 1, 2 (mean over seeds)'],
    ]);
  });

  it('refuses shared differences when members differ in more than the seed', () => {
    const study = makeStudy();
    study.cells[2].comparison!.config_diffs.push({ path: 'execution.queue_model.type', a: 'power_prob3', b: 'log_prob' });
    expect(controlledDifferences(study, sel(study, 'group.normal_liquidity.power_prob3.jitter.0')).problem).toMatch(/more than the seed/);
  });

  it('adds the dataset when the comparison is with another regime’s baseline run', () => {
    const study = withThinBook();
    const d = controlledDifferences(study, sel(study, runCellKey('5555555555555555')));
    expect(d.problem).toBeNull();
    expect(d.against).toMatch(/Thin book baseline run 4444444444444444/);
    expect(d.diffs.map((x) => [x.label, x.baseline, x.value])).toEqual([
      ['Dataset', 'study-normal_liquidity', 'study-thin_book'],
      ['Queue-position model', 'power_prob3', 'risk_adverse'],
    ]);
  });

  it('does not derive differences against an unknown reference', () => {
    const study = makeStudy();
    study.cells[1].comparison!.reference_run_id = 'ffffffffffffffff';
    expect(controlledDifferences(study, sel(study, runCellKey(JITTER_IDS[0]))).problem).toMatch(/neither the study baseline/);
  });

  it('formats nanosecond and latency-leg values and names unknown paths', () => {
    const study = published;
    const cancel = controlledDifferences(study, sel(study, 'group.normal_liquidity.power_prob3.cancel_delay.0'));
    const leg = cancel.diffs.find((d) => d.path === 'execution.order_latency.cancel');
    expect(leg).toMatchObject({ label: 'Cancel entry latency distribution', baseline: 'not set', value: 'log-normal, median 20 ms, sigma 0' });
    const feed = controlledDifferences(study, sel(study, 'group.normal_liquidity.power_prob3.constant.20000000'));
    expect(feed.diffs).toEqual([{ path: 'execution.feed_latency_offset_ns', label: 'Feed latency offset', baseline: '0 ms', value: '20 ms' }]);
    expect(pathLabel('execution.fees.maker_rate_ns')).toBe('Maker rate');
  });
});

describe('classifyComparison', () => {
  const base: PairFacts = { sameDataset: true, aligned: true, factors: [], runs: 1, derivable: true };
  const cases: [Partial<PairFacts>, ComparisonKind[]][] = [
    [{ factors: ['queue_model'] }, ['paired']],
    [{ factors: ['queue_model', 'order_latency'] }, ['paired', 'not_attributable']],
    [{ factors: ['order_latency'], runs: 5 }, ['multi_seed', 'paired']],
    [{ factors: ['order_latency', 'seed'] }, ['paired', 'not_attributable']],
    [{ aligned: false, factors: ['queue_model'] }, ['metrics_only']],
    [{ sameDataset: false, aligned: false }, ['cross_dataset', 'metrics_only']],
    [{ sameDataset: false, aligned: false, factors: ['queue_model'] }, ['cross_dataset', 'metrics_only', 'not_attributable']],
    [{ derivable: false }, ['paired', 'not_attributable']],
  ];
  it.each(cases)('%j -> %j', (facts, kinds) => {
    expect(classifyComparison({ ...base, ...facts })).toEqual(kinds);
  });
});

describe('labels from artefact fields', () => {
  it('labels the published groups by their hashes, flags and diffs', () => {
    const g = (regime: string, queue: string, latency: string, feed = 0) => labelGroup(published, findGroup(published, regime, queue, latency, feed)!);
    expect(g('normal_liquidity', 'power_prob3', 'constant')).toBeNull();
    expect(g('normal_liquidity', 'risk_adverse', 'constant')?.kinds).toEqual(['paired']);
    expect(g('normal_liquidity', 'power_prob3', 'heavy_tail')?.kinds).toEqual(['multi_seed', 'paired']);
    expect(g('normal_liquidity', 'risk_adverse', 'heavy_tail')?.kinds).toEqual(['multi_seed', 'paired', 'not_attributable']);
    expect(g('widening_spreads', 'power_prob3', 'constant')?.kinds).toEqual(['cross_dataset', 'metrics_only']);
    expect(g('widening_spreads', 'risk_adverse', 'constant')).toMatchObject({ kinds: ['paired'], referenceKey: 'group.widening_spreads.power_prob3.constant.0' });
    expect(g('normal_liquidity', 'power_prob3', 'constant', 20_000_000)?.kinds).toEqual(['paired']);
  });

  it('labels runs: a single seed of a stochastic model is confounded with the seed', () => {
    const run = (id: string) => labelRun(published, [...published.cells, ...published.demo_anchors.runs].find((r) => r.run_id === id)!);
    expect(run('ccc243055c1a7e59')).toBeNull();
    expect(run('7ded836f06325de6')?.kinds).toEqual(['paired']);
    expect(run('3ff80b2d60d3ccc8')?.kinds).toEqual(['paired', 'not_attributable']);
    expect(run('130a33f1b4d955ce')?.kinds).toEqual(['cross_dataset', 'metrics_only']);
    expect(run('253bbc23b342f373')).toBeNull();
    expect(run('08200b05ca35723f')?.kinds).toEqual(['paired']);
    expect(run('88e0971baa6491ab')?.kinds).toEqual(['cross_dataset', 'metrics_only']);
    expect(run('69ca873c448e2dd1')).toMatchObject({ kinds: ['paired'], referenceKey: 'run.88e0971baa6491ab' });
  });

  it('does not depend on the order of the artefact arrays', () => {
    const shuffled: StudyResult = { ...published, cells: [...published.cells].reverse(), aggregates: [...published.aggregates].reverse() };
    for (const a of published.aggregates) {
      expect(labelGroup(shuffled, findGroup(shuffled, a.regime, a.queue_model, a.latency, a.feed_offset_ns)!)).toEqual(labelGroup(published, a));
    }
    expect(groupCellKey(published.aggregates[3])).toBe(sel(shuffled, groupCellKey(published.aggregates[3])).key);
  });
});
