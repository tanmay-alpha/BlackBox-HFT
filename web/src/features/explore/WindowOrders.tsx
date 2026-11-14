// The orders alive in the replay window, one row per decision slot, with each scenario's
// exchange arrival and result. Rows are the keyboard path to the chart markers.

import type { InstrumentMeta, OrderPair, OrderRecord } from '../../../api/types';
import { fmtPrice } from '../../../components/fmt';
import type { LatencyStory } from '../story';
import { fmtSec, outcomeWords, pairVerdict, shows, type View, type Which, type WindowOrder } from './data';

function Result({ o }: { o: OrderRecord | null }) {
  if (!o) return <span className="pa-muted">not placed</span>;
  const when = o.outcome === 'filled' || o.outcome === 'partially_filled' ? o.first_fill_ts : o.terminal_ts;
  return (
    <>
      <span className="pa-num">{fmtSec(o.exch_arrival_ts)}</span>
      <span className="pa-muted"> → </span>
      {outcomeWords(o)}
      {when !== null ? <span className="pa-num"> {fmtSec(when)}</span> : null}
    </>
  );
}

export interface WindowOrdersProps {
  story: LatencyStory;
  rows: WindowOrder[];
  view: View;
  labels: Record<Which, string>;
  inst: InstrumentMeta;
  pairs: Map<number, OrderPair> | null;
  marks: Map<number, string>;
  selected: number | null;
  onSelectOrder: (orderId: number) => void;
}

export function WindowOrders({ story, rows, view, labels, inst, pairs, marks, selected, onSelectOrder }: WindowOrdersProps) {
  const visible = rows.filter((r) => (shows(view, 'fast') && r.fast) || (shows(view, 'slow') && r.slow));
  if (visible.length === 0) return <p className="pa-muted">No simulated orders in this window for the selected scenario.</p>;
  return (
    <div className="pa-scroll-x pa-orders">
      <table className="pa-table">
        <caption className="visually-hidden">Simulated orders in the replay window</caption>
        <thead>
          <tr>
            <th scope="col">Order</th>
            <th scope="col">Decided</th>
            <th scope="col">Side and price</th>
            {shows(view, 'fast') ? (
              <th scope="col">
                <span className="pa-swatch pa-swatch-fast" aria-hidden="true" /> {labels.fast}: arrives → result
              </th>
            ) : null}
            {shows(view, 'slow') ? (
              <th scope="col">
                <span className="pa-swatch pa-swatch-slow" aria-hidden="true" /> {labels.slow}: arrives → result
              </th>
            ) : null}
            <th scope="col">Comparison</th>
          </tr>
        </thead>
        <tbody>
          {visible.map((r) => {
            const ref = r.fast ?? r.slow;
            const pair = pairs?.get(r.orderId) ?? null;
            const mark = marks.get(r.orderId);
            const isSel = r.orderId === selected;
            return (
              <tr key={r.orderId} className={isSel ? 'is-selected' : undefined} aria-current={isSel ? 'true' : undefined}>
                <td>
                  <button type="button" className="pa-linkbtn" onClick={() => onSelectOrder(r.orderId)} aria-label={`Inspect order ${r.orderId}`}>
                    #{r.orderId}
                  </button>
                  {mark ? <span className="pa-tag">{mark}</span> : null}
                </td>
                <td className="pa-num">{fmtSec(r.decisionTs)}</td>
                <td>
                  {ref ? (
                    <>
                      <span className={ref.side === 'buy' ? 'pa-buy' : 'pa-sell'}>{ref.side}</span> <span className="pa-num">{fmtPrice(ref.price_tick, inst)}</span>
                    </>
                  ) : null}
                </td>
                {shows(view, 'fast') ? (
                  <td>
                    <Result o={r.fast} />
                  </td>
                ) : null}
                {shows(view, 'slow') ? (
                  <td>
                    <Result o={r.slow} />
                  </td>
                ) : null}
                <td>{pair ? pairVerdict(story, pair) : <span className="pa-muted">—</span>}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
