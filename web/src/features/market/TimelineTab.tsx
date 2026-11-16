// Step 4, Event timeline: the source's events in exact source order, one bounded page at a time,
// filtered by message type. Selecting a row selects that event for the whole workbench.

import { useState } from 'react';
import type { FormEvent, ReactNode } from 'react';

import { DataTable, type Column } from '../../components/DataTable';
import { fmtInt } from '../../components/fmt';
import { PaneTabs } from '../../components/PaneTabs';
import { SplitPane } from '../../components/SplitPane';
import { Badge, ErrorBox, Panel, Spinner } from '../../components/ui';
import type { MarketEvent, MarketSummary } from './api';
import { fmtPrice4, fmtSessionNs, fmtShares, itchTypeName, MAX_FROM_INDEX, topOfBook, typeCountRows } from './model';
import { PAGE_SIZE, type EventPager } from './useEvents';

const COLUMNS: Column<MarketEvent>[] = [
  { id: 'idx', header: 'Event', numeric: true, rowHeader: true, cell: (e) => `#${fmtInt(e.event_index)}` },
  {
    id: 'type',
    header: 'Type',
    cell: (e) => (
      <span title={itchTypeName(e.message_type)}>
        <span className="mono">{e.message_type}</span> {itchTypeName(e.message_type)}
      </span>
    ),
  },
  { id: 'ts', header: 'Session time', numeric: true, cell: (e) => fmtSessionNs(e.ts_ns) },
  { id: 'side', header: 'Side', cell: (e) => (e.side === null ? '\u2014' : <span className={e.side === 'buy' ? 'bid' : 'ask'}>{e.side}</span>) },
  { id: 'price', header: 'Price', numeric: true, cell: (e) => fmtPrice4(e.price4) },
  { id: 'shares', header: 'Shares', numeric: true, cell: (e) => fmtShares(e.shares) },
  {
    id: 'ref',
    header: 'Order ref',
    numeric: true,
    cell: (e) => (e.order_ref === null ? '\u2014' : e.new_order_ref === null ? e.order_ref : `${e.order_ref} \u2192 ${e.new_order_ref}`),
  },
  { id: 'bid', header: 'Best bid after', numeric: true, cell: (e) => fmtPrice4(e.best_bid_price4) },
  { id: 'ask', header: 'Best ask after', numeric: true, cell: (e) => fmtPrice4(e.best_ask_price4) },
  {
    id: 'applied',
    header: 'Outcome',
    cell: (e) => (e.applied === 'quarantined' || e.applied === 'invalid' ? <Badge kind="error">{e.applied}</Badge> : <span title={e.applied}>{e.applied.replace(/_/g, ' ')}</span>),
  },
];

function StartAt({ from, onFrom }: { from: number | null; onFrom: (from: number | null) => void }) {
  const [text, setText] = useState(from === null ? '' : String(from));
  const value = text.trim() === '' ? null : Number(text);
  const invalid = value !== null && (!Number.isSafeInteger(value) || value < 0 || value > MAX_FROM_INDEX);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!invalid) onFrom(value);
  };
  return (
    <form className="row mk-inline-form" onSubmit={submit}>
      <label htmlFor="mk-from">Start at event</label>
      <input
        id="mk-from"
        className="mk-index-input"
        inputMode="numeric"
        value={text}
        placeholder="0"
        aria-invalid={invalid || undefined}
        aria-describedby="mk-from-help"
        onChange={(e) => setText(e.target.value)}
      />
      <button type="submit" disabled={invalid}>
        Show
      </button>
      <span id="mk-from-help" className="visually-hidden">
        Event index from 0 to {fmtInt(MAX_FROM_INDEX)}; empty starts at the first event.
      </span>
    </form>
  );
}

export function TimelineTab({
  summary,
  pager,
  type,
  from,
  selected,
  selectedEvent,
  onSelect,
  onType,
  onFrom,
  phone,
  bookLink,
}: {
  summary: MarketSummary;
  pager: EventPager;
  type: string | null;
  from: number | null;
  selected: number | null;
  selectedEvent: ReactNode;
  onSelect: (index: number) => void;
  onType: (type: string | null) => void;
  onFrom: (from: number | null) => void;
  phone: boolean;
  bookLink: ReactNode;
}) {
  const [pane, setPane] = useState<'events' | 'selected'>('events');
  const types = typeCountRows(summary.messages_by_type).filter((r) => r.count > 0);
  const page = pager.page;
  const items = page.data?.items ?? [];
  const first = items[0]?.event_index ?? null;
  const last = items.length > 0 ? items[items.length - 1].event_index : null;

  const filters = (
    <div className="row mk-filters">
      <label htmlFor="mk-type">Type</label>
      <select id="mk-type" value={type ?? ''} onChange={(e) => onType(e.target.value === '' ? null : e.target.value)}>
        <option value="">All types</option>
        {types.map((t) => (
          <option key={t.type} value={t.type}>
            {t.type} {'\u2014'} {t.name} ({fmtInt(t.count)})
          </option>
        ))}
      </select>
      <StartAt key={from ?? -1} from={from} onFrom={onFrom} />
    </div>
  );

  const footer = (
    <div className="row mk-pager">
      <span>
        {first === null || last === null ? 'no events on this page' : `events #${fmtInt(first)}\u2013#${fmtInt(last)} \u00b7 ${fmtInt(items.length)} shown`}
        {' \u00b7 '}page {fmtInt(pager.number)} of at most {fmtInt(PAGE_SIZE)} rows
        {page.data?.truncated_by === 'bytes' ? ' \u00b7 cut short by the response size cap' : ''}
        {page.data && page.data.next_cursor === null ? ' \u00b7 last page' : ''}
      </span>
      <button type="button" disabled={!pager.hasPrevious} onClick={pager.first}>
        First
      </button>
      <button type="button" disabled={!pager.hasPrevious} onClick={pager.previous}>
        Previous page
      </button>
      <button type="button" disabled={!page.data?.next_cursor} onClick={pager.next}>
        Next page
      </button>
    </div>
  );

  let table: ReactNode;
  if (page.error) table = <ErrorBox error={page.error} onRetry={page.reload} />;
  else if (!page.data) table = <Spinner label="Loading events" />;
  else
    table = (
      <DataTable
        label="Events in source order"
        columns={COLUMNS}
        rows={items}
        rowKey={(e) => String(e.event_index)}
        selectedKey={selected === null ? null : String(selected)}
        onSelect={(e) => onSelect(e.event_index)}
        onActivate={(e) => onSelect(e.event_index)}
        fill
        footer={footer}
        empty={type === null ? 'No events from this index on.' : `No ${type} (${itchTypeName(type)}) events from this index on.`}
      />
    );

  const events = (
    <Panel
      title="Events"
      basis="observed"
      fill
      flush
      caption={`Source order (event index = frame sequence; gaps are other symbols' frames). ${pager.start > 0 ? `Starting at #${fmtInt(pager.start)}. ` : ''}Space or click selects; J/K step through events.`}
    >
      <div className="mk-events-body">
        {filters}
        <div className="mk-events-table">{table}</div>
      </div>
    </Panel>
  );

  const detail = (
    <Panel title="Selected event" basis="observed" fill>
      {selected === null ? (
        <p className="note">Select an event to see it here and in the order book.</p>
      ) : (
        <div className="col">
          {selectedEvent}
          {bookLink}
        </div>
      )}
    </Panel>
  );

  if (phone) {
    return (
      <PaneTabs<'events' | 'selected'>
        label="Timeline panes"
        active={pane}
        onChange={setPane}
        tabs={[
          { id: 'events', label: 'Events', content: events },
          { id: 'selected', label: 'Selected', content: detail },
        ]}
      />
    );
  }
  return (
    <SplitPane
      id="market.timeline"
      panes={[
        { id: 'events', label: 'Events', defaultSize: '70%', minSize: '45%', stackHeight: 'max(420px, 70dvh)', content: events },
        { id: 'selected', label: 'Selected event', defaultSize: '30%', minSize: 240, stackHeight: 'auto', content: detail },
      ]}
    />
  );
}

/** The touch after an event, as the event record carries it. */
export function EventTouch({ event }: { event: MarketEvent }) {
  const top = topOfBook(event.best_bid_price4, event.best_ask_price4);
  return (
    <dl className="kv">
      <dt>Best bid after</dt>
      <dd className="mono bid">{fmtPrice4(top.bid)}</dd>
      <dt>Best ask after</dt>
      <dd className="mono ask">{fmtPrice4(top.ask)}</dd>
      <dt>Spread after</dt>
      <dd className="mono">
        {top.spread4 === null ? '\u2014' : fmtPrice4(top.spread4)} <span className="dim">{top.state}</span>
      </dd>
    </dl>
  );
}
