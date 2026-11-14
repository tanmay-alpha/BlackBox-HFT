// Flight-recorder events near the cursor, on both clocks, with source and clock badges.

import type { FlightEvent, InstrumentMeta } from '../../api/types';
import { DataTable } from '../../components/DataTable';
import type { Column } from '../../components/DataTable';
import { fmtInt } from '../../components/fmt';
import { Badge, ClockText } from '../../components/ui';
import { eventSummary, eventTitle } from '../forensics/eventFormat';

const BEFORE = 12;
const AFTER = 24;

/** Index of the last event with `ts <= t` in `events` sorted by `ts`, or -1. */
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

export interface EventLogProps {
  /** Filtered events, sorted by `ts` then `seq`. */
  events: FlightEvent[];
  cursor: number;
  instrument: InstrumentMeta;
  selectedOrder: number | null;
  onSelectOrder: (orderId: number) => void;
  onJump: (ev: FlightEvent) => void;
}

export function EventLog({ events, cursor, instrument, selectedOrder, onSelectOrder, onJump }: EventLogProps) {
  const at = lastEventAtOrBefore(events, cursor);
  const from = Math.max(0, at - BEFORE + 1);
  const to = Math.min(events.length, Math.max(at + 1, from) + AFTER);
  const slice = events.slice(from, to);
  const current = at >= 0 ? events[at] : null;

  const columns: Column<FlightEvent>[] = [
    {
      id: 'time',
      header: 'Time',
      width: 150,
      cell: (ev) => (
        <button type="button" className="btn-link" onClick={() => onJump(ev)} title="Move the cursor here">
          <ClockText ns={ev.ts} />
        </button>
      ),
      raw: (ev) => String(ev.ts),
    },
    {
      id: 'clock',
      header: 'Clock',
      term: 'clocks',
      width: 64,
      cell: (ev) => (
        <Badge kind="neutral" title={`${ev.clock} clock`}>
          {ev.clock === 'exchange' ? 'exch' : 'local'}
        </Badge>
      ),
    },
    {
      id: 'source',
      header: 'Source',
      width: 84,
      cell: (ev) => <Badge kind={ev.source}>{ev.source}</Badge>,
    },
    {
      id: 'order',
      header: 'Order',
      width: 80,
      cell: (ev) =>
        ev.order_id === null ? (
          <span className="dim">run</span>
        ) : (
          <button type="button" className="btn-link mono" onClick={() => onSelectOrder(ev.order_id as number)}>
            {ev.order_id}
          </button>
        ),
      raw: (ev) => (ev.order_id === null ? '' : String(ev.order_id)),
    },
    {
      id: 'event',
      header: 'Event',
      cell: (ev) => (
        <span className="term-ev">
          <span>{eventTitle(ev)}</span> <span className="dim term-ev-summary">{eventSummary(ev, instrument)}</span>
        </span>
      ),
    },
  ];

  return (
    <DataTable
      label="Event log"
      rows={slice}
      rowKey={(ev) => String(ev.seq)}
      columns={columns}
      fill
      selectedKey={current ? String(current.seq) : null}
      onActivate={(ev) => onJump(ev)}
      rowClassName={(ev) =>
        [ev.ts <= cursor ? '' : 'term-future', ev.order_id !== null && ev.order_id === selectedOrder ? 'term-ev-sel' : '']
          .filter(Boolean)
          .join(' ') || undefined
      }
      empty="No events in this window match the filters."
      footer={
        events.length > 0 ? (
          <span className="dim small">
            Showing {fmtInt(to - from)} of {fmtInt(events.length)} matching events around the cursor (rows {fmtInt(from + 1)}
            {'\u2013'}
            {fmtInt(to)}). Times on both clocks share the dataset origin; local and exchange are different clocks, so their
            difference is modeled latency.
          </span>
        ) : null
      }
    />
  );
}
