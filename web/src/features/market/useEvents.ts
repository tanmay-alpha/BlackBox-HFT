// Event loading for the workbench: one bounded timeline page at a time (cursor paging with a
// back stack), the single selected event, and the server-side search for the neighbour of an
// event that is not on the loaded page. Every request is bounded and aborted when superseded.

import { useCallback, useState } from 'react';

import { useAsync, type AsyncResult } from '../../components/ui';
import { marketApi, type EventsPage, type MarketEvent } from './api';
import { MAX_FROM_INDEX } from './model';

/** Events per timeline page (the server allows 1..500). */
export const PAGE_SIZE = 100;

/** Width of each backward window when searching for a previous event. */
const BACK_WINDOW = 200;
/** Backward windows searched before giving up (BACK_WINDOW x BACK_TRIES frames). */
const BACK_TRIES = 25;
/** Cursor pages followed inside one window when the server cut a page by bytes. */
const MAX_FOLLOW = 5;

interface PageRef {
  cursor: string | null;
  /** Lowest event index this page can hold (the filter's `from`, or the previous page's last + 1). */
  start: number;
}

export interface EventPager {
  page: AsyncResult<EventsPage>;
  /** Lowest event index the current page can hold. */
  start: number;
  /** 1-based number of the page within this filter. */
  number: number;
  hasPrevious: boolean;
  next: () => void;
  previous: () => void;
  first: () => void;
}

/** The timeline page for `type` starting at `from`, with Next/Previous/First paging. */
export function useEventPager(datasetId: string, type: string | null, from: number | null, enabled: boolean): EventPager {
  const key = `${datasetId}|${type ?? ''}|${from ?? ''}`;
  const [state, setState] = useState<{ key: string; stack: PageRef[] }>({ key, stack: [{ cursor: null, start: from ?? 0 }] });
  const stack = state.key === key ? state.stack : [{ cursor: null, start: from ?? 0 }];
  const current = stack[stack.length - 1];

  const page = useAsync<EventsPage>(
    (signal) =>
      enabled
        ? marketApi.events(
            datasetId,
            { cursor: current.cursor, limit: PAGE_SIZE, message_type: type, from_event_index: from },
            { signal },
          )
        : new Promise<EventsPage>(() => undefined),
    [datasetId, type, from, current.cursor, enabled],
  );

  const items = page.data?.items ?? [];
  const nextCursor = page.data?.next_cursor ?? null;
  const lastIndex = items.length > 0 ? items[items.length - 1].event_index : null;

  const next = useCallback(() => {
    if (nextCursor === null || lastIndex === null) return;
    setState({ key, stack: [...stack, { cursor: nextCursor, start: lastIndex + 1 }] });
  }, [key, stack, nextCursor, lastIndex]);
  const previous = useCallback(() => {
    if (stack.length > 1) setState({ key, stack: stack.slice(0, -1) });
  }, [key, stack]);
  const first = useCallback(() => setState({ key, stack: stack.slice(0, 1) }), [key, stack]);

  return { page, start: current.start, number: stack.length, hasPrevious: stack.length > 1, next, previous, first };
}

/** The event at exactly `index` (any type), or `null` when that frame belongs to another symbol. */
export function useSelectedEvent(datasetId: string, index: number | null): AsyncResult<MarketEvent | null> {
  return useAsync<MarketEvent | null>(
    async (signal) => {
      if (index === null) return null;
      if (index > MAX_FROM_INDEX) throw new Error(`the events endpoint looks up indices up to ${MAX_FROM_INDEX.toLocaleString('en-US')}`);
      const page = await marketApi.events(datasetId, { from_event_index: index, limit: 1 }, { signal });
      const ev = page.items[0];
      return ev && ev.event_index === index ? ev : null;
    },
    [datasetId, index],
  );
}

/**
 * The nearest event of `type` (any type when `null`) after (`dir` 1) or before (`dir` -1)
 * `selected`, asked of the server; `null` when there is none.
 */
export async function findNeighbour(
  datasetId: string,
  type: string | null,
  selected: number,
  dir: 1 | -1,
  signal: AbortSignal,
): Promise<number | null> {
  if (dir === 1) {
    if (selected + 1 > MAX_FROM_INDEX) {
      throw new Error(`The events endpoint pages from event indices up to ${MAX_FROM_INDEX.toLocaleString('en-US')}; use the timeline pages beyond it.`);
    }
    const page = await marketApi.events(datasetId, { from_event_index: selected + 1, limit: 1, message_type: type }, { signal });
    return page.items[0]?.event_index ?? null;
  }
  let hi = Math.min(selected, MAX_FROM_INDEX + 1);
  for (let tries = 0; hi > 0 && tries < BACK_TRIES; tries += 1) {
    const from = Math.max(0, hi - BACK_WINDOW);
    let cursor: string | null = null;
    let best: number | null = null;
    for (let follow = 0; follow < MAX_FOLLOW; follow += 1) {
      const page: EventsPage = await marketApi.events(
        datasetId,
        { from_event_index: from, limit: BACK_WINDOW, message_type: type, cursor },
        { signal },
      );
      let reachedHi = false;
      for (const e of page.items) {
        if (e.event_index < hi) best = e.event_index;
        else reachedHi = true;
      }
      if (reachedHi || page.next_cursor === null) break;
      cursor = page.next_cursor;
    }
    if (best !== null) return best;
    hi = from;
  }
  if (hi > 0) {
    throw new Error(`No earlier matching event within ${(BACK_WINDOW * BACK_TRIES).toLocaleString('en-US')} frames; enter an event index instead.`);
  }
  return null;
}
