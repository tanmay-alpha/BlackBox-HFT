// Previous/next order within a run, by `order_id`.
//
// The orders endpoint is paged, so the ids are scanned page by page and remembered per run (a run
// is immutable: its id is a hash of its inputs). When the listing is ascending the scan stops as
// soon as it has passed the current order; if it ever is not, the whole run is scanned so the
// answer never depends on the listing order.

import { api } from "../../api/client";

const PAGE_SIZE = 2000;

interface Scan {
  ids: number[];
  total: number | null;
  ascending: boolean;
}

const scans = new Map<string, Scan>();

export interface Neighbours {
  prev: number | null;
  next: number | null;
  /** 1-based position among the run's orders; null when the listing is not in id order. */
  position: number | null;
  total: number;
}

export async function findNeighbours(runId: string, orderId: number): Promise<Neighbours> {
  let scan = scans.get(runId);
  if (!scan) {
    scan = { ids: [], total: null, ascending: true };
    scans.set(runId, scan);
  }

  for (;;) {
    const complete = scan.total !== null && scan.ids.length >= scan.total;
    const lastId = scan.ids.length > 0 ? (scan.ids[scan.ids.length - 1] as number) : null;
    // In an ascending listing everything not yet fetched is larger than the last id seen.
    const passed = scan.ascending && lastId !== null && lastId > orderId;
    if (complete || passed) break;

    const offset = scan.ids.length;
    const page = await api.orders(runId, { offset, limit: PAGE_SIZE });
    // Another call for the same run appended this page while we were waiting.
    if (scan.ids.length !== offset) continue;
    scan.total = page.orders.length === 0 ? scan.ids.length : page.total;
    for (const order of page.orders) {
      const before = scan.ids.length > 0 ? (scan.ids[scan.ids.length - 1] as number) : null;
      if (before !== null && order.order_id <= before) scan.ascending = false;
      scan.ids.push(order.order_id);
    }
  }

  let prev: number | null = null;
  let next: number | null = null;
  let index = -1;
  // A plain loop: the compiler does not track assignments made inside a callback.
  for (let i = 0; i < scan.ids.length; i++) {
    const id = scan.ids[i] as number;
    if (id === orderId) index = i;
    else if (id < orderId && (prev === null || id > prev)) prev = id;
    else if (id > orderId && (next === null || id < next)) next = id;
  }

  return {
    prev,
    next,
    position: scan.ascending && index >= 0 ? index + 1 : null,
    total: scan.total ?? scan.ids.length,
  };
}
