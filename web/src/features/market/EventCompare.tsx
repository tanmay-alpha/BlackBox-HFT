// Step 3, Order book: one event observed two ways. The depth ladder in the middle shows, for the
// book immediately before the event (after event i - 1) and immediately after it (after event i),
// each level's L2 total and its L3 FIFO queue drawn order by order on one common share scale. The
// side column states what each view records, the changed levels' queues row by row, and the
// same-timestamp messages up to this one. Every pane renders the one server response for the
// selected source event index; the full raw tables stay available in the expert view below.

import { Fragment, useState } from 'react';
import type { ReactNode } from 'react';
import { Link, useNavigate } from 'react-router-dom';

import { fmtInt } from '../../components/fmt';
import { ErrorBox, Spinner, useAsync } from '../../components/ui';
import { paths } from '../../shell/paths';
import { marketApi, type CuratedExample, type DepthLevel, type EventDetail } from './api';
import {
  depthRows,
  depthScale,
  fmtSigned,
  levelLabel,
  mergeQueues,
  observability,
  sideOf,
  type DepthRow,
  type QueueRow,
} from './compare';
import { EventFacts } from './EventFacts';
import { fmtPrice4, fmtSessionNs, fmtShares, itchTypeName, topOfBook, type Top } from './model';
import { RawBook } from './BookView';

const LEVEL_CHOICES = [5, 10];
/** Orders listed per level (the selected orders are always listed besides). */
const ORDERS_PER_LEVEL = 50;

const ROLE: Record<string, string> = {
  order: 'changed level',
  replaced: 'replaced in place',
  replaced_from: 'replaced from',
  replaced_to: 'replaced to',
};

// ----- curated examples ---------------------------------------------------------------------

export function ExamplePicker({
  datasetId,
  examples,
  selected,
  phone,
}: {
  datasetId: string;
  examples: CuratedExample[];
  selected: number;
  phone: boolean;
}) {
  const navigate = useNavigate();
  const current = examples.find((x) => x.event_index === selected) ?? null;
  const href = (x: CuratedExample) => `${paths.marketDataset(datasetId, { tab: 'book', event: x.event_index })}&example=${x.id}`;
  return (
    <nav className="mk-examples" aria-label="Curated examples">
      {phone ? (
        <label className="mk-examples-select">
          <span className="mk-examples-label">Example</span>{' '}
          <select
            value={current?.id ?? ''}
            onChange={(e) => {
              const x = examples.find((y) => y.id === e.target.value);
              if (x) navigate(href(x));
            }}
          >
            {current ? null : <option value="">Event #{fmtInt(selected)} (not a curated example)</option>}
            {examples.map((x) => (
              <option key={x.id} value={x.id}>
                {x.title} (#{fmtInt(x.event_index)})
              </option>
            ))}
          </select>
        </label>
      ) : (
        <>
          <span className="mk-examples-label">Examples</span>
          <ul>
            {examples.map((x) => (
              <li key={x.id}>
                <Link to={href(x)} aria-current={x.event_index === selected ? 'true' : undefined} title={x.rule}>
                  {x.title}
                  <span className="mk-examples-idx"> #{fmtInt(x.event_index)}</span>
                </Link>
              </li>
            ))}
          </ul>
        </>
      )}
      {current ? (
        <details className="mk-examples-rule" open={!phone}>
          <summary>Selection rule</summary> {current.rule}
        </details>
      ) : null}
    </nav>
  );
}

// ----- depth ladder -------------------------------------------------------------------------

function QueueBar({
  level,
  scale,
  mark,
  when,
}: {
  level: DepthLevel | null | undefined;
  scale: number;
  mark: ReadonlyMap<string, 'sel' | 'new'>;
  when: 'before' | 'after';
}) {
  if (level === undefined) return <div className="mk-bar mk-bar-unknown" aria-hidden="true" />;
  if (level === null || scale <= 0) return <div className="mk-bar" aria-hidden="true" />;
  let listed = 0;
  for (const o of level.orders) listed += o.shares;
  const rest = level.shares - listed;
  const pct = (n: number) => `${(n / scale) * 100}%`;
  return (
    <div className={`mk-bar mk-bar-${when}`} aria-hidden="true">
      {level.orders.map((o, i) => (
        <span
          key={o.order_ref}
          className={`mk-seg${mark.has(o.order_ref) ? ` mk-seg-${mark.get(o.order_ref)}` : ''}`}
          style={{ width: pct(o.shares) }}
          title={`${when}: #${i + 1} order ${o.order_ref}, ${fmtShares(o.shares)} shares`}
        />
      ))}
      {rest > 0 ? (
        <span
          className="mk-seg mk-seg-rest"
          style={{ width: pct(rest) }}
          title={`${when}: ${fmtInt(level.order_count - level.orders.length)} later orders, ${fmtShares(rest)} shares, not listed`}
        />
      ) : null}
    </div>
  );
}

function L2Cell({ level }: { level: DepthLevel | null | undefined }) {
  if (level === undefined)
    return (
      <td className="num dim" title="Beyond the levels listed at this instant">
        {'\u2026'}
      </td>
    );
  if (level === null) return <td className="num dim">{'\u2014'}</td>;
  return (
    <td className="num">
      {fmtShares(level.shares)}
      <span className="mk-oc" title={`${fmtInt(level.order_count)} orders`}>
        {fmtInt(level.order_count)}
      </span>
    </td>
  );
}

function delta(row: DepthRow): string {
  if (row.before === undefined || row.after === undefined) return '';
  const d = (row.after?.shares ?? 0) - (row.before?.shares ?? 0);
  return d === 0 ? '' : fmtSigned(d);
}

function DepthLadder({ detail, phone }: { detail: EventDetail; phone: boolean }) {
  const rows = depthRows(detail);
  const scale = depthScale(rows);
  const mark = new Map<string, 'sel' | 'new'>();
  if (detail.event.order_ref !== null) mark.set(detail.event.order_ref, 'sel');
  if (detail.event.new_order_ref !== null) mark.set(detail.event.new_order_ref, 'new');
  if ((detail.event.message_type === 'A' || detail.event.message_type === 'F') && detail.order_after) mark.set(detail.order_after.order_ref, 'new');
  const before = topOfBook(detail.before.bids[0]?.price4, detail.before.asks[0]?.price4);
  const after = topOfBook(detail.after.bids[0]?.price4, detail.after.asks[0]?.price4);
  const spreadText = (t: Top) => (t.spread4 === null ? t.state : fmtPrice4(t.spread4));
  const asks = rows.filter((r) => r.side === 'ask');
  const bids = rows.filter((r) => r.side === 'bid');
  const line = (r: DepthRow, i: number, list: DepthRow[]) => {
    const prev = list[i - 1];
    const gap = prev !== undefined && prev.deep !== r.deep;
    return (
      <Fragment key={`${r.side}${r.price4}`}>
        {gap ? (
          <tr className="mk-ladder-gap">
            <td colSpan={5}>{'\u22ef'} levels between not listed</td>
          </tr>
        ) : null}
        <tr className={`mk-r-${r.side}${r.affected ? ' mk-ladder-hit' : ''}`}>
          <th scope="row" className={`num mono ${r.side}`}>
            {fmtPrice4(r.price4)}
          </th>
          <L2Cell level={r.before} />
          <L2Cell level={r.after} />
          <td className="num mk-delta">{delta(r)}</td>
          <td className="mk-queue-cell">
            <QueueBar level={r.before} scale={scale} mark={mark} when="before" />
            <QueueBar level={r.after} scale={scale} mark={mark} when="after" />
          </td>
        </tr>
      </Fragment>
    );
  };
  return (
    <div className="table-scroll mk-ladder-wrap">
      <table className="table mk-ladder">
        <caption className="visually-hidden">
          Depth before and after event {detail.event.event_index}: asks above, bids below; L2 level totals and the L3 FIFO queue of each
          level
        </caption>
        <colgroup>
          <col className="mk-col-price" />
          <col className="mk-col-l2" />
          <col className="mk-col-l2" />
          <col className="mk-col-delta" />
          <col />
        </colgroup>
        <thead>
          <tr>
            <th scope="col" className="num">
              Price
            </th>
            <th scope="col" className="num" title={detail.before_event_index === null ? 'Empty book before the first frame' : `After event #${fmtInt(detail.before_event_index)}`}>
              {phone ? 'L2 bef.' : 'L2 before'}
            </th>
            <th scope="col" className="num" title={`After event #${fmtInt(detail.event.event_index)}`}>
              {phone ? 'L2 aft.' : 'L2 after'}
            </th>
            <th scope="col" className="num">
              {'\u0394'}
            </th>
            <th scope="col">L3 queue{phone ? '' : ', FIFO left to right'}</th>
          </tr>
        </thead>
        <tbody>
          {asks.map((r, i) => line(r, i, asks))}
          <tr className="mk-spread-row">
            <td colSpan={5}>
              spread {spreadText(before)} {'\u2192'} {spreadText(after)}
              {before.bid !== after.bid ? ` \u00b7 best bid ${fmtPrice4(before.bid)} \u2192 ${fmtPrice4(after.bid)}` : ''}
              {before.ask !== after.ask ? ` \u00b7 best ask ${fmtPrice4(before.ask)} \u2192 ${fmtPrice4(after.ask)}` : ''}
            </td>
          </tr>
          {bids.map((r, i) => line(r, i, bids))}
        </tbody>
      </table>
      <p className="mk-ladder-note">
        Each level shows two bars on one share scale: before (upper) and after (lower). A bar segment is one order in arrival order; a
        hatched tail is orders beyond the {fmtInt(detail.orders_per_level)} listed. The small figure after an L2 total is its order count,
        which a market-by-price feed does not carry. {'\u2026'} marks a price beyond the {fmtInt(detail.levels)} levels listed at that
        instant.
      </p>
    </div>
  );
}

// ----- queue of each changed level ----------------------------------------------------------

const CHANGE_TEXT: Record<string, string> = {
  removed: 'removed',
  added: 'added',
  reduced: 'reduced',
  increased: 'increased',
  advanced: 'moved up',
  unchanged: '',
};

function pair(b: number | undefined, a: number | undefined, fmt: (n: number) => string): ReactNode {
  if (b === undefined && a === undefined) return '\u2014';
  if (b === undefined) return <>{'\u2014 \u2192 '}{fmt(a as number)}</>;
  if (a === undefined) return <>{fmt(b)}{' \u2192 \u2014'}</>;
  return b === a ? fmt(b) : `${fmt(b)} \u2192 ${fmt(a)}`;
}

function QueueTable({ rows, highlight }: { rows: QueueRow[]; highlight: ReadonlySet<string> }) {
  return (
    <div className="table-scroll mk-queue-wrap">
      <table className="table mk-queue">
        <thead>
          <tr>
            <th scope="col">Order</th>
            <th scope="col" className="num" title="One-based FIFO position at this price, before → after">
              Queue #
            </th>
            <th scope="col" className="num" title="Shares of the orders ahead at this price (excluding the order), before → after">
              Ahead
            </th>
            <th scope="col" className="num">
              Shares
            </th>
            <th scope="col">Change</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) =>
            r.kind === 'gap' ? (
              <tr key={`gap${i}`} className="mk-queue-gap">
                <td colSpan={5}>
                  {'\u22ef'} {fmtInt(r.count)} order{r.count === 1 ? '' : 's'} not listed
                </td>
              </tr>
            ) : (
              <tr key={r.order_ref} className={highlight.has(r.order_ref) ? 'mk-hl' : r.change !== 'unchanged' ? 'mk-queue-moved' : undefined}>
                <th scope="row" className="mono">
                  {r.order_ref}
                </th>
                <td className="num">{pair(r.before?.queue_position, r.after?.queue_position, fmtInt)}</td>
                <td className="num">{pair(r.before?.shares_ahead, r.after?.shares_ahead, fmtShares)}</td>
                <td className="num">{pair(r.before?.shares, r.after?.shares, fmtShares)}</td>
                <td className={`mk-change mk-change-${r.change}`}>{CHANGE_TEXT[r.change]}</td>
              </tr>
            ),
          )}
        </tbody>
      </table>
    </div>
  );
}

// ----- same timestamp -----------------------------------------------------------------------

function SameTimestampList({ detail, onSelect }: { detail: EventDetail; onSelect: (index: number) => void }) {
  const st = detail.same_timestamp;
  return (
    <section className="mk-side-block" aria-labelledby="mk-st-h">
      <h3 id="mk-st-h" className="mk-side-h">
        Same timestamp {fmtSessionNs(st.ts_ns)}: {fmtInt(st.position)} message{st.position === 1 ? '' : 's'} up to this one
      </h3>
      {st.events_truncated ? <p className="note">Earlier messages at this timestamp start at event #{fmtInt(st.first_event_index)}; the latest {fmtInt(st.events.length)} are listed.</p> : null}
      <ol className="mk-st">
        {st.events.map((e) => (
          <li key={e.event_index} aria-current={e.event_index === detail.event.event_index ? 'true' : undefined}>
            <button type="button" className="btn-link mono" onClick={() => onSelect(e.event_index)} disabled={e.event_index === detail.event.event_index}>
              #{fmtInt(e.event_index)}
            </button>{' '}
            <span className="mono" title={itchTypeName(e.message_type)}>
              {e.message_type}
            </span>{' '}
            {e.level_changes.length === 0 ? (
              <span className="dim">{e.applied.replace(/_/g, ' ')}</span>
            ) : (
              e.level_changes.map((c) => (
                <span key={`${c.side}${c.price4}`} className="mk-st-change">
                  <span className={sideOf(c.side)}>{levelLabel(c.side, c.price4)}</span> {fmtSigned(c.after_shares - c.before_shares)}
                </span>
              ))
            )}
          </li>
        ))}
      </ol>
      <p className="note">Later messages with the same timestamp are not used for this event{'\u2019'}s state.</p>
    </section>
  );
}

// ----- the step -----------------------------------------------------------------------------

export function EventCompare({
  datasetId,
  eventIndex,
  examples,
  phone,
  onSelect,
}: {
  datasetId: string;
  eventIndex: number;
  examples: CuratedExample[];
  phone: boolean;
  onSelect: (index: number) => void;
}) {
  const [levels, setLevels] = useState(5);
  const [expert, setExpert] = useState(false);
  const detail = useAsync(
    (signal) => marketApi.eventDetail(datasetId, eventIndex, { levels, orders_per_level: ORDERS_PER_LEVEL }, { signal }),
    [datasetId, eventIndex, levels],
  );
  const d = detail.data;

  const head = (
    <div className="mk-cmp-head">
      {examples.length > 0 ? <ExamplePicker datasetId={datasetId} examples={examples} selected={eventIndex} phone={phone} /> : null}
      <div className="mk-cmp-event">
        <EventFacts index={eventIndex} event={d ? d.event : detail.error ? null : undefined} failure={detail.error?.message ?? null} />
        {d ? (
          <p className="mk-cmp-basis">
            Before = book after {d.before_event_index === null ? 'no event (empty)' : `#${fmtInt(d.before_event_index)}`}; after = book after #
            {fmtInt(d.event.event_index)}; both from source frames up to #{fmtInt(d.event.event_index)} only.
            {phone ? null : <span className="dim"> J / K: next / previous event.</span>}
          </p>
        ) : null}
      </div>
    </div>
  );

  let body: ReactNode;
  if (detail.error) body = <ErrorBox error={detail.error} onRetry={detail.reload} />;
  else if (!d) body = <Spinner label={`Reconstructing the book around event #${fmtInt(eventIndex)}`} />;
  else {
    const obs = observability(d);
    const highlight = new Set([d.event.order_ref, d.event.new_order_ref].filter((r): r is string => r !== null));
    body = (
      <div className="mk-cmp-grid">
        <section className="mk-cmp-main" aria-labelledby="mk-depth-h">
          <div className="mk-cmp-bar">
            <h2 id="mk-depth-h" className="mk-side-h">
              Depth before {'\u2192'} after
            </h2>
            <span className="mk-legend" aria-hidden="true">
              <span className="mk-key mk-key-sel" /> order named by the event <span className="mk-key mk-key-new" /> new order
            </span>
            <label className="mk-cmp-levels">
              Levels per side{' '}
              <select value={levels} onChange={(e) => setLevels(Number(e.target.value))}>
                {LEVEL_CHOICES.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <DepthLadder detail={d} phone={phone} />
        </section>
        <div className="mk-cmp-side">
          <section className="mk-side-block mk-obs" aria-labelledby="mk-obs-h">
            <h2 id="mk-obs-h" className="mk-side-h">
              What each view records
            </h2>
            <dl className="mk-obs-list">
              <dt>L2 (price levels)</dt>
              {obs.l2.map((s) => (
                <dd key={s}>{s}</dd>
              ))}
              <dt>L3 adds (orders)</dt>
              {obs.l3.map((s) => (
                <dd key={s}>{s}</dd>
              ))}
            </dl>
          </section>
          {d.affected.map((lv) => (
            <section key={`${lv.role}${lv.price4}`} className="mk-side-block" aria-label={`${levelLabel(lv.side, lv.price4)} queue`}>
              <h3 className="mk-side-h">
                <span className={sideOf(lv.side)}>{levelLabel(lv.side, lv.price4)}</span> {'\u00b7'} {ROLE[lv.role] ?? lv.role} {'\u00b7'}{' '}
                <span className="mono">
                  {fmtInt(lv.before.order_count)} {'\u2192'} {fmtInt(lv.after.order_count)}
                </span>{' '}
                orders
              </h3>
              <QueueTable rows={mergeQueues(lv.before, lv.after, d.event.event_index)} highlight={highlight} />
            </section>
          ))}
          {d.same_timestamp.position > 1 ? <SameTimestampList detail={d} onSelect={onSelect} /> : null}
        </div>
      </div>
    );
  }

  return (
    <div className="mk-cmp">
      {head}
      {body}
      <details className="mk-expert" open={expert} onToggle={(e) => setExpert(e.currentTarget.open)}>
        <summary>Expert view: raw L3 order table and L2 ladder after #{fmtInt(eventIndex)}</summary>
        {expert ? <RawBook datasetId={datasetId} eventIndex={eventIndex} event={d?.event} /> : null}
      </details>
    </div>
  );
}
