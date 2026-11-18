// Loads one replay window's run timeline completely: `GET /runs/{id}/timeline` pages its events
// by tape `seq` (`next_cursor`), and only the first page carries fills, equity and orders. A
// window is returned only when every page arrived and the events add up to `events_total`, so
// a partial window is never shown as complete.

import type { RunTimeline } from '../../api/model';
import type { FlightEvent } from '../../api/types';

/**
 * Index of the last event with `ts <= t` in `events` sorted by `ts`, or -1. Lives here, not in
 * EventLog, so pages that only search a timeline (Explore) do not load the table code.
 */
export function lastEventAtOrBefore(events: FlightEvent[], t: number): number {
  let lo = 0;
  let hi = events.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (events[mid].ts <= t) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}

/** The first event after `t` (forward) or last before it (backward), on the sorted time axis. */
export function findStep(events: FlightEvent[], t: number, dir: 1 | -1): FlightEvent | null {
  let i = lastEventAtOrBefore(events, t);
  if (dir === 1) return events[i + 1] ?? null;
  while (i >= 0 && events[i].ts >= t) i -= 1;
  return i >= 0 ? events[i] : null;
}

export interface TimelineWindowRange {
  t0: number;
  t1: number;
}

export interface TimelineProgress {
  /** Events received so far. */
  loaded: number;
  /** Events in the whole window (`events_total`). */
  total: number;
}

export type FetchTimelinePage = (cursor: string | null, signal: AbortSignal) => Promise<RunTimeline>;

/** Upper bound on pages per window; a server that never ends a window is an error, not a hang. */
const MAX_PAGES = 1000;

/** Checks that `page` continues `first` (same run, window and total) after the last seq `lastSeq`. */
function checkPage(first: RunTimeline, page: RunTimeline, lastSeq: number, index: number): void {
  const same =
    page.run_id === first.run_id &&
    page.dataset_id === first.dataset_id &&
    page.t0 === first.t0 &&
    page.t1 === first.t1 &&
    page.events_total === first.events_total;
  if (!same) throw new Error(`timeline page ${index} belongs to a different run or window`);
  let prev = lastSeq;
  for (const e of page.events) {
    if (e.seq <= prev) throw new Error(`timeline page ${index} repeats or reorders tape seq ${e.seq}`);
    prev = e.seq;
  }
}

/**
 * Merges the pages of one window (first page first) into a complete timeline. Throws when the
 * pages are of different windows, overlap, end with a cursor or do not add up to `events_total`.
 */
export function mergeTimelinePages(pages: readonly RunTimeline[]): RunTimeline {
  const first = pages[0];
  if (first === undefined) throw new Error('no timeline pages');
  const events: RunTimeline['events'] = [];
  let lastSeq = -1;
  pages.forEach((page, i) => {
    checkPage(first, page, lastSeq, i);
    events.push(...page.events);
    if (page.events.length > 0) lastSeq = page.events[page.events.length - 1].seq;
  });
  const last = pages[pages.length - 1];
  if (last.next_cursor !== null) throw new Error('the timeline window is incomplete: the last page has a cursor');
  if (events.length !== first.events_total) {
    throw new Error(`the timeline window is incomplete: ${events.length} of ${first.events_total} events`);
  }
  return { ...first, events, events_truncated: false, truncated_by: null, next_cursor: null };
}

/**
 * Fetches every page of the window `range` one after another, reporting progress after each,
 * and returns the merged window. Rejects with an `AbortError` once `signal` aborts (no further
 * page is requested and nothing is returned), and when a page is not of `range`.
 */
export async function fetchTimelineWindow(
  range: TimelineWindowRange,
  fetchPage: FetchTimelinePage,
  signal: AbortSignal,
  onProgress?: (p: TimelineProgress) => void,
): Promise<RunTimeline> {
  const pages: RunTimeline[] = [];
  let cursor: string | null = null;
  let loaded = 0;
  for (;;) {
    if (signal.aborted) throw new DOMException('The window load was aborted', 'AbortError');
    const page = await fetchPage(cursor, signal);
    if (signal.aborted) throw new DOMException('The window load was aborted', 'AbortError');
    if (page.t0 !== range.t0 || page.t1 !== range.t1) {
      throw new Error(`timeline page for [${page.t0}, ${page.t1}] while loading [${range.t0}, ${range.t1}]`);
    }
    // Only the first page may end before its first event (its fills/equity/orders took the room).
    if (pages.length > 0 && page.events.length === 0 && page.next_cursor !== null) {
      throw new Error('a timeline page made no progress');
    }
    pages.push(page);
    loaded += page.events.length;
    onProgress?.({ loaded, total: pages[0].events_total });
    if (page.next_cursor === null) return mergeTimelinePages(pages);
    if (pages.length >= MAX_PAGES) throw new Error(`the timeline window needs more than ${MAX_PAGES} pages`);
    cursor = page.next_cursor;
  }
}
