// Observed order book at the cursor (reconstructed L2 of the synthetic dataset) with each
// scenario's simulated resting orders marked at their price. The simulated orders are never part
// of the observed book; the replay has no market impact.

import type { DepthFrame, InstrumentMeta, OrderRecord } from '../../../api/types';
import { fmtPrice, fmtQty } from '../../../components/fmt';
import { fmtSec, shows, type View, type Which } from './data';

export interface LadderProps {
  frame: DepthFrame | null;
  cursor: number;
  inst: InstrumentMeta;
  resting: Record<Which, OrderRecord[]>;
  view: View;
  labels: Record<Which, string>;
  selected: number | null;
  onSelectOrder: (orderId: number) => void;
}

const ROWS = 6;

export function Ladder({ frame, cursor, inst, resting, view, labels, selected, onSelectOrder }: LadderProps) {
  if (!frame) return <p className="pa-muted">No book snapshot at this time.</p>;
  const asks = frame.asks.slice(0, ROWS).reverse();
  const bids = frame.bids.slice(0, ROWS);
  const maxQty = Math.max(1e-12, ...asks.map((l) => l[1]), ...bids.map((l) => l[1]));
  const own = (tick: number) =>
    (['fast', 'slow'] as const)
      .filter((w) => shows(view, w))
      .flatMap((w) => resting[w].filter((o) => o.price_tick === tick).map((o) => ({ w, o })));

  const row = (side: 'bid' | 'ask', [tick, qty]: [number, number]) => (
    <tr key={`${side}${tick}`} className={`pa-ladder-${side}`}>
      <td className="pa-num">{fmtPrice(tick, inst)}</td>
      <td className="pa-num pa-ladder-qty">
        <span className="pa-ladder-bar" style={{ width: `${(qty / maxQty) * 100}%` }} aria-hidden="true" />
        <span className="pa-ladder-qty-text">{fmtQty(qty, inst)}</span>
      </td>
      <td className="pa-ladder-own">
        {own(tick).map(({ w, o }) => (
          <button
            key={`${w}${o.order_id}`}
            type="button"
            className={`pa-chip pa-chip-${w}${o.order_id === selected ? ' is-selected' : ''}`}
            aria-pressed={o.order_id === selected}
            title={`Simulated ${o.side} order #${o.order_id} in the ${labels[w]} scenario, ${fmtQty(o.qty, inst)} (not part of the observed book)`}
            onClick={() => onSelectOrder(o.order_id)}
          >
            {labels[w]} #{o.order_id}
          </button>
        ))}
      </td>
    </tr>
  );

  return (
    <div className="pa-ladder">
      <p className="pa-muted">
        Observed book at <span className="pa-num">{fmtSec(frame.ts, 4)}</span> (cursor {fmtSec(cursor, 4)}). Chips are simulated resting orders.
      </p>
      <table className="pa-table pa-ladder-table">
        <caption className="visually-hidden">Observed order book with simulated resting orders</caption>
        <thead>
          <tr>
            <th scope="col">Price</th>
            <th scope="col">Size</th>
            <th scope="col">Simulated orders</th>
          </tr>
        </thead>
        <tbody>
          {asks.map((l) => row('ask', l))}
          {bids.map((l) => row('bid', l))}
        </tbody>
      </table>
    </div>
  );
}
