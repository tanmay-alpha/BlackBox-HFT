// Table editor for `ScriptedParams.orders`, generated from `SCRIPTED_ORDER_FIELDS`.

import type { ScriptedOrder } from '../../api/types';
import { Empty } from '../../components/ui';
import { FieldControl, fieldDomId } from './FieldInput';
import { SCRIPTED_ORDER_FIELDS, fieldProblem, newScriptedOrder } from './fields';

export interface ScriptedOrdersEditorProps {
  orders: ScriptedOrder[];
  onChange: (orders: ScriptedOrder[]) => void;
}

export function ScriptedOrdersEditor({ orders, onChange }: ScriptedOrdersEditorProps) {
  const replace = (index: number, next: ScriptedOrder) =>
    onChange(orders.map((o, i) => (i === index ? next : o)));
  const remove = (index: number) => onChange(orders.filter((_, i) => i !== index));
  const add = () => onChange([...orders, newScriptedOrder(orders)]);

  return (
    <div className="col scripted">
      <div className="row">
        <h3>Scripted orders</h3>
        <span className="note">
          The strategy places exactly these orders. The order id is (tick {'\u00d7'} 256) + slot.
        </span>
        <span className="spacer" />
        <button type="button" onClick={add}>
          Add order
        </button>
      </div>
      {orders.length === 0 ? (
        <Empty>
          The script is empty: a run would place no orders. Add at least one order, or choose
          another strategy.
        </Empty>
      ) : (
        <div className="table-scroll">
          <table className="table scripted-table">
            <thead>
              <tr>
                <th scope="col" className="num">
                  #
                </th>
                {SCRIPTED_ORDER_FIELDS.map((f) => (
                  <th key={f.id} scope="col" title={f.help}>
                    {f.label}
                  </th>
                ))}
                <th scope="col">
                  <span className="visually-hidden">Remove</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {orders.map((order, row) => (
                <tr key={row}>
                  <th scope="row" className="num">
                    {row + 1}
                  </th>
                  {SCRIPTED_ORDER_FIELDS.map((f) => {
                    const problem = fieldProblem(f, order);
                    return (
                      <td key={f.id} className={`scripted-cell scripted-cell-${f.kind}`}>
                        <FieldControl
                          field={f}
                          obj={order}
                          onChange={(next) => replace(row, next)}
                          domId={fieldDomId(`lab-script-${row}`, f.id)}
                          ariaLabel={`${f.label}, order ${row + 1}`}
                        />
                        {problem ? (
                          <span className="field-error" role="alert">
                            {problem}
                          </span>
                        ) : null}
                      </td>
                    );
                  })}
                  <td>
                    <button
                      type="button"
                      className="btn-danger"
                      onClick={() => remove(row)}
                      aria-label={`Remove order ${row + 1}`}
                    >
                      Remove
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <ul className="note scripted-help">
        {SCRIPTED_ORDER_FIELDS.map((f) => (
          <li key={f.id}>
            <span className="mono">{f.label}</span>: {f.help}
          </li>
        ))}
      </ul>
    </div>
  );
}
