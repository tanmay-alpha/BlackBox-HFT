// Risk & Performance (UI_DESIGN_SPEC.md §11.7). Everything shown is served by the v1 run
// endpoints: `GET /runs/{id}` (bb-core `Metrics`) and `GET /runs/{id}/equity` (account samples or
// downsampled buckets). Series derived here (drawdown) are computed from the served equity samples
// only and labelled DERIVED with their formula. The only figure combined in the browser is gross
// trading P&L = net P&L + fees, both served. Caveats follow docs/research/MARKET_MICROSTRUCTURE.md
// §3 and §5.3: one path of a replay without market impact, no annualised Sharpe or Sortino, no
// ratio extrapolated beyond the data span.
//
// Reading order: the answer (net result and what made it), the charts, then every metric.

import { useMemo, type ReactNode } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';

import { api } from '../../api/client';
import { isBucketed, type Comparison, type EquityBucket, type EquityPage, type Run, type RunDetail } from '../../api/model';
import type { Basis, EquitySample, Metrics } from '../../api/types';
import { DataTable, type Column } from '../../components/DataTable';
import { strategyName } from '../../components/describe';
import { QuantileTable } from '../../components/data';
import { DASH, fmtDuration, fmtInt, fmtNum, fmtPct, fmtPnl, fmtTicks } from '../../components/fmt';
import { cssVar } from '../../components/theme';
import { Badge, BasisBadge, BASIS_MEANING, Empty, ErrorBox, Notice, Panel, Spinner, useAsync } from '../../components/ui';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { RunContextLinks } from '../runs/RunContextLinks';
import { lastPerSecond, TimeSeriesChart, type ChartSeries } from '../runs/TimeSeriesChart';
import { ExecutionAnalytics, RiskAnalytics } from './AnalyticsPanels';
import './risk.css';

const MAX_POINTS = 5000;

/** Units as words, so a reader never has to guess what "quote" means. */
const UNIT = {
  quote: 'quote currency',
  base: 'base quantity',
  count: 'count',
  ratio: 'percent',
  ticks: 'ticks',
} as const;

/** Drawdown from the running peak of the served equity, per sample (DERIVED). */
export function drawdownSeries<T>(items: readonly T[], equity: (x: T) => number, peakOf: (x: T) => number = equity): number[] {
  let peak = Number.NEGATIVE_INFINITY;
  return items.map((x) => {
    peak = Math.max(peak, peakOf(x));
    return peak - equity(x);
  });
}

/** Time covered by the served equity series, in ns (first to last sample or bucket). */
function equitySpanNs(page: EquityPage): number | null {
  if (page.items.length === 0) return null;
  if (isBucketed(page)) {
    const b = page.items as EquityBucket[];
    return b[b.length - 1].t1_ns - b[0].t0_ns;
  }
  const s = page.items as EquitySample[];
  return s[s.length - 1].ts - s[0].ts;
}

function RunPicker() {
  const navigate = useNavigate();
  const runs = useAsync((signal) => api.runs({}, { signal }), []);
  const columns = useMemo<Column<Run>[]>(
    () => [
      { id: 'run', header: 'Run', rowHeader: true, cell: (r) => <Link className="mono" to={paths.riskRun(r.run_id)}>{r.run_id}</Link>, raw: (r) => r.run_id },
      { id: 'label', header: 'Label', cell: (r) => r.label || <span className="dim">unlabelled</span>, sortValue: (r) => r.label ?? '' },
      { id: 'strategy', header: 'Strategy', cell: (r) => <span>{strategyName(r.strategy_kind)} every {fmtDuration(r.interval_ns)}</span> },
      { id: 'dataset', header: 'Dataset', cell: (r) => <span className="mono">{r.dataset.id} v{r.dataset.version}</span> },
      { id: 'net', header: 'Net P&L', unit: 'quote', numeric: true, cell: (r) => fmtPnl(r.metrics.net_pnl), sortValue: (r) => r.metrics.net_pnl },
      { id: 'dd', header: 'Max drawdown', unit: 'quote', numeric: true, cell: (r) => fmtNum(r.metrics.max_drawdown, 4), sortValue: (r) => r.metrics.max_drawdown },
    ],
    [],
  );
  return (
    <Workspace toolbar={<span className="note">Pick a run to see its risk and performance.</span>}>
      <Panel title="Runs" fill flush caption={runs.data?.incomplete ? `List incomplete: first ${runs.data.items.length} runs shown.` : undefined}>
        {runs.loading ? <Spinner label="Loading runs" /> : null}
        <ErrorBox error={runs.error} onRetry={runs.reload} />
        {runs.data ? (
          <DataTable
            label="Runs"
            columns={columns}
            rows={runs.data.items}
            rowKey={(r) => r.run_id}
            onActivate={(r) => navigate(paths.riskRun(r.run_id))}
            fill
            empty={<Empty>No completed runs are visible in this workspace.</Empty>}
          />
        ) : null}
      </Panel>
    </Workspace>
  );
}

// ----- answer ----------------------------------------------------------------------------------

/** "gain" / "loss" / "no change" by sign: a smaller loss is still a loss. */
function outcomeWord(x: number): string {
  return x > 0 ? 'gain' : x < 0 ? 'loss' : 'no change';
}

function Figure({ label, value, unit, basis, detail }: { label: string; value: string; unit: string; basis: Basis; detail: ReactNode }) {
  return (
    <div className="risk-figure">
      <dt>
        {label} <BasisBadge basis={basis} />
      </dt>
      <dd className="risk-figure-value mono">{value}</dd>
      <dd className="risk-figure-detail">
        <span className="risk-figure-unit">{unit}</span> · {detail}
      </dd>
    </div>
  );
}

/** Stored comparisons that include this run, like-for-like ones first. */
function ComparisonLinks({ runId }: { runId: string }) {
  const list = useAsync((signal) => api.readableComparisons({ signal }), []);
  if (list.error) return null;
  if (!list.data) return <span className="dim">Looking for comparisons with this run…</span>;
  const mine = list.data.items
    .filter((c) => c.run_a === runId || c.run_b === runId)
    .sort((x, y) => Number(y.eligibility.like_for_like) - Number(x.eligibility.like_for_like));
  if (mine.length === 0) {
    return (
      <span className="dim">
        No stored comparison includes this run. <Link to={paths.comparePair(runId)}>Compare with another run</Link>
      </span>
    );
  }
  return (
    <ul className="risk-compare-links" aria-label="Comparisons with this run">
      {mine.slice(0, 4).map((c) => {
        const o = c.run_a === runId ? { id: c.run_b, label: c.label_b } : { id: c.run_a, label: c.label_a };
        return (
          <li key={c.id}>
            <Link to={paths.comparePair(c.run_a, c.run_b)}>
              Compare with {o.label || <span className="mono">{o.id}</span>}
            </Link>{' '}
            {c.eligibility.like_for_like ? <Badge kind="ok">like-for-like</Badge> : <Badge kind="warn">not like-for-like</Badge>}
          </li>
        );
      })}
    </ul>
  );
}

function Answer({ run, synthetic, spanNs }: { run: RunDetail; synthetic: boolean | null; spanNs: number | null }) {
  const m = run.metrics;
  const gross = m.net_pnl + m.fees;
  const filled = m.orders_filled + m.orders_partially_filled;
  const caveats = [
    synthetic === null ? null : synthetic ? 'Synthetic data, not a market recording' : 'Recorded market data',
    'fills and queue position are modeled',
    'no market impact',
    spanNs === null ? 'one short sample' : `one ${fmtDuration(spanNs)} sample`,
    'nothing annualised',
  ].filter((x): x is string => x !== null);
  return (
    <section className="risk-answer" aria-labelledby="risk-answer-title">
      <h2 id="risk-answer-title" className="risk-answer-title">
        Simulated net {outcomeWord(m.net_pnl)} of <span className="mono">{fmtNum(Math.abs(m.net_pnl), 4)}</span> {UNIT.quote}
      </h2>
      <p className="risk-answer-lead">
        Before fees, trading made a {outcomeWord(gross)} of <span className="mono">{fmtNum(Math.abs(gross), 4)}</span>;{' '}
        {m.fees >= 0 ? (
          <>
            modeled fees cost <span className="mono">{fmtNum(m.fees, 4)}</span>
          </>
        ) : (
          <>
            modeled fees paid a net rebate of <span className="mono">{fmtNum(-m.fees, 4)}</span>
          </>
        )}
        . Net P&amp;L = gross trading P&amp;L − fees.
      </p>
      <dl className="risk-figures">
        <Figure label="Net P&L" value={fmtPnl(m.net_pnl)} unit={UNIT.quote} basis="derived" detail={`realized ${fmtPnl(m.realized_pnl)} + unrealized ${fmtPnl(m.unrealized_pnl)} − fees ${fmtNum(m.fees, 4)}`} />
        <Figure label="Gross trading P&L" value={fmtPnl(gross)} unit={UNIT.quote} basis="derived" detail="realized + unrealized, before fees; includes open inventory" />
        <Figure label="Fees" value={fmtNum(m.fees, 4)} unit={UNIT.quote} basis="modeled" detail="positive is charged, negative is a rebate" />
        <Figure label="Max drawdown" value={fmtNum(m.max_drawdown, 4)} unit={UNIT.quote} basis="derived" detail="running peak minus sampled net equity; an amount, not %" />
        <Figure label="Fill ratio" value={fmtPct(m.fill_ratio_orders)} unit="of orders" basis="modeled" detail={`${fmtInt(filled)} of ${fmtInt(m.orders_submitted)} submitted orders with any fill, as of run end`} />
        <Figure
          label="Inventory at end"
          value={fmtNum(m.final_position, 4)}
          unit={UNIT.base}
          basis="derived"
          detail={`mean |position| ${fmtNum(m.mean_abs_position, 4)}; max long ${fmtNum(m.max_long, 4)}, max short ${fmtNum(m.max_short, 4)}`}
        />
      </dl>
      <p className="risk-caveat">
        <span className="risk-caveat-tag">SIMULATED</span> {caveats.join(' · ')}.{m.risk_halted ? ' The strategy was halted by a risk rule.' : ''}
      </p>
      <div className="risk-answer-compare">
        <ComparisonLinks runId={run.run_id} />
      </div>
    </section>
  );
}

// ----- charts ----------------------------------------------------------------------------------

function Charts({ page }: { page: EquityPage }) {
  const charts = useMemo(() => {
    if (isBucketed(page)) {
      const b = page.items as EquityBucket[];
      const at = (x: EquityBucket) => x.t1_ns;
      const dd = drawdownSeries(b, (x) => x.min_equity, (x) => x.max_equity);
      return {
        equity: [
          { id: 'last', label: 'Equity, last in bucket', color: cssVar('--derived'), points: lastPerSecond(b, (x) => x.last, at) },
          { id: 'min', label: 'Bucket minimum', color: cssVar('--fg-2'), dashed: true, points: lastPerSecond(b, (x) => x.min_equity, at) },
        ] as ChartSeries[],
        drawdown: [{ id: 'dd', label: 'Drawdown upper bound', color: cssVar('--down'), points: lastPerSecond(b.map((x, i) => ({ t: at(x), v: dd[i] })), (p) => p.v, (p) => p.t) }] as ChartSeries[],
        position: null,
        fees: null,
      };
    }
    const s = page.items as EquitySample[];
    const ts = (x: EquitySample) => x.ts;
    const dd = drawdownSeries(s, (x) => x.equity);
    return {
      equity: [{ id: 'equity', label: 'Net equity (quote)', color: cssVar('--derived'), points: lastPerSecond(s, (x) => x.equity, ts) }] as ChartSeries[],
      drawdown: [{ id: 'dd', label: 'Drawdown from running peak', color: cssVar('--down'), points: lastPerSecond(s.map((x, i) => ({ t: x.ts, v: dd[i] })), (p) => p.v, (p) => p.t) }] as ChartSeries[],
      position: [
        { id: 'position', label: 'Position (exchange-side fills)', color: cssVar('--fg-0'), stepped: true, points: lastPerSecond(s, (x) => x.position, ts) },
        { id: 'known', label: 'Position known to strategy', color: cssVar('--accent'), stepped: true, dashed: true, points: lastPerSecond(s, (x) => x.known_position, ts) },
      ] as ChartSeries[],
      fees: [{ id: 'fees', label: 'Cumulative fees', color: cssVar('--warn'), points: lastPerSecond(s, (x) => x.fees, ts) }] as ChartSeries[],
    };
  }, [page]);
  return (
    <>
      {page.downsampled ? (
        <Notice tag="DOWNSAMPLED">
          The server returned {fmtInt(page.items.length)} buckets of {fmtDuration(page.bucket_ns)} instead of every sample. Equity shows each bucket's last value at its end;
          drawdown is an upper bound (running max of bucket maxima minus bucket minimum). Position and fees need raw samples and are not shown.
        </Notice>
      ) : null}
      <div className="risk-charts">
        <Panel title="Equity" basis="derived" caption={`Net P&L in ${UNIT.quote}: realized + unrealized − fees, exchange-truth fills marked at the local mid.`}>
          <TimeSeriesChart series={charts.equity} formatValue={(v) => fmtPnl(v, 2)} ariaLabel="Simulated net equity over time after dataset origin" />
        </Panel>
        <Panel title="Drawdown" basis="derived" caption={`Running peak minus sampled net equity, in ${UNIT.quote} (not %).`}>
          <TimeSeriesChart series={charts.drawdown} formatValue={(v) => fmtNum(v, 4)} ariaLabel="Drawdown from the running equity peak over time" />
        </Panel>
        {charts.position ? (
          <Panel title="Position / inventory" basis="modeled" caption={`Position in ${UNIT.base}; fills are modeled.`}>
            <TimeSeriesChart series={charts.position} formatValue={(v) => fmtNum(v, 4)} ariaLabel="Simulated position over time" />
          </Panel>
        ) : null}
        {charts.fees ? (
          <Panel title="Fees" basis="modeled" caption={`Cumulative modeled fees in ${UNIT.quote}.`}>
            <TimeSeriesChart series={charts.fees} formatValue={(v) => fmtNum(v, 4)} ariaLabel="Cumulative modeled fees over time" />
          </Panel>
        ) : null}
      </div>
      <p className="note">
        Local sample clock, relative to the dataset origin; each series shows the last point per second (no averaging), the chart library's time resolution.
      </p>
    </>
  );
}

// ----- full metric tables ----------------------------------------------------------------------

interface MetricRow {
  key: string;
  label: string;
  value: string;
  unit: string;
  basis: Basis;
  caveat: string;
}

function runMetricRows(m: Metrics, sampledNote: string): MetricRow[] {
  return [
    {
      key: 'net',
      label: 'Net P&L',
      value: fmtPnl(m.net_pnl),
      unit: UNIT.quote,
      basis: 'derived',
      caveat: `Net = realized P&L ${fmtPnl(m.realized_pnl)} + unrealized P&L ${fmtPnl(m.unrealized_pnl)} − fees ${fmtNum(m.fees, 4)}. Realized and unrealized are before fees.`,
    },
    { key: 'gross', label: 'Gross trading P&L (before fees)', value: fmtPnl(m.net_pnl + m.fees), unit: UNIT.quote, basis: 'derived', caveat: 'Realized + unrealized P&L, before fees; includes open inventory. Equals net P&L + fees.' },
    { key: 'fees', label: 'Fees', value: fmtNum(m.fees, 4), unit: UNIT.quote, basis: 'modeled', caveat: 'Quote currency; positive is charged, negative is a rebate. From the configured maker/taker rates.' },
    { key: 'turnover', label: 'Turnover', value: fmtNum(m.turnover, 2), unit: UNIT.quote, basis: 'derived', caveat: 'Traded notional.' },
    { key: 'volume', label: 'Volume', value: fmtNum(m.volume, 4), unit: UNIT.base, basis: 'derived', caveat: 'Traded quantity.' },
    { key: 'dd', label: 'Maximum drawdown', value: fmtNum(m.max_drawdown, 4), unit: UNIT.quote, basis: 'derived', caveat: `Peak to trough on the equity series as sampled (${sampledNote}). Path-dependent; not a risk forecast.` },
    { key: 'mean_abs', label: 'Mean |position|', value: fmtNum(m.mean_abs_position, 4), unit: UNIT.base, basis: 'derived', caveat: 'Inventory risk, time-averaged over samples.' },
    { key: 'max_long', label: 'Max long / max short', value: `${fmtNum(m.max_long, 4)} / ${fmtNum(m.max_short, 4)}`, unit: UNIT.base, basis: 'derived', caveat: '' },
    { key: 'final', label: 'Final position', value: fmtNum(m.final_position, 4), unit: UNIT.base, basis: 'derived', caveat: 'Open inventory; unrealized P&L uses the last valid local mid.' },
    { key: 'risk_blocks', label: 'Risk blocks', value: fmtInt(m.risk_blocks), unit: UNIT.count, basis: 'modeled', caveat: m.risk_halted ? 'The strategy was HALTED by a risk rule.' : 'No risk halt.' },
  ];
}

function executionRows(m: Metrics): MetricRow[] {
  const submitted = m.orders_submitted;
  return [
    { key: 'orders', label: 'Orders submitted', value: fmtInt(submitted), unit: UNIT.count, basis: 'modeled', caveat: `${fmtInt(m.orders_intended)} intended, ${fmtInt(m.orders_risk_blocked)} risk-blocked, ${fmtInt(m.orders_refused)} refused.` },
    { key: 'fills', label: 'Fills', value: fmtInt(m.fill_count), unit: UNIT.count, basis: 'modeled', caveat: `${fmtInt(m.maker_fill_count)} maker, ${fmtInt(m.taker_fill_count)} taker; from the engine's queue and exchange models.` },
    {
      key: 'fill_ratio_orders',
      label: 'Fill ratio (orders)',
      value: fmtPct(m.fill_ratio_orders),
      unit: UNIT.ratio,
      basis: 'modeled',
      caveat: `${fmtInt(m.orders_filled)} filled + ${fmtInt(m.orders_partially_filled)} partially filled over ${fmtInt(submitted)} submitted. End-of-run fraction, not a censoring-adjusted probability. Outstanding, partially filled and in-flight orders may still change after the replay ends.`,
    },
    { key: 'fill_ratio_qty', label: 'Fill ratio (quantity)', value: fmtPct(m.fill_ratio_qty), unit: UNIT.ratio, basis: 'modeled', caveat: 'Filled over submitted quantity. End-of-run fraction, not a censoring-adjusted probability. Outstanding, partially filled and in-flight orders may still change after the replay ends.' },
    {
      key: 'spread_capture',
      label: 'Spread capture',
      value: fmtNum(m.spread_capture, 4),
      unit: UNIT.quote,
      basis: 'derived',
      caveat: 'Against the mid at fill time t, same-timestamp book batch included; on recorded data this choice can flip the sign.',
    },
    { key: 'spread_capture_ticks', label: 'Spread capture per unit', value: fmtTicks(m.spread_capture_ticks), unit: UNIT.ticks, basis: 'derived', caveat: 'Same reference mid "at t" as above.' },
    ...m.markouts.map((k) => ({
      key: `markout_${k.horizon_ns}`,
      label: `Markout ${fmtDuration(k.horizon_ns)}`,
      value: fmtTicks(k.mean_ticks),
      unit: UNIT.ticks,
      basis: 'derived' as const,
      caveat: `Quantity-weighted mean over ${fmtInt(k.fills)} fills; negative is adverse selection. No confidence interval is served.`,
    })),
    { key: 'cancels', label: 'Cancels sent', value: fmtInt(m.cancels_sent), unit: UNIT.count, basis: 'modeled', caveat: `${fmtInt(m.cancels_rejected)} rejected.` },
  ];
}

const METRIC_COLUMNS: Column<MetricRow>[] = [
  { id: 'label', header: 'Metric', rowHeader: true, cell: (r) => r.label },
  { id: 'value', header: 'Value', numeric: true, cell: (r) => r.value, raw: (r) => r.value },
  { id: 'unit', header: 'Unit', width: 132, cell: (r) => <span className="dim">{r.unit}</span> },
  { id: 'basis', header: 'Basis', width: 112, cell: (r) => <BasisBadge basis={r.basis} />, raw: (r) => r.basis },
  { id: 'caveat', header: 'How it is measured', className: () => 'wrap', cell: (r) => <span className="note">{r.caveat || DASH}</span> },
];

// ----- page ------------------------------------------------------------------------------------

function RunRisk({ runId }: { runId: string }) {
  const run = useAsync((signal) => api.run(runId, { signal }), [runId]);
  const equity = useAsync((signal) => api.equity(runId, { max_points: MAX_POINTS }, { signal }), [runId]);
  const analytics = useAsync((signal) => api.runAnalytics(runId, { signal }), [runId]);
  const r = run.data;
  const datasetId = r?.dataset.id ?? null;
  const dataset = useAsync((signal) => (datasetId === null ? Promise.resolve(null) : api.dataset(datasetId, { signal })), [datasetId]);
  const sampledNote = r?.config ? `every ${fmtInt(r.config.record.sample_every_ticks)} ticks` : 'sampling as recorded';
  const spanNs = equity.data ? equitySpanNs(equity.data) : null;

  const toolbar = (
    <>
      <span className="mono">{runId}</span>
      {r ? <span className="note risk-identity">{`${r.label || 'unlabelled'} \u00b7 ${strategyName(r.strategy_kind)} \u00b7 ${r.dataset.id} v${r.dataset.version}`}</span> : null}
      <RunContextLinks runId={runId} omit={['risk']} />
      <Link to={paths.risk()} className="risk-other">
        Other run
      </Link>
    </>
  );

  return (
    <Workspace context={`run ${runId}`} toolbar={toolbar} scroll>
      <div className="col risk-body">
        {run.loading ? <Spinner label="Loading run" /> : null}
        <ErrorBox error={run.error} onRetry={run.reload} />
        {r ? (
          <>
            <Answer run={r} synthetic={dataset.data ? dataset.data.kind === 'synthetic' : null} spanNs={spanNs} />
            {equity.loading ? <Spinner label="Loading equity samples" /> : null}
            <ErrorBox error={equity.error} onRetry={equity.reload} />
            {equity.data && equity.data.items.length === 0 ? <Empty>No account samples were recorded.</Empty> : null}
            {equity.data && equity.data.items.length > 0 ? <Charts page={equity.data} /> : null}
            {equity.data?.next_cursor ? <p className="note">The equity window was cut by the server ({equity.data.truncated_by}); later samples are not shown.</p> : null}

            <Panel title="Run metrics" flush caption={`bb-core Metrics recorded with the run. ${BASIS_MEANING.derived} Modeled values come from the simulation's fill, queue and fee models.`}>
              <DataTable label="Run metrics" columns={METRIC_COLUMNS} rows={runMetricRows(r.metrics, sampledNote)} rowKey={(x) => x.key} />
            </Panel>
            <Panel title="Execution counts and markouts" flush>
              <DataTable label="Execution metrics" columns={METRIC_COLUMNS} rows={executionRows(r.metrics)} rowKey={(x) => x.key} />
            </Panel>
            {analytics.loading ? <Spinner label="Loading analytics" /> : null}
            <ErrorBox error={analytics.error} onRetry={analytics.reload} />
            {analytics.data ? <RiskAnalytics risk={analytics.data.risk} /> : null}
            <Panel title="Timing" basis="modeled" flush caption="Latencies are the configured latency model's draws; order age is right-censored at the end.">
              <QuantileTable
                label="Timing distributions"
                rows={[
                  { label: 'Entry latency', q: r.metrics.entry_latency_ns, fmt: fmtDuration },
                  { label: 'Response latency', q: r.metrics.response_latency_ns, fmt: fmtDuration },
                  { label: 'Round trip', q: r.metrics.round_trip_ns, fmt: fmtDuration },
                  { label: 'Time to first fill', q: r.metrics.time_to_first_fill_ns, fmt: fmtDuration },
                  { label: 'Order age', q: r.metrics.order_age_ns, fmt: fmtDuration },
                ]}
              />
            </Panel>
            {analytics.data?.execution_unavailable ? (
              <Notice tag="EXECUTION ANALYTICS UNAVAILABLE" tone="info">
                {analytics.data.execution_unavailable}
              </Notice>
            ) : null}
            {analytics.data?.execution ? <ExecutionAnalytics execution={analytics.data.execution} /> : null}
          </>
        ) : null}
      </div>
    </Workspace>
  );
}

export default function RiskPage() {
  const { runId } = useParams();
  return runId ? <RunRisk runId={runId} /> : <RunPicker />;
}
