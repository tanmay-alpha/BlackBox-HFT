import { describe, expect, it } from 'vitest';

import { normalize } from '../../api/normalize';
import type { ExecutionConfig, OrderDetail } from '../../api/types';
import { buildLedger, minQueueMeaning, type LedgerRow } from './ledger';
import { ORDER_1281, ORDER_256, ORDER_306688, ORDER_6145 } from './ledger.fixtures';

// Run 253bbc23b342f373's execution config, as normalized from GET /runs/{id}.
const EXECUTION: ExecutionConfig = {
  exchange_model: 'no_partial_fill',
  queue_model: { type: 'power_prob3', n: 3 },
  order_latency: { type: 'constant', entry_ns: 1_000_000, response_ns: 1_000_000 },
  feed_latency_offset_ns: 0,
  fees: { maker: 0.0002, taker: 0.0004 },
};
const DATASET = { id: 'synthetic-demo', kind: 'synthetic' as const };

const ledger = (detail: OrderDetail, execution: ExecutionConfig | null = EXECUTION) =>
  buildLedger({ detail, execution, dataset: DATASET });
const row = (rows: LedgerRow[], id: LedgerRow['id']) => rows.find((r) => r.id === id)!;
const fact = (r: LedgerRow, label: string) => r.facts.find((f) => f.label.startsWith(label));

describe('buildLedger', () => {
  it('maps a filled maker order (256) to six rows, one per evidence class', () => {
    const rows = ledger(normalize.orderDetail(ORDER_256));
    expect(rows.map((r) => [r.id, r.basis, r.evidence])).toEqual([
      ['observed', 'observed', 'observed'],
      ['placement', 'modeled', 'modeled'],
      ['queue', 'modeled', 'estimated'],
      ['fill', 'modeled', 'simulated'],
      ['performance', 'derived', 'derived'],
      ['counterfactual', 'unobservable', 'unobservable'],
    ]);

    const observed = row(rows, 'observed');
    expect(observed.summary).toBe('0.088 displayed at 29999.9 on arrival \u00b7 0.088 traded at price');
    expect(fact(observed, 'Best bid / ask')?.value).toBe('29999.9 \u00d7 0.088 / 30000.0 \u00d7 0.060');
    expect(fact(observed, 'Displayed at')?.time).toEqual({ ns: 201_000_000, clock: 'exchange' });
    expect(fact(observed, 'Traded at')?.value).toBe('0.088 in 1 trade print');
    expect(fact(observed, 'Changes in displayed')?.value).toBe('1');
    expect(observed.restsOn).toBe('synthetic-demo (synthetic)');
    expect(observed.assumption).toMatch(/SYNTHETIC/);
    expect(observed.seqs).toEqual([17, 24, 28, 29]);

    const placement = row(rows, 'placement');
    expect(fact(placement, 'Decision')?.time).toEqual({ ns: 200_000_000, clock: 'local' });
    expect(fact(placement, 'Modeled entry latency')?.value).toBe('1.00 ms');
    expect(fact(placement, 'Arrival')?.time).toEqual({ ns: 201_000_000, clock: 'exchange' });
    expect(fact(placement, 'Arrival')?.value).toBe('+1.00 ms after decision');
    expect(fact(placement, 'Cancel sent')).toBeUndefined();
    expect(placement.restsOn).toBe('constant latency 1.00 ms / 1.00 ms');

    const queue = row(rows, 'queue');
    expect(fact(queue, 'Queue model')?.value).toBe('Power probability 3 (n = 3)');
    expect(fact(queue, 'Initial estimate')?.value).toBe('0.088 ahead');
    expect(fact(queue, 'Estimate path')?.value).toBe('0.088 \u2192 0.000 over 3 recorded estimates');
    expect(fact(queue, 'Minimum raw')?.value).toBe('0.000');
    expect(fact(queue, 'Minimum raw')?.note).toMatch(/^Zero: modeled at the front/);
    expect(queue.seqs).toEqual([24, 28, 29]);

    const fill = row(rows, 'fill');
    expect(fill.summary).toBe('0.010 of 0.010 filled as maker at 29999.9, +58.3 ms after decision');
    expect(fact(fill, 'Fill condition')?.value).toBe("filled because the market traded through the order's price");
    expect(fact(fill, 'Fill condition')?.note).toBeUndefined();
    expect(fact(fill, 'First fill')?.time).toEqual({ ns: 258_326_758, clock: 'exchange' });
    expect(fact(fill, 'First fill')?.note).toBe('57.3 ms after arrival');
    expect(fact(fill, 'Maker / taker')?.value).toBe('0.010 / 0.000');
    expect(fact(fill, 'Fee')?.value).toBe('+0.0600');
    expect(fill.restsOn).toBe('No partial fills');
    expect(fill.seqs).toEqual([37]);

    const perf = row(rows, 'performance');
    expect(perf.summary).toBe('fee +0.0600 quote \u00b7 realized 0.0000 before fees \u00b7 markout +100 ms 1.500 ticks');
    expect(fact(perf, 'Markouts of fill #37')?.value).toBe(
      '+100 ms 1.500 \u00b7 +1.00 s \u22127.500 \u00b7 +5.00 s 5.500 \u00b7 +30.0 s \u22129.500',
    );
    expect(perf.restsOn).toBe('fees 2.00 bp / 4.00 bp');
  });

  it('maps a cancelled order (1281) without inventing a fill', () => {
    const rows = ledger(normalize.orderDetail(ORDER_1281));
    expect(row(rows, 'observed').summary).toBe('1.191 displayed at 30000.0 on arrival \u00b7 0.203 traded at price');
    const placement = row(rows, 'placement');
    expect(fact(placement, 'Cancel sent')?.value).toBe('+100 ms after decision');
    expect(fact(placement, 'Cancel sent')?.note).toBe('modeled cancel entry latency 1.00 ms');
    const queue = row(rows, 'queue');
    expect(queue.summary).toBe('1.191 ahead at arrival \u00b7 minimum raw 0.513');
    expect(fact(queue, 'Minimum raw')?.note).toMatch(/never reached the modeled front/);
    const fill = row(rows, 'fill');
    expect(fill.summary).toBe('No fill: cancelled first');
    expect(fact(fill, 'Filled')?.value).toBe('0.000 of 0.010');
    expect(fact(fill, 'Last exchange status')?.value).toBe('canceled');
    expect(fact(fill, 'Last exchange status')?.time).toEqual({ ns: 701_000_000, clock: 'exchange' });
    expect(fact(fill, 'First fill')).toBeUndefined();
    const perf = row(rows, 'performance');
    expect(perf.summary).toBe('Nothing to derive: no simulated fill');
    expect(perf.link).toBeNull();
    expect(fact(perf, 'Markouts and realized')?.value).toBeNull();
  });

  it('maps an expired post-only order (6145) that never rested', () => {
    const rows = ledger(normalize.orderDetail(ORDER_6145));
    const observed = row(rows, 'observed');
    expect(fact(observed, 'Displayed at')?.value).toBeNull();
    expect(fact(observed, 'Changes in displayed')).toBeUndefined();
    const queue = row(rows, 'queue');
    expect(queue.summary).toBe('No estimate: the order never rested');
    expect(fact(queue, 'Initial estimate')?.value).toBeNull();
    expect(fact(queue, 'Estimate path')?.value).toBeNull();
    expect(fact(queue, 'Minimum raw')?.value).toBeNull();
    expect(queue.link).toBeNull();
    expect(row(rows, 'fill').summary).toBe('No fill: post-only order would have crossed');
  });

  it('maps an order still working when the data ended (306688)', () => {
    const rows = ledger(normalize.orderDetail(ORDER_306688));
    const status = fact(row(rows, 'fill'), 'Last exchange status');
    expect(status?.value).toBe('new');
    expect(status?.time).toBeUndefined();
    expect(status?.note).toBe('not terminal at the exchange when the data ended');
    expect(row(rows, 'queue').summary).toBe('1.517 ahead at arrival \u00b7 minimum raw 0.450');
  });

  it('keeps a negative minimum signed and explains queue exhaustion', () => {
    const detail = normalize.orderDetail(ORDER_256);
    const rows = ledger({ ...detail, order: { ...detail.order, min_queue_ahead: -0.771 } });
    const min = fact(row(rows, 'queue'), 'Minimum raw');
    expect(min?.value).toBe('\u22120.771');
    expect(min?.note).toMatch(/^Negative: 0\.771 traded beyond the modeled front\. Not negative displayed depth/);
  });

  it('handles a risk-blocked order that never reached the exchange', () => {
    const detail = normalize.orderDetail(ORDER_6145);
    const rows = ledger({
      ...detail,
      order: { ...detail.order, outcome: 'risk_blocked', exch_arrival_ts: null, first_response_ts: null, terminal_ts: null },
      events: detail.events.filter((e) => e.body.type === 'order_intent'),
      explanation: { ...detail.explanation, verdict: 'not_filled_risk_blocked', latency: { ...detail.explanation.latency, entry_latency_ns: null } },
    });
    const placement = row(rows, 'placement');
    expect(placement.summary).toBe('Never reached the simulated exchange (risk blocked)');
    expect(fact(placement, 'Modeled entry latency')?.value).toBeNull();
    expect(fact(placement, 'Arrival')?.value).toBeNull();
  });

  it('says the run config is loading instead of naming a model it does not have', () => {
    const rows = ledger(normalize.orderDetail(ORDER_256), null);
    expect(row(rows, 'placement').restsOn).toBe('run config loading');
    expect(row(rows, 'queue').restsOn).toBe('run config loading');
    expect(fact(row(rows, 'queue'), 'Queue model')?.value).toBeNull();
    expect(row(rows, 'fill').restsOn).toBe('run config loading');
  });

  it('states the unobservable counterfactual in words, never numbers', () => {
    for (const fixture of [ORDER_256, ORDER_1281, ORDER_6145, ORDER_306688]) {
      const r = row(ledger(normalize.orderDetail(fixture)), 'counterfactual');
      expect(r.facts.map((f) => f.label)).toEqual([
        'Real fill',
        'True queue position',
        "Other participants' reaction",
        'Market impact',
      ]);
      const text = [r.summary, r.what, r.assumption, ...r.facts.map((f) => `${f.value ?? ''} ${f.note ?? ''}`)].join(' ');
      expect(r.facts.every((f) => f.value === null)).toBe(true);
      expect(text).not.toMatch(/\d/);
    }
  });
});

describe('minQueueMeaning', () => {
  const fmt = (q: number) => q.toFixed(3);
  it('distinguishes exhausted, front, still-ahead and missing estimates', () => {
    expect(minQueueMeaning(-0.45, fmt)).toMatch(/^Negative: 0\.450 traded beyond/);
    expect(minQueueMeaning(0, fmt)).toMatch(/not by itself proof of a fill/);
    expect(minQueueMeaning(0.2, fmt)).toMatch(/never fell below this/);
    expect(minQueueMeaning(null, fmt)).toMatch(/No queue estimate/);
  });
});
