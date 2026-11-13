// Latency Lab: several runs side by side, the first as baseline. Every number is a stored run
// metric or a row of the server's comparison; every interpretive sentence is the server's own
// interpretation note, rendered verbatim with its evidence basis.

import { useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';

import { api } from '../../api/client';
import type {
  Comparison,
  Demo,
  JsonValue,
  MetricRow,
  OrderPair,
  PairClass,
  RunSummary,
} from '../../api/types';
import { assumptionsLabel, strategyLabel } from '../../components/describe';
import { fmtDuration, fmtNum, fmtSigned, words } from '../../components/fmt';
import { PageHeader, usePageTitle } from '../../components/page';
import { Badge, BasisBadge, ClockText, Empty, ErrorBox, Panel, Spinner, useAsync } from '../../components/ui';
import { outcomeKind } from '../forensics/labels';
import { fmtQuoteSigned } from '../runs/format';
import '../compare/compare.css';
import './latency.css';

/** Comparison metric keys shown, in display order. Rows the server does not return are skipped. */
const KEY_METRICS = [
  'orders_submitted',
  'fill_count',
  'fill_ratio_orders',
  'time_to_first_fill_ns',
  'round_trip_ns',
  'cancels_sent',
  'cancels_rejected',
  'fees',
  'spread_capture',
  'spread_capture_ticks',
  'markout_100ms',
  'markout_1000ms',
  'markout_5000ms',
  'markout_30000ms',
  'realized_pnl',
  'unrealized_pnl',
  'net_pnl',
  'max_drawdown',
];

const PAIR_CLASSES: PairClass[] = [
  'identical',
  'same_decision_different_outcome',
  'different_decision',
  'only_a',
  'only_b',
];

const CLASS_LABEL: Record<PairClass, string> = {
  identical: 'Identical',
  same_decision_different_outcome: 'Same decision, different outcome',
  different_decision: 'Different decision',
  only_a: 'Only in baseline',
  only_b: 'Only in this run',
};

const UNIT_SUFFIX: Record<string, string> = {
  quote: 'quote',
  qty: 'base qty',
  count: 'count',
  ratio: '%',
  ns: 'time',
  ticks: 'ticks',
};

function fmtMetric(v: number | null, unit: string): string {
  if (v === null) return '\u2014';
  switch (unit) {
    case 'count':
      return fmtNum(v, 0);
    case 'ratio':
      return `${(v * 100).toFixed(2)}%`;
    case 'ns':
      return fmtDuration(v);
    case 'ticks':
      return fmtNum(v, 3);
    default:
      return fmtNum(v, 4);
  }
}

function fmtDelta(v: number | null, unit: string): string {
  if (v === null) return '\u2014';
  switch (unit) {
    case 'count':
      return fmtSigned(v, 0);
    case 'ratio':
      return `${fmtSigned(v * 100, 2)} pp`;
    case 'ns':
      return `${v > 0 ? '+' : ''}${fmtDuration(v)}`;
    case 'ticks':
      return fmtSigned(v, 3);
    default:
      return fmtSigned(v, 4);
  }
}

function jsonText(v: JsonValue): string {
  return typeof v === 'string' ? v : JSON.stringify(v);
}

function parseRuns(raw: string | null): string[] {
  if (!raw) return [];
  const seen = new Set<string>();
  for (const part of raw.split(',')) {
    const id = part.trim();
    if (id !== '') seen.add(id);
  }
  return [...seen];
}

type Settled<T> = { ok: true; value: T } | { ok: false; error: unknown };

function settle<T>(p: Promise<T>): Promise<Settled<T>> {
  return p.then(
    (value) => ({ ok: true, value }),
    (error: unknown) => ({ ok: false, error }),
  );
}

export default function LatencyLabPage() {
  usePageTitle('Latency Lab');
  const [params, setParams] = useSearchParams();
  const ids = parseRuns(params.get('runs'));
  const demoId = params.get('demo');
  const key = ids.join(',');

  const setIds = (next: string[]) => {
    const p = new URLSearchParams(params);
    if (next.length === 0) p.delete('runs');
    else p.set('runs', next.join(','));
    setParams(p);
  };

  const runs = useAsync(() => Promise.all(ids.map((id) => settle(api.run(id)))), [key]);
  const comparisons = useAsync(
    () =>
      ids.length < 2
        ? Promise.resolve([] as Settled<Comparison>[])
        : Promise.all(ids.slice(1).map((id) => settle(api.compare(ids[0], id)))),
    [key],
  );
  const baseRun = runs.data?.[0]?.ok ? runs.data[0].value : null;
  const baseDataset = baseRun?.manifest.dataset.id ?? null;
  const demos = useAsync(
    () => (demoId === null || baseDataset === null ? Promise.resolve(null) : api.demos(baseDataset)),
    [demoId, baseDataset],
  );
  const demo = demos.data?.find((d) => d.id === demoId) ?? null;

  return (
    <div className="col">
      <PageHeader
        title="Latency Lab"
        lead="Runs side by side on the same historical replay, the first as baseline. Differences are between simulations under different assumptions; they are not forecasts and say nothing about how the market would have reacted."
        actions={
          <Link className="btn" to="/">
            Mission Control
          </Link>
        }
      />
      <RunSetEditor ids={ids} onChange={setIds} />
      {demoId !== null ? (
        <DemoPanel demoId={demoId} demo={demo} loading={demos.loading} error={demos.error} />
      ) : null}
      {ids.length === 0 ? (
        <Empty>
          Choose at least two runs above, or run one of the canonical experiments from{' '}
          <Link to="/">Mission Control</Link>.
        </Empty>
      ) : null}
      {runs.loading ? <Spinner label="Loading runs" /> : null}
      {runs.error ? <ErrorBox error={runs.error} /> : null}
      {runs.data ? <RunCards ids={ids} runs={runs.data} /> : null}
      {ids.length === 1 ? <Empty>Add a second run to compare against the baseline.</Empty> : null}
      {comparisons.loading && ids.length >= 2 ? <Spinner label="Comparing each run with the baseline" /> : null}
      {comparisons.error ? <ErrorBox error={comparisons.error} /> : null}
      {comparisons.data && comparisons.data.length > 0 && runs.data ? (
        <Pairs ids={ids} runs={runs.data} comparisons={comparisons.data} />
      ) : null}
      {demoId === 'gross_vs_net' && runs.data ? <GrossToNet ids={ids} runs={runs.data} /> : null}
    </div>
  );
}

function RunSetEditor({ ids, onChange }: { ids: string[]; onChange: (ids: string[]) => void }) {
  const all = useAsync(() => api.runs(), []);
  const [pick, setPick] = useState('');
  const available = (all.data ?? []).filter((r) => !ids.includes(r.manifest.run_id));
  return (
    <details className="lat-editor" open={ids.length < 2}>
      <summary>Run set ({ids.length} runs)</summary>
      <div className="col">
        {ids.length > 0 ? (
          <ol className="lat-set">
            {ids.map((id, i) => (
              <li key={id} className="row">
                <span className="mono">{id}</span>
                {i === 0 ? <Badge kind="neutral">baseline</Badge> : null}
                {i > 0 ? (
                  <button type="button" onClick={() => onChange([id, ...ids.filter((x) => x !== id)])}>
                    Make baseline
                  </button>
                ) : null}
                <button type="button" onClick={() => onChange(ids.filter((x) => x !== id))}>
                  Remove
                </button>
              </li>
            ))}
          </ol>
        ) : null}
        {all.error ? <ErrorBox error={all.error} /> : null}
        <form
          className="row"
          onSubmit={(e) => {
            e.preventDefault();
            if (pick !== '') {
              onChange([...ids, pick]);
              setPick('');
            }
          }}
        >
          <label htmlFor="lat-add">Add run</label>
          <select id="lat-add" value={pick} onChange={(e) => setPick(e.target.value)} className="lat-select">
            <option value="">{all.loading ? 'Loading runs\u2026' : 'Choose a stored run'}</option>
            {available.map((r) => (
              <option key={r.manifest.run_id} value={r.manifest.run_id}>
                {r.label || 'unlabelled'} · {r.manifest.run_id} · {r.manifest.dataset.id}
              </option>
            ))}
          </select>
          <button type="submit" disabled={pick === ''}>
            Add
          </button>
        </form>
      </div>
    </details>
  );
}

function DemoPanel({
  demoId,
  demo,
  loading,
  error,
}: {
  demoId: string;
  demo: Demo | null;
  loading: boolean;
  error: Error | null;
}) {
  return (
    <Panel title={<>Experiment {demo ? demo.title : <span className="mono">{demoId}</span>}</>}>
      {loading ? <Spinner label="Loading experiment description" /> : null}
      {error ? <ErrorBox error={error} /> : null}
      {!loading && !error && !demo ? (
        <p className="note">
          The server lists no experiment <span className="mono">{demoId}</span> for the baseline
          run's dataset.
        </p>
      ) : null}
      {demo ? (
        <div className="col">
          <p className="lat-question">{demo.question}</p>
          <dl className="kv">
            <dt>Varied</dt>
            <dd>{demo.varied.join(', ') || 'nothing'}</dd>
            <dt>Controlled</dt>
            <dd className="prose">
              <ul className="lat-list">
                {demo.controlled.map((c) => (
                  <li key={c}>{c}</li>
                ))}
              </ul>
            </dd>
          </dl>
          <p className="note">{demo.how_to_read}</p>
        </div>
      ) : null}
    </Panel>
  );
}

function RunCards({ ids, runs }: { ids: string[]; runs: Settled<RunSummary>[] }) {
  return (
    <div className="lat-cards">
      {ids.map((id, i) => {
        const r = runs[i];
        return (
          <article key={id} className="lat-card">
            <header className="row">
              <Badge kind={i === 0 ? 'neutral' : 'modeled'}>{i === 0 ? 'baseline' : `run ${i + 1}`}</Badge>
              <span className="mono small">{id}</span>
            </header>
            {r && !r.ok ? <ErrorBox error={r.error} /> : null}
            {r && r.ok ? (
              <>
                <h3>{r.value.label || 'unlabelled'}</h3>
                <p className="mono small">
                  {strategyLabel(r.value.manifest.config.strategy.kind)} · {r.value.manifest.dataset.id}
                </p>
                <p className="small">{assumptionsLabel(r.value.manifest.config)}</p>
                <div className="row small">
                  <Link to={`/runs/${encodeURIComponent(id)}`}>Overview</Link>
                  <Link to={`/runs/${encodeURIComponent(id)}/replay`}>Replay Terminal</Link>
                </div>
              </>
            ) : null}
          </article>
        );
      })}
    </div>
  );
}

function runLabel(runs: Settled<RunSummary>[], ids: string[], i: number): string {
  const r = runs[i];
  return r && r.ok && r.value.label ? r.value.label : ids[i];
}

function Pairs({
  ids,
  runs,
  comparisons,
}: {
  ids: string[];
  runs: Settled<RunSummary>[];
  comparisons: Settled<Comparison>[];
}) {
  const ok = comparisons.map((c) => (c.ok ? c.value : null));
  return (
    <>
      {comparisons.map((c, i) =>
        c.ok ? null : (
          <div key={ids[i + 1]} className="col">
            <p className="note">
              Comparison of the baseline with <span className="mono">{ids[i + 1]}</span> failed:
            </p>
            <ErrorBox error={c.error} />
          </div>
        ),
      )}
      <VariablesPanel ids={ids} runs={runs} comparisons={ok} />
      <MetricsPanel ids={ids} runs={runs} comparisons={ok} />
      <AlignmentPanel ids={ids} runs={runs} comparisons={ok} />
      <InterpretationPanel ids={ids} runs={runs} comparisons={ok} />
    </>
  );
}

interface PairPanelProps {
  ids: string[];
  runs: Settled<RunSummary>[];
  comparisons: (Comparison | null)[];
}

function VariablesPanel({ ids, runs, comparisons }: PairPanelProps) {
  return (
    <Panel title="Varied versus controlled">
      <div className="col">
        {comparisons.map((cmp, i) =>
          cmp ? (
            <section key={ids[i + 1]} className="col">
              <h3>
                Baseline vs {runLabel(runs, ids, i + 1)}{' '}
                <Badge kind={cmp.same_dataset ? 'ok' : 'warn'}>
                  {cmp.same_dataset ? 'same dataset' : 'different dataset'}
                </Badge>{' '}
                <Badge kind={cmp.same_strategy ? 'ok' : 'warn'}>
                  {cmp.same_strategy ? 'same strategy' : 'different strategy'}
                </Badge>
              </h3>
              {cmp.config_diffs.length === 0 ? (
                <p className="note">The configurations are identical.</p>
              ) : (
                <div className="table-scroll">
                  <table className="table">
                    <caption>
                      Varied: every configuration path that differs. Everything else in the run
                      configuration is identical (controlled).
                    </caption>
                    <thead>
                      <tr>
                        <th scope="col">Configuration path</th>
                        <th scope="col">Baseline</th>
                        <th scope="col">This run</th>
                      </tr>
                    </thead>
                    <tbody>
                      {cmp.config_diffs.map((d) => (
                        <tr key={d.path}>
                          <th scope="row" className="mono">
                            {d.path}
                          </th>
                          <td className="mono wrap">{jsonText(d.a)}</td>
                          <td className="mono wrap">{jsonText(d.b)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </section>
          ) : null,
        )}
      </div>
    </Panel>
  );
}

function MetricsPanel({ ids, runs, comparisons }: PairPanelProps) {
  const first = comparisons.find((c) => c !== null) ?? null;
  if (!first) return null;
  const rowsByKey = comparisons.map((c) => new Map<string, MetricRow>((c?.metrics ?? []).map((r) => [r.key, r])));
  const keys = KEY_METRICS.filter((k) => rowsByKey.some((m) => m.has(k)));
  return (
    <Panel title={<>Key metrics <BasisBadge basis="derived" /></>}>
      <div className="table-scroll">
        <table className="table lat-metrics">
          <caption>
            Values and deltas exactly as returned by the comparison of each run with the baseline.
            Delta is this run minus baseline. A dash means the value is undefined for that run.
          </caption>
          <thead>
            <tr>
              <th scope="col">Metric</th>
              <th scope="col" className="num">
                Baseline
              </th>
              {ids.slice(1).map((id, i) => (
                <th key={id} scope="col" className="num" colSpan={2}>
                  {runLabel(runs, ids, i + 1)}
                  <div className="dim small">value · delta</div>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {keys.map((k) => {
              const any = rowsByKey.map((m) => m.get(k)).find((r) => r !== undefined) as MetricRow;
              return (
                <tr key={k}>
                  <th scope="row" title={k}>
                    {any.label} <span className="dim small">({UNIT_SUFFIX[any.unit] ?? any.unit})</span>
                  </th>
                  <td className="num">{fmtMetric(any.a, any.unit)}</td>
                  {rowsByKey.map((m, i) => {
                    const r = m.get(k);
                    return r ? (
                      <FragmentCells key={ids[i + 1]} row={r} />
                    ) : (
                      <td key={ids[i + 1]} colSpan={2} className="num dim">
                        {'\u2014'}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </Panel>
  );
}

function FragmentCells({ row }: { row: MetricRow }) {
  return (
    <>
      <td className="num">{fmtMetric(row.b, row.unit)}</td>
      <td className="num dim">{fmtDelta(row.delta, row.unit)}</td>
    </>
  );
}

function DivergenceSide({ label, pair, side }: { label: string; pair: OrderPair; side: 'a' | 'b' }) {
  const o = pair[side];
  if (!o) return <p className="small">{label}: no such decision.</p>;
  return (
    <p className="small">
      {label}: {o.side} {o.qty} @ tick {o.price_tick} ·{' '}
      <Badge kind={outcomeKind(o.outcome)}>{words(o.outcome)}</Badge> · filled {o.filled_qty} · arrival{' '}
      <ClockText ns={o.exch_arrival_ts} /> · first fill <ClockText ns={o.first_fill_ts} />
    </p>
  );
}

function AlignmentPanel({ ids, runs, comparisons }: PairPanelProps) {
  return (
    <Panel title="Decision alignment and first divergence">
      <div className="col">
        <p className="note">
          Orders are aligned by order id, which encodes the strategy tick and slot of the decision.
        </p>
        {comparisons.map((cmp, i) => {
          if (!cmp) return null;
          const al = cmp.alignment;
          const other = ids[i + 1];
          const total = PAIR_CLASSES.reduce((s, c) => s + al[c], 0);
          const fd = al.first_divergence;
          return (
            <section key={other} className="col lat-pair">
              <h3>Baseline vs {runLabel(runs, ids, i + 1)}</h3>
              {total > 0 ? (
                <div className="cmp-bar" aria-hidden="true">
                  {PAIR_CLASSES.map((c) =>
                    al[c] > 0 ? (
                      <span key={c} className={`cmp-seg cmp-seg-${c}`} style={{ flexGrow: al[c] }} title={`${CLASS_LABEL[c]}: ${al[c]}`} />
                    ) : null,
                  )}
                </div>
              ) : null}
              <ul className="row lat-classes small">
                {PAIR_CLASSES.map((c) => (
                  <li key={c}>
                    <span className={`cmp-swatch cmp-seg-${c}`} aria-hidden="true" /> {CLASS_LABEL[c]}:{' '}
                    <span className="mono">{al[c]}</span>
                  </li>
                ))}
              </ul>
              {fd ? (
                <div className="lat-divergence col">
                  <p>
                    First divergence: order <span className="mono">{fd.order_id}</span> (tick{' '}
                    {fd.tick_no}), decided at <ClockText ns={fd.decision_ts} /> ·{' '}
                    <Badge kind="neutral">{CLASS_LABEL[fd.class]}</Badge>
                  </p>
                  <DivergenceSide label="Baseline" pair={fd} side="a" />
                  <DivergenceSide label="This run" pair={fd} side="b" />
                  <div className="row small">
                    <Link to={`/runs/${encodeURIComponent(ids[0])}/replay?order=${fd.order_id}`}>
                      Baseline in Replay Terminal
                    </Link>
                    <Link to={`/runs/${encodeURIComponent(other)}/replay?order=${fd.order_id}`}>
                      This run in Replay Terminal
                    </Link>
                    {fd.a ? (
                      <Link to={`/runs/${encodeURIComponent(ids[0])}/orders/${fd.order_id}`}>Baseline forensics</Link>
                    ) : null}
                    {fd.b ? (
                      <Link to={`/runs/${encodeURIComponent(other)}/orders/${fd.order_id}`}>This run forensics</Link>
                    ) : null}
                    <Link to={`/compare?a=${encodeURIComponent(ids[0])}&b=${encodeURIComponent(other)}`}>
                      Full comparison
                    </Link>
                  </div>
                </div>
              ) : (
                <p className="note">No divergence: every aligned decision and outcome is identical.</p>
              )}
            </section>
          );
        })}
      </div>
    </Panel>
  );
}

function InterpretationPanel({ ids, runs, comparisons }: PairPanelProps) {
  return (
    <Panel title="Interpretation notes (from the comparison, verbatim)">
      <div className="col">
        {comparisons.map((cmp, i) =>
          cmp ? (
            <section key={ids[i + 1]} className="col">
              <h3>Baseline vs {runLabel(runs, ids, i + 1)}</h3>
              {cmp.interpretation.length === 0 ? (
                <p className="note">The comparison returned no interpretation notes.</p>
              ) : (
                <ul className="lat-notes">
                  {cmp.interpretation.map((n, k) => (
                    <li key={k}>
                      <BasisBadge basis={n.basis} />
                      <span>{n.statement}</span>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          ) : null,
        )}
      </div>
    </Panel>
  );
}

interface WaterfallStep {
  label: string;
  from: number;
  to: number;
  total?: boolean;
}

function GrossToNet({ ids, runs }: { ids: string[]; runs: Settled<RunSummary>[] }) {
  const rows = ids.flatMap((id, i) => {
    const r = runs[i];
    if (!r || !r.ok) return [];
    const m = r.value.metrics;
    const gross = m.realized_pnl + m.unrealized_pnl;
    const steps: WaterfallStep[] = [
      { label: 'Realized P&L', from: 0, to: m.realized_pnl },
      { label: 'Unrealized P&L', from: m.realized_pnl, to: gross },
      { label: 'Gross (realized + unrealized)', from: 0, to: gross, total: true },
      { label: 'Minus fees (a rebate adds)', from: gross, to: gross - m.fees },
      { label: 'Net P&L (reported)', from: 0, to: m.net_pnl, total: true },
    ];
    const gap = m.net_pnl - (gross - m.fees);
    return [{ id, label: runLabel(runs, ids, i), m, steps, gap }];
  });
  if (rows.length === 0) return null;
  const points = rows.flatMap((r) => [...r.steps.flatMap((s) => [s.from, s.to]), r.m.spread_capture, 0]);
  const lo = Math.min(...points);
  const hi = Math.max(...points);
  const span = hi - lo || 1;
  const pct = (v: number) => ((v - lo) / span) * 100;
  return (
    <Panel title={<>Gross to net, per run <BasisBadge basis="derived" /></>}>
      <div className="col">
        <p className="note">
          Built only from each run's reported metrics: realized and unrealized P&amp;L, fees and net
          P&amp;L (net = realized + unrealized − fees at the last account sample). Spread capture is
          shown separately for reference; it is a measure over maker fills, not a step of the sum.
          All bars share one scale.
        </p>
        {rows.map((r) => (
          <section key={r.id} className="col lat-waterfall">
            <h3>{r.label}</h3>
            {r.steps.map((s) => {
              const a = Math.min(s.from, s.to);
              const b = Math.max(s.from, s.to);
              const dir = s.total ? 'total' : s.to >= s.from ? 'up' : 'down';
              return (
                <div key={s.label} className="lat-wf-row">
                  <span className="lat-wf-label small">{s.label}</span>
                  <span className="lat-wf-track">
                    <span className="lat-wf-zero" style={{ left: `${pct(0)}%` }} aria-hidden="true" />
                    <span
                      className={`lat-wf-bar lat-wf-${dir}`}
                      style={{ left: `${pct(a)}%`, width: `max(1px, ${pct(b) - pct(a)}%)` }}
                    />
                  </span>
                  <span className="lat-wf-value mono small">
                    {s.total ? fmtQuoteSigned(s.to) : fmtQuoteSigned(s.to - s.from)}
                  </span>
                </div>
              );
            })}
            <div className="lat-wf-row lat-wf-ref">
              <span className="lat-wf-label small">Spread capture (reference)</span>
              <span className="lat-wf-track">
                <span className="lat-wf-zero" style={{ left: `${pct(0)}%` }} aria-hidden="true" />
                <span
                  className="lat-wf-bar lat-wf-refbar"
                  style={{
                    left: `${pct(Math.min(0, r.m.spread_capture))}%`,
                    width: `max(1px, ${Math.abs(pct(r.m.spread_capture) - pct(0))}%)`,
                  }}
                />
              </span>
              <span className="lat-wf-value mono small">{fmtQuoteSigned(r.m.spread_capture)}</span>
            </div>
            {Math.abs(r.gap) > 1e-9 * Math.max(1, Math.abs(r.m.net_pnl)) ? (
              <p className="notice">
                Reported net P&amp;L differs from realized + unrealized − fees by{' '}
                <span className="mono">{fmtSigned(r.gap, 8)}</span> (quote).
              </p>
            ) : null}
          </section>
        ))}
      </div>
    </Panel>
  );
}
