// Observed order book at the replay time (reconstructed L2 of the synthetic dataset) with each
// scenario's simulated resting orders marked at their price. The simulated orders are never part
// of the observed book; the replay has no market impact.

import type { DepthFrame, InstrumentMeta, OrderRecord } from '../../api/types';
import { fmtPrice, fmtQty } from '../../components/fmt';
import { fmtSec, shows, type Focus, type ScenarioKey } from './data';

export interface LadderProps {
  frame: DepthFrame | null;
  /** The shared replay time the book is shown at. */
  t: number;
  inst: InstrumentMeta;
  resting: Record<ScenarioKey, OrderRecord[]>;
  focus: Focus;
  names: Record<ScenarioKey, string>;
  selected: number | null;
  onSelectOrder: (orderId: number) => void;
}

const ROWS = 6;

export function Ladder({ frame, t, inst, resting, focus, names, selected, onSelectOrder }: LadderProps) {
  if (!frame || frame.bids.length === 0 || frame.asks.length === 0) {
    return <p className="ex-muted">No two-sided book at {fmtSec(t, 4)}.</p>;
  }
  const asks = frame.asks.slice(0, ROWS).reverse();
  const bids = frame.bids.slice(0, ROWS);
  const maxQty = Math.max(1e-12, ...asks.map((l) => l[1]), ...bids.map((l) => l[1]));
  const own = (tick: number) =>
    (['fast', 'slow'] as const)
      .filter((k) => shows(focus, k))
      .flatMap((k) => resting[k].filter((o) => o.price_tick === tick).map((o) => ({ k, o })));

  const row = (side: 'bid' | 'ask', [tick, qty]: [number, number]) => (
    <tr key={`${side}${tick}`} className={`ex-ladder-${side}`}>
      <td className="ex-num">{fmtPrice(tick, inst)}</td>
      <td className="ex-num ex-ladder-qty">
        <span className="ex-ladder-bar" style={{ width: `${(qty / maxQty) * 100}%` }} aria-hidden="true" />
        <span className="ex-ladder-qty-text">{fmtQty(qty, inst)}</span>
      </td>
      <td>
        <span className="ex-ladder-own">
          {own(tick).map(({ k, o }) => (
            <button
              key={`${k}${o.order_id}`}
              type="button"
              className={`ex-chip ex-chip-${k}`}
              aria-pressed={o.order_id === selected}
              title={`Simulated ${o.side} order ${o.order_id} in the ${names[k]} scenario, ${fmtQty(o.qty, inst)} (not part of the observed book)`}
              onClick={() => onSelectOrder(o.order_id)}
            >
              {names[k]} #{o.order_id}
            </button>
          ))}
        </span>
      </td>
    </tr>
  );

  return (
    <div className="ex-ladder">
      <p className="ex-muted">
        Observed book at <span className="ex-num">{fmtSec(frame.ts, 4)}</span>, the last change at or before the replay time. Chips are
        simulated resting orders.
      </p>
      <table className="ex-table ex-ladder-table">
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
