// The reconstructed book at the playback cursor: top N levels per side, asks above bids, with a
// Side column, the quantity always printed and a decorative depth bar. The run view adds a column
// of own simulated orders through `own`.

import { useLayoutEffect, useRef } from 'react';
import type { ReactNode } from 'react';

import type { DepthFrame, InstrumentMeta } from '../../api/types';
import { fmtPrice, fmtQty } from '../../components/fmt';
import { ClockText, Empty } from '../../components/ui';
import './replay.css';

export interface DepthLadderProps {
  /** The depth frame in force at the cursor, or `null` before the first frame. */
  frame: DepthFrame | null;
  instrument: InstrumentMeta;
  /** Levels to show per side. */
  rows: number;
  /** Table caption after "Book at frame <t>". */
  caption?: ReactNode;
  /** Extra last column: header and cell per price tick. */
  own?: { header: string; cell: (tick: number) => ReactNode };
}

type LadderSide = 'bid' | 'ask';

function spreadText(bestBid: number | null, bestAsk: number | null, inst: InstrumentMeta): string {
  if (bestBid === null && bestAsk === null) return 'the reconstructed book is empty at this frame';
  if (bestBid === null || bestAsk === null) return `one-sided book: no ${bestBid === null ? 'bids' : 'asks'}`;
  if (bestAsk <= bestBid) return `crossed or locked book (${bestAsk - bestBid} ticks)`;
  const ticks = bestAsk - bestBid;
  return `spread ${ticks} tick${ticks === 1 ? '' : 's'} \u00b7 mid ${fmtPrice((bestAsk + bestBid) / 2, inst)}`;
}

/** The nearest ancestor inside the ladder's panel that scrolls vertically (never the page). */
function scrollParent(el: HTMLElement): HTMLElement | null {
  const stop = el.closest('.panel');
  if (!stop) return null;
  for (let p = el.parentElement; p && p !== stop; p = p.parentElement) {
    const oy = getComputedStyle(p).overflowY;
    if ((oy === 'auto' || oy === 'scroll') && p.scrollHeight > p.clientHeight) return p;
  }
  return null;
}

export function DepthLadder({ frame, instrument, rows, caption, own }: DepthLadderProps) {
  // The best bid and ask meet at the spread row: centre it once in the scrolling pane, so a short
  // pane opens on the inside of the book instead of the far asks. Later scrolling is the user's.
  const spreadRef = useRef<HTMLTableRowElement | null>(null);
  const centred = useRef(false);
  useLayoutEffect(() => {
    const row = spreadRef.current;
    if (centred.current || !row) return;
    centred.current = true;
    const sc = scrollParent(row);
    if (!sc) return;
    const r = row.getBoundingClientRect();
    const s = sc.getBoundingClientRect();
    sc.scrollTop += r.top + r.height / 2 - (s.top + s.height / 2);
  });
  if (!frame) {
    return <Empty>No depth frame at or before the cursor in this window.</Empty>;
  }
  const asks = frame.asks.slice(0, rows);
  const bids = frame.bids.slice(0, rows);
  if (asks.length === 0 && bids.length === 0 && !own) {
    return (
      <Empty>
        The reconstructed book is empty at <ClockText ns={frame.ts} />.
      </Empty>
    );
  }
  let maxQty = 0;
  for (const [, q] of asks) if (q > maxQty) maxQty = q;
  for (const [, q] of bids) if (q > maxQty) maxQty = q;
  const cols = own ? 5 : 4;

  const row = (side: LadderSide, level: [number, number], best: boolean) => {
    const [tick, qty] = level;
    const pct = maxQty > 0 ? Math.max(0, Math.min(100, (qty / maxQty) * 100)) : 0;
    const cls = best ? `ladder-row ladder-best ladder-best-${side}` : 'ladder-row';
    return (
      <tr key={`${side}${tick}`} className={cls}>
        <th scope="row" className={`ladder-side ${side}`}>
          {side === 'bid' ? 'BID' : 'ASK'}
        </th>
        <td className="num">{fmtPrice(tick, instrument)}</td>
        <td className="num">{fmtQty(qty, instrument)}</td>
        <td className="ladder-bar-cell">
          <span className={`ladder-bar ladder-bar-${side}`} style={{ width: `${pct}%` }} aria-hidden="true" />
        </td>
        {own ? <td>{own.cell(tick)}</td> : null}
      </tr>
    );
  };

  return (
    <div className="table-scroll">
      <table className="table ladder">
        <caption>
          Book at frame <ClockText ns={frame.ts} /> {caption ?? '(displayed aggregate quantity per price level)'}
        </caption>
        <thead>
          <tr>
            <th scope="col">Side</th>
            <th scope="col" className="num">
              Price
            </th>
            <th scope="col" className="num">
              Qty
            </th>
            <th scope="col">
              <span className="visually-hidden">Relative size</span>
            </th>
            {own ? <th scope="col">{own.header}</th> : null}
          </tr>
        </thead>
        <tbody>
          {asks.map((level, i) => row('ask', level, i === 0)).reverse()}
          <tr className="ladder-spread" ref={spreadRef}>
            <td colSpan={cols}>
              {spreadText(bids.length > 0 ? bids[0][0] : null, asks.length > 0 ? asks[0][0] : null, instrument)}
            </td>
          </tr>
          {bids.map((level, i) => row('bid', level, i === 0))}
        </tbody>
      </table>
    </div>
  );
}
