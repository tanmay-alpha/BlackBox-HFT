import { describe, expect, it } from 'vitest';

import { ApiError } from './http';
import { normalize, toWire } from './normalize';
import type { FillRecord, RunConfig } from './types';
import type * as W from './v1';

const page = <T>(items: T[]): W.CursorPage<T> => ({ items, next_cursor: null, truncated_by: null });
const fill = (ts: unknown) => ({ seq: 1, order_id: 7, ts }) as unknown as FillRecord;

const CONFIG = {
  dataset_id: 'synthetic-demo',
  seed: '7',
  strategy: { interval_ns: '100000000', kind: { type: 'touch_maker', order_qty: 0.01, max_position: 0.05, offset_ticks: 0, skew_ticks_per_unit: 20 } },
  execution: {
    exchange_model: 'no_partial_fill',
    queue_model: { type: 'risk_adverse' },
    order_latency: { type: 'jitter', entry_ns: '1000000', response_ns: '2000000', jitter_ns: '500' },
    feed_latency_offset_ns: '-5000',
    fees: { maker: 0.0002, taker: 0.0005 },
  },
  risk: { max_position: 1, max_order_qty: 1, max_open_orders: 10, max_drawdown: null },
  record: { sample_every_ticks: 1, max_queue_updates_per_order: 100 },
};

describe('normalize: ns strings', () => {
  it('converts v1 decimal strings exactly, up to 2^53 - 1', () => {
    const p = normalize.fills(page([fill('9007199254740991'), fill('-5')]));
    expect(p.items.map((f) => f.ts)).toEqual([Number.MAX_SAFE_INTEGER, -5]);
  });

  it('rejects a decimal string that does not fit a JS number exactly, naming the field', () => {
    try {
      normalize.fills(page([fill('0'), fill('9007199254740993')]));
      expect.unreachable();
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as ApiError).code).toBe('unsafe_integer');
      expect((e as ApiError).message).toMatch(/^fills\.items\[1\]\.ts: /);
    }
  });

  it('rejects a JSON number above 2^53 - 1 (already rounded by JSON.parse)', () => {
    expect(() => normalize.fills(page([fill(2 ** 53 + 2)]))).toThrow(/fills\.items\[0\]\.ts/);
  });

  it('rejects a malformed or fractional ns value', () => {
    expect(() => normalize.fills(page([fill('1.5')]))).toThrow(ApiError);
    expect(() => normalize.fills(page([fill('12a')]))).toThrow(ApiError);
    expect(() => normalize.fills(page([fill(1.5)]))).toThrow(/not an integer/);
  });

  it('leaves null optional timestamps alone and converts present ones (offset page)', () => {
    const p = normalize.orders({
      items: [{ decision_ts: '10', exch_arrival_ts: null, first_response_ts: '12', first_fill_ts: null, last_fill_ts: null, cancel_sent_ts: '11', terminal_ts: null }],
      offset: 0,
      total: 1,
      next_offset: null,
      truncated_by: null,
    } as never);
    const o = p.items[0];
    expect([o.decision_ts, o.exch_arrival_ts, o.first_response_ts, o.cancel_sent_ts]).toEqual([10, null, 12, 11]);
  });

  it('walks nested flight-event bodies of a timeline', () => {
    const t = normalize.timeline({
      run_id: 'r',
      dataset_id: 'd',
      t0_ns: '0',
      t1_ns: '5',
      run_start_ns: '0',
      run_end_ns: '5',
      events: [
        { seq: 0, ts: '1', clock: 'local', source: 'engine', order_id: 1, body: { type: 'request_sent', req: 'new', entry_latency_ns: '1000000', exch_arrival_ts: '1000001' } },
        { seq: 1, ts: '2', clock: 'exchange', source: 'engine', order_id: 1, body: { type: 'local_response', exch_ts: '3' } },
      ],
      events_total: 2,
      next_cursor: null,
      fills: [],
      equity: [],
      orders: [],
      truncated_by: null,
    } as never);
    expect(t.events[0].body).toMatchObject({ entry_latency_ns: 1_000_000, exch_arrival_ts: 1_000_001 });
    expect(t.events[1].body).toMatchObject({ exch_ts: 3 });
    expect(t.t1).toBe(5);
    expect([t.events_truncated, t.next_cursor]).toEqual([false, null]);
  });

  it('keeps absolute epoch ns as validated strings and converts relative span fields', () => {
    const d = normalize.dataset({
      id: 'synthetic-demo',
      title: 'Synthetic',
      kind: 'synthetic',
      level: 'l2_mbp',
      owner: 'global',
      created_at_ns: '1763095313707237000',
      current_version: {
        origin_ns: '1700000000123456789',
        start_ns: '0',
        end_ns: '120000000000',
        retired_at_ns: null,
        manifest: {
          origin_ns: '1700000000123456789',
          start: '0',
          end: '120000000000',
          timestamps: { input_unit: 'nanos', local_ts: 'synthetic', local_ts_shift_ns: '0', note: '' },
        },
      },
    } as never);
    expect(d.current_version?.origin_ns).toBe('1700000000123456789');
    expect(d.current_version?.end_ns).toBe(120_000_000_000);
    expect(d.current_version?.manifest.end).toBe(120_000_000_000);
    expect(() => normalize.dataset({ created_at_ns: 1.7e18, current_version: null } as never)).toThrow(/dataset\.created_at_ns/);
  });

  it('converts a run config, including the u64 seed, and turns it back into the same wire strings', () => {
    const wire = structuredClone(CONFIG);
    const run = normalize.run({
      run_id: 'abc',
      dataset: { id: 'synthetic-demo', version: 1, events_sha256: 'x', origin_ns: '1700000000000000000' },
      interval_ns: '100000000',
      created_at_ns: '1763095313707237000',
      metrics: { markouts: [{ horizon_ns: '100000000', fills: 1, mean_ticks: 0.5 }] },
      config: wire,
    } as never);
    expect(run.interval_ns).toBe(100_000_000);
    expect(run.metrics.markouts[0].horizon_ns).toBe(100_000_000);
    const config = run.config as RunConfig;
    expect(config.seed).toBe(7);
    expect(config.execution.feed_latency_offset_ns).toBe(-5000);
    expect(toWire.config(config)).toEqual(CONFIG);
  });

  const runWith = (config: unknown) =>
    normalize.run({
      run_id: 'abc',
      dataset: { id: 'synthetic-demo', version: 1, events_sha256: 'x', origin_ns: '1700000000000000000' },
      interval_ns: '100000000',
      created_at_ns: '1763095313707237000',
      metrics: { markouts: [] },
      config,
    } as never).config as RunConfig;

  it('round-trips a log_normal latency, think time and a glft strategy exactly', () => {
    const wire = {
      ...CONFIG,
      strategy: {
        interval_ns: '100000000',
        kind: { type: 'glft', order_qty: 0.01, max_position: 0.1, gamma: 100, kappa: 5, intensity_a: 1, sigma_source: 'weighted_mid', vol_halflife_ticks: 50 },
      },
      execution: {
        ...CONFIG.execution,
        think_time_ns: '2000000',
        order_latency: { type: 'log_normal', entry: { median_ns: '1000000', sigma: 0.5 }, response: { median_ns: '59999999999', sigma: 0 }, cancel: null },
      },
    };
    const config = runWith(structuredClone(wire));
    expect(config.execution.think_time_ns).toBe(2_000_000);
    const lat = config.execution.order_latency;
    expect(lat.type === 'log_normal' && [lat.entry.median_ns, lat.response.median_ns, lat.cancel]).toEqual([1_000_000, 59_999_999_999, null]);
    expect(toWire.config(config)).toEqual(wire);
  });

  it('round-trips a heavy_tail latency with a cancel leg and omits a zero think time', () => {
    const wire = {
      ...CONFIG,
      execution: {
        ...CONFIG.execution,
        order_latency: {
          type: 'heavy_tail',
          entry: { min_ns: '1000000', jitter: 0.5, clip: 0.1, unit: 10 },
          response: { min_ns: '2000000', jitter: 0, clip: 0.5, unit: 1 },
          cancel: { min_ns: '9007199254740991', jitter: 1, clip: 0.9, unit: 2.5 },
        },
      },
    };
    const config = runWith(structuredClone(wire));
    const lat = config.execution.order_latency;
    expect(lat.type === 'heavy_tail' && [lat.entry.min_ns, lat.response.min_ns, lat.cancel?.min_ns]).toEqual([1_000_000, 2_000_000, 2 ** 53 - 1]);
    expect(toWire.config(config)).toEqual(wire);
    const zero = toWire.config({ ...config, execution: { ...config.execution, think_time_ns: 0 } }) as { execution: object };
    expect('think_time_ns' in zero.execution).toBe(false);
  });

  it('names the leg of an inexact latency median', () => {
    const order_latency = { type: 'log_normal', entry: { median_ns: '1.5', sigma: 0.5 }, response: { median_ns: '1', sigma: 0 }, cancel: null };
    const wire = { ...CONFIG, execution: { ...CONFIG.execution, order_latency } };
    expect(() => runWith(wire)).toThrow(/order_latency\.entry\.median_ns/);
  });

  it('validates every job instant as an absolute decimal string', () => {
    const job = { id: 'j', state: 'running', version: 1, created_at_ns: '1', queued_at_ns: '2', started_at_ns: null, finished_at_ns: null, last_heartbeat_at_ns: 'x', cancel_requested_at_ns: null };
    expect(() => normalize.job(job as never)).toThrow(/job\.last_heartbeat_at_ns/);
  });

  it('converts downsampled equity buckets and raw samples', () => {
    const b = normalize.equity({ items: [{ t0_ns: '0', t1_ns: '1000', first: 1, last: 2, min_equity: 0, max_equity: 3, n: 4 }], downsampled: true, bucket_ns: '1000', next_cursor: null, truncated_by: null } as never);
    expect(b.bucket_ns).toBe(1000);
    expect(b.items[0]).toMatchObject({ t0_ns: 0, t1_ns: 1000 });
    const s = normalize.equity({ items: [{ ts: '5', equity: 1 }], downsampled: false, bucket_ns: null, next_cursor: null, truncated_by: null } as never);
    expect(s.items[0]).toMatchObject({ ts: 5 });
  });
});

describe('toWire', () => {
  it('sends ns query values and grid axes as decimal strings', () => {
    expect(toWire.ns(20_100_000_000)).toBe('20100000000');
    expect(toWire.ns('-7')).toBe('-7');
    expect(toWire.axis({ type: 'order_latency_ns', values: [1_000_000, 5_000_000] })).toEqual({ type: 'order_latency_ns', values: ['1000000', '5000000'] });
    expect(toWire.axis({ type: 'maker_fee', values: [0.0001] })).toEqual({ type: 'maker_fee', values: [0.0001] });
  });

  it('refuses a value it cannot represent exactly', () => {
    expect(() => toWire.ns(2 ** 60)).toThrow(ApiError);
  });
});
