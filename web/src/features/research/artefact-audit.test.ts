// Independent oracles use raw artefact fields, not the selectors/formatters under test.
import { expect, it } from 'vitest';

import queueText from '../../../public/study-results/queue-shadow/study-result.json?raw';
import studyText from '../../../public/study-results/exec-validation/study-result.json?raw';
import { parseQueueShadow } from '../queuelab/contract';
import { cellAt, checkpointAt, gridRows, outcomeSummary, volumeAt } from '../queuelab/model';
import { parseStudyResult, type AggregateMetric } from '../study/contract';
import { allGroups, decomposition, latencyComparison, queueComparison, regimeComparison, sensitivityMatrices } from '../study/model';
import { feeEffect, latencyEffect, regimeEffect, sensitivityRanking } from './investigations';

const study = parseStudyResult(studyText);
const queue = parseQueueShadow(queueText);
const point = (m: AggregateMetric) => m.kind === 'deterministic_single'
  ? { value: m.value, interval: null, n: 1, dataClass: 'deterministic' }
  : m.bootstrap && { value: m.bootstrap.mean, interval: [m.bootstrap.lower_95, m.bootstrap.upper_95], n: m.bootstrap.n, dataClass: 'seed_variation' };
const value = (m: AggregateMetric) => m.kind === 'deterministic_single' ? m.value : m.bootstrap!.mean;

it('independently checks every study chart/matrix metric and accounting decomposition', () => {
  for (const metric of Object.keys(study.aggregates[0].metrics)) {
    for (const select of [allGroups, latencyComparison, queueComparison, regimeComparison]) {
      for (const row of select(study, metric)) {
        expect(row.point).toEqual(point(row.group.metrics[metric]));
      }
    }
    for (const matrix of sensitivityMatrices(study, metric)) {
      for (const row of matrix.cells) for (const cell of row) {
        if (cell) expect(cell.point).toEqual(point(cell.group.metrics[metric]));
      }
    }
  }
  for (const row of decomposition(study)) {
    const group = study.aggregates.find((a) => row.key === `group.${a.regime}.${a.queue_model}.${a.latency}.${a.feed_offset_ns}`)!;
    const members = study.cells.filter((c) => group.members.includes(c.run_id));
    for (const [field, key] of [['gross', 'gross_pnl'], ['fees', 'fees'], ['net', 'net_pnl']] as const) {
      expect(row[field]).toBeCloseTo(members.reduce((sum, c) => sum + c.accounting[key], 0) / members.length, 12);
    }
    expect(row.n).toBe(members.length);
  }
});

it('independently checks every study-derived research-home number', () => {
  const fees = feeEffect(study);
  const bases = study.cells.filter((c) => c.design === 'regime_baseline');
  const baseline = study.cells.find((c) => c.run_id === study.baseline.run_id)!;
  expect(fees.baseline).toMatchObject({ gross: baseline.accounting.gross_pnl, fees: baseline.accounting.fees, net: baseline.accounting.net_pnl, fills: baseline.measures.fills });
  expect(fees.feesExceedGross).toBe(bases.filter((c) => c.accounting.fees > Math.abs(c.accounting.gross_pnl)).length);
  for (const row of fees.regimes) expect(row).toMatchObject({ gross: bases.find((c) => c.run_id === row.runId)!.accounting.gross_pnl, fees: bases.find((c) => c.run_id === row.runId)!.accounting.fees, net: bases.find((c) => c.run_id === row.runId)!.accounting.net_pnl });
  for (const row of latencyEffect(study).rows) {
    expect(row.fillRate.value).toBe(value(row.group.metrics.fill_rate));
    expect(row.net.value).toBe(value(row.group.metrics.net_pnl));
    expect(row.fees.value).toBe(value(row.group.metrics.fees));
  }
  for (const row of regimeEffect(study).rows) {
    const c = bases.find((c) => c.run_id === row.runId)!;
    expect([row.fillRate, row.fills, row.net]).toEqual([c.measures.fill_rate, c.measures.fills, c.accounting.net_pnl]);
  }
  const base = study.aggregates.find((a) => a.members.includes(baseline.run_id))!;
  const expected = study.aggregates.filter((a) => a.design === 'one_factor' && a.regime === study.baseline.regime)
    .map((a) => ({ delta: value(a.metrics.net_pnl) - value(base.metrics.net_pnl), fill: value(a.metrics.fill_rate) - value(base.metrics.fill_rate) }))
    .sort((a, b) => Math.abs(b.delta) - Math.abs(a.delta));
  expect(sensitivityRanking(study).ranked.map((r) => ({ delta: r.delta, fill: r.fillRateDelta }))).toEqual(expected);
});

it('independently checks all queue-lab cell, interval, checkpoint, volume and outcome values', () => {
  for (const c of queue.results.cells) expect(cellAt(queue, c.symbol, c.latency_ns, c.horizon_ns, c.model)).toEqual(c);
  for (const c of queue.results.checkpoints) expect(checkpointAt(queue, c.symbol, c.latency_ns, c.checkpoint_ns, c.model)).toEqual(c);
  for (const c of queue.results.cancelled_volume) expect(volumeAt(queue, c.symbol, c.latency_ns, c.horizon_ns)).toEqual(c);
  for (const symbol of queue.results.symbols) for (const model of queue.models.slice(1)) {
    const expected = queue.results.cells.filter((c) => c.symbol === symbol.symbol && c.model === model.id);
    expect(gridRows(queue, symbol.symbol, model.id).map((r) => r.model)).toEqual(expected);
  }
  const eligible = queue.hypotheses.cells.filter((c) => c.h_a_supported !== null);
  const tested = queue.results.cells.filter((c) => c.vs_l3 && c.eligible > 0);
  expect(outcomeSummary(queue)).toMatchObject({ cells: queue.hypotheses.cells.length, testable: eligible.length, haSupported: eligible.filter((c) => c.h_a_supported).length, h0Tested: tested.length, h0Rejected: tested.filter((c) => c.vs_l3!.excludes_zero).length });
});
