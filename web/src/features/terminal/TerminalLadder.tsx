// Depth ladder at the cursor with the run's own resting orders marked at their price. The book
// is observed (reconstructed L2); the own orders are simulated and never part of that book.

import type { DepthFrame, InstrumentMeta, OrderRecord } from '../../api/types';
import { fmtPrice, fmtQty } from '../../components/fmt';
import { DepthLadder } from '../replay/DepthLadder';

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
          className={`term-own-mark term-own-${o.side === 'buy' ? 'bid' : 'ask'}${o.order_id === selected ? ' is-selected' : ''}`}
          aria-pressed={o.order_id === selected}
          title={`Simulated ${o.side} order ${o.order_id}, ${fmtQty(o.qty, inst)} (modeled; not part of the historical book)`}
          onClick={() => onSelect(o.order_id)}
        >
          #{o.order_id} {o.side} {fmtQty(o.qty, inst)}
        </button>
      ))}
    </span>
  );
}

export function TerminalLadder({ frame, instrument, rows, resting, selectedOrder, onSelectOrder }: TerminalLadderProps) {
  const byTick = new Map<number, OrderRecord[]>();
  for (const o of resting) {
    const list = byTick.get(o.price_tick) ?? [];
    list.push(o);
    byTick.set(o.price_tick, list);
  }
  const shown = new Set<number>(
    frame ? [...frame.asks.slice(0, rows).map((l) => l[0]), ...frame.bids.slice(0, rows).map((l) => l[0])] : [],
  );
  const offLadder = frame ? resting.filter((o) => !shown.has(o.price_tick)) : [];

  return (
    <div className="col">
      <DepthLadder
        frame={frame}
        instrument={instrument}
        rows={rows}
        caption="(observed, displayed quantity per level) with own simulated orders resting at the cursor (modeled)."
        own={{
          header: 'Own orders (order qty)',
          cell: (tick) => (
            <OwnMarks orders={byTick.get(tick) ?? []} inst={instrument} selected={selectedOrder} onSelect={onSelectOrder} />
          ),
        }}
      />
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
