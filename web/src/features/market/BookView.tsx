// Step 3, Order book at the selected event: the L3 book (every resting order per price level in
// FIFO order) beside the L2 aggregate of the same instant, which is all a market-by-price feed
// shows. Both come from one server response, so they always describe the same event.

import { useState } from 'react';
import type { ReactNode } from 'react';

import { fmtDuration, fmtInt } from '../../components/fmt';
import { PaneTabs } from '../../components/PaneTabs';
import { SplitPane } from '../../components/SplitPane';
import { Badge, Empty, ErrorBox, Notice, Panel, Spinner, useAsync } from '../../components/ui';
import { marketApi, type BookLevel, type MarketBook, type MarketEvent } from './api';
import { EventFacts } from './EventFacts';
import { fmtPrice4, fmtSessionNs, fmtShares, listedOf, nsBetween, topOfBook } from './model';

const LEVEL_CHOICES = [5, 10, 20];
const ORDER_CHOICES = [20, 50, 100];

type Side = 'bid' | 'ask';

function L3Side({ side, levels, book, highlight }: { side: Side; levels: BookLevel[]; book: MarketBook; highlight: ReadonlySet<string> }) {
  const name = side === 'bid' ? 'Bids' : 'Asks';
  if (levels.length === 0) return <p className="note">No resting {name.toLowerCase()} at this event.</p>;
  return (
    <div className="table-scroll">
      <table className="table mk-l3">
        <caption>
          {name}, best price first; orders within a level in FIFO (arrival) order
        </caption>
        <thead>
          <tr>
            <th scope="col" className="num">
              Queue #
            </th>
            <th scope="col">Order ref</th>
            <th scope="col" className="num">
              Shares
            </th>
            <th scope="col" className="num">
              Shares ahead
            </th>
            <th scope="col" className="num">
              Age
            </th>
            <th scope="col" className="num">
              Entered at
            </th>
          </tr>
        </thead>
        {levels.map((lv) => {
          const listed = listedOf(lv);
          return (
            <tbody key={lv.price4}>
              <tr className="mk-level-row">
                <th scope="rowgroup" colSpan={6}>
                  <span className={`mono ${side}`}>{fmtPrice4(lv.price4)}</span> {'\u00b7'} <span className="mono">{fmtShares(lv.shares)}</span> shares in{' '}
                  <span className="mono">{fmtInt(lv.order_count)}</span> order{lv.order_count === 1 ? '' : 's'}
                  {lv.orders_truncated ? (
                    <span className="dim">
                      {' '}
                      {'\u00b7'} first {fmtInt(listed.orders)} listed ({fmtShares(listed.shares)} shares); {fmtInt(lv.order_count - listed.orders)} later
                      orders not listed
                    </span>
                  ) : null}
                </th>
              </tr>
              {lv.orders.map((o) => {
                const mine = highlight.has(o.order_ref);
                const age = nsBetween(o.entered_ns, book.ts_ns);
                return (
                  <tr key={o.order_ref} className={mine ? 'mk-hl' : undefined}>
                    <td className="num">{fmtInt(o.queue_position)}</td>
                    <td className="mono">
                      {o.order_ref}
                      {mine ? (
                        <>
                          {' '}
                          <Badge kind="neutral">this event</Badge>
                        </>
                      ) : null}
                    </td>
                    <td className="num">{fmtShares(o.shares)}</td>
                    <td className="num">{fmtShares(o.shares_ahead)}</td>
                    <td className="num" title={o.entered_ns === null ? 'Entry time not recorded' : `Entered ${fmtSessionNs(o.entered_ns)}`}>
                      {age === null ? '\u2014' : fmtDuration(age)}
                    </td>
                    <td className="num">#{fmtInt(o.entered_event_index)}</td>
                  </tr>
                );
              })}
            </tbody>
          );
        })}
      </table>
    </div>
  );
}

function L2Ladder({ book }: { book: MarketBook }) {
  const asks = [...book.asks].reverse();
  const top = topOfBook(book.bids[0]?.price4, book.asks[0]?.price4);
  return (
    <div className="table-scroll">
      <table className="table mk-l2">
        <caption>Price levels, asks above bids; shares are the level totals</caption>
        <thead>
          <tr>
            <th scope="col">Side</th>
            <th scope="col" className="num">
              Price
            </th>
            <th scope="col" className="num">
              Shares
            </th>
          </tr>
        </thead>
        <tbody>
          {asks.map((lv) => (
            <tr key={`a${lv.price4}`}>
              <td className="ask">ask</td>
              <td className="num ask">{fmtPrice4(lv.price4)}</td>
              <td className="num">{fmtShares(lv.shares)}</td>
            </tr>
          ))}
          <tr className="mk-spread-row">
            <td colSpan={3}>
              {top.spread4 === null ? (top.state === 'empty' ? 'empty book' : 'one-sided book') : `spread ${fmtPrice4(top.spread4)} (${top.state})`}
            </td>
          </tr>
          {book.bids.map((lv) => (
            <tr key={`b${lv.price4}`}>
              <td className="bid">bid</td>
              <td className="num bid">{fmtPrice4(lv.price4)}</td>
              <td className="num">{fmtShares(lv.shares)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Touch({ book }: { book: MarketBook }) {
  const bid = book.bids[0] ?? null;
  const ask = book.asks[0] ?? null;
  const top = topOfBook(bid?.price4, ask?.price4);
  const level = (lv: BookLevel | null, cls: Side) =>
    lv === null ? (
      <dd>none</dd>
    ) : (
      <dd className={`mono ${cls}`}>
        {fmtPrice4(lv.price4)} {'\u00d7'} {fmtShares(lv.shares)}{' '}
        <span className="dim">
          ({fmtInt(lv.order_count)} order{lv.order_count === 1 ? '' : 's'})
        </span>
      </dd>
    );
  return (
    <dl className="kv mk-touch">
      <dt>Best bid</dt>
      {level(bid, 'bid')}
      <dt>Best ask</dt>
      {level(ask, 'ask')}
      <dt>Spread</dt>
      <dd className="mono">
        {top.spread4 === null
          ? '\u2014'
          : `${fmtPrice4(top.spread4)} (${fmtInt(top.spread4)} price unit${Math.abs(top.spread4) === 1 ? '' : 's'} of 1/10,000 of a dollar)`}
        {top.state === 'crossed' || top.state === 'locked' ? (
          <>
            {' '}
            <Badge kind="warn">{top.state}</Badge>
          </>
        ) : null}
      </dd>
      <dt>Time</dt>
      <dd className="mono">{fmtSessionNs(book.ts_ns)}</dd>
    </dl>
  );
}

export function BookView({
  datasetId,
  eventIndex,
  event,
  phone,
  eventFailure,
}: {
  datasetId: string;
  eventIndex: number;
  /** The selected event: `undefined` while loading, `null` for another symbol's frame. */
  event: MarketEvent | null | undefined;
  eventFailure: string | null;
  phone: boolean;
}) {
  const [levels, setLevels] = useState(5);
  const [perLevel, setPerLevel] = useState(20);
  const [pane, setPane] = useState<'l3' | 'l2'>('l3');
  const book = useAsync(
    (signal) => marketApi.book(datasetId, { event_index: eventIndex, levels, orders_per_level: perLevel }, { signal }),
    [datasetId, eventIndex, levels, perLevel],
  );
  const highlight = new Set([event?.order_ref, event?.new_order_ref].filter((r): r is string => typeof r === 'string'));

  const controls = (
    <div className="row mk-book-controls">
      <label>
        Levels per side{' '}
        <select value={levels} onChange={(e) => setLevels(Number(e.target.value))}>
          {LEVEL_CHOICES.map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
      </label>
      <label>
        Orders per level{' '}
        <select value={perLevel} onChange={(e) => setPerLevel(Number(e.target.value))}>
          {ORDER_CHOICES.map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
      </label>
    </div>
  );

  let content: ReactNode;
  if (book.error) content = <ErrorBox error={book.error} onRetry={book.reload} />;
  else if (!book.data) content = <Spinner label={`Reconstructing the book after event #${fmtInt(eventIndex)}`} />;
  else {
    const b = book.data;
    const l3 = (
      <Panel
        title="L3: individual orders"
        basis="observed"
        fill
        caption="Every resting order, its reference, size, FIFO position at its price, and age at this event. Queue position is the reconstruction's arrival order, not a proof of the venue's matching sequence."
      >
        <div className="col">
          <L3Side side="ask" levels={b.asks} book={b} highlight={highlight} />
          <L3Side side="bid" levels={b.bids} book={b} highlight={highlight} />
        </div>
      </Panel>
    );
    const l2 = (
      <Panel title="L2: price levels" basis="derived" fill caption="The same book aggregated per price, as a market-by-price feed shows it.">
        <div className="col">
          <Notice tag="L2 hides" tone="info">
            A Level-2 view shows only price and total shares. It hides which orders make up a level, their order references, their
            ages and every order{'\u2019'}s queue position, so a cancel or fill cannot be placed in the queue from L2 alone.
          </Notice>
          <L2Ladder book={b} />
        </div>
      </Panel>
    );
    content = (
      <div className="mk-book">
        <div className="mk-book-head">
          <Touch book={b} />
          {b.bids.length === 0 && b.asks.length === 0 ? <Empty>The book is empty after this event.</Empty> : null}
        </div>
        <div className="mk-book-panes">
          {phone ? (
            <PaneTabs<'l3' | 'l2'>
              label="Book views"
              active={pane}
              onChange={setPane}
              tabs={[
                { id: 'l3', label: 'L3 orders', content: l3 },
                { id: 'l2', label: 'L2 levels', content: l2 },
              ]}
            />
          ) : (
            <SplitPane
              id="market.book"
              panes={[
                { id: 'l3', label: 'L3 individual orders', defaultSize: '62%', minSize: '35%', content: l3 },
                { id: 'l2', label: 'L2 price levels', defaultSize: '38%', minSize: 240, content: l2 },
              ]}
            />
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="mk-book-wrap">
      <div className="mk-book-bar">
        <EventFacts index={eventIndex} event={event} failure={eventFailure} />
        {controls}
      </div>
      {content}
    </div>
  );
}
