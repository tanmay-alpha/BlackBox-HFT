// Small data-display components shared by the dataset and run pages.

import type { ReactNode } from 'react';

import type { Quantiles } from '../api/types';
import { DASH, fmtInt } from './fmt';

export interface QuantileRow {
  label: ReactNode;
  q: Quantiles | null;
  /** Formats one value of this row (durations, ticks, ...). */
  fmt: (x: number) => string;
}

/** Distribution summaries side by side. A row without data says so instead of showing zeros. */
export function QuantileTable({ caption, rows }: { caption?: ReactNode; rows: QuantileRow[] }) {
  return (
    <div className="table-scroll">
      <table className="table">
        {caption ? <caption>{caption}</caption> : null}
        <thead>
          <tr>
            <th scope="col">Measure</th>
            <th scope="col" className="num">
              n
            </th>
            <th scope="col" className="num">
              min
            </th>
            <th scope="col" className="num">
              p50
            </th>
            <th scope="col" className="num">
              p90
            </th>
            <th scope="col" className="num">
              p99
            </th>
            <th scope="col" className="num">
              max
            </th>
            <th scope="col" className="num">
              mean
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i}>
              <th scope="row">{r.label}</th>
              {r.q ? (
                <>
                  <td className="num">{fmtInt(r.q.n)}</td>
                  <td className="num">{r.fmt(r.q.min)}</td>
                  <td className="num">{r.fmt(r.q.p50)}</td>
                  <td className="num">{r.fmt(r.q.p90)}</td>
                  <td className="num">{r.fmt(r.q.p99)}</td>
                  <td className="num">{r.fmt(r.q.max)}</td>
                  <td className="num">{r.fmt(r.q.mean)}</td>
                </>
              ) : (
                <td className="dim" colSpan={7}>
                  {DASH} no samples
                </td>
              )}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Collapsible pretty-printed JSON, exactly as received. */
export function JsonBlock({ summary, value }: { summary: ReactNode; value: unknown }) {
  return (
    <details>
      <summary>{summary}</summary>
      <pre className="json-block">{JSON.stringify(value, null, 2)}</pre>
    </details>
  );
}

export interface PagerProps {
  offset: number;
  limit: number;
  total: number;
  onOffset: (offset: number) => void;
  /** Noun for the rows, plural (`orders`). */
  noun: string;
}

/** Previous/next paging over `total` rows. */
export function Pager({ offset, limit, total, onOffset, noun }: PagerProps) {
  const first = total === 0 ? 0 : offset + 1;
  const last = Math.min(offset + limit, total);
  return (
    <div className="row pager">
      <span className="mono dim">
        {fmtInt(first)}
        {'\u2013'}
        {fmtInt(last)} of {fmtInt(total)} {noun}
      </span>
      <button type="button" disabled={offset <= 0} onClick={() => onOffset(0)}>
        First
      </button>
      <button
        type="button"
        disabled={offset <= 0}
        onClick={() => onOffset(Math.max(0, offset - limit))}
      >
        Previous
      </button>
      <button
        type="button"
        disabled={offset + limit >= total}
        onClick={() => onOffset(offset + limit)}
      >
        Next
      </button>
    </div>
  );
}
