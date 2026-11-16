// One-line description of an event: index, type, time, side, size, price, order and outcome.

import { fmtInt } from '../../components/fmt';
import { Badge } from '../../components/ui';
import type { MarketEvent } from './api';
import { fmtPrice4, fmtSessionNs, fmtShares, itchTypeName } from './model';

export function EventFacts({ index, event, failure }: { index: number; event: MarketEvent | null | undefined; failure?: string | null }) {
  if (!event) {
    return (
      <p className="mk-event-facts">
        <strong className="mono">#{fmtInt(index)}</strong>{' '}
        <span className="dim">
          {failure
            ? `event details unavailable: ${failure}`
            : event === undefined
              ? 'loading event details'
              : 'not an event of this symbol (the book is shown as of this source frame)'}
        </span>
      </p>
    );
  }
  return (
    <p className="mk-event-facts">
      <strong className="mono">#{fmtInt(event.event_index)}</strong> <span className="mono">{event.message_type}</span>{' '}
      {itchTypeName(event.message_type)} {'\u00b7'} <span className="mono">{fmtSessionNs(event.ts_ns)}</span>
      {event.side ? (
        <>
          {' '}
          {'\u00b7'} <span className={event.side === 'buy' ? 'bid' : 'ask'}>{event.side}</span>
        </>
      ) : null}
      {event.shares !== null ? (
        <>
          {' '}
          <span className="mono">{fmtShares(event.shares)}</span>
        </>
      ) : null}
      {event.price4 !== null ? (
        <>
          {' '}
          @ <span className="mono">{fmtPrice4(event.price4)}</span>
        </>
      ) : null}
      {event.order_ref !== null ? (
        <>
          {' '}
          {'\u00b7'} ref <span className="mono">{event.order_ref}</span>
          {event.new_order_ref !== null ? (
            <>
              {' '}
              {'\u2192'} <span className="mono">{event.new_order_ref}</span>
            </>
          ) : null}
        </>
      ) : null}{' '}
      <Badge kind={event.applied === 'quarantined' || event.applied === 'invalid' ? 'error' : 'neutral'} title={event.applied}>
        {event.applied.replace(/_/g, ' ')}
      </Badge>
    </p>
  );
}
