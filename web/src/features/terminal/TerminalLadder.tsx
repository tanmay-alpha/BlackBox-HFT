// Depth ladder at the cursor with the run's own resting orders marked at their price. The book
// is observed (reconstructed L2); the own orders are simulated and never part of that book.

import type { DepthFrame, InstrumentMeta, OrderRecord } from '../../api/types';
import { fmtPrice, fmtQty } from '../../components/fmt';
import { ClockText, Empty } from '../../components/ui';

export interface TerminalLadderProps {
  frame: DepthFrame | null;
  instrument: InstrumentMeta;
  rows: number;
  /** Own orders resting at the simulated exchange at the cursor. */
  resting: OrderRecord[];
  selectedOrder: number | null;
  onSelectOrder: (orderId: number) => void;
}

function OwnMarks({
  orders,
  inst,
  selected,
  onSelect,
}: {
  orders: OrderRecord[];
  inst: InstrumentMeta;
  selected: number | null;
  onSelect: (id: number) => void;
}) {
  if (orders.length === 0) return null;
  return (
    <span className="term-own">
      {orders.map((o) => (
        <button
          key={o.order_id}
          type="button"
          className={`term-own-mark term-own-${o.side}${o.order_id === selected ? ' is-selected' : ''}`}
          title={`Simulated ${o.side} order ${o.order_id}, ${fmtQty(o.qty, inst)} (modeled; not part of the historical book)`}
          onClick={() => onSelect(o.order_id)}
        >
          #{o.order_id} {o.side} {fmtQty(o.qty, inst)}
        </button>
      ))}
    </span>
  );
}

export function TerminalLadder({
  frame,
  instrument,
  rows,
  resting,
  selectedOrder,
  onSelectOrder,
}: TerminalLadderProps) {
  if (!frame) return <Empty>No depth frame at or before the cursor in this window.</Empty>;
  const asks = frame.asks.slice(0, rows);
  const bids = frame.bids.slice(0, rows);
  let maxQty = 0;
  for (const [, q] of asks) if (q > maxQty) maxQty = q;
  for (const [, q] of bids) if (q > maxQty) maxQty = q;

  const byTick = new Map<number, OrderRecord[]>();
  for (const o of resting) {
    const list = byTick.get(o.price_tick) ?? [];
    list.push(o);
    byTick.set(o.price_tick, list);
  }
  const shown = new Set<number>([...asks.map((l) => l[0]), ...bids.map((l) => l[0])]);
  const offLadder = resting.filter((o) => !shown.has(o.price_tick));

  const row = (side: 'bid' | 'ask', level: [number, number], best: boolean) => {
    const [tick, qty] = level;
    const pct = maxQty > 0 ? Math.max(0, Math.min(100, (qty / maxQty) * 100)) : 0;
    const own = byTick.get(tick) ?? [];
    return (
      <tr key={`${side}${tick}`} className={best ? 'ladder-best' : undefined}>
        <td className={`num ${side === 'bid' ? 'buy' : 'sell'}`}>{fmtPrice(tick, instrument)}</td>
        <td className="num">{fmtQty(qty, instrument)}</td>
        <td className="ladder-bar-cell">
          <span className={`ladder-bar ladder-bar-${side}`} style={{ width: `${pct}%` }} aria-hidden="true" />
        </td>
        <td>
          <OwnMarks orders={own} inst={instrument} selected={selectedOrder} onSelect={onSelectOrder} />
        </td>
      </tr>
    );
  };

  return (
    <div className="col">
      <div className="table-scroll">
        <table className="table ladder term-ladder">
          <caption>
            Book at frame <ClockText ns={frame.ts} /> (observed, displayed quantity per level) with
            own simulated orders resting at the cursor (modeled).
          </caption>
          <thead>
            <tr>
              <th scope="col" className="num">
                Price
              </th>
              <th scope="col" className="num">
                Qty
              </th>
              <th scope="col">Size</th>
              <th scope="col">Own orders (order qty)</th>
            </tr>
          </thead>
          <tbody>
            {asks.map((l, i) => row('ask', l, i === 0)).reverse()}
            <tr className="ladder-spread">
              <td colSpan={4} className="mono dim">
                {bids.length > 0 && asks.length > 0
                  ? `spread ${asks[0][0] - bids[0][0]} ticks`
                  : bids.length === 0 && asks.length === 0
                    ? 'the reconstructed book is empty at this frame'
                    : `one-sided book: no ${bids.length === 0 ? 'bids' : 'asks'}`}
              </td>
            </tr>
            {bids.map((l, i) => row('bid', l, i === 0))}
          </tbody>
        </table>
      </div>
      {offLadder.length > 0 ? (
        <p className="note">
          Resting at prices not on the ladder above (empty level or beyond the top {rows}):{' '}
          {offLadder.map((o) => (
            <span key={o.order_id} className="nowrap">
              {fmtPrice(o.price_tick, instrument)}{' '}
              <OwnMarks orders={[o]} inst={instrument} selected={selectedOrder} onSelect={onSelectOrder} />{' '}
            </span>
          ))}
        </p>
      ) : null}
    </div>
  );
}
