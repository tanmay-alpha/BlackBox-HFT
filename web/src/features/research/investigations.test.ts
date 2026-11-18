import { describe, expect, it } from 'vitest';

import publishedText from '../../../public/study-results/exec-validation/study-result.json?raw';
import type { BookOrder, EventDetail } from '../market/api';
import { parseStudyResult, type StudyResult } from '../study/contract';
import {
  SourceProblem,
  cancelAhead,
  feeEffect,
  identityHolds,
  latencyEffect,
  regimeEffect,
  sensitivityRanking,
  workbenchCommand,
  valueOf,
} from './investigations';

/** The published, hash-verified artefact (contract.test.ts checks its hash). */
const published = (): StudyResult => parseStudyResult(publishedText);

// ----- (a) cancel-ahead ------------------------------------------------------------------------

const order = (ref: string, position: number, shares: number, ahead: number): BookOrder => ({
  order_ref: ref,
  shares,
  queue_position: position,
  shares_ahead: ahead,
  entered_ns: '0',
  entered_event_index: position,
});

/** A delete of order "d" (position 2 of 4, 300 shares) at the best ask 100.0300. */
function deleteDetail(overrides: Partial<EventDetail> = {}): EventDetail {
  const before = [order('a', 1, 100, 0), order('d', 2, 300, 100), order('b', 3, 200, 400), order('c', 4, 50, 600)];
  const after = [order('a', 1, 100, 0), order('b', 2, 200, 100), order('c', 3, 50, 300)];
  const level = (orders: BookOrder[]) => ({ price4: 1_000_300, shares: orders.reduce((s, o) => s + o.shares, 0), order_count: orders.length, orders, orders_truncated: false });
  return {
    event: {
      event_index: 1370,
      message_type: 'D',
      ts_ns: '1',
      order_ref: 'd',
      new_order_ref: null,
      side: 'sell',
      price4: 1_000_300,
      shares: 300,
      best_bid_price4: 1_000_200,
      best_ask_price4: 1_000_300,
      applied: 'book',
    },
    before_event_index: 1369,
    levels: 5,
    orders_per_level: 100,
    before: { bids: [], asks: [] },
    after: { bids: [], asks: [] },
    affected: [{ side: 'sell', price4: 1_000_300, role: 'order', before: level(before), after: level(after) }],
    order_before: {
      order_ref: 'd',
      side: 'sell',
      price4: 1_000_300,
      shares: 300,
      queue_position: 2,
      level_orders: 4,
      shares_ahead: 100,
      level_shares: 650,
      entered_ns: '0',
      entered_event_index: 2,
      age_ns: null,
    },
    order_after: null,
    lifetime_ns: '1',
    same_timestamp: { ts_ns: '1', position: 1, first_event_index: 1370, events: [], events_truncated: false },
    ...overrides,
  };
}

describe('cancelAhead', () => {
  it('sets the L2 level change against the L3 shift of the orders behind', () => {
    const r = cancelAhead(deleteDetail());
    expect(r.l2).toEqual({ before: 650, after: 350, delta: -300, ordersBefore: 4, ordersAfter: 3 });
    expect([r.position, r.levelOrders, r.ordersAhead, r.ordersBehind, r.sharesAheadOfDeleted]).toEqual([2, 4, 1, 2, 100]);
    expect(r.nextBehind).toEqual({ order_ref: 'b', before: { position: 3, shares_ahead: 400 }, after: { position: 2, shares_ahead: 100 } });
    expect([r.behindListed, r.behindConsistent, r.aheadUnchanged]).toEqual([2, true, true]);
  });

  it('reports a queue that does not move as the rule says instead of hiding it', () => {
    const d = deleteDetail();
    d.affected[0].after.orders[1] = order('b', 2, 200, 150);
    expect(cancelAhead(d).behindConsistent).toBe(false);
  });

  it('refuses an event that is not an accepted delete or lacks the order position', () => {
    const d = deleteDetail();
    expect(() => cancelAhead({ ...d, event: { ...d.event, message_type: 'X' } })).toThrow(SourceProblem);
    expect(() => cancelAhead({ ...d, event: { ...d.event, applied: 'quarantined' } })).toThrow(/not an accepted D/);
    expect(() => cancelAhead({ ...d, order_before: null })).toThrow(/no position of the deleted order/);
    expect(() => cancelAhead({ ...d, affected: [] })).toThrow(/no changed level/);
  });
});

// ----- study derivations on the published artefact ----------------------------------------------

describe('valueOf', () => {
  it('reads deterministic values, seed means, and nothing for a missing bootstrap', () => {
    expect(valueOf({ kind: 'deterministic_single', value: 2 })).toEqual({ value: 2, interval: null, n: 1 });
    expect(valueOf({ kind: 'stochastic_seed_mean', bootstrap: { n: 5, mean: 1, lower_95: 0.5, upper_95: 1.5, replicates: 2000, seed: 1 } })).toEqual({
      value: 1,
      interval: [0.5, 1.5],
      n: 5,
    });
    expect(valueOf({ kind: 'stochastic_seed_mean', bootstrap: null })).toBeNull();
    expect(valueOf(undefined)).toBeNull();
  });
});

describe('workbenchCommand', () => {
  it('rebuilds the generator command from the declared settings only', () => {
    const sha = 'b'.repeat(64);
    expect(workbenchCommand({ events: 30000, seed: '7', symbol: 'SYNTH', inject_duplicate_add: false, inject_unknown_execute: false }, sha)).toBe(
      `blackbox market l3-synth --seed 7 --events 30000 --symbol SYNTH --out data/workbench-synthetic\n# expect source SHA-256 ${sha} in the written provenance`,
    );
    expect(workbenchCommand({ events: 30000, seed: '7', symbol: 'SYNTH', inject_duplicate_add: true }, sha)).toBeNull();
    expect(workbenchCommand({ seed: '7', symbol: 'SYNTH' }, sha)).toBeNull();
    expect(workbenchCommand(null, sha)).toBeNull();
  });
});

describe('feeEffect', () => {
  it('takes the baseline run accounting and counts regimes where fees exceed |gross|', () => {
    const s = published();
    const f = feeEffect(s);
    const cell = s.cells.find((c) => c.run_id === s.baseline.run_id)!;
    expect(f.baseline).toMatchObject({ runId: s.baseline.run_id, gross: cell.accounting.gross_pnl, fees: cell.accounting.fees, net: cell.accounting.net_pnl });
    expect(f.baseline.net).toBeCloseTo(-19.508749399999573, 12);
    expect(f.feeRates).toEqual({ maker: 0.0002, taker: 0.0004 });
    expect(f.regimes).toHaveLength(6);
    expect(f.feesExceedGross).toBe(6);
  });

  it('refuses a decomposition where gross − fees ≠ net', () => {
    const s = published();
    const cell = s.cells.find((c) => c.run_id === s.baseline.run_id)!;
    cell.accounting = { ...cell.accounting, net_pnl: cell.accounting.net_pnl + 1 };
    expect(() => feeEffect(s)).toThrow(/decomposition is inconsistent/);
    expect(identityHolds(1, 0.25, 0.75)).toBe(true);
  });

  it('refuses a study without its baseline cell', () => {
    const s = published();
    s.cells = s.cells.filter((c) => c.run_id !== s.baseline.run_id);
    expect(() => feeEffect(s)).toThrow(SourceProblem);
  });
});

describe('latencyEffect', () => {
  it('lists the latency levels in study order, then the feed delay, against constant 1 ms', () => {
    const v = latencyEffect(published());
    expect(v.rows.map((r) => `${r.group.latency}|${r.group.feed_offset_ns}`)).toEqual([
      'constant|0',
      'jitter|0',
      'log_normal|0',
      'heavy_tail|0',
      'entry_slow|0',
      'response_slow|0',
      'cancel_delay|0',
      'constant|20000000',
    ]);
    expect(v.reference.fillRate.value).toBeCloseTo(0.6101694915254238, 15);
    expect(v.slowEntry?.fillRate.value).toBeCloseTo(0.5583173996175909, 15);
    expect(v.slowEntry?.net.value).toBeCloseTo(-17.5787700000006, 12);
    expect(v.feedDelay?.fillRate.value).toBeCloseTo(0.47900763358778625, 15);
    expect(v.rows.find((r) => r.group.latency === 'heavy_tail')?.fillRate.interval).not.toBeNull();
  });

  it('fails visibly when a level has no value', () => {
    const s = published();
    const g = s.aggregates.find((a) => a.regime === s.baseline.regime && a.queue_model === s.baseline.queue_model && a.latency === 'jitter')!;
    g.metrics.fill_rate = { kind: 'stochastic_seed_mean', bootstrap: null };
    expect(() => latencyEffect(s)).toThrow(/no value for fill rate of Uniform jitter/);
  });
});

describe('regimeEffect', () => {
  it('compares regime baselines and pairs anchors only by dataset regime and equal execution', () => {
    const v = regimeEffect(published());
    expect(v.rows.map((r) => r.regime)).toEqual(['normal_liquidity', 'widening_spreads', 'liquidity_depletion', 'volatility_burst', 'thin_book', 'feed_staleness']);
    expect(v.reference.fillRate).toBeCloseTo(0.6101694915254238, 15);
    expect(v.rows.find((r) => r.regime === 'liquidity_depletion')?.fillRate).toBeCloseTo(0.6078066914498141, 15);
    expect(v.rows.filter((r) => r.shock).map((r) => r.regime)).toEqual(['widening_spreads', 'liquidity_depletion']);
    // Pair (4, 5) changes the queue model, (0, 1..3) the latency: neither is a regime contrast.
    expect(v.anchorPairs.map((p) => [p.reference.run_id, p.shocked.run_id, p.regime])).toEqual([
      ['253bbc23b342f373', '88e0971baa6491ab', 'liquidity_depletion'],
      ['253bbc23b342f373', '4d0de6620234a7fe', 'widening_spreads'],
    ]);
  });

  it('refuses a study without a shock regime', () => {
    const s = published();
    s.cells = s.cells.filter((c) => c.regime !== 'liquidity_depletion' && c.regime !== 'widening_spreads');
    expect(() => regimeEffect(s)).toThrow(/no liquidity-depletion or widening-spreads/);
  });
});

describe('sensitivityRanking', () => {
  it('ranks one-factor groups by |net P&L − baseline|, ties in study order', () => {
    const r = sensitivityRanking(published(), 'net_pnl');
    expect(r.baseline.value).toBeCloseTo(-19.508749399999573, 12);
    // Expected order and deltas computed independently with jq over the same file.
    const expected: [string, number][] = [
      ['power_prob3.constant.20000000', 4.442460599999306],
      ['power_prob3.cancel_delay.0', 3.1319605999997435],
      ['power_prob3.entry_slow.0', 1.9299793999989738],
      ['power_prob3.heavy_tail.0', 1.236282359999585],
      ['power_prob3.response_slow.0', 0.4909942000001273],
      ['power_prob3.log_normal.0', 0.3028891999998464],
      ['power_prob3.jitter.0', 0.17439527999962934],
      ['power_prob2.constant.0', -0.1129998000001784],
      ['power_prob.constant.0', -1.8474111129762605e-13],
      ['risk_adverse.constant.0', 0],
      ['log_prob.constant.0', 0],
      ['log_prob2.constant.0', 0],
    ];
    expect(r.ranked.map((x) => `${x.group.queue_model}.${x.group.latency}.${x.group.feed_offset_ns}`)).toEqual(expected.map(([k]) => k));
    r.ranked.forEach((x, i) => expect(x.delta).toBeCloseTo(expected[i][1], 9));
    expect(r.ranked[0].label).toBe('Feed latency offset +20 ms');
    expect(r.ranked[0].fillRateDelta).toBeCloseTo(0.47900763358778625 - 0.6101694915254238, 15);
    expect(r.excluded).toEqual([]);
    // PowerProb's −1.8e-13 is floating-point rounding, not a change.
    expect(r.unchanged).toEqual(['Queue model: PowerProb, n = 3', 'Queue model: Risk-adverse', 'Queue model: LogProb', 'Queue model: LogProb2']);
  });

  it('leaves out groups without a value and says which', () => {
    const s = published();
    const g = s.aggregates.find((a) => a.design === 'one_factor' && a.latency === 'jitter')!;
    g.metrics.net_pnl = { kind: 'stochastic_seed_mean', bootstrap: null };
    const r = sensitivityRanking(s, 'net_pnl');
    expect(r.ranked).toHaveLength(11);
    expect(r.excluded).toEqual(['Order latency: Uniform jitter, median 1 ms']);
  });

  it('fails without a baseline value', () => {
    const s = published();
    s.aggregates = s.aggregates.filter((a) => a.design !== 'regime_baseline' || a.regime !== s.baseline.regime);
    expect(() => sensitivityRanking(s)).toThrow(SourceProblem);
  });
});
