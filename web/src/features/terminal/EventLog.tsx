// Flight-recorder events near the cursor, on both clocks, with source and clock badges.

import type { FlightEvent, InstrumentMeta } from '../../api/types';
import { Badge, ClockText, Empty } from '../../components/ui';
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
  if (events.length === 0) {
    return <Empty>No events in this window match the filters.</Empty>;
  }
  const at = lastEventAtOrBefore(events, cursor);
  const from = Math.max(0, at - BEFORE + 1);
  const to = Math.min(events.length, Math.max(at + 1, from) + AFTER);
  const slice = events.slice(from, to);
  return (
    <div className="col">
      <p className="note">
        Showing {to - from} of {events.length} matching events around the cursor (rows {from + 1}{'\u2013'}
        {to}). Times on both clocks share the dataset origin; local and exchange are different
        clocks, so their difference is modeled latency.
      </p>
      <div className="table-scroll term-events">
        <table className="table">
          <thead>
            <tr>
              <th scope="col">Time</th>
              <th scope="col">Clock</th>
              <th scope="col">Source</th>
              <th scope="col">Order</th>
              <th scope="col">Event</th>
            </tr>
          </thead>
          <tbody>
            {slice.map((ev, i) => {
              const idx = from + i;
              const past = ev.ts <= cursor;
              const current = idx === at;
              const cls = [
                current ? 'selected' : '',
                past ? '' : 'term-future',
                ev.order_id !== null && ev.order_id === selectedOrder ? 'term-ev-sel' : '',
              ]
                .filter(Boolean)
                .join(' ');
              return (
                <tr key={ev.seq} className={cls || undefined} aria-current={current ? 'true' : undefined}>
                  <td className="nowrap">
                    <button type="button" className="btn-link" onClick={() => onJump(ev)} title="Move the cursor here">
                      <ClockText ns={ev.ts} />
                    </button>
                  </td>
                  <td>
                    <Badge kind="neutral" title={`${ev.clock} clock`}>
                      {ev.clock === 'exchange' ? 'exch' : 'local'}
                    </Badge>
                  </td>
                  <td>
                    <Badge kind={ev.source}>{ev.source}</Badge>
                  </td>
                  <td className="mono">
                    {ev.order_id === null ? (
                      <span className="dim">run</span>
                    ) : (
                      <button type="button" className="btn-link mono" onClick={() => onSelectOrder(ev.order_id as number)}>
                        {ev.order_id}
                      </button>
                    )}
                  </td>
                  <td className="wrap">
                    <span>{eventTitle(ev)}</span>
                    <div className="dim small term-ev-summary">{eventSummary(ev, instrument)}</div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
