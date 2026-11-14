// Small data-display components shared by the dataset and run views.

import type { ReactNode } from 'react';

import type { Quantiles } from '../api/types';
import { DataTable, type Column } from './DataTable';
import { DASH, fmtInt } from './fmt';

export interface QuantileRow {
  label: string;
  /** Unit of the formatted values, shown in the `Unit` column (mixed-unit table, §6.1). */
  unit?: string;
  q: Quantiles | null;
  /** Formats one value of this row (durations, ticks, ...). */
  fmt: (x: number) => string;
}

const STATS = ['min', 'p50', 'p90', 'p99', 'max', 'mean'] as const;

const QUANTILE_COLUMNS: Column<QuantileRow>[] = [
  { id: 'measure', header: 'Measure', rowHeader: true, cell: (r) => r.label },
  { id: 'n', header: 'n', unit: 'count', numeric: true, cell: (r) => (r.q ? fmtInt(r.q.n) : DASH) },
  ...STATS.map<Column<QuantileRow>>((s) => ({
    id: s,
    header: s,
    numeric: true,
    cell: (r) => (r.q ? r.fmt(r.q[s]) : DASH),
  })),
];

/** Distribution summaries side by side. A row without data shows dashes and `n` reads 0 samples. */
export function QuantileTable({ label, rows }: { label: string; rows: QuantileRow[] }) {
  return <DataTable label={label} columns={QUANTILE_COLUMNS} rows={rows} rowKey={(r) => r.label} />;
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

/** Server paging: states exactly which rows of how many are shown, never implying completeness. */
export function Pager({ offset, limit, total, onOffset, noun }: PagerProps) {
  const first = total === 0 ? 0 : offset + 1;
  const last = Math.min(offset + limit, total);
  return (
    <div className="row pager">
      <span>
        showing {fmtInt(first)}
        {'\u2013'}
        {fmtInt(last)} of {fmtInt(total)} {noun}
      </span>
      <button type="button" disabled={offset <= 0} onClick={() => onOffset(0)}>
        First
      </button>
      <button type="button" disabled={offset <= 0} onClick={() => onOffset(Math.max(0, offset - limit))}>
        Previous
      </button>
      <button type="button" disabled={offset + limit >= total} onClick={() => onOffset(offset + limit)}>
        Next
      </button>
    </div>
  );
}
