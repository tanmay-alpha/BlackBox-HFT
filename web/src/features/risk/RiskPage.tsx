// Risk & Performance (UI_DESIGN_SPEC.md §11.7). Everything shown is served by the v1 run
// endpoints: `GET /runs/{id}` (bb-core `Metrics`) and `GET /runs/{id}/equity` (account samples or
// downsampled buckets). Series derived here (drawdown, time under water) are computed from the
// served equity samples only and labelled DERIVED with their formula. Caveats follow
// docs/research/MARKET_MICROSTRUCTURE.md §3 and §5.3: one path of a replay without market
// impact, no annualised Sharpe or Sortino, no ratio extrapolated beyond the data span.

import { useMemo } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';

import { api } from '../../api/client';
import { isBucketed, type EquityBucket, type EquityPage, type Run } from '../../api/model';
import type { EquitySample, Metrics } from '../../api/types';
import { DataTable, type Column } from '../../components/DataTable';
import { QuantileTable } from '../../components/data';
import { DASH, fmtDuration, fmtInt, fmtNum, fmtPct, fmtPnl, fmtTicks } from '../../components/fmt';
import { cssVar } from '../../components/theme';
import { Empty, ErrorBox, Notice, Panel, Spinner, Stat, useAsync } from '../../components/ui';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { lastPerSecond, TimeSeriesChart, type ChartSeries } from '../runs/TimeSeriesChart';
import { ExecutionAnalytics, RiskAnalytics } from './AnalyticsPanels';
import './risk.css';

const MAX_POINTS = 5000;

const REPLAY_CAVEAT = 'Replay: no market impact; queue position modeled. One simulated path, not a forecast.';

/** Drawdown from the running peak of the served equity, per sample (DERIVED). */
export function drawdownSeries<T>(items: readonly T[], equity: (x: T) => number, peakOf: (x: T) => number = equity): number[] {
  let peak = Number.NEGATIVE_INFINITY;
  return items.map((x) => {
    peak = Math.max(peak, peakOf(x));
    return peak - equity(x);
  });
}

function RunPicker() {
  const navigate = useNavigate();
  const runs = useAsync((signal) => api.runs({}, { signal }), []);
  const columns = useMemo<Column<Run>[]>(
    () => [
      { id: 'run', header: 'Run', rowHeader: true, cell: (r) => <Link className="mono" to={paths.riskRun(r.run_id)}>{r.run_id}</Link>, raw: (r) => r.run_id },
      { id: 'label', header: 'Label', cell: (r) => r.label || <span className="dim">unlabelled</span>, sortValue: (r) => r.label ?? '' },
      { id: 'strategy', header: 'Strategy', cell: (r) => <span className="mono">{r.strategy_kind} every {fmtDuration(r.interval_ns)}</span> },
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

interface MetricRow {
  key: string;
  label: string;
  value: string;
  unit: string;
  caveat: string;
}

function executionRows(m: Metrics): MetricRow[] {
  const submitted = m.orders_submitted;
  const rows: MetricRow[] = [
    { key: 'orders', label: 'Orders submitted', value: fmtInt(submitted), unit: 'count', caveat: `${fmtInt(m.orders_intended)} intended, ${fmtInt(m.orders_risk_blocked)} risk-blocked, ${fmtInt(m.orders_refused)} refused.` },
    { key: 'fills', label: 'Fills', value: fmtInt(m.fill_count), unit: 'count', caveat: `${fmtInt(m.maker_fill_count)} maker, ${fmtInt(m.taker_fill_count)} taker. Fills are modeled by the engine's queue and exchange models.` },
    {
      key: 'fill_ratio_orders',
      label: 'Fill ratio (orders)',
      value: fmtPct(m.fill_ratio_orders),
      unit: 'ratio',
      caveat: `Numerator: ${fmtInt(m.orders_filled)} filled + ${fmtInt(m.orders_partially_filled)} partially filled; denominator ${fmtInt(submitted)} submitted. ${fmtInt(m.orders_working_at_end)} still working at the end (right-censored).`,
    },
    { key: 'fill_ratio_qty', label: 'Fill ratio (quantity)', value: fmtPct(m.fill_ratio_qty), unit: 'ratio', caveat: 'Filled quantity over submitted quantity; right-censored for orders working at the end.' },
    {
      key: 'spread_capture',
      label: 'Spread capture',
      value: fmtNum(m.spread_capture, 4),
      unit: 'quote',
      caveat: 'Measured against the mid at the fill time t, including the same-timestamp book batch; on recorded data this choice can change the sign (MARKET_MICROSTRUCTURE §3).',
    },
    { key: 'spread_capture_ticks', label: 'Spread capture per unit', value: fmtTicks(m.spread_capture_ticks), unit: 'ticks', caveat: 'Same reference mid "at t" as above.' },
    ...m.markouts.map((k) => ({
      key: `markout_${k.horizon_ns}`,
      label: `Markout ${fmtDuration(k.horizon_ns)}`,
      value: fmtTicks(k.mean_ticks),
      unit: 'ticks',
      caveat: `Quantity-weighted mean over n = ${fmtInt(k.fills)} fills; negative is adverse selection. Horizon-dependent; no confidence interval is served.`,
    })),
    { key: 'cancels', label: 'Cancels sent', value: fmtInt(m.cancels_sent), unit: 'count', caveat: `${fmtInt(m.cancels_rejected)} rejected.` },
  ];
  return rows;
}

function runMetricRows(m: Metrics, sampledNote: string): MetricRow[] {
  return [
    { key: 'net', label: 'Net P&L', value: fmtPnl(m.net_pnl), unit: 'quote', caveat: `Realized ${fmtPnl(m.realized_pnl)} + unrealized ${fmtPnl(m.unrealized_pnl)} (fees included). ${REPLAY_CAVEAT}` },
    { key: 'gross', label: 'Gross P&L (before fees)', value: fmtPnl(m.net_pnl + m.fees), unit: 'quote', caveat: 'Net P&L + fees, both served.' },
    { key: 'fees', label: 'Fees', value: fmtNum(m.fees, 4), unit: 'quote', caveat: 'Modeled from the configured maker/taker rates.' },
    { key: 'turnover', label: 'Turnover', value: fmtNum(m.turnover, 2), unit: 'quote', caveat: 'Traded notional.' },
    { key: 'volume', label: 'Volume', value: fmtNum(m.volume, 4), unit: 'base', caveat: 'Traded quantity.' },
    { key: 'dd', label: 'Maximum drawdown', value: fmtNum(m.max_drawdown, 4), unit: 'quote', caveat: `Peak to trough on the equity series as sampled (${sampledNote}). Path-dependent; one realisation; not a risk forecast.` },
    { key: 'mean_abs', label: 'Mean |position|', value: fmtNum(m.mean_abs_position, 4), unit: 'base', caveat: 'Inventory risk, time-averaged over samples.' },
    { key: 'max_long', label: 'Max long / max short', value: `${fmtNum(m.max_long, 4)} / ${fmtNum(m.max_short, 4)}`, unit: 'base', caveat: '' },
    { key: 'final', label: 'Final position', value: fmtNum(m.final_position, 4), unit: 'base', caveat: 'Marked at the last mid (unrealized P&L).' },
    { key: 'risk_blocks', label: 'Risk blocks', value: fmtInt(m.risk_blocks), unit: 'count', caveat: m.risk_halted ? 'The strategy was HALTED by a risk rule.' : 'No risk halt.' },
  ];
}

const METRIC_COLUMNS: Column<MetricRow>[] = [
  { id: 'label', header: 'Metric', rowHeader: true, cell: (r) => r.label },
  { id: 'value', header: 'Value', numeric: true, cell: (r) => <span className="mono">{r.value}</span>, raw: (r) => r.value },
  { id: 'unit', header: 'Unit', cell: (r) => <span className="dim">{r.unit}</span> },
  { id: 'caveat', header: 'Caveat', cell: (r) => <span className="note">{r.caveat || DASH}</span> },
];

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
    <div className="col risk-charts">
      {page.downsampled ? (
        <Notice tag="DOWNSAMPLED">
          The server returned {fmtInt(page.items.length)} buckets of {fmtDuration(page.bucket_ns)} instead of every sample. Equity shows each bucket's last value at its end;
          drawdown is an upper bound (running max of bucket maxima minus bucket minimum). Position and fees need raw samples and are not shown.
        </Notice>
      ) : null}
      <Panel title="Equity" basis="derived">
        <TimeSeriesChart series={charts.equity} formatValue={(v) => fmtPnl(v, 2)} ariaLabel="Simulated net equity over time after dataset origin" />
      </Panel>
      <Panel title="Drawdown" basis="derived" caption="Running peak of equity minus equity, from the served samples.">
        <TimeSeriesChart series={charts.drawdown} formatValue={(v) => fmtNum(v, 4)} ariaLabel="Drawdown from the running equity peak over time" />
      </Panel>
      {charts.position ? (
        <Panel title="Position / inventory" basis="derived">
          <TimeSeriesChart series={charts.position} formatValue={(v) => fmtNum(v, 4)} ariaLabel="Simulated position over time" />
        </Panel>
      ) : null}
      {charts.fees ? (
        <Panel title="Fees" basis="modeled">
          <TimeSeriesChart series={charts.fees} formatValue={(v) => fmtNum(v, 4)} ariaLabel="Cumulative modeled fees over time" />
        </Panel>
      ) : null}
      <p className="note">
        Lightweight Charts has second-resolution time: each series is resampled to the last point per second, without averaging. Time is relative to the
        dataset origin.
      </p>
    </div>
  );
}

function RunRisk({ runId }: { runId: string }) {
  const run = useAsync((signal) => api.run(runId, { signal }), [runId]);
  const equity = useAsync((signal) => api.equity(runId, { max_points: MAX_POINTS }, { signal }), [runId]);
  const analytics = useAsync((signal) => api.runAnalytics(runId, { signal }), [runId]);
  const r = run.data;
  const sampledNote = r?.config ? `every ${fmtInt(r.config.record.sample_every_ticks)} ticks` : 'sampling as recorded';

  const toolbar = (
    <>
      <span className="mono">{runId}</span>
      {r ? <span className="note">{`${r.label || 'unlabelled'} \u00b7 ${r.strategy_kind} \u00b7 ${r.dataset.id} v${r.dataset.version}`}</span> : null}
      <Link to={paths.run(runId)}>Forensics</Link>
      <Link to={paths.report(runId)}>Report</Link>
      <Link to={paths.risk()}>Other run</Link>
    </>
  );

  return (
    <Workspace context={`run ${runId}`} toolbar={toolbar} scroll>
      <div className="col risk-body">
        <Notice tag="SIMULATED" tone="modeled">{REPLAY_CAVEAT} No annualised Sharpe or Sortino is shown: one replay span supports none.</Notice>
        {run.loading ? <Spinner label="Loading run" /> : null}
        <ErrorBox error={run.error} onRetry={run.reload} />
        {r ? (
          <>
            <div className="stats risk-kpis" role="group" aria-label="Key figures">
              <Stat label="Net P&L (quote)" value={fmtPnl(r.metrics.net_pnl)} hint="DERIVED" />
              <Stat label="Fees (quote)" value={fmtNum(r.metrics.fees, 4)} hint="MODELED" />
              <Stat label="Max drawdown (quote)" value={fmtNum(r.metrics.max_drawdown, 4)} hint="as sampled" />
              <Stat label="Turnover (quote)" value={fmtNum(r.metrics.turnover, 2)} />
              <Stat label="Fill ratio" value={fmtPct(r.metrics.fill_ratio_orders)} hint={`${fmtInt(r.metrics.orders_filled + r.metrics.orders_partially_filled)} / ${fmtInt(r.metrics.orders_submitted)} orders`} />
              <Stat label="Mean |position|" value={fmtNum(r.metrics.mean_abs_position, 4)} />
            </div>
            <div className="risk-grid">
              <div className="col">
                {equity.loading ? <Spinner label="Loading equity samples" /> : null}
                <ErrorBox error={equity.error} onRetry={equity.reload} />
                {equity.data && equity.data.items.length === 0 ? <Empty>No account samples were recorded.</Empty> : null}
                {equity.data && equity.data.items.length > 0 ? <Charts page={equity.data} /> : null}
                {equity.data?.next_cursor ? <p className="note">The equity window was cut by the server ({equity.data.truncated_by}); later samples are not shown.</p> : null}
              </div>
              <div className="col">
                {analytics.loading ? <Spinner label="Loading analytics" /> : null}
                <ErrorBox error={analytics.error} onRetry={analytics.reload} />
                {analytics.data ? <RiskAnalytics risk={analytics.data.risk} /> : null}
                <Panel title="Run metrics" basis="derived" flush caption="bb-core Metrics recorded with the run.">
                  <DataTable label="Run metrics" columns={METRIC_COLUMNS} rows={runMetricRows(r.metrics, sampledNote)} rowKey={(x) => x.key} />
                </Panel>
                <Panel title="Execution counts and markouts" basis="derived" flush>
                  <DataTable label="Execution metrics" columns={METRIC_COLUMNS} rows={executionRows(r.metrics)} rowKey={(x) => x.key} />
                </Panel>
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
              </div>
            </div>
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
