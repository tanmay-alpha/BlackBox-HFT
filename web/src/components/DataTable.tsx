// Virtualized data table (UI_DESIGN_SPEC.md §6.1, §8.2). Sorting and the column model come from
// @tanstack/react-table, row windowing from @tanstack/react-virtual (above 200 rows, overscan 10,
// fixed row height from the density token). Semantics follow the APG grid pattern: role="grid",
// total aria-rowcount/aria-colcount, aria-rowindex per rendered row, aria-sort on headers.
// Keyboard: Up/Down, PageUp/PageDown, Home/End (Ctrl: first/last row) move the row focus; Enter
// activates the row (its primary link), Space selects it, Ctrl+C copies it as raw TSV.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import {
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef,
  type SortingState,
} from '@tanstack/react-table';
import { useVirtualizer } from '@tanstack/react-virtual';

import { rawText } from './fmt';
import type { GlossaryKey } from './glossary';
import { Definition, Tooltip } from './Tooltip';

export type SortValue = number | string | bigint | null | undefined;

export interface Column<T> {
  id: string;
  /** Header label (uppercase by style). */
  header: string;
  /** Unit shown in the header in fg-2 (`USDT`, `ms`, `count`); never repeated in cells. */
  unit?: string;
  /** Glossary entry shown as the header's tooltip. */
  term?: GlossaryKey;
  /** Right-aligned mono tabular numerals. */
  numeric?: boolean;
  cell: (row: T, index: number) => ReactNode;
  /** Enables sorting on this column. `null`/`undefined` sort last. */
  sortValue?: (row: T) => SortValue;
  /** Raw value for copies (ASCII minus, no separators); default: the cell's text, normalised. */
  raw?: (row: T) => string;
  /** Rendered as a row header (`th scope="row"`). */
  rowHeader?: boolean;
  /** Fixed width in px. */
  width?: number;
  /** Cell class (e.g. `up`/`down` for signed values). */
  className?: (row: T) => string | undefined;
}

export interface DataTableProps<T> {
  columns: Column<T>[];
  rows: readonly T[];
  rowKey: (row: T) => string;
  /** Accessible name of the grid. */
  label: string;
  /** Enter or double click on a row. */
  onActivate?: (row: T) => void;
  /** Click or Space on a row. */
  onSelect?: (row: T) => void;
  selectedKey?: string | null;
  initialSort?: SortingState;
  /** Shown under the table: paging and completeness (`showing 1–200 of 2,060`). */
  footer?: ReactNode;
  /** One line shown in the body when there are no rows. */
  empty?: ReactNode;
  /** Fill the parent's height (docked panels); otherwise grow up to `maxHeight`. */
  fill?: boolean;
  maxHeight?: number;
  rowClassName?: (row: T) => string | undefined;
  /** Rows above which only the visible window is rendered. */
  virtualizeAbove?: number;
}

const OVERSCAN = 10;

function compareSort(a: SortValue, b: SortValue): number {
  const an = a === null || a === undefined;
  const bn = b === null || b === undefined;
  if (an || bn) return an === bn ? 0 : an ? 1 : -1;
  if (typeof a === 'string' && typeof b === 'string') return a < b ? -1 : a > b ? 1 : 0;
  return a < b ? -1 : a > b ? 1 : 0;
}

function rowHeightPx(el: HTMLElement | null): number {
  if (!el) return 24;
  const v = Number.parseFloat(getComputedStyle(el).getPropertyValue('--h-row'));
  return Number.isFinite(v) && v > 0 ? v : 24;
}

export function DataTable<T>({
  columns,
  rows,
  rowKey,
  label,
  onActivate,
  onSelect,
  selectedKey = null,
  initialSort = [],
  footer,
  empty,
  fill = false,
  maxHeight,
  rowClassName,
  virtualizeAbove = 200,
}: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>(initialSort);
  const [focusIndex, setFocusIndex] = useState(0);
  const scrollRef = useRef<HTMLDivElement>(null);
  const pendingFocus = useRef(false);

  const defs = useMemo<ColumnDef<T>[]>(
    () =>
      columns.map((c) => ({
        id: c.id,
        accessorFn: c.sortValue ? (row: T) => c.sortValue!(row) : undefined,
        enableSorting: Boolean(c.sortValue),
        sortingFn: (a, b, id) => compareSort(a.getValue<SortValue>(id), b.getValue<SortValue>(id)),
        sortUndefined: 'last',
        header: c.header,
        cell: (ctx) => c.cell(ctx.row.original, ctx.row.index),
        meta: c,
      })),
    [columns],
  );

  const table = useReactTable<T>({
    data: rows as T[],
    columns: defs,
    state: { sorting },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getRowId: (row) => rowKey(row),
    enableMultiSort: true,
  });

  const model = table.getRowModel().rows;
  const virtual = model.length > virtualizeAbove;
  const [rowHeight, setRowHeight] = useState(24);
  // Re-read after every render: the density setting changes the token without remounting tables.
  useLayoutEffect(() => {
    const h = rowHeightPx(scrollRef.current);
    if (h !== rowHeight) setRowHeight(h);
  });

  const virtualizer = useVirtualizer({
    count: model.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: OVERSCAN,
    enabled: virtual,
  });

  const clampedFocus = Math.min(focusIndex, Math.max(0, model.length - 1));
  const items = virtual ? virtualizer.getVirtualItems() : null;
  const visible = items ? items.map((v) => v.index) : model.map((_, i) => i);
  const padTop = items && items.length > 0 ? items[0].start : 0;
  const padBottom = items && items.length > 0 ? virtualizer.getTotalSize() - items[items.length - 1].end : 0;

  // Move DOM focus after a keyboard move once the target row is rendered.
  useEffect(() => {
    if (!pendingFocus.current) return;
    const el = scrollRef.current?.querySelector<HTMLElement>(`tr[data-index="${clampedFocus}"]`);
    if (el) {
      pendingFocus.current = false;
      el.focus({ preventScroll: true });
      el.scrollIntoView({ block: 'nearest' });
    }
  });

  const moveTo = (i: number) => {
    const next = Math.max(0, Math.min(model.length - 1, i));
    setFocusIndex(next);
    pendingFocus.current = true;
    if (virtual) virtualizer.scrollToIndex(next, { align: 'auto' });
  };

  const copyRow = (index: number) => {
    const row = model[index];
    if (!row) return;
    const tr = scrollRef.current?.querySelector<HTMLElement>(`tr[data-index="${index}"]`);
    const cells = tr ? Array.from(tr.children) : [];
    const values = columns.map((c, ci) => (c.raw ? c.raw(row.original) : rawText(cells[ci]?.textContent ?? '')));
    void navigator.clipboard?.writeText(values.join('\t'));
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTableRowElement>, index: number) => {
    // Keys pressed in a control inside a cell (a button, link or input) belong to that control.
    if (e.target !== e.currentTarget) return;
    const page = Math.max(1, Math.floor((scrollRef.current?.clientHeight ?? rowHeight * 10) / rowHeight) - 1);
    switch (e.key) {
      case 'ArrowDown':
        moveTo(index + 1);
        break;
      case 'ArrowUp':
        moveTo(index - 1);
        break;
      case 'PageDown':
        moveTo(index + page);
        break;
      case 'PageUp':
        moveTo(index - page);
        break;
      case 'Home':
        moveTo(0);
        break;
      case 'End':
        moveTo(model.length - 1);
        break;
      case 'Enter':
        if (!onActivate) return;
        onActivate(model[index].original);
        break;
      case ' ':
        if (!onSelect) return;
        onSelect(model[index].original);
        break;
      case 'c':
        if (!(e.ctrlKey || e.metaKey)) return;
        if (window.getSelection()?.toString()) return;
        copyRow(index);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const headerGroup = table.getHeaderGroups()[0];
  const style = fill ? undefined : { maxHeight: maxHeight ?? 480 };

  return (
    <div className={fill ? 'dt dt-fill' : 'dt'}>
      <div className="dt-scroll" ref={scrollRef} style={style}>
        <table className="dt-table" role="grid" aria-label={label} aria-rowcount={model.length + 1} aria-colcount={columns.length}>
          <thead>
            <tr aria-rowindex={1}>
              {headerGroup.headers.map((h, ci) => {
                const c = h.column.columnDef.meta as Column<T>;
                const sorted = h.column.getIsSorted();
                const ariaSort = sorted === 'asc' ? 'ascending' : sorted === 'desc' ? 'descending' : c.sortValue ? 'none' : undefined;
                const labelNode = (
                  <>
                    <span className={c.term ? 'dt-label term' : 'dt-label'}>{c.header}</span>
                    {c.unit ? <span className="unit">{c.unit}</span> : null}
                    {sorted ? <span aria-hidden="true">{sorted === 'asc' ? ' \u25b4' : ' \u25be'}</span> : null}
                  </>
                );
                const inner = c.sortValue ? (
                  <button
                    type="button"
                    className="dt-sort"
                    onClick={(e) => h.column.toggleSorting(undefined, e.shiftKey)}
                    title={c.term ? undefined : 'Sort (Shift: add a secondary key)'}
                  >
                    {labelNode}
                  </button>
                ) : (
                  labelNode
                );
                return (
                  <th
                    key={h.id}
                    scope="col"
                    aria-colindex={ci + 1}
                    aria-sort={ariaSort}
                    className={c.numeric ? 'num' : undefined}
                    style={c.width ? { width: c.width } : undefined}
                  >
                    {c.term ? (
                      <Tooltip content={<Definition term={c.term} />} focusable={!c.sortValue}>
                        {inner}
                      </Tooltip>
                    ) : (
                      inner
                    )}
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {padTop > 0 ? (
              <tr aria-hidden="true" className="dt-pad" style={{ height: padTop }}>
                <td colSpan={columns.length} />
              </tr>
            ) : null}
            {model.length === 0 ? (
              <tr>
                <td colSpan={columns.length} className="dt-empty">
                  {empty ?? 'No rows.'}
                </td>
              </tr>
            ) : null}
            {visible.map((index) => {
              const row = model[index];
              const key = row.id;
              const selected = selectedKey !== null && key === selectedKey;
              const extra = rowClassName?.(row.original);
              return (
                <tr
                  key={key}
                  data-index={index}
                  aria-rowindex={index + 2}
                  aria-selected={onSelect ? selected : undefined}
                  tabIndex={index === clampedFocus ? 0 : -1}
                  className={[selected ? 'selected' : '', extra ?? ''].filter(Boolean).join(' ') || undefined}
                  onFocus={() => setFocusIndex(index)}
                  onClick={onSelect ? () => onSelect(row.original) : undefined}
                  onDoubleClick={onActivate ? () => onActivate(row.original) : undefined}
                  onKeyDown={(e) => onKeyDown(e, index)}
                >
                  {row.getVisibleCells().map((cell, ci) => {
                    const c = cell.column.columnDef.meta as Column<T>;
                    const cls = [c.numeric ? 'num' : '', c.className?.(row.original) ?? ''].filter(Boolean).join(' ') || undefined;
                    const content = flexRender(cell.column.columnDef.cell, cell.getContext());
                    return c.rowHeader ? (
                      <th key={cell.id} scope="row" aria-colindex={ci + 1} className={cls}>
                        {content}
                      </th>
                    ) : (
                      <td key={cell.id} aria-colindex={ci + 1} className={cls}>
                        {content}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
            {padBottom > 0 ? (
              <tr aria-hidden="true" className="dt-pad" style={{ height: padBottom }}>
                <td colSpan={columns.length} />
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      {footer ? <div className="dt-footer">{footer}</div> : null}
    </div>
  );
}
