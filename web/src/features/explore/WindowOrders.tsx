// The orders alive in the replay window, one row per decision slot, with each scenario's
// exchange arrival and result. The "Inspect" buttons are the keyboard path to the chart markers.

import type { InstrumentMeta, OrderRecord } from '../../api/types';
import { fmtPrice } from '../../components/fmt';
import { fmtSec, outcomeWords, shows, type Focus, type ScenarioKey, type WindowOrder } from './data';

function Result({ o }: { o: OrderRecord | null }) {
  if (!o) return <span className="ex-muted">not placed</span>;
  const when = o.outcome === 'filled' || o.outcome === 'partially_filled' ? o.first_fill_ts : o.terminal_ts;
  return (
    <>
      <span className="ex-num">{fmtSec(o.exch_arrival_ts)}</span>
      <span className="ex-muted"> → </span>
      {outcomeWords(o)}
      {when !== null ? <span className="ex-num"> {fmtSec(when)}</span> : null}
    </>
  );
}

export interface WindowOrdersProps {
  rows: WindowOrder[];
  focus: Focus;
  names: Record<ScenarioKey, string>;
  inst: InstrumentMeta;
  /** The comparison class of a row in words, or null while it is not known. */
  classText: ((row: WindowOrder) => string | null) | null;
  marks: Map<number, string>;
  selected: number | null;
  onSelectOrder: (row: WindowOrder) => void;
}

export function WindowOrders({ rows, focus, names, inst, classText, marks, selected, onSelectOrder }: WindowOrdersProps) {
  const visible = rows.filter((r) => (shows(focus, 'fast') && r.fast) || (shows(focus, 'slow') && r.slow));
  if (visible.length === 0) return <p className="ex-muted">No simulated orders in this window for the selected scenario.</p>;
  return (
    <div className="ex-scroll-x ex-orders">
      <table className="ex-table">
        <caption className="visually-hidden">Simulated orders in the replay window</caption>
        <thead>
          <tr>
            <th scope="col">Order</th>
            <th scope="col">Decided</th>
            <th scope="col">Side and price</th>
            {(['fast', 'slow'] as const)
              .filter((k) => shows(focus, k))
              .map((k) => (
                <th key={k} scope="col">
                  <span className={`ex-swatch ex-swatch-${k}`} aria-hidden="true" /> {names[k]}: reaches exchange → result
                </th>
              ))}
            {classText ? <th scope="col">Comparison</th> : null}
          </tr>
        </thead>
        <tbody>
          {visible.map((r) => {
            const ref = r.fast ?? r.slow;
            const mark = marks.get(r.orderId);
            const isSel = r.orderId === selected;
            return (
              <tr key={r.orderId} className={isSel ? 'is-selected' : undefined} aria-current={isSel ? 'true' : undefined}>
                <td>
                  <button type="button" className="ex-linkbtn" onClick={() => onSelectOrder(r)} aria-label={`Inspect order ${r.orderId}`}>
                    #{r.orderId}
                  </button>
                  {mark ? <span className="ex-tag">{mark}</span> : null}
                </td>
                <td className="ex-num">{fmtSec(r.decisionTs)}</td>
                <td>
                  {ref ? (
                    <>
                      <span className={ref.side === 'buy' ? 'ex-buy' : 'ex-sell'}>{ref.side}</span> <span className="ex-num">{fmtPrice(ref.price_tick, inst)}</span>
                    </>
                  ) : null}
                </td>
                {(['fast', 'slow'] as const)
                  .filter((k) => shows(focus, k))
                  .map((k) => (
                    <td key={k}>
                      <Result o={r[k]} />
                    </td>
                  ))}
                {classText ? <td>{classText(r) ?? <span className="ex-muted">not known</span>}</td> : null}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
