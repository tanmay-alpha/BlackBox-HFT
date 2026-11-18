// Queue lab charts, drawn the study viewer's way (features/study/charts.tsx, `stc-*` classes):
// rows of positioned marks on one shared percentage scale, every number printed beside its marks.
// Whiskers here are the artefact's own nominal 95% intervals (Wilson, Newcombe or Greenwood), never
// seed variation, so the caption names the method it shows.

import type { ReactNode } from 'react';

export interface MarkRow {
  key: string;
  label: ReactNode;
  value: number | null;
  /** The artefact's nominal 95% interval, when both bounds are available. */
  interval: [number, number] | null;
  /** Printed beside the marks: value, interval, n or why it is unavailable. */
  text: ReactNode;
  reference?: boolean;
  selected?: boolean;
  /** Makes the row label a button (it selects the row's model). */
  onSelect?: () => void;
  /** `data-*` attributes of the row, for tests and deep links. */
  data?: Record<`data-${string}`, string>;
}

/** Value range of the marks, padded; `zero` keeps 0 in range, `fixed` overrides it. */
export function markDomain(rows: MarkRow[], { zero = false, fixed }: { zero?: boolean; fixed?: [number, number] } = {}): [number, number] {
  if (fixed) return fixed;
  const values = rows.flatMap((r) => (r.value === null ? [] : r.interval ? [r.value, ...r.interval] : [r.value]));
  let lo = Math.min(...values, zero ? 0 : Infinity);
  let hi = Math.max(...values, zero ? 0 : -Infinity);
  if (!Number.isFinite(lo) || !Number.isFinite(hi)) return [0, 1];
  if (hi === lo) {
    const pad = Math.abs(lo) > 0 ? Math.abs(lo) * 0.05 : 1;
    return [lo - pad, hi + pad];
  }
  const pad = (hi - lo) * 0.06;
  lo -= zero && lo === 0 ? 0 : pad;
  hi += pad;
  return [lo, hi];
}

const at = (v: number, [lo, hi]: [number, number]) => `${(((v - lo) / (hi - lo)) * 100).toFixed(3)}%`;

export function MarkChart({
  rows,
  domain,
  caption,
  legend,
  labels,
  testId,
}: {
  rows: MarkRow[];
  domain: [number, number];
  /** What is drawn and its unit, for assistive technology. */
  caption: string;
  /** Visible line under the chart: scale and what the marks mean. */
  legend: ReactNode;
  labels?: ReactNode;
  testId?: string;
}) {
  const reference = rows.find((r) => r.reference && r.value !== null)?.value ?? null;
  const zeroIn = domain[0] < 0 && domain[1] > 0;
  return (
    <figure className="stc-chart" data-testid={testId}>
      {labels ? <div className="row stc-chart-labels">{labels}</div> : null}
      <table className="stc-rows">
        <caption className="visually-hidden">{caption}</caption>
        <thead className="visually-hidden">
          <tr>
            <th scope="col">Path</th>
            <th scope="col">Marks</th>
            <th scope="col">Value</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.key} className={`${r.reference ? 'stc-ref' : ''}${r.selected ? ' stc-selected' : ''}`} {...r.data}>
              <th scope="row" className="stc-label">
                {r.onSelect ? (
                  <button type="button" className="stc-pick" aria-pressed={r.selected === true} onClick={r.onSelect}>
                    {r.label}
                  </button>
                ) : (
                  r.label
                )}
              </th>
              <td className="stc-track-cell" aria-hidden="true">
                <div className="stc-track">
                  {zeroIn ? <span className="stc-zero" style={{ left: at(0, domain) }} /> : null}
                  {reference !== null ? <span className="stc-refline" style={{ left: at(reference, domain) }} /> : null}
                  {r.interval ? (
                    <span
                      className="stc-whisker"
                      style={{ left: at(r.interval[0], domain), width: `${(((r.interval[1] - r.interval[0]) / (domain[1] - domain[0])) * 100).toFixed(3)}%` }}
                    />
                  ) : null}
                  {r.value !== null ? <span className={r.reference ? 'stc-dot ql-dot-ref' : 'stc-dot'} style={{ left: at(r.value, domain) }} /> : null}
                </div>
              </td>
              <td className="stc-value mono num">{r.text}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <figcaption className="note small">{legend}</figcaption>
    </figure>
  );
}
