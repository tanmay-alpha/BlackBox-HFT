// The reconstructed book at the playback cursor: top N levels per side with quantity bars.

import type { DepthFrame, InstrumentMeta } from '../../api/types';
import { fmtPrice, fmtQty } from '../../components/fmt';
import { ClockText, Empty } from '../../components/ui';

export interface DepthLadderProps {
  /** The depth frame in force at the cursor, or `null` before the first frame. */
  frame: DepthFrame | null;
  instrument: InstrumentMeta;
  /** Levels to show per side. */
  rows: number;
}

function LevelRow({
  side,
  level,
  maxQty,
  inst,
  best,
}: {
  side: 'bid' | 'ask';
  level: [number, number];
  maxQty: number;
  inst: InstrumentMeta;
  best: boolean;
}) {
  const [tick, qty] = level;
  const pct = maxQty > 0 ? Math.max(0, Math.min(100, (qty / maxQty) * 100)) : 0;
  return (
    <tr className={best ? 'ladder-best' : undefined}>
      <th scope="row" className={side === 'bid' ? 'buy' : 'sell'}>
        {side}
      </th>
      <td className={`num ${side === 'bid' ? 'buy' : 'sell'}`}>{fmtPrice(tick, inst)}</td>
      <td className="num">{fmtQty(qty, inst)}</td>
      <td className="ladder-bar-cell">
        <span
          className={`ladder-bar ladder-bar-${side}`}
          style={{ width: `${pct}%` }}
          aria-hidden="true"
        />
      </td>
    </tr>
  );
}

export function DepthLadder({ frame, instrument, rows }: DepthLadderProps) {
  if (!frame) {
    return <Empty>No depth frame at or before the cursor in this window.</Empty>;
  }
  const asks = frame.asks.slice(0, rows);
  const bids = frame.bids.slice(0, rows);
  if (asks.length === 0 && bids.length === 0) {
    return (
      <Empty>
        The reconstructed book is empty at <ClockText ns={frame.ts} />.
      </Empty>
    );
  }
  let maxQty = 0;
  for (const [, q] of asks) if (q > maxQty) maxQty = q;
  for (const [, q] of bids) if (q > maxQty) maxQty = q;

  const bestBid = bids.length > 0 ? bids[0][0] : null;
  const bestAsk = asks.length > 0 ? asks[0][0] : null;
  let spread: string;
  if (bestBid === null || bestAsk === null) {
    spread = `one-sided book: no ${bestBid === null ? 'bids' : 'asks'}`;
  } else if (bestAsk <= bestBid) {
    spread = `crossed or locked book (${bestAsk - bestBid} ticks)`;
  } else {
    const ticks = bestAsk - bestBid;
    spread = `spread ${ticks} tick${ticks === 1 ? '' : 's'} \u00b7 mid ${fmtPrice((bestAsk + bestBid) / 2, instrument)}`;
  }

  return (
    <div className="table-scroll">
      <table className="table ladder">
        <caption>
          Book at frame <ClockText ns={frame.ts} /> (displayed aggregate quantity per price level)
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
            <th scope="col">Relative size</th>
          </tr>
        </thead>
        <tbody>
          {asks
            .map((level, i) => (
              <LevelRow
                key={`a${level[0]}`}
                side="ask"
                level={level}
                maxQty={maxQty}
                inst={instrument}
                best={i === 0}
              />
            ))
            .reverse()}
          <tr className="ladder-spread">
            <td colSpan={4} className="mono dim">
              {spread}
            </td>
          </tr>
          {bids.map((level, i) => (
            <LevelRow
              key={`b${level[0]}`}
              side="bid"
              level={level}
              maxQty={maxQty}
              inst={instrument}
              best={i === 0}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}
