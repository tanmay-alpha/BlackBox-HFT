import { useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';

import { api, exportUrl, reportUrl } from '../../api/client';
import type {
  DatasetManifest, FillRecord, InstrumentMeta, Metrics, OrderOutcome, OrderRecord, RunSummary, Side,
} from '../../api/types';
import { JsonBlock, Pager, QuantileTable } from '../../components/data';
import { DataTable, type Column } from '../../components/DataTable';
import { ORDER_OUTCOMES, OUTCOME_MEANING } from '../../components/describe';
import {
  DASH, fmtDuration, fmtInt, fmtMoney, fmtPct, fmtPnl, fmtPrice, fmtQty, fmtTicks, fmtUnixMs, words,
} from '../../components/fmt';
import { digitsForUnit } from '../../components/ns';
import { SplitPane } from '../../components/SplitPane';
import { Tabs } from '../../components/Tabs';
import { cssVar } from '../../components/theme';
import { Term } from '../../components/Tooltip';
import {
  BasisBadge, ClockText, Empty, ErrorBox, HashText, Notice, Panel, Pill, SideBadge, Spinner, Stat,
  TimeDigitsContext, useAsync,
} from '../../components/ui';
import type { AsyncResult } from '../../components/ui';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { outcomeTone } from '../forensics/labels';
import { lastPerSecond, TimeSeriesChart } from './TimeSeriesChart';
import type { ChartSeries } from './TimeSeriesChart';
import './runs.css';

const PAGE_SIZE = 100;

const ATTRIBUTION =
  'Charts on run pages use TradingView Lightweight Charts\u2122, Copyright (c) 2025 TradingView, Inc. https://www.tradingview.com/';

type RunTab = 'metrics' | 'charts' | 'fills' | 'manifest' | 'engine';

/** Independent requests keep one missing resource from hiding the rest of a stored run. */
function LoadState<T>({ result, label }: { result: AsyncResult<T>; label: string }) {
  return (
    <>
      {result.loading ? <Spinner label={label} /> : null}
      {result.error ? <ErrorBox error={result.error} onRetry={result.reload} /> : null}
    </>
  );
}

/** Quantity text; without the instrument the raw base-unit value (the unit goes in the header). */
function quantity(value: number | null, instrument: InstrumentMeta | null): string {
  if (value === null) return DASH;
  return instrument ? fmtQty(value, instrument) : String(value);
}

/** Price text; without the instrument the raw tick value (the unit goes in the header). */
function price(value: number | null, instrument: InstrumentMeta | null): string {
  if (value === null) return DASH;
  return instrument ? fmtPrice(value, instrument) : String(value);
}

function signClass(x: number | null | undefined): string | undefined {
  return x === null || x === undefined ? undefined : x > 0 ? 'up' : x < 0 ? 'down' : undefined;
}

export default function RunOverviewPage() {
  const { runId = '' } = useParams();
  return runId ? (
    <RunOverview key={runId} runId={runId} />
  ) : (
    <Workspace context={null} scroll>
      <Empty>No run id in this address.</Empty>
    </Workspace>
  );
}

function RunOverview({ runId }: { runId: string }) {
  const run = useAsync(() => api.run(runId), [runId]);
  const label = run.data?.label ?? '';
  return (
    <Workspace
      context={label || runId}
      layout
      toolbar={
        <>
          <Link className="btn" to={paths.runs()}>All runs</Link>
          <span className="run-context">
            <HashText hash={runId} />
            {label ? <span className="run-context-label">{label}</span> : null}
          </span>
          <Link className="btn btn-primary" to={paths.bookRun(runId)}>Open in Order Book Lab</Link>
          <a className="btn" href={reportUrl(runId)} download>Download report (.md)</a>
          <a className="btn" href={exportUrl(runId)} download>Download run (.json)</a>
          <Link className="btn" to={paths.comparePair(runId)}>Compare with…</Link>
          <Link className="btn" to={paths.sweep(runId)}>Sweep from this run</Link>
          <Link className="btn" to={paths.strategy({ from: runId })}>Open in Strategy Lab</Link>
          <button type="button" onClick={run.reload} disabled={run.loading}>Reload run</button>
        </>
      }
    >
      <LoadState result={run} label="Loading run manifest and metrics" />
      {run.data ? <RunContents run={run.data} /> : null}
    </Workspace>
  );
}

function RunContents({ run }: { run: RunSummary }) {
  const m = run.manifest;
  const dataset = useAsync(() => api.dataset(m.dataset.id), [m.dataset.id]);
  const instrument = dataset.data?.instrument ?? null;
  const digits = dataset.data ? digitsForUnit(dataset.data.timestamps.input_unit) : 9;
  const [tab, setTab] = useState<RunTab>('metrics');

  return (
    <TimeDigitsContext.Provider value={digits}>
      <div className="run-body">
        <KpiStrip metrics={run.metrics} />
        <div className="run-split">
          <SplitPane
            id="forensics.run"
            panes={[
              {
                id: 'orders',
                label: 'Order blotter',
                defaultSize: '55%',
                minSize: '30%',
                content: <OrdersBlotter runId={m.run_id} instrument={instrument} />,
              },
              {
                id: 'details',
                label: 'Run details',
                defaultSize: '45%',
                minSize: '25%',
                collapsible: true,
                content: (
                  <Tabs<RunTab>
                    label="Run details"
                    active={tab}
                    onChange={setTab}
                    tabs={[
                      { id: 'metrics', label: 'Metrics' },
                      { id: 'charts', label: 'Account charts' },
                      { id: 'fills', label: 'Fills', count: fmtInt(run.fill_count) },
                      { id: 'manifest', label: 'Manifest' },
                      { id: 'engine', label: 'Engine cross-check' },
                    ]}
                  >
                    {tab === 'metrics' ? <RunMetrics metrics={run.metrics} instrument={instrument} /> : null}
                    {tab === 'charts' ? <AccountCharts runId={m.run_id} instrument={instrument} /> : null}
                    {tab === 'fills' ? <FillsTable runId={m.run_id} instrument={instrument} /> : null}
                    {tab === 'manifest' ? <Manifest run={run} dataset={dataset} /> : null}
                    {tab === 'engine' ? (
                      <EngineCheck
                        runId={m.run_id}
                        instrument={instrument}
                        partialFill={m.config.execution.exchange_model === 'partial_fill'}
                      />
                    ) : null}
                  </Tabs>
                ),
              },
            ]}
          />
        </div>
      </div>
    </TimeDigitsContext.Provider>
  );
}

function KpiStrip({ metrics: m }: { metrics: Metrics }) {
  const markout = m.markouts.length > 0 ? m.markouts[0] : null;
  return (
    <div className="stats run-kpis" aria-label="Run key figures">
      <Stat label={<>Net P&amp;L <span className="unit">quote</span></>} value={<span className={signClass(m.net_pnl)}>{fmtPnl(m.net_pnl, 2)}</span>} hint="simulated" />
      <Stat label={<>Realized P&amp;L <span className="unit">quote</span></>} value={<span className={signClass(m.realized_pnl)}>{fmtPnl(m.realized_pnl, 2)}</span>} hint="before fees" />
      <Stat label={<><Term term="fees">Fees</Term> <span className="unit">quote</span></>} value={fmtPnl(m.fees, 2)} hint="negative is a rebate" />
      <Stat label="Orders submitted" value={fmtInt(m.orders_submitted)} />
      <Stat label="Simulated fills" value={fmtInt(m.fill_count)} />
      <Stat label={<Term term="fill_ratio">Order fill ratio</Term>} value={fmtPct(m.fill_ratio_orders)} />
      <Stat
        label={<><Term term="markout">Markout</Term> {markout ? `+${fmtDuration(markout.horizon_ns)}` : ''} <span className="unit">ticks</span></>}
        value={<span className={signClass(markout?.mean_ticks)}>{fmtTicks(markout ? markout.mean_ticks : null)}</span>}
        hint={markout ? `${fmtInt(markout.fills)} measured fills` : 'no horizon recorded'}
      />
      <Stat label={<><Term term="spread_capture">Spread capture</Term> <span className="unit">ticks</span></>} value={fmtTicks(m.spread_capture_ticks)} hint="maker fills" />
      <Stat label={<><Term term="drawdown">Max drawdown</Term> <span className="unit">quote</span></>} value={fmtPnl(m.max_drawdown, 2)} hint="sampled equity" />
    </div>
  );
}

function Manifest({ run, dataset }: { run: RunSummary; dataset: AsyncResult<DatasetManifest> }) {
  const m = run.manifest;
  return (
    <Panel title="Reproducibility manifest" fill>
      <div className="col">
        <dl className="kv">
          <dt>Label</dt><dd>{run.label || <span className="dim">unlabelled</span>}</dd>
          <dt>Run id</dt><dd className="mono wrap-anywhere">{m.run_id}</dd>
          <dt>Config SHA-256</dt><dd className="mono wrap-anywhere">{m.config_sha256}</dd>
          <dt>Result SHA-256</dt><dd className="mono wrap-anywhere">{m.result_sha256}</dd>
          <dt>BlackBox / contract</dt><dd className="mono">{m.blackbox_version} / v{m.contract_version}</dd>
          <dt>Engine</dt><dd className="mono">{m.engine.name} {m.engine.version}</dd>
          <dt>Engine features</dt><dd className="mono">{m.engine.features.join(', ') || DASH}</dd>
          <dt>Dataset id</dt><dd className="mono wrap-anywhere"><Link to={paths.dataset(m.dataset.id)}>{m.dataset.id}</Link></dd>
          <dt>Dataset SHA-256</dt><dd className="mono wrap-anywhere">{m.dataset.sha256}</dd>
          <dt>Platform</dt><dd className="mono">{m.platform}</dd>
          <dt>Profile</dt><dd className="mono">{m.profile}</dd>
          <dt>Seed</dt><dd className="mono">{m.config.seed}</dd>
          <dt>Created</dt><dd className="mono">{fmtUnixMs(m.created_unix_ms)}</dd>
          <dt>Wall time</dt><dd className="mono">{fmtDuration(m.wall_time_ms * 1_000_000)}</dd>
          <dt>Strategy ticks / tape events</dt><dd className="mono">{fmtInt(run.ticks)} / {fmtInt(run.tape_len)}</dd>
        </dl>
        <p className="note">Identical inputs on the same platform reproduce the same result hash. Config identity covers the contract version, engine name and version, dataset hash, full configuration and seed; labels and wall-clock timing are not hashed.</p>
        <JsonBlock summary="Full run configuration (JSON)" value={m.config} />
        <h3 className="run-subhead">Dataset instrument</h3>
        <div className="row">
          <button type="button" onClick={dataset.reload} disabled={dataset.loading}>Reload dataset</button>
        </div>
        <LoadState result={dataset} label="Loading instrument for price and quantity formatting" />
        {dataset.data ? (
          <>
            <p className="mono">{dataset.data.source.symbol} · tick {dataset.data.instrument.tick_size} · lot {dataset.data.instrument.lot_size} · contract size {dataset.data.instrument.contract_size}</p>
            {dataset.data.events_sha256 !== m.dataset.sha256 ? (
              <Notice tag="HASH MISMATCH">The current dataset hash differs from the hash stored with this run. The instrument below belongs to the current dataset manifest.</Notice>
            ) : null}
          </>
        ) : <p className="note">Until the instrument is available, prices are explicitly shown in ticks and quantities in base units.</p>}
      </div>
    </Panel>
  );
}

function EngineCheck({ runId, instrument, partialFill }: { runId: string; instrument: InstrumentMeta | null; partialFill: boolean }) {
  const check = useAsync(() => api.engineCheck(runId), [runId]);
  const c = check.data;
  const qtyUnit = instrument ? '' : ' (base units)';
  return (
    <Panel title="Engine cross-check" basis="modeled" fill actions={<button type="button" onClick={check.reload} disabled={check.loading}>Reload cross-check</button>}>
      <div className="col">
        <LoadState result={check} label="Loading engine cross-check" />
        {c ? (
          <>
            <div className="row">
              <Pill tone={c.agrees ? 'ok' : 'warn'}>{c.agrees ? 'agrees' : 'disagrees'}</Pill>
              <span className="note">BlackBox ledger versus the engine's final local-side account state.</span>
            </div>
            {!c.agrees ? <Notice tag="DISAGREES">{partialFill ? 'Disagreement under the partial-fill model is expected; the engine note below explains why.' : 'The final ledger and engine state do not agree. Review the engine note below.'}</Notice> : null}
            <table className="table">
              <thead><tr><th scope="col">Measure</th><th scope="col" className="num">Ledger</th><th scope="col" className="num">Engine</th></tr></thead>
              <tbody>
                <tr><th scope="row">Position{qtyUnit}</th><td className="num">{quantity(c.ledger_position, instrument)}</td><td className="num">{quantity(c.engine_position, instrument)}</td></tr>
                <tr><th scope="row">Cash / balance (quote, before fees)</th><td className="num">{fmtPnl(c.ledger_cash, 4)}</td><td className="num">{fmtPnl(c.engine_balance, 4)}</td></tr>
                <tr><th scope="row">Fees (quote)</th><td className="num">{fmtPnl(c.ledger_fees, 4)}</td><td className="num">{fmtPnl(c.engine_fee, 4)}</td></tr>
                <tr><th scope="row">Fill / trade count</th><td className="num">{fmtInt(c.ledger_fills)}</td><td className="num">{fmtInt(c.engine_num_trades)}</td></tr>
              </tbody>
            </table>
            {c.note !== '' ? <p className="note run-engine-note">{c.note}</p> : <p className="note">No engine note.</p>}
          </>
        ) : null}
      </div>
    </Panel>
  );
}

function MetricGroup({ title, children }: { title: string; children: ReactNode }) {
  return <section className="col"><h3 className="run-subhead">{title}</h3><div className="stats">{children}</div></section>;
}

function RunMetrics({ metrics: m, instrument: inst }: { metrics: Metrics; instrument: InstrumentMeta | null }) {
  const orders = [
    ['Intended', m.orders_intended], ['Submitted', m.orders_submitted],
    ['Risk blocked', m.orders_risk_blocked], ['Refused', m.orders_refused],
    ['Filled', m.orders_filled], ['Partially filled', m.orders_partially_filled],
    ['Canceled', m.orders_canceled], ['Expired', m.orders_expired],
    ['Working at end', m.orders_working_at_end], ['Cancels sent', m.cancels_sent],
    ['Cancels rejected', m.cancels_rejected],
  ] as const;
  const baseUnits = inst ? '' : ' (base units)';
  return (
    <Panel title="Run metrics" basis="derived" fill caption={`Arithmetic over the simulated tape, fills and sampled account state. Missing measurements are shown as ${DASH}, never as zero.`}>
      <div className="col">
        <MetricGroup title="Orders">{orders.map(([label, value]) => <Stat key={label} label={label} value={fmtInt(value)} />)}</MetricGroup>
        <MetricGroup title="Fills and activity">
          <Stat label="Simulated fills" value={fmtInt(m.fill_count)} />
          <Stat label="Maker fills" value={fmtInt(m.maker_fill_count)} />
          <Stat label="Taker fills" value={fmtInt(m.taker_fill_count)} />
          <Stat label={<Term term="fill_ratio">Order fill ratio</Term>} value={fmtPct(m.fill_ratio_orders)} hint="At least one fill / submitted orders" />
          <Stat label="Quantity fill ratio" value={fmtPct(m.fill_ratio_qty)} />
          <Stat label={`Volume${baseUnits}`} value={quantity(m.volume, inst)} />
          <Stat label={<>Turnover <span className="unit">quote</span></>} value={fmtMoney(m.turnover, 4)} />
        </MetricGroup>
        <section className="col">
          <h3 className="run-subhead">Timing quantiles</h3>
          <QuantileTable label="Timing quantiles" rows={[
            { label: 'Order age (exchange)', q: m.order_age_ns, fmt: fmtDuration },
            { label: 'Arrival to first fill (exchange)', q: m.time_to_first_fill_ns, fmt: fmtDuration },
            { label: 'Entry latency', q: m.entry_latency_ns, fmt: fmtDuration },
            { label: 'Response latency', q: m.response_latency_ns, fmt: fmtDuration },
            { label: 'New-order round trip (local)', q: m.round_trip_ns, fmt: fmtDuration },
          ]} />
        </section>
        <MetricGroup title={`Inventory${baseUnits}`}>
          <Stat label="Final position" value={quantity(m.final_position, inst)} />
          <Stat label="Maximum long" value={quantity(m.max_long, inst)} />
          <Stat label="Maximum short" value={quantity(m.max_short, inst)} />
          <Stat label="Mean absolute position" value={quantity(m.mean_abs_position, inst)} hint="Over account samples" />
        </MetricGroup>
        <MetricGroup title="P&L (quote)">
          <Stat label="Realized P&L" value={fmtPnl(m.realized_pnl, 4)} hint="Before fees" />
          <Stat label="Unrealized P&L" value={fmtPnl(m.unrealized_pnl, 4)} />
          <Stat label={<Term term="fees">Fees</Term>} value={fmtPnl(m.fees, 4)} hint="Negative is a rebate" />
          <Stat label="Net P&L" value={fmtPnl(m.net_pnl, 4)} hint="Realized + unrealized − fees, last sample" />
        </MetricGroup>
        <MetricGroup title="Execution quality">
          <Stat label={<><Term term="spread_capture">Spread capture</Term> <span className="unit">quote</span></>} value={fmtPnl(m.spread_capture, 4)} hint="Maker fills, mid at fill" />
          <Stat label={<><Term term="spread_capture">Spread capture</Term> <span className="unit">ticks</span></>} value={fmtTicks(m.spread_capture_ticks)} hint="Quantity-weighted" />
        </MetricGroup>
        <section className="col">
          <h3 className="run-subhead"><Term term="markout">Markouts</Term></h3>
          {m.markouts.length === 0 ? <Empty>No markout horizons were recorded.</Empty> : (
            <table className="table">
              <caption>Post-fill mid-price markouts — negative means adverse selection. Quantity-weighted mean in ticks; horizons without a valid future mid have no measurement.</caption>
              <thead><tr><th scope="col">Horizon</th><th scope="col" className="num">Measured fills</th><th scope="col" className="num">Mean markout <span className="unit">ticks</span></th></tr></thead>
              <tbody>{m.markouts.map((markout) => <tr key={markout.horizon_ns}><th scope="row">{fmtDuration(markout.horizon_ns)}</th><td className="num">{fmtInt(markout.fills)}</td><td className={`num ${signClass(markout.mean_ticks) ?? ''}`}>{fmtTicks(markout.mean_ticks)}</td></tr>)}</tbody>
            </table>
          )}
        </section>
        <MetricGroup title="Risk">
          <Stat label="Risk blocks" value={fmtInt(m.risk_blocks)} />
          <Stat label="Risk halted" value={<Pill tone={m.risk_halted ? 'warn' : 'neutral'}>{m.risk_halted ? 'yes' : 'no'}</Pill>} />
          <Stat label={<><Term term="drawdown">Maximum drawdown</Term> <span className="unit">quote</span></>} value={fmtPnl(m.max_drawdown, 4)} hint="Peak-to-trough sampled equity" />
        </MetricGroup>
      </div>
    </Panel>
  );
}

function AccountCharts({ runId, instrument }: { runId: string; instrument: InstrumentMeta | null }) {
  const equity = useAsync(() => api.equity(runId), [runId]);
  const samples = equity.data;
  const series = useMemo((): { equity: ChartSeries[]; position: ChartSeries[] } => ({
    equity: [{ id: 'equity', label: 'Net equity (quote)', color: cssVar('--derived'), points: lastPerSecond(samples ?? [], (s) => s.equity) }],
    position: [
      { id: 'position', label: 'Position from exchange-side simulated fills', color: cssVar('--fg-0'), points: lastPerSecond(samples ?? [], (s) => s.position), stepped: true },
      { id: 'known', label: 'Position known to strategy', color: cssVar('--accent'), points: lastPerSecond(samples ?? [], (s) => s.known_position), stepped: true, dashed: true },
    ],
  }), [samples]);
  return (
    <Panel title="Account charts" basis="derived" fill actions={<button type="button" onClick={equity.reload} disabled={equity.loading}>Reload charts</button>}>
      <div className="col">
        <LoadState result={equity} label="Loading equity and position samples" />
        {samples && samples.length === 0 ? <Empty>No account samples were recorded.</Empty> : null}
        {samples && samples.length > 0 ? (
          <>
            <section className="col">
              <h3 className="run-subhead">Equity <span className="unit">quote</span></h3>
              {series.equity.some((s) => s.points.length > 0) ? <TimeSeriesChart series={series.equity} formatValue={(v) => fmtPnl(v, 2)} ariaLabel="Simulated net equity over time after dataset origin" /> : <Empty>No finite equity samples.</Empty>}
              <p className="note">Lightweight Charts has second-resolution time. Resampled to the last sample per second, without averaging; time is relative to dataset origin.</p>
            </section>
            <section className="col">
              <h3 className="run-subhead">Position <span className="unit">base quantity</span></h3>
              {series.position.some((s) => s.points.length > 0) ? <TimeSeriesChart series={series.position} formatValue={(value) => quantity(value, instrument)} ariaLabel="Simulated position and strategy-known position over time after dataset origin" /> : <Empty>No finite position samples.</Empty>}
              <p className="note">Lightweight Charts has second-resolution time. Resampled to the last sample per second. Dashed position includes only fill notifications already received by the strategy.</p>
            </section>
          </>
        ) : null}
        <p className="note">{ATTRIBUTION}</p>
      </div>
    </Panel>
  );
}

function OrdersBlotter({ runId, instrument }: { runId: string; instrument: InstrumentMeta | null }) {
  const navigate = useNavigate();
  const [outcome, setOutcome] = useState<OrderOutcome | ''>('');
  const [side, setSide] = useState<Side | ''>('');
  const [offset, setOffset] = useState(0);
  const orders = useAsync(() => api.orders(runId, { outcome: outcome || undefined, side: side || undefined, offset, limit: PAGE_SIZE }), [runId, outcome, side, offset]);
  const page = orders.data;
  const priceUnit = instrument ? undefined : 'ticks';
  const qtyUnit = instrument ? undefined : 'base units';

  const columns = useMemo<Column<OrderRecord>[]>(() => [
    { id: 'order', header: 'Order', rowHeader: true, cell: (o) => <Link className="mono" to={paths.order(runId, o.order_id)} tabIndex={-1}>{o.order_id}</Link>, raw: (o) => String(o.order_id), sortValue: (o) => o.order_id },
    { id: 'decision', header: 'Decision time', unit: 'local', cell: (o) => <ClockText ns={o.decision_ts} />, raw: (o) => String(o.decision_ts), sortValue: (o) => o.decision_ts },
    { id: 'side', header: 'Side', cell: (o) => <SideBadge side={o.side} />, sortValue: (o) => o.side },
    { id: 'price', header: 'Price', unit: priceUnit, numeric: true, cell: (o) => price(o.price_tick, instrument), sortValue: (o) => o.price_tick },
    { id: 'qty', header: 'Qty', unit: qtyUnit, numeric: true, cell: (o) => quantity(o.qty, instrument), sortValue: (o) => o.qty },
    { id: 'tif', header: 'TIF', cell: (o) => <span className="mono">{o.tif}</span>, sortValue: (o) => o.tif },
    { id: 'outcome', header: 'Outcome', cell: (o) => <Pill tone={outcomeTone(o.outcome)} title={OUTCOME_MEANING[o.outcome]}>{words(o.outcome)}</Pill>, sortValue: (o) => o.outcome },
    { id: 'filled', header: 'Filled', unit: qtyUnit, numeric: true, cell: (o) => quantity(o.filled_qty, instrument), sortValue: (o) => o.filled_qty },
    { id: 'qentry', header: 'Queue ahead at entry', unit: qtyUnit, term: 'queue_position', numeric: true, cell: (o) => quantity(o.queue_ahead_at_entry, instrument), sortValue: (o) => o.queue_ahead_at_entry },
    { id: 'qmin', header: 'Min queue ahead', unit: qtyUnit, term: 'queue_position', numeric: true, cell: (o) => quantity(o.min_queue_ahead, instrument), sortValue: (o) => o.min_queue_ahead },
    { id: 'reason', header: 'Reason', cell: (o) => o.reason || DASH, sortValue: (o) => o.reason },
  ], [runId, instrument, priceUnit, qtyUnit]);

  return (
    <Panel
      title="Order blotter"
      basis="modeled"
      flush
      fill
      caption="Decision time is local-clock time after dataset origin. Queue quantities are modeled, not observed. Open an order for its full simulated lifecycle."
      actions={
        <>
          <label className="row" htmlFor="run-outcome">Outcome
            <select id="run-outcome" value={outcome} onChange={(event) => { setOutcome(event.target.value as OrderOutcome | ''); setOffset(0); }}>
              <option value="">All outcomes</option>
              {ORDER_OUTCOMES.map((value) => <option key={value} value={value}>{words(value)}</option>)}
            </select>
          </label>
          <label className="row" htmlFor="run-side">Side
            <select id="run-side" value={side} onChange={(event) => { setSide(event.target.value as Side | ''); setOffset(0); }}>
              <option value="">Both sides</option><option value="buy">Buy</option><option value="sell">Sell</option>
            </select>
          </label>
          <button type="button" onClick={orders.reload} disabled={orders.loading}>Reload orders</button>
        </>
      }
    >
      <LoadState result={orders} label="Loading orders" />
      {page ? (
        <DataTable
          label="Orders"
          rows={page.orders}
          rowKey={(o) => String(o.order_id)}
          columns={columns}
          onActivate={(o) => navigate(paths.order(runId, o.order_id))}
          empty={page.total === 0 ? 'No orders match these filters.' : 'No orders on this page. Return to the first page.'}
          footer={<Pager offset={page.offset} limit={PAGE_SIZE} total={page.total} onOffset={setOffset} noun="orders" />}
          fill
        />
      ) : null}
    </Panel>
  );
}

function FillsTable({ runId, instrument }: { runId: string; instrument: InstrumentMeta | null }) {
  const navigate = useNavigate();
  const fills = useAsync(() => api.fills(runId), [runId]);
  const list = fills.data;
  const priceUnit = instrument ? undefined : 'ticks';
  const qtyUnit = instrument ? undefined : 'base units';
  const columns = useMemo<Column<FillRecord>[]>(() => [
    { id: 'ts', header: 'Time', unit: 'exchange', cell: (f) => <ClockText ns={f.ts} />, raw: (f) => String(f.ts), sortValue: (f) => f.ts },
    { id: 'order', header: 'Order', rowHeader: true, cell: (f) => <Link className="mono" to={paths.order(runId, f.order_id)} tabIndex={-1}>{f.order_id}</Link>, raw: (f) => String(f.order_id), sortValue: (f) => f.order_id },
    { id: 'side', header: 'Side', cell: (f) => <SideBadge side={f.side} />, sortValue: (f) => f.side },
    { id: 'price', header: 'Price', unit: priceUnit, numeric: true, cell: (f) => price(f.price_tick, instrument), sortValue: (f) => f.price_tick },
    { id: 'qty', header: 'Qty', unit: qtyUnit, numeric: true, cell: (f) => quantity(f.qty, instrument), sortValue: (f) => f.qty },
    { id: 'liq', header: 'Liquidity', cell: (f) => (f.maker ? 'maker' : 'taker'), sortValue: (f) => (f.maker ? 'maker' : 'taker') },
    { id: 'fee', header: 'Fee', unit: 'quote', term: 'fees', numeric: true, cell: (f) => fmtPnl(f.fee, 4), sortValue: (f) => f.fee },
    { id: 'value', header: 'Value', unit: 'quote', numeric: true, cell: (f) => fmtMoney(f.value, 4), sortValue: (f) => f.value },
    { id: 'pos', header: 'Position after', unit: qtyUnit, numeric: true, cell: (f) => quantity(f.position_after, instrument), sortValue: (f) => f.position_after },
    { id: 'rpnl', header: 'Realized P&L', unit: 'quote', numeric: true, cell: (f) => fmtPnl(f.realized_pnl, 4), className: (f) => signClass(f.realized_pnl), sortValue: (f) => f.realized_pnl },
    { id: 'mid', header: 'Mid at fill (derived)', unit: priceUnit, numeric: true, cell: (f) => price(f.mid_tick, instrument), sortValue: (f) => f.mid_tick },
  ], [runId, instrument, priceUnit, qtyUnit]);
  return (
    <Panel
      title="Simulated fills"
      basis="modeled"
      flush
      fill
      caption={<>Fill time is exchange-clock time after dataset origin. Monetary amounts are quote currency; realized P&amp;L is before fees. Mid at fill is <BasisBadge basis="derived" /> from historical data.</>}
      actions={<button type="button" onClick={fills.reload} disabled={fills.loading}>Reload fills</button>}
    >
      <LoadState result={fills} label="Loading simulated fills" />
      {list ? (
        <DataTable
          label="Simulated fills"
          rows={list}
          rowKey={(f) => String(f.seq)}
          columns={columns}
          onActivate={(f) => navigate(paths.order(runId, f.order_id))}
          empty="No simulated fills in this run."
          footer={<span className="note">{fmtInt(list.length)} fills, all loaded</span>}
          fill
        />
      ) : null}
    </Panel>
  );
}
