// Pure shaping of one event's before/after observation (`GET .../events/{index}`): the depth
// ladder rows, the synchronized FIFO queue of each changed level, and the statements of what the
// order-level (L3) view shows beyond the price-level (L2) totals. Every statement is built from
// the server's numbers for this event; nothing is inferred about orders the response does not list.

import { fmtDuration } from '../../components/fmt';
import { MINUS } from '../../components/ns';
import type { BookLevel, BookOrder, DepthLevel, EventDetail, LevelChange } from './api';
import { fmtPrice4, fmtSessionNs, fmtShares } from './model';

export type BookSide = 'bid' | 'ask';

export const sideOf = (side: string): BookSide => (side === 'buy' ? 'bid' : 'ask');

/** Signed whole shares with an explicit sign: `+300`, `−300`, `0`. */
export function fmtSigned(n: number): string {
  if (n === 0) return '0';
  return `${n > 0 ? '+' : MINUS}${fmtShares(Math.abs(n))}`;
}

const plural = (n: number, one: string, many = `${one}s`) => `${fmtShares(n)} ${n === 1 ? one : many}`;

// ----- depth ladder -------------------------------------------------------------------------

/**
 * One price of the ladder. `before`/`after` are `null` when no order rests there at that instant,
 * and `undefined` when the price lies beyond the levels listed for that instant (not known here).
 */
export interface DepthRow {
  side: BookSide;
  price4: number;
  before: DepthLevel | null | undefined;
  after: DepthLevel | null | undefined;
  /** The event changed this level. */
  affected: boolean;
  /** Beyond the listed depth of both instants (shown only because the event changed it). */
  deep: boolean;
}

/** Prefix of a level as drawn: the listed orders while their positions are contiguous from 1. */
export function contiguousPrefix(level: BookLevel): BookOrder[] {
  const out: BookOrder[] = [];
  for (const o of level.orders) {
    if (o.queue_position !== out.length + 1) break;
    out.push(o);
  }
  return out;
}

function slotsOf(level: BookLevel): DepthLevel {
  // Listed rows of an affected level can skip positions; a bar draws only the FIFO prefix.
  const prefix = contiguousPrefix(level);
  return {
    price4: level.price4,
    shares: level.shares,
    order_count: level.order_count,
    orders: prefix.map((o) => ({ order_ref: o.order_ref, shares: o.shares })),
    orders_truncated: prefix.length < level.order_count,
  };
}

function lookup(listed: DepthLevel[], levels: number, side: BookSide, price4: number): DepthLevel | null | undefined {
  const hit = listed.find((l) => l.price4 === price4);
  if (hit) return hit;
  if (listed.length < levels) return null;
  const worst = listed[listed.length - 1].price4;
  const within = side === 'bid' ? price4 > worst : price4 < worst;
  return within ? null : undefined;
}

/**
 * Ladder rows of both sides, asks first from the highest price down to the best ask, then bids
 * from the best bid down: the union of the listed levels before and after the event plus every
 * level the event changed (whose complete totals the response always carries).
 */
export function depthRows(detail: EventDetail): DepthRow[] {
  const out: DepthRow[] = [];
  for (const side of ['ask', 'bid'] as const) {
    const key = side === 'bid' ? 'bids' : 'asks';
    const before = detail.before[key];
    const after = detail.after[key];
    const affected = detail.affected.filter((a) => sideOf(a.side) === side);
    const prices = new Set<number>([...before.map((l) => l.price4), ...after.map((l) => l.price4), ...affected.map((a) => a.price4)]);
    const rows: DepthRow[] = [...prices].map((price4) => {
      const hit = affected.find((a) => a.price4 === price4);
      let b = lookup(before, detail.levels, side, price4);
      let a = lookup(after, detail.levels, side, price4);
      if (hit) {
        b = hit.before.order_count === 0 ? null : (before.find((l) => l.price4 === price4) ?? slotsOf(hit.before));
        a = hit.after.order_count === 0 ? null : (after.find((l) => l.price4 === price4) ?? slotsOf(hit.after));
      }
      const listed = before.some((l) => l.price4 === price4) || after.some((l) => l.price4 === price4);
      const inRange = lookup(before, detail.levels, side, price4) !== undefined || lookup(after, detail.levels, side, price4) !== undefined;
      return { side, price4, before: b, after: a, affected: hit !== undefined, deep: !listed && !inRange };
    });
    rows.sort((x, y) => y.price4 - x.price4);
    out.push(...rows);
  }
  return out;
}

/** Largest level total among the rows (before or after), the common scale of every queue bar. */
export function depthScale(rows: readonly DepthRow[]): number {
  let max = 0;
  for (const r of rows) max = Math.max(max, r.before?.shares ?? 0, r.after?.shares ?? 0);
  return max;
}

// ----- synchronized queue -------------------------------------------------------------------

export type QueueChange = 'unchanged' | 'removed' | 'added' | 'reduced' | 'increased' | 'advanced';

export type QueueRow =
  | { kind: 'order'; order_ref: string; before: BookOrder | null; after: BookOrder | null; change: QueueChange }
  /** Orders the response does not list (between listed rows, or after the last one). */
  | { kind: 'gap'; count: number };

function changeOf(b: BookOrder | null, a: BookOrder | null, eventIndex: number): QueueChange {
  if (b && !a) return 'removed';
  if (!b && a) return a.entered_event_index === eventIndex ? 'added' : 'advanced';
  if (!b || !a) return 'unchanged';
  if (a.shares < b.shares) return 'reduced';
  if (a.shares > b.shares) return 'increased';
  if (a.queue_position !== b.queue_position || a.shares_ahead !== b.shares_ahead) return 'advanced';
  return 'unchanged';
}

/**
 * The before and after FIFO queues of one level as aligned rows: every listed order once, in its
 * before order, then orders listed only afterwards (an add or replacement joins at the back).
 */
export function mergeQueues(before: BookLevel, after: BookLevel, eventIndex: number): QueueRow[] {
  const afterBy = new Map(after.orders.map((o) => [o.order_ref, o]));
  const seen = new Set<string>();
  const rows: QueueRow[] = [];
  let lastB = 0;
  let lastA = 0;
  const push = (ref: string, b: BookOrder | null, a: BookOrder | null) => {
    const gap = Math.max(b ? b.queue_position - lastB - 1 : 0, a ? a.queue_position - lastA - 1 : 0);
    if (gap > 0) rows.push({ kind: 'gap', count: gap });
    if (b) lastB = b.queue_position;
    if (a) lastA = a.queue_position;
    rows.push({ kind: 'order', order_ref: ref, before: b, after: a, change: changeOf(b, a, eventIndex) });
  };
  for (const b of before.orders) {
    seen.add(b.order_ref);
    push(b.order_ref, b, afterBy.get(b.order_ref) ?? null);
  }
  for (const a of after.orders) if (!seen.has(a.order_ref)) push(a.order_ref, null, a);
  const tail = Math.max(before.order_count - lastB, after.order_count - lastA);
  if (tail > 0) rows.push({ kind: 'gap', count: tail });
  return rows;
}

// ----- what each view shows -----------------------------------------------------------------

export interface Observability {
  /** What a price-level feed shows for this event. */
  l2: string[];
  /** What only the order-level view shows. */
  l3: string[];
}

function levelText(side: BookSide, price4: number): string {
  return `${side} ${fmtPrice4(price4)}`;
}

function l2Line(side: BookSide, price4: number, b: number, a: number, bo: number, ao: number): string {
  const what = bo === 0 ? ' (new level)' : ao === 0 ? ' (level removed)' : '';
  return `${levelText(side, price4)}: ${fmtShares(b)} \u2192 ${fmtShares(a)} shares (${fmtSigned(a - b)})${what}`;
}

function at(o: { queue_position: number; level_orders: number; shares_ahead: number }): string {
  return `position ${fmtShares(o.queue_position)} of ${fmtShares(o.level_orders)}, ${fmtShares(o.shares_ahead)} shares ahead`;
}

/** Net and gross depth change per level over the same-timestamp messages up to this event. */
export function sameTimestampNetting(changes: readonly LevelChange[][]): { side: BookSide; price4: number; net: number; gross: number; messages: number }[] {
  const by = new Map<string, { side: BookSide; price4: number; net: number; gross: number; messages: number }>();
  for (const list of changes) {
    for (const c of list) {
      const key = `${c.side}|${c.price4}`;
      const d = c.after_shares - c.before_shares;
      const cur = by.get(key) ?? { side: sideOf(c.side), price4: c.price4, net: 0, gross: 0, messages: 0 };
      cur.net += d;
      cur.gross += Math.abs(d);
      cur.messages += 1;
      by.set(key, cur);
    }
  }
  return [...by.values()];
}

/**
 * The L2 and L3 observations of one event, each a short statement computed from this response.
 * L2: per changed level, the total before and after. L3: the order identity, its queue position
 * and shares ahead, what happened to the orders behind it, its lifetime, and the source order of
 * same-timestamp messages, none of which level totals carry.
 */
export function observability(d: EventDetail): Observability {
  const l2: string[] = [];
  const l3: string[] = [];
  const e = d.event;
  for (const lv of d.affected) {
    l2.push(l2Line(sideOf(lv.side), lv.price4, lv.before.shares, lv.after.shares, lv.before.order_count, lv.after.order_count));
  }
  if (d.affected.length === 0) {
    l2.push(e.applied === 'print' ? 'A print; no displayed depth changed.' : 'No displayed depth changed.');
  }
  const ob = d.order_before;
  const oa = d.order_after;
  const qty = e.shares ?? 0;
  if (e.applied !== 'book') {
    l3.push(`The message was ${e.applied.replace(/_/g, ' ')}: no resting order changed.`);
    return { l2, l3 };
  }
  switch (e.message_type) {
    case 'A':
    case 'F':
      if (oa) l3.push(`New order ${oa.order_ref} (${fmtShares(oa.shares)} shares) joined at ${at(oa)}.`);
      break;
    case 'X':
      if (ob) {
        l3.push(`Order ${ob.order_ref} at ${at(ob)} cancelled ${fmtShares(qty)} of its ${fmtShares(ob.shares)} shares.`);
        const behind = ob.level_orders - ob.queue_position;
        if (oa) l3.push(`It keeps position ${fmtShares(oa.queue_position)}; ${plural(behind, 'order')} behind it now have ${fmtShares(qty)} fewer shares ahead.`);
        else if (behind > 0) l3.push(`The cancel removed the order; ${plural(behind, 'order')} behind it each moved up one position.`);
      }
      break;
    case 'D':
      if (ob) {
        l3.push(`Order ${ob.order_ref} at ${at(ob)} was deleted (${fmtShares(ob.shares)} shares).`);
        const behind = ob.level_orders - ob.queue_position;
        if (behind > 0) l3.push(`${plural(behind, 'order')} behind it each moved up one position and ${fmtShares(ob.shares)} shares.`);
      }
      break;
    case 'E':
    case 'C':
      if (ob) {
        l3.push(`${fmtShares(qty)} shares executed against order ${ob.order_ref} at ${at(ob)}.`);
        l3.push(oa ? `${fmtShares(oa.shares)} shares of it remain at position ${fmtShares(oa.queue_position)}.` : 'The execution removed the order.');
      }
      break;
    case 'U':
      if (ob) {
        const to = oa ? ` at ${fmtPrice4(oa.price4)}, now ${at(oa)}` : '';
        l3.push(`Order ${ob.order_ref} (${fmtPrice4(ob.price4)}, ${at(ob)}) was replaced by new order ${e.new_order_ref ?? '\u2014'}${to}.`);
      }
      break;
    default:
      break;
  }
  if (d.lifetime_ns !== null && ob) {
    l3.push(`Order ${ob.order_ref} rested ${fmtDuration(d.lifetime_ns)} from event #${fmtShares(ob.entered_event_index)}: a completed lifetime.`);
  } else if (oa && oa.age_ns !== null && oa.entered_event_index !== e.event_index) {
    l3.push(`Order ${oa.order_ref} has rested ${fmtDuration(oa.age_ns)} so far; its lifetime is right-censored here.`);
  }
  const st = d.same_timestamp;
  if (st.position > 1) {
    l3.push(
      `Message ${fmtShares(st.position)} of those so far at ${fmtSessionNs(st.ts_ns)}; L3 keeps their source order (events #${fmtShares(st.first_event_index)} to #${fmtShares(e.event_index)}).`,
    );
    for (const n of sameTimestampNetting(st.events.map((x) => x.level_changes))) {
      if (n.messages > 1 && n.gross !== Math.abs(n.net)) {
        l2.push(`Once per timestamp, ${levelText(n.side, n.price4)} shows only the net ${fmtSigned(n.net)} of ${fmtShares(n.gross)} gross shares changed by ${n.messages} messages.`);
      }
    }
  }
  if (l3.length === 0) l3.push('This message names no resting order.');
  return { l2, l3 };
}

/** `price` with the side prefix for headings: `bid 100.0100`. */
export const levelLabel = (side: string, price4: number) => levelText(sideOf(side), price4);
