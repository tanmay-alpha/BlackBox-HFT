// A small study result in contract version 1 for the unit tests: one regime baseline, one
// stochastic two-seed group, and two demo anchors (a reference run and one aligned pair).

import type { AnchorRun, StudyCell, StudyResult } from './contract';

const h = (c: string) => c.repeat(64);

function run(runId: string, fill: string, extra: Partial<AnchorRun & StudyCell>): AnchorRun & StudyCell {
  return {
    run_id: runId,
    config_sha256: `${runId}${fill.repeat(48)}`,
    result_sha256: h('e'),
    dataset: { id: 'study-normal_liquidity', sha256: h('d'), rows: 100 },
    seed: 1,
    queue_model: 'power_prob3',
    latency: 'constant',
    feed_offset_ns: 0,
    measures: { fill_rate: 0.5, net_pnl: -1, fills: 10, fees: 0.5 },
    accounting: { realized_pnl: -0.25, unrealized_pnl: -0.25, gross_pnl: -0.5, fees: 0.5, net_pnl: -1 },
    orders: { submitted: 20, filled: 10, partially_filled: 0, canceled: 10 },
    comparison: null,
    regime: 'normal_liquidity',
    design: 'regime_baseline',
    stochastic: false,
    detail: `cells/${runId}.json`,
    index: 0,
    label: 'Normal-liquidity baseline',
    reference_index: null,
    ...extra,
  };
}

export const BASE_ID = '1111111111111111';
export const JITTER_IDS = ['2222222222222222', '3333333333333333'];
export const ANCHOR_IDS = ['253bbc23b342f373', '08200b05ca35723f'];

export function makeStudy(): StudyResult {
  const base = run(BASE_ID, 'a', {});
  const jitter = JITTER_IDS.map((id, i) =>
    run(id, 'b', {
      seed: i === 0 ? 1 : 2,
      latency: 'jitter',
      design: 'one_factor',
      stochastic: true,
      measures: { fill_rate: 0.6 + i * 0.02, net_pnl: -2 - i, fills: 12, fees: 0.6 },
      accounting: { realized_pnl: -1, unrealized_pnl: -0.4 - i, gross_pnl: -1.4 - i, fees: 0.6, net_pnl: -2 - i },
      comparison: {
        reference_run_id: BASE_ID,
        aligned: true,
        like_for_like: true,
        config_diffs: [{ path: 'execution.order_latency.type', a: 'constant', b: 'jitter' }],
        first_divergence: { order_id: 7, tick_no: 3, decision_ts_ns: '300000000', class: 'same_outcome_different_timing', differences: ['exch_arrival_ts'] },
        first_outcome_divergence: null,
        lifecycle_counts: { identical: 5 },
      },
    }),
  );
  const anchorDataset = { id: 'synthetic-demo', sha256: h('f'), rows: 62929 };
  const anchors = [
    run(ANCHOR_IDS[0], 'c', { dataset: anchorDataset }),
    run(ANCHOR_IDS[1], 'c', {
      dataset: anchorDataset,
      index: 1,
      label: 'Heavy-tail order latency (1 ms median, seed 1)',
      latency: 'heavy_tail',
      reference_index: 0,
      comparison: {
        reference_run_id: ANCHOR_IDS[0],
        aligned: true,
        like_for_like: true,
        config_diffs: [],
        first_divergence: { order_id: 42, tick_no: 9, decision_ts_ns: '900000000', class: 'same_outcome_different_timing', differences: [] },
        first_outcome_divergence: null,
        lifecycle_counts: { identical: 0 },
      },
    }),
  ];
  return {
    schema: 'blackbox.study-result',
    schema_version: 1,
    study_id: 'exec-validation-quick',
    title: 'Execution model validation',
    data_class: 'synthetic',
    generator: {
      command: 'blackbox study exec-validation --out <dir> --quick',
      blackbox_version: '0.1.0',
      simulation_revision: 3,
      run_contract_version: 1,
      engine: { name: 'hftbacktest', version: '0.9.4' },
      platform: 'windows-x86_64-gnu+libm-0352e099',
      source_commit: null,
    },
    design: {
      generator_seed: 7,
      duration_ns: 2_000_000_000,
      latency_seeds: [1, 2],
      bootstrap_seed: 3105,
      bootstrap_replicates: 2000,
      bootstrap_method: 'percentile mean',
      matched_median_ns: 1_000_000,
      cross_regime_alignment: 'unavailable',
    },
    baseline: {
      preset: 'maker_baseline',
      regime: 'normal_liquidity',
      queue_model: 'power_prob3',
      latency: 'constant',
      feed_offset_ns: 0,
      run_id: BASE_ID,
      config_sha256: base.config_sha256,
      dataset: { id: 'study-normal_liquidity', sha256: h('d') },
      config: { seed: 1, strategy: { interval_ns: 100_000_000, kind: { type: 'touch_maker', order_qty: 0.01 } } },
    },
    queue_models: [
      { name: 'power_prob3', config: { type: 'power_prob3', n: 3 } },
      { name: 'risk_adverse', config: { type: 'risk_adverse' } },
    ],
    latency_models: [
      { name: 'constant', config: { type: 'constant', entry_ns: 1_000_000, response_ns: 1_000_000 }, stochastic: false },
      { name: 'jitter', config: { type: 'jitter', entry_ns: 500_000, response_ns: 500_000, jitter_ns: 1_000_000 }, stochastic: true },
    ],
    uncertainty: {
      source: 'seed_variation',
      method: 'percentile bootstrap',
      level: 0.95,
      replicates: 2000,
      seed: 3105,
      applies_to_latencies: ['jitter'],
      excludes: 'market sampling uncertainty',
    },
    datasets: [
      {
        id: 'study-normal_liquidity',
        title: 'Synthetic',
        regime: 'normal_liquidity',
        events_sha256: h('d'),
        events_bytes: 1000,
        counts: { rows: 100, trades: 10, depth_updates: 90 },
        generator: { seed: 7, duration_ns: 2_000_000_000 },
      },
    ],
    cells: [base, ...jitter],
    aggregates: [
      {
        regime: 'normal_liquidity',
        queue_model: 'power_prob3',
        latency: 'constant',
        feed_offset_ns: 0,
        design: 'regime_baseline',
        members: [BASE_ID],
        metrics: { fill_rate: { kind: 'deterministic_single', value: 0.5 }, net_pnl: { kind: 'deterministic_single', value: -1 } },
      },
      {
        regime: 'normal_liquidity',
        queue_model: 'power_prob3',
        latency: 'jitter',
        feed_offset_ns: 0,
        design: 'one_factor',
        members: JITTER_IDS,
        metrics: {
          fill_rate: { kind: 'stochastic_seed_mean', bootstrap: { n: 2, mean: 0.61, lower_95: 0.6, upper_95: 0.62, replicates: 2000, seed: 3105 } },
          net_pnl: { kind: 'stochastic_seed_mean', bootstrap: { n: 2, mean: -2.5, lower_95: -3, upper_95: -2, replicates: 2000, seed: 3105 } },
        },
      },
    ],
    sensitivity_surfaces: [],
    demo_anchors: {
      demo_id: 'execution_validation',
      title: 'Execution model validation',
      question: 'How do simulated fills shift?',
      datasets: [
        {
          id: 'synthetic-demo',
          title: 'Synthetic demo market',
          regime: null,
          events_sha256: h('f'),
          events_bytes: 5000,
          counts: { rows: 62929, trades: 1, depth_updates: 1 },
          generator: { seed: 7, duration_ns: 120_000_000_000 },
        },
      ],
      runs: anchors,
      pairs: [[0, 1]],
    },
    caveats: ['Synthetic assumptions.'],
  };
}

/** The UTF-8 bytes of `study` and a matching `sha256sum` line, as published. */
export async function published(study: unknown, sha256Hex: (b: Uint8Array) => Promise<string>): Promise<{ bytes: Uint8Array; sums: string }> {
  const bytes = new TextEncoder().encode(JSON.stringify(study));
  return { bytes, sums: `${await sha256Hex(bytes)}  study-result.json\n` };
}
