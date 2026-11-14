import { describe, expect, it } from 'vitest';

import { ApiError } from './client';
import { normalize } from './normalize';
import type { FillRecord, OrdersPage, ReplayWindow, RunTimeline } from './types';

const fill = (ts: unknown) => ({ seq: 1, order_id: 7, ts }) as unknown as FillRecord;

describe('normalize', () => {
  it('keeps safe legacy numbers exactly', () => {
    const [f] = normalize.fills([fill(20_100_000_123)]);
    expect(f.ts).toBe(20_100_000_123);
  });

  it('accepts v1 decimal strings and converts them exactly', () => {
    const [f] = normalize.fills([fill('9007199254740991')]);
    expect(f.ts).toBe(Number.MAX_SAFE_INTEGER);
    const w = normalize.replay({
      t0: '0',
      t1: '60000000000',
      step_ns: '200000000',
      levels: 25,
      bbo: { ts: ['1', 2], bid_tick: [], ask_tick: [], bid_qty: [], ask_qty: [] },
      bbo_downsampled: false,
      trades: { ts: ['-5'], price_tick: [], qty: [], side: [] },
      trades_truncated: false,
      depth: [{ ts: '3', bids: [], asks: [] }],
    } as unknown as ReplayWindow);
    expect([w.t0, w.t1, w.step_ns, ...w.bbo.ts, ...w.trades.ts, w.depth[0].ts]).toEqual([0, 60e9, 2e8, 1, 2, -5, 3]);
  });

  it('rejects a number above 2^53 - 1 with an explicit error naming the field', () => {
    expect(() => normalize.fills([fill(0), fill(2 ** 53 + 2)])).toThrow(ApiError);
    try {
      normalize.fills([fill(0), fill(2 ** 53 + 2)]);
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as ApiError).code).toBe('unsafe_integer');
      expect((e as ApiError).message).toMatch(/^fills\[1\]\.ts: .*safe JS integer range/);
    }
  });

  it('rejects a decimal string that does not fit a JS number exactly', () => {
    expect(() => normalize.fills([fill('9007199254740993')])).toThrow(/fills\[0\]\.ts/);
  });

  it('leaves null optional timestamps alone and converts present ones', () => {
    const page = normalize.orders({
      total: 1,
      offset: 0,
      orders: [
        {
          decision_ts: '10',
          exch_arrival_ts: null,
          first_response_ts: 12,
          first_fill_ts: null,
          last_fill_ts: null,
          cancel_sent_ts: '11',
          terminal_ts: null,
        },
      ],
    } as unknown as OrdersPage);
    const o = page.orders[0];
    expect([o.decision_ts, o.exch_arrival_ts, o.first_response_ts, o.cancel_sent_ts]).toEqual([10, null, 12, 11]);
  });

  it('walks nested flight-event bodies', () => {
    const t = normalize.timeline({
      run_id: 'r',
      dataset_id: 'd',
      t0: 0,
      t1: '5',
      run_start: 0,
      run_end: 5,
      events: [
        { seq: 0, ts: '1', clock: 'local', source: 'engine', order_id: 1, body: { type: 'request_sent', req: 'new', entry_latency_ns: '1000000', exch_arrival_ts: '1000001' } },
        { seq: 1, ts: 2, clock: 'exchange', source: 'engine', order_id: 1, body: { type: 'local_response', exch_ts: '3' } },
      ],
      events_total: 2,
      events_truncated: false,
      fills: [],
      equity: [],
      orders: [],
    } as unknown as RunTimeline);
    expect(t.events[0].body).toMatchObject({ entry_latency_ns: 1_000_000, exch_arrival_ts: 1_000_001 });
    expect(t.events[1].body).toMatchObject({ exch_ts: 3 });
    expect(t.t1).toBe(5);
  });

  it('rejects a fractional nanosecond value', () => {
    expect(() =>
      normalize.timeline({
        t0: 0,
        t1: 1,
        run_start: 0,
        run_end: 1,
        events: [{ seq: 0, ts: 1, body: { type: 'local_response', exch_ts: 1.5 } }],
        fills: [],
        equity: [],
        orders: [],
      } as unknown as RunTimeline),
    ).toThrow(/timeline\.events\[0\]\.body\.exch_ts: .*not an integer/);
  });

  it('validates absolute origins as decimal strings without converting them', () => {
    const d = normalize.dataset({
      origin_ns: '1700000000123456789',
      start: 0,
      end: '120000000000',
      timestamps: { input_unit: 'nanos', local_ts: 'synthetic', local_ts_shift_ns: 0, note: '' },
    } as never);
    expect(d.origin_ns).toBe('1700000000123456789');
    expect(d.end).toBe(120_000_000_000);
    expect(() => normalize.dataset({ origin_ns: 1.7e18, start: 0, end: 0, timestamps: { local_ts_shift_ns: 0 } } as never)).toThrow(
      /dataset\.origin_ns/,
    );
  });
});
