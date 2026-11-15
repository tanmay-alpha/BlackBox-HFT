import { describe, expect, it } from 'vitest';

import type { RunTimeline } from '../../api/model';
import type { FlightEvent } from '../../api/types';
import { fetchTimelineWindow, findStep, mergeTimelinePages } from './timelinePaging';
import type { TimelineProgress } from './timelinePaging';

const RANGE = { t0: 100, t1: 200 };

function event(seq: number, ts: number): FlightEvent {
  return { seq, ts, clock: 'exchange', source: 'engine', order_id: null, body: { type: 'request_sent' } } as unknown as FlightEvent;
}

function page(seqs: number[], next: string | null, extra: Partial<RunTimeline> = {}): RunTimeline {
  return {
    run_id: 'r',
    dataset_id: 'd',
    t0: RANGE.t0,
    t1: RANGE.t1,
    // Identical timestamps in pairs, so page boundaries fall inside equal-ts groups.
    events: seqs.map((s) => event(s, RANGE.t0 + Math.floor(s / 2))),
    events_total: 5,
    events_truncated: next !== null,
    fills: [],
    equity: [],
    orders: [],
    run_start: 0,
    run_end: 1000,
    truncated_by: next === null ? null : 'limit',
    next_cursor: next,
    ...extra,
  };
}

/** A fake server: `pages[cursor]` answers the page after `cursor` (`''` = first page). */
function server(pages: Record<string, RunTimeline>) {
  const calls: (string | null)[] = [];
  const fetchPage = (cursor: string | null) => {
    calls.push(cursor);
    const p = pages[cursor ?? ''];
    return p ? Promise.resolve(p) : Promise.reject(new Error(`no page for ${cursor}`));
  };
  return { calls, fetchPage };
}

describe('mergeTimelinePages', () => {
  it('concatenates events in seq order and keeps the first page context', () => {
    const merged = mergeTimelinePages([
      page([0, 1], 'a', { fills: [{ seq: 9 }] as never }),
      page([2, 3], 'b'),
      page([4], null),
    ]);
    expect(merged.events.map((e) => e.seq)).toEqual([0, 1, 2, 3, 4]);
    expect(merged.fills).toHaveLength(1);
    expect([merged.events_truncated, merged.truncated_by, merged.next_cursor]).toEqual([false, null, null]);
  });

  it('refuses incomplete, overlapping or mixed pages', () => {
    expect(() => mergeTimelinePages([])).toThrow();
    expect(() => mergeTimelinePages([page([0, 1], 'a')])).toThrow(/incomplete/);
    expect(() => mergeTimelinePages([page([0, 1], 'a'), page([2, 3], null)])).toThrow(/4 of 5/);
    expect(() => mergeTimelinePages([page([0, 1, 2], 'a'), page([2, 3, 4], null)])).toThrow(/repeats/);
    expect(() => mergeTimelinePages([page([0, 1], 'a'), page([2, 3, 4], null, { t1: 300 })])).toThrow(/different/);
    expect(() => mergeTimelinePages([page([0, 1], 'a'), page([2, 3, 4], null, { run_id: 'x' })])).toThrow(/different/);
  });
});

describe('fetchTimelineWindow', () => {
  it('follows next_cursor to the end, sequentially, reporting progress', async () => {
    const s = server({ '': page([0, 1], 'a'), a: page([2, 3], 'b'), b: page([4], null) });
    const progress: TimelineProgress[] = [];
    const tl = await fetchTimelineWindow(RANGE, s.fetchPage, new AbortController().signal, (p) => progress.push(p));
    expect(s.calls).toEqual([null, 'a', 'b']);
    expect(tl.events.map((e) => e.seq)).toEqual([0, 1, 2, 3, 4]);
    expect(progress).toEqual([
      { loaded: 2, total: 5 },
      { loaded: 4, total: 5 },
      { loaded: 5, total: 5 },
    ]);
  });

  it('accepts a first page without events (the context took its room)', async () => {
    const s = server({ '': page([], 'a'), a: page([0, 1, 2, 3, 4], null) });
    const tl = await fetchTimelineWindow(RANGE, s.fetchPage, new AbortController().signal);
    expect(tl.events).toHaveLength(5);
  });

  it('rejects a later page that makes no progress', async () => {
    const s = server({ '': page([0, 1], 'a'), a: page([], 'a') });
    await expect(fetchTimelineWindow(RANGE, s.fetchPage, new AbortController().signal)).rejects.toThrow(/no progress/);
  });

  it('rejects a page of another window', async () => {
    const s = server({ '': page([0, 1, 2, 3, 4], null, { t0: 0 }) });
    await expect(fetchTimelineWindow(RANGE, s.fetchPage, new AbortController().signal)).rejects.toThrow(/while loading/);
  });

  it('stops requesting pages once aborted and never resolves a partial window', async () => {
    const ctl = new AbortController();
    const calls: (string | null)[] = [];
    const fetchPage = (cursor: string | null) => {
      calls.push(cursor);
      if (cursor === null) {
        ctl.abort(); // a seek while the first page is in flight
        return Promise.resolve(page([0, 1], 'a'));
      }
      return Promise.resolve(page([2, 3, 4], null));
    };
    const progress: TimelineProgress[] = [];
    await expect(fetchTimelineWindow(RANGE, fetchPage, ctl.signal, (p) => progress.push(p))).rejects.toMatchObject({ name: 'AbortError' });
    expect(calls).toEqual([null]);
    expect(progress).toEqual([]);
  });

  it('rejects at once when the signal is already aborted', async () => {
    const ctl = new AbortController();
    ctl.abort();
    const s = server({ '': page([0, 1, 2, 3, 4], null) });
    await expect(fetchTimelineWindow(RANGE, s.fetchPage, ctl.signal)).rejects.toMatchObject({ name: 'AbortError' });
    expect(s.calls).toEqual([]);
  });
});

describe('findStep', () => {
  const events = [event(0, 100), event(1, 101), event(2, 101), event(3, 102)];

  it('reaches the first and last event and stops at both ends', () => {
    expect(findStep(events, 99, 1)).toBe(events[0]);
    expect(findStep(events, 101, 1)).toBe(events[3]);
    expect(findStep(events, 102, 1)).toBeNull();
    expect(findStep(events, 103, -1)).toBe(events[3]);
    expect(findStep(events, 101, -1)).toBe(events[0]);
    expect(findStep(events, 100, -1)).toBeNull();
    expect(findStep([], 100, 1)).toBeNull();
    expect(findStep([], 100, -1)).toBeNull();
  });

  it('steps across an equal-timestamp page boundary after merging the whole window', () => {
    const merged = mergeTimelinePages([
      page([0, 1, 2], 'a'),
      page([3, 4], null),
    ]);
    expect(findStep(merged.events, 100, 1)?.seq).toBe(2);
    expect(findStep(merged.events, 101, 1)?.seq).toBe(4);
    expect(findStep(merged.events, 102, -1)?.seq).toBe(3);
    expect(findStep(merged.events, 101, -1)?.seq).toBe(1);
  });
});
