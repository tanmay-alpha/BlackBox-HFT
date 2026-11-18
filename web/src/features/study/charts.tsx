// Study charts: rows of labelled marks on one shared horizontal scale, drawn as positioned boxes
// so they reflow with the layout without measuring. Every chart prints its numbers next to the
// marks (and in a table for assistive technology); the marks only make differences visible.
// Colours encode the series, never good or bad.

import type { ReactNode } from 'react';

import { COMPARISON_LABEL, COMPARISON_MEANING, type ComparisonKind, type ComparisonLabel } from './explore';
import type { ChartRow, Decomposition } from './model';
import type { SeedStrip } from './views';

const SHORT: Record<ComparisonKind, string> = {
  paired: 'paired',
  metrics_only: 'metrics only',
  cross_dataset: 'cross-dataset',
  multi_seed: 'multi-seed',
  not_attributable: 'not attributable',
};

/** The comparison kinds of a value, as tags; `short` for chart rows. The meaning is on hover. */
export function ComparisonTags({ label, short = false }: { label: ComparisonLabel | null; short?: boolean }) {
  if (!label) return <span className="stc-cmp stc-cmp-reference">reference</span>;
  return (
    <span className="stc-cmps" data-testid="comparison-label" data-kinds={label.kinds.join(' ')}>
      {label.kinds.map((k) => (
        <span key={k} className={`stc-cmp stc-cmp-${k}`} title={`${COMPARISON_LABEL[k]}: ${COMPARISON_MEANING[k]} Compared with ${label.against}.`}>
          {short ? SHORT[k] : COMPARISON_LABEL[k]}
        </span>
      ))}
    </span>
  );
}

/** Row selection shared by the charts: the selected `?cell=` key and how to select one. */
export interface Pick {
  selected: string | null;
  onSelect: (key: string) => void;
}

function PickLabel({ id, pick, children, title }: { id: string; pick?: Pick; children: ReactNode; title?: string }) {
  if (!pick) return <>{children}</>;
  return (
    <button type="button" className="stc-pick" aria-pressed={pick.selected === id} onClick={() => pick.onSelect(id)} title={title ?? 'Show this cell'}>
      {children}
    </button>
  );
}

/** Padded value range covering every number in `values` (and zero when `withZero`). */
function extent(values: number[], withZero: boolean): [number, number] {
  let lo = Math.min(...values, withZero ? 0 : Infinity);
  let hi = Math.max(...values, withZero ? 0 : -Infinity);
  if (!Number.isFinite(lo) || !Number.isFinite(hi)) return [0, 1];
  if (hi === lo) {
    const pad = Math.abs(lo) > 0 ? Math.abs(lo) * 0.05 : 1;
    return [lo - pad, hi + pad];
  }
  const pad = (hi - lo) * 0.06;
  return [lo - pad, hi + pad];
}

/** Position of `v` on the scale, as a CSS percentage. */
function percent(v: number, [lo, hi]: [number, number]): string {
  return `${(((v - lo) / (hi - lo)) * 100).toFixed(3)}%`;
}

/** Length `len` (in value units) on the scale, as a CSS percentage. */
function lengthPercent(len: number, [lo, hi]: [number, number]): string {
  return `${((len / (hi - lo)) * 100).toFixed(3)}%`;
}

export interface IntervalChartProps {
  rows: ChartRow[];
  fmt: (v: number) => string;
  /** What is drawn and its unit, for the table caption. */
  caption: string;
  /** Badge row above the chart: data class and synthetic labels. */
  labels: ReactNode;
  testId?: string;
  pick?: Pick;
  /** Comparison kinds of a row against its reference (B3). */
  labelOf?: (row: ChartRow) => ComparisonLabel | null;
}

/**
 * One dot per row; a whisker where the value is a seed-variation interval. A dashed rule marks the
 * reference row's value, a solid one zero when it is in range. Row labels select the row's cell.
 */
export function IntervalChart({ rows, fmt, caption, labels, testId, pick, labelOf }: IntervalChartProps) {
  const drawn = rows.filter((r) => r.point !== null);
  const values = drawn.flatMap((r) => (r.point!.interval ? [r.point!.value, ...r.point!.interval] : [r.point!.value]));
  const domain = extent(values, false);
  const reference = rows.find((r) => r.reference)?.point ?? null;
  const zeroIn = domain[0] < 0 && domain[1] > 0;
  return (
    <figure className="stc-chart" data-testid={testId}>
      <div className="row stc-chart-labels">{labels}</div>
      <table className="stc-rows">
        <caption className="visually-hidden">{caption}</caption>
        <thead className="visually-hidden">
          <tr>
            <th scope="col">Group</th>
            <th scope="col">Value</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.key} className={`${r.reference ? 'stc-ref' : ''}${pick?.selected === r.key ? ' stc-selected' : ''}`} data-cell={r.key}>
              <th scope="row" className="stc-label">
                <PickLabel id={r.key} pick={pick}>
                  {r.label}
                  {r.reference ? <span className="dim"> (reference)</span> : null}
                </PickLabel>
                {labelOf ? (
                  <div>
                    <ComparisonTags label={labelOf(r)} short />
                  </div>
                ) : null}
              </th>
              <td className="stc-track-cell" aria-hidden="true" onClick={pick ? () => pick.onSelect(r.key) : undefined}>
                <div className={pick ? 'stc-track stc-track-pick' : 'stc-track'}>
                  {zeroIn ? <span className="stc-zero" style={{ left: percent(0, domain) }} /> : null}
                  {reference ? <span className="stc-refline" style={{ left: percent(reference.value, domain) }} /> : null}
                  {r.point?.interval ? (
                    <span
                      className="stc-whisker"
                      style={{ left: percent(r.point.interval[0], domain), width: lengthPercent(r.point.interval[1] - r.point.interval[0], domain) }}
                    />
                  ) : null}
                  {r.point ? <span className={r.point.interval ? 'stc-dot stc-dot-seed' : 'stc-dot'} style={{ left: percent(r.point.value, domain) }} /> : null}
                </div>
              </td>
              <td className="stc-value mono num">
                {r.point === null ? 'unavailable' : fmt(r.point.value)}
                {r.point?.interval ? (
                  <>
                    {' '}
                    <span className="stc-ci">
                      [{fmt(r.point.interval[0])}, {fmt(r.point.interval[1])}]
                    </span>{' '}
                    <span className="dim stc-ci">{r.point.n} seeds</span>
                  </>
                ) : null}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <figcaption className="note small">
        Scale {fmt(domain[0])} to {fmt(domain[1])}. Dots: values; whiskers: 95% seed-variation interval (only for stochastic latency
        models); dashed rule: the reference row.{pick ? ' Select a row to open its cell.' : ''}
      </figcaption>
    </figure>
  );
}

/**
 * Every seed value of each stochastic group on its own row and scale: one mark per seed (each
 * selects its run), the seed mean as a tick and the published 95% bootstrap interval as a band.
 */
export function SeedStripChart({ strips, fmt, pick, testId }: { strips: SeedStrip[]; fmt: (v: number) => string; pick: Pick; testId?: string }) {
  return (
    <figure className="stc-chart" data-testid={testId}>
      <table className="stc-rows stc-strips">
        <caption className="visually-hidden">Seed values of each stochastic group, its seed mean and 95% seed-variation interval</caption>
        <thead className="visually-hidden">
          <tr>
            <th scope="col">Group</th>
            <th scope="col">Seed marks</th>
            <th scope="col">Seed values</th>
          </tr>
        </thead>
        <tbody>
          {strips.map((s) => {
            const domain = extent(s.points.map((p) => p.value).concat(s.interval), false);
            return (
              <tr key={s.key} className={pick.selected === s.key ? 'stc-selected' : undefined} data-cell={s.key}>
                <th scope="row" className="stc-label">
                  <PickLabel id={s.key} pick={pick}>
                    {s.label}
                  </PickLabel>
                </th>
                <td className="stc-track-cell">
                  <div className="stc-track stc-strip">
                    <span
                      className="stc-band"
                      aria-hidden="true"
                      style={{ left: percent(s.interval[0], domain), width: lengthPercent(s.interval[1] - s.interval[0], domain) }}
                    />
                    <span className="stc-mean" aria-hidden="true" style={{ left: percent(s.mean, domain) }} />
                    {s.points.map((p) => (
                      <button
                        key={p.runId}
                        type="button"
                        className="stc-seed"
                        tabIndex={-1}
                        aria-hidden="true"
                        data-selected={pick.selected === p.key ? 'true' : undefined}
                        style={{ left: percent(p.value, domain) }}
                        title={`Seed ${p.seed}: ${fmt(p.value)} (run ${p.runId})`}
                        onClick={() => pick.onSelect(p.key)}
                      />
                    ))}
                  </div>
                  <div className="dim small mono stc-strip-scale">
                    {fmt(domain[0])} to {fmt(domain[1])}
                  </div>
                </td>
                <td className="stc-value mono num">
                  <div>
                    mean {fmt(s.mean)}{' '}
                    <span className="stc-ci">
                      [{fmt(s.interval[0])}, {fmt(s.interval[1])}]
                    </span>
                  </div>
                  <div className="stc-seedvals small">
                    {s.points.map((p) => (
                      <button key={p.runId} type="button" className="stc-pick stc-ci" aria-pressed={pick.selected === p.key} onClick={() => pick.onSelect(p.key)}>
                        s{p.seed} {fmt(p.value)}
                      </button>
                    ))}
                  </div>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <figcaption className="note small">
        Each row has its own scale (printed under the marks). Marks: one simulated run per latency seed; tick: seed mean; band: 95%
        percentile-bootstrap interval of that mean. Seed variation of the latency model on one generated market only.
      </figcaption>
    </figure>
  );
}

const PARTS = [
  { key: 'gross', label: 'Gross P&L' },
  { key: 'fees', label: 'Fees (cost)' },
  { key: 'net', label: 'Net P&L' },
] as const;

/** Gross P&L, fees (drawn as a cost below zero) and net P&L per group, on one signed scale. */
export function DecompositionChart({ rows, fmt, labels, pick }: { rows: Decomposition[]; fmt: (v: number) => string; labels: ReactNode; pick?: Pick }) {
  const signed = (r: Decomposition, key: (typeof PARTS)[number]['key']) => (key === 'fees' ? -r.fees : r[key]);
  const domain = extent(
    rows.flatMap((r) => PARTS.map((p) => signed(r, p.key))),
    true,
  );
  const zero = percent(0, domain);
  return (
    <figure className="stc-chart" data-testid="study-decomposition">
      <div className="row stc-chart-labels">{labels}</div>
      <ul className="row stc-key small" aria-label="Series">
        {PARTS.map((p) => (
          <li key={p.key}>
            <span className={`stc-swatch stc-part-${p.key}`} aria-hidden="true" /> {p.label}
          </li>
        ))}
      </ul>
      <table className="stc-rows">
        <caption className="visually-hidden">Gross P&L, fees and net P&L per study group, quote currency, mean over the group&apos;s runs</caption>
        <thead className="visually-hidden">
          <tr>
            <th scope="col">Group</th>
            <th scope="col">Bars</th>
            <th scope="col">Gross, fees, net</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.key} className={pick?.selected === r.key ? 'stc-selected' : undefined} data-cell={r.key}>
              <th scope="row" className="stc-label">
                <PickLabel id={r.key} pick={pick}>
                  {r.label}
                  {r.n > 1 ? <span className="dim"> (mean of {r.n} seeds)</span> : null}
                </PickLabel>
              </th>
              <td className="stc-track-cell" aria-hidden="true">
                <div className="stc-track stc-track-bars">
                  <span className="stc-zero" style={{ left: zero }} />
                  {PARTS.map((p, i) => {
                    const v = signed(r, p.key);
                    const from = Math.min(0, v);
                    return (
                      <span
                        key={p.key}
                        className={`stc-bar stc-part-${p.key}`}
                        style={{ left: percent(from, domain), width: lengthPercent(Math.abs(v), domain), top: `${2 + i * 6}px` }}
                      />
                    );
                  })}
                </div>
              </td>
              <td className="stc-value mono num">
                <span className="stc-ci">{fmt(r.gross)}</span> <span className="dim">-</span> <span className="stc-ci">{fmt(r.fees)}</span>{' '}
                <span className="dim">=</span> <span className="stc-ci">{fmt(r.net)}</span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <figcaption className="note small">
        Scale {fmt(domain[0])} to {fmt(domain[1])} quote currency. Net = gross (realized + unrealized at the end of the session) - fees,
        per run, then averaged over the group&apos;s runs.
      </figcaption>
    </figure>
  );
}
