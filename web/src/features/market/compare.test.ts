import { describe, expect, it } from 'vitest';

import type { BookLevel, BookOrder, DepthLevel, EventDetail, MarketEvent } from './api';
import { depthRows, depthScale, fmtSigned, mergeQueues, observability, sameTimestampNetting } from './compare';

const BID = 1_000_100; // 100.0100

function order(ref: string, shares: number, position: number, ahead: number, entered = 3): BookOrder {
  return { order_ref: ref, shares, queue_position: position, shares_ahead: ahead, entered_ns: '34200000000000', entered_event_index: entered };
}

function level(price4: number, orders: BookOrder[], count = orders.length, truncated = false): BookLevel {
  return { price4, shares: orders.reduce((s, o) => s + o.shares, 0), order_count: count, orders, orders_truncated: truncated };
}

function depth(l: BookLevel): DepthLevel {
  return { ...l, orders: l.orders.map((o) => ({ order_ref: o.order_ref, shares: o.shares })) };
}

function event(over: Partial<MarketEvent>): MarketEvent {
  return {
    event_index: 40,
    message_type: 'D',
    ts_ns: '34200001000000',
    order_ref: '11',
    new_order_ref: null,
    side: 'buy',
    price4: BID,
    shares: 200,
    best_bid_price4: BID,
    best_ask_price4: 1_000_200,
    applied: 'book',
    ...over,
  };
}

/** Order 11 (second of four at 100.0100, 100 shares ahead) is deleted at event #40. */
function deleteDetail(): EventDetail {
  const before = level(BID, [order('10', 100, 1, 0), order('11', 200, 2, 100), order('12', 300, 3, 300), order('13', 50, 4, 600)]);
  const after = level(BID, [order('10', 100, 1, 0), order('12', 300, 2, 100), order('13', 50, 3, 400)]);
  const ask = level(1_000_200, [order('20', 500, 1, 0)]);
  return {
    event: event({}),
    before_event_index: 39,
    levels: 5,
    orders_per_level: 50,
    before: { bids: [depth(before)], asks: [depth(ask)] },
    after: { bids: [depth(after)], asks: [depth(ask)] },
    affected: [{ side: 'buy', price4: BID, role: 'order', before, after }],
    order_before: {
      order_ref: '11',
      side: 'buy',
      price4: BID,
      shares: 200,
      queue_position: 2,
      level_orders: 4,
      shares_ahead: 100,
      level_shares: 650,
      entered_ns: '34200000000000',
      entered_event_index: 3,
      age_ns: '1000000',
    },
    order_after: null,
    lifetime_ns: '1000000',
    same_timestamp: {
      ts_ns: '34200001000000',
      position: 1,
      first_event_index: 40,
      events: [{ event_index: 40, message_type: 'D', applied: 'book', level_changes: [] }],
      events_truncated: false,
    },
  };
}

describe('fmtSigned', () => {
  it('signs non-zero share changes with a true minus', () => {
    expect(fmtSigned(1200)).toBe('+1,200');
    expect(fmtSigned(-200)).toBe('\u2212200');
    expect(fmtSigned(0)).toBe('0');
  });
});

describe('mergeQueues', () => {
  it('aligns before and after FIFO rows and names each change', () => {
    const d = deleteDetail();
    const rows = mergeQueues(d.affected[0].before, d.affected[0].after, 40);
    expect(rows.map((r) => (r.kind === 'order' ? `${r.order_ref}:${r.change}` : `gap${r.count}`))).toEqual([
      '10:unchanged',
      '11:removed',
      '12:advanced',
      '13:advanced',
    ]);
  });

  it('marks unlisted orders as gaps instead of inventing them', () => {
    const before = level(BID, [order('1', 100, 1, 0), order('2', 100, 2, 100), order('5', 100, 5, 400)], 6, true);
    const after = level(BID, [order('1', 100, 1, 0), order('2', 40, 2, 100), order('5', 100, 5, 340)], 6, true);
    const rows = mergeQueues(before, after, 9);
    expect(rows.map((r) => (r.kind === 'order' ? `${r.order_ref}:${r.change}` : `gap${r.count}`))).toEqual([
      '1:unchanged',
      '2:reduced',
      'gap2',
      '5:advanced',
      'gap1',
    ]);
  });

  it('distinguishes an order added by this event from one only listed afterwards', () => {
    const before = level(BID, [order('1', 100, 1, 0)], 2, true);
    const after = level(BID, [order('1', 100, 1, 0), order('2', 100, 2, 100, 3), order('9', 100, 3, 200, 7)], 3);
    const rows = mergeQueues(before, after, 7);
    expect(rows.filter((r) => r.kind === 'order').map((r) => (r.kind === 'order' ? r.change : ''))).toEqual(['unchanged', 'advanced', 'added']);
  });
});

describe('depthRows', () => {
  it('orders asks above bids and keeps unknown depth distinct from an empty level', () => {
    // Two levels listed per side; the event removes bid 99.99 and bid 99.97 enters the listing.
    const top = level(BID, [order('1', 100, 1, 0)]);
    const removed = level(999_900, [order('2', 300, 1, 0)]);
    const next = level(999_700, [order('3', 400, 1, 0)]);
    const ask = level(1_000_200, [order('4', 500, 1, 0)]);
    const d: EventDetail = {
      ...deleteDetail(),
      event: event({ order_ref: '2', price4: 999_900, shares: 300 }),
      levels: 2,
      before: { bids: [depth(top), depth(removed)], asks: [depth(ask)] },
      after: { bids: [depth(top), depth(next)], asks: [depth(ask)] },
      affected: [{ side: 'buy', price4: 999_900, role: 'order', before: removed, after: level(999_900, [], 0) }],
    };
    const rows = depthRows(d);
    expect(rows.map((r) => `${r.side}${r.price4}`)).toEqual(['ask1000200', 'bid1000100', 'bid999900', 'bid999700']);
    const removedRow = rows[2];
    expect(removedRow.affected).toBe(true);
    expect(removedRow.before?.shares).toBe(300);
    expect(removedRow.after).toBeNull();
    // 99.97 lay beyond the two levels listed before the event: unknown, not empty.
    expect(rows[3].before).toBeUndefined();
    expect(rows[3].after?.shares).toBe(400);
    expect(depthScale(rows)).toBe(500);
  });
});

describe('observability', () => {
  it('states the L2 total change and the L3 queue facts of a delete', () => {
    const o = observability(deleteDetail());
    expect(o.l2).toEqual(['bid 100.0100: 650 \u2192 450 shares (\u2212200)']);
    expect(o.l3[0]).toBe('Order 11 at position 2 of 4, 100 shares ahead was deleted (200 shares).');
    expect(o.l3[1]).toBe('2 orders behind it each moved up one position and 200 shares.');
    expect(o.l3[2]).toMatch(/^Order 11 rested .* from event #3: a completed lifetime\.$/);
  });

  it('marks a still-resting order as right-censored, not completed', () => {
    const d = deleteDetail();
    const after = level(BID, [order('10', 100, 1, 0), order('11', 150, 2, 100), order('12', 300, 3, 250), order('13', 50, 4, 550)]);
    const o = observability({
      ...d,
      event: event({ message_type: 'X', shares: 50 }),
      affected: [{ ...d.affected[0], after }],
      order_after: { ...d.order_before!, shares: 150 },
      lifetime_ns: null,
    });
    expect(o.l3).toContain('It keeps position 2; 2 orders behind it now have 50 fewer shares ahead.');
    expect(o.l3.some((s) => s.endsWith('its lifetime is right-censored here.'))).toBe(true);
    expect(o.l3.some((s) => s.includes('completed lifetime'))).toBe(false);
  });

  it('claims nothing for a quarantined message', () => {
    const d = deleteDetail();
    const o = observability({ ...d, event: event({ applied: 'quarantined' }), affected: [] });
    expect(o.l2).toEqual(['No displayed depth changed.']);
    expect(o.l3).toEqual(['The message was quarantined: no resting order changed.']);
  });

  it('reports same-timestamp netting from the messages up to this one only', () => {
    const d = deleteDetail();
    const change = (b: number, a: number) => ({ side: 'buy' as const, price4: BID, before_shares: b, after_shares: a, before_orders: 1, after_orders: 1 });
    const o = observability({
      ...d,
      same_timestamp: {
        ts_ns: '34200001000000',
        position: 2,
        first_event_index: 39,
        events: [
          { event_index: 39, message_type: 'A', applied: 'book', level_changes: [change(450, 650)] },
          { event_index: 40, message_type: 'D', applied: 'book', level_changes: [change(650, 450)] },
        ],
        events_truncated: false,
      },
    });
    expect(o.l2).toContain('Once per timestamp, bid 100.0100 shows only the net 0 of 400 gross shares changed by 2 messages.');
    expect(o.l3.some((s) => s.startsWith('Message 2 of those so far at 09:30:00.001'))).toBe(true);
  });

  it('nets per level', () => {
    const c = (price4: number, b: number, a: number) => ({ side: 'sell' as const, price4, before_shares: b, after_shares: a, before_orders: 1, after_orders: 1 });
    expect(sameTimestampNetting([[c(5, 100, 0)], [c(5, 0, 50), c(6, 10, 20)]])).toEqual([
      { side: 'ask', price4: 5, net: -50, gross: 150, messages: 2 },
      { side: 'ask', price4: 6, net: 10, gross: 10, messages: 1 },
    ]);
  });
});
