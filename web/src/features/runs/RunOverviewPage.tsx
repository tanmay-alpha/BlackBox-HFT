import { useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { Link, useParams } from 'react-router-dom';

import { api } from '../../api/client';
import type { InstrumentMeta, Metrics, OrderOutcome, RunSummary, Side } from '../../api/types';
import { JsonBlock, Pager, QuantileTable } from '../../components/data';
import { ORDER_OUTCOMES, OUTCOME_MEANING } from '../../components/describe';
import {
  DASH, fmtClock, fmtDuration, fmtInt, fmtNum, fmtPct, fmtPrice, fmtQty, fmtSigned,
  fmtUnixMs, words,
} from '../../components/fmt';
import { PageHeader, usePageTitle } from '../../components/page';
import { cssVar } from '../../components/theme';
import {
  Badge, BasisBadge, Empty, ErrorBox, Panel, SideBadge, Spinner, Stat, useAsync,
} from '../../components/ui';
import type { AsyncResult } from '../../components/ui';
import { fmtQuoteSigned } from './format';
import { lastPerSecond, TimeSeriesChart } from './TimeSeriesChart';
import type { ChartSeries } from './TimeSeriesChart';

const PAGE_SIZE = 100;

/** Independent requests keep one missing resource from hiding the rest of a stored run. */
function LoadState<T>({ result, label }: { result: AsyncResult<T>; label: string }) {
  return (
    <>
      {result.loading ? <Spinner label={label} /> : null}
      {result.error ? <ErrorBox error={result.error} /> : null}
    </>
  );
}

function quantity(value: number | null, instrument: InstrumentMeta | null): string {
  if (value === null) return DASH;
  return instrument ? fmtQty(value, instrument) : `${value} base units`;
}

function price(value: number | null, instrument: InstrumentMeta | null): string {
  if (value === null) return DASH;
  return instrument ? fmtPrice(value, instrument) : `${value} ticks`;
}

export default function RunOverviewPage() {
  const { runId = '' } = useParams();
  usePageTitle(runId ? `Run ${runId}` : 'Run');
  return runId ? <RunOverview key={runId} runId={runId} /> : <Empty>No run id in this address.</Empty>;
}

function RunOverview({ runId }: { runId: string }) {
  const run = useAsync(() => api.run(runId), [runId]);
  return (
    <div className="col">
      <PageHeader
        title={run.data?.label || `Run ${runId}`}
        lead="Simulated on historical replay. Account values and order outcomes are outputs of this run's execution assumptions, not exchange executions."
        actions={
          <>
            <Link className="btn" to="/runs">All runs</Link>
            <Link className="btn" to={`/compare?a=${encodeURIComponent(runId)}`}>Compare with…</Link>
            <Link className="btn btn-primary" to={`/lab?from=${encodeURIComponent(runId)}`}>Open in Lab</Link>
            <button type="button" onClick={run.reload} disabled={run.loading}>Reload run</button>
          </>
        }
      />
      <LoadState result={run} label="Loading run manifest and metrics" />
      {run.data ? <RunContents run={run.data} /> : null}
    </div>
  );
}

function RunContents({ run }: { run: RunSummary }) {
  const m = run.manifest;
  const dataset = useAsync(() => api.dataset(m.dataset.id), [m.dataset.id]);
  const instrument = dataset.data?.instrument ?? null;
  return (
    <>
      <Panel title="Reproducibility manifest">
        <div className="col">
          <dl className="kv run-manifest">
            <dt>Label</dt><dd>{run.label || <span className="dim">unlabelled</span>}</dd>
            <dt>Run id</dt><dd>{m.run_id}</dd>
            <dt>Config SHA-256</dt><dd>{m.config_sha256}</dd>
            <dt>Result SHA-256</dt><dd>{m.result_sha256}</dd>
            <dt>BlackBox / contract</dt><dd>{m.blackbox_version} / v{m.contract_version}</dd>
            <dt>Engine</dt><dd>{m.engine.name} {m.engine.version}</dd>
            <dt>Engine features</dt><dd>{m.engine.features.join(', ') || DASH}</dd>
            <dt>Dataset id</dt><dd><Link to={`/replay/${encodeURIComponent(m.dataset.id)}`}>{m.dataset.id}</Link></dd>
            <dt>Dataset SHA-256</dt><dd>{m.dataset.sha256}</dd>
            <dt>Platform</dt><dd>{m.platform}</dd>
            <dt>Profile</dt><dd>{m.profile}</dd>
            <dt>Seed</dt><dd>{m.config.seed}</dd>
            <dt>Created</dt><dd>{fmtUnixMs(m.created_unix_ms)}</dd>
            <dt>Wall time</dt><dd>{fmtDuration(m.wall_time_ms * 1_000_000)}</dd>
            <dt>Strategy ticks / tape events</dt><dd>{fmtInt(run.ticks)} / {fmtInt(run.tape_len)}</dd>
          </dl>
          <p className="note">Identical inputs on the same platform reproduce the same result hash. Config identity covers the contract version, engine name and version, dataset hash, full configuration and seed; labels and wall-clock timing are not hashed.</p>
          <JsonBlock summary="Full run configuration (JSON)" value={m.config} />
        </div>
      </Panel>
      <Panel
        title="Dataset instrument"
        actions={<button type="button" onClick={dataset.reload} disabled={dataset.loading}>Reload dataset</button>}
      >
        <div className="col">
          <LoadState result={dataset} label="Loading instrument for price and quantity formatting" />
          {dataset.data ? (
            <>
              <p className="mono">{dataset.data.source.symbol} · tick {dataset.data.instrument.tick_size} · lot {dataset.data.instrument.lot_size} · contract size {dataset.data.instrument.contract_size}</p>
              {dataset.data.events_sha256 !== m.dataset.sha256 ? <p className="notice">The current dataset hash differs from the hash stored with this run. The instrument below belongs to the current dataset manifest.</p> : null}
            </>
          ) : <p className="note">Until the instrument is available, prices are explicitly shown in ticks and quantities in base units.</p>}
        </div>
      </Panel>
      <EngineCheck runId={m.run_id} instrument={instrument} partialFill={m.config.execution.exchange_model === 'partial_fill'} />
      <RunMetrics metrics={run.metrics} instrument={instrument} />
      <AccountCharts runId={m.run_id} instrument={instrument} />
      <OrdersTable runId={m.run_id} instrument={instrument} />
      <FillsTable runId={m.run_id} instrument={instrument} />
    </>
  );
}

function EngineCheck({ runId, instrument, partialFill }: { runId: string; instrument: InstrumentMeta | null; partialFill: boolean }) {
  const check = useAsync(() => api.engineCheck(runId), [runId]);
  const c = check.data;
  return (
    <Panel title="Engine cross-check" actions={<button type="button" onClick={check.reload} disabled={check.loading}>Reload cross-check</button>}>
      <div className="col">
        <LoadState result={check} label="Loading engine cross-check" />
        {c ? (
          <>
            <div className="row">
              <Badge kind={c.agrees ? 'ok' : 'warn'}>{c.agrees ? 'agrees' : 'disagrees'}</Badge>
              <BasisBadge basis="modeled" />
              <span className="note">BlackBox ledger versus the engine's final local-side account state.</span>
            </div>
            {!c.agrees ? <p className="notice" role="note">{partialFill ? 'Disagreement under the partial-fill model is expected; the engine note below explains why.' : 'The final ledger and engine state do not agree. Review the engine note below.'}</p> : null}
            <div className="table-scroll">
              <table className="table">
                <thead><tr><th scope="col">Measure</th><th scope="col" className="num">Ledger</th><th scope="col" className="num">Engine</th></tr></thead>
                <tbody>
                  <tr><th scope="row">Position</th><td className="num">{quantity(c.ledger_position, instrument)}</td><td className="num">{quantity(c.engine_position, instrument)}</td></tr>
                  <tr><th scope="row">Cash / balance (quote, before fees)</th><td className="num">{fmtSigned(c.ledger_cash)}</td><td className="num">{fmtSigned(c.engine_balance)}</td></tr>
                  <tr><th scope="row">Fees (quote)</th><td className="num">{fmtSigned(c.ledger_fees)}</td><td className="num">{fmtSigned(c.engine_fee)}</td></tr>
                  <tr><th scope="row">Fill / trade count</th><td className="num">{fmtInt(c.ledger_fills)}</td><td className="num">{fmtInt(c.engine_num_trades)}</td></tr>
                </tbody>
              </table>
            </div>
            {c.note !== '' ? <p className={c.agrees ? 'note engine-note' : 'notice engine-note'}>{c.note}</p> : <p className="note">No engine note.</p>}
          </>
        ) : null}
      </div>
    </Panel>
  );
}

function MetricGroup({ title, children }: { title: string; children: ReactNode }) {
  return <section className="col"><h3>{title}</h3><div className="stats">{children}</div></section>;
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
  return (
    <Panel title={<>Run metrics <BasisBadge basis="derived" /></>}>
      <div className="col">
        <p className="note">Arithmetic over the simulated tape, fills and sampled account state. Missing measurements are shown as {DASH}, never as zero.</p>
        <MetricGroup title="Orders">{orders.map(([label, value]) => <Stat key={label} label={label} value={fmtInt(value)} />)}</MetricGroup>
        <MetricGroup title="Fills and activity">
          <Stat label="Simulated fills" value={fmtInt(m.fill_count)} />
          <Stat label="Maker fills" value={fmtInt(m.maker_fill_count)} />
          <Stat label="Taker fills" value={fmtInt(m.taker_fill_count)} />
          <Stat label="Order fill ratio" value={fmtPct(m.fill_ratio_orders)} hint="At least one fill / submitted orders" />
          <Stat label="Quantity fill ratio" value={fmtPct(m.fill_ratio_qty)} />
          <Stat label="Volume" value={quantity(m.volume, inst)} />
          <Stat label="Turnover" value={fmtNum(m.turnover)} hint="Quote currency" />
        </MetricGroup>
        <section className="col">
          <h3>Timing quantiles</h3>
          <QuantileTable rows={[
            { label: 'Order age (exchange)', q: m.order_age_ns, fmt: fmtDuration },
            { label: 'Arrival to first fill (exchange)', q: m.time_to_first_fill_ns, fmt: fmtDuration },
            { label: 'Entry latency', q: m.entry_latency_ns, fmt: fmtDuration },
            { label: 'Response latency', q: m.response_latency_ns, fmt: fmtDuration },
            { label: 'New-order round trip (local)', q: m.round_trip_ns, fmt: fmtDuration },
          ]} />
        </section>
        <MetricGroup title="Inventory">
          <Stat label="Final position" value={quantity(m.final_position, inst)} />
          <Stat label="Maximum long" value={quantity(m.max_long, inst)} />
          <Stat label="Maximum short" value={quantity(m.max_short, inst)} />
          <Stat label="Mean absolute position" value={quantity(m.mean_abs_position, inst)} hint="Over account samples" />
        </MetricGroup>
        <MetricGroup title="P&L (quote currency)">
          <Stat label="Realized P&L" value={fmtQuoteSigned(m.realized_pnl)} hint="Before fees" />
          <Stat label="Unrealized P&L" value={fmtQuoteSigned(m.unrealized_pnl)} />
          <Stat label="Fees" value={fmtQuoteSigned(m.fees)} hint="Negative is a rebate" />
          <Stat label="Net P&L" value={fmtQuoteSigned(m.net_pnl)} hint="Realized + unrealized − fees, last sample" />
        </MetricGroup>
        <MetricGroup title="Execution quality">
          <Stat label="Spread capture" value={fmtQuoteSigned(m.spread_capture)} hint="Maker fills, mid at fill" />
          <Stat label="Spread capture (ticks)" value={fmtSigned(m.spread_capture_ticks, 4)} hint="Quantity-weighted" />
        </MetricGroup>
        {m.markouts.length === 0 ? <Empty>No markout horizons were recorded.</Empty> : (
          <div className="table-scroll">
            <table className="table">
              <caption>Post-fill mid-price markouts — negative means adverse selection. Quantity-weighted mean in ticks; horizons without a valid future mid have no measurement.</caption>
              <thead><tr><th scope="col">Horizon</th><th scope="col" className="num">Measured fills</th><th scope="col" className="num">Mean markout (ticks)</th></tr></thead>
              <tbody>{m.markouts.map((markout) => <tr key={markout.horizon_ns}><th scope="row">{fmtDuration(markout.horizon_ns)}</th><td className="num">{fmtInt(markout.fills)}</td><td className="num">{fmtSigned(markout.mean_ticks, 4)}</td></tr>)}</tbody>
            </table>
          </div>
        )}
        <MetricGroup title="Risk">
          <Stat label="Risk blocks" value={fmtInt(m.risk_blocks)} />
          <Stat label="Risk halted" value={<Badge kind={m.risk_halted ? 'warn' : 'neutral'}>{m.risk_halted ? 'yes' : 'no'}</Badge>} />
          <Stat label="Maximum drawdown" value={fmtNum(m.max_drawdown)} hint="Quote; peak-to-trough sampled equity" />
        </MetricGroup>
      </div>
    </Panel>
  );
}

function AccountCharts({ runId, instrument }: { runId: string; instrument: InstrumentMeta | null }) {
  const equity = useAsync(() => api.equity(runId), [runId]);
  const samples = equity.data;
  const series = useMemo((): { equity: ChartSeries[]; position: ChartSeries[] } => ({
    equity: [{ id: 'equity', label: 'Net equity (quote)', color: cssVar('--derived', '#b596ee'), points: lastPerSecond(samples ?? [], (s) => s.equity) }],
    position: [
      { id: 'position', label: 'Position from exchange-side simulated fills', color: cssVar('--fg', '#d7dbe0'), points: lastPerSecond(samples ?? [], (s) => s.position), stepped: true },
      { id: 'known', label: 'Position known to strategy', color: cssVar('--accent', '#9db8dc'), points: lastPerSecond(samples ?? [], (s) => s.known_position), stepped: true, dashed: true },
    ],
  }), [samples]);
  return (
    <Panel title={<>Account charts <BasisBadge basis="derived" /></>} actions={<button type="button" onClick={equity.reload} disabled={equity.loading}>Reload charts</button>}>
      <div className="col">
        <LoadState result={equity} label="Loading equity and position samples" />
        {samples && samples.length === 0 ? <Empty>No account samples were recorded.</Empty> : null}
        {samples && samples.length > 0 ? (
          <div className="grid-2">
            <section className="col">
              <h3>Equity (quote currency)</h3>
              {series.equity.some((s) => s.points.length > 0) ? <TimeSeriesChart series={series.equity} formatValue={fmtQuoteSigned} ariaLabel="Simulated net equity over time after dataset origin" /> : <Empty>No finite equity samples.</Empty>}
              <p className="note">Lightweight Charts has second-resolution time. Resampled to the last sample per second, without averaging; time is relative to dataset origin.</p>
            </section>
            <section className="col">
              <h3>Position (base quantity)</h3>
              {series.position.some((s) => s.points.length > 0) ? <TimeSeriesChart series={series.position} formatValue={(value) => quantity(value, instrument)} ariaLabel="Simulated position and strategy-known position over time after dataset origin" /> : <Empty>No finite position samples.</Empty>}
              <p className="note">Lightweight Charts has second-resolution time. Resampled to the last sample per second. Dashed position includes only fill notifications already received by the strategy.</p>
            </section>
          </div>
        ) : null}
      </div>
    </Panel>
  );
}

function OrdersTable({ runId, instrument }: { runId: string; instrument: InstrumentMeta | null }) {
  const [outcome, setOutcome] = useState<OrderOutcome | ''>('');
  const [side, setSide] = useState<Side | ''>('');
  const [offset, setOffset] = useState(0);
  const orders = useAsync(() => api.orders(runId, { outcome: outcome || undefined, side: side || undefined, offset, limit: PAGE_SIZE }), [runId, outcome, side, offset]);
  const page = orders.data;
  return (
    <Panel title={<>Orders <BasisBadge basis="modeled" /></>} actions={<button type="button" onClick={orders.reload} disabled={orders.loading}>Reload orders</button>}>
      <div className="col">
        <div className="row">
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
        </div>
        <LoadState result={orders} label="Loading orders" />
        {page ? (
          <>
            {page.orders.length === 0 ? <Empty>{page.total === 0 ? 'No orders match these filters.' : 'No orders on this page. Return to the first page.'}</Empty> : (
              <div className="table-scroll">
                <table className="table">
                  <caption>Decision time is local-clock time after dataset origin. Queue quantities are modeled, not observed. Open an order for its full simulated lifecycle.</caption>
                  <thead><tr><th scope="col">Order</th><th scope="col">Decision time</th><th scope="col">Side</th><th scope="col" className="num">Price</th><th scope="col" className="num">Qty</th><th scope="col">TIF</th><th scope="col">Outcome</th><th scope="col" className="num">Filled</th><th scope="col" className="num">Queue ahead at entry</th><th scope="col" className="num">Min queue ahead</th><th scope="col">Reason</th></tr></thead>
                  <tbody>{page.orders.map((order) => (
                    <tr key={order.order_id}>
                      <th scope="row"><Link className="mono" to={`/runs/${encodeURIComponent(runId)}/orders/${order.order_id}`}>{order.order_id}</Link></th>
                      <td className="mono">{fmtClock(order.decision_ts)}</td>
                      <td><SideBadge side={order.side} /></td>
                      <td className="num">{price(order.price_tick, instrument)}</td>
                      <td className="num">{quantity(order.qty, instrument)}</td>
                      <td className="mono">{order.tif}</td>
                      <td title={OUTCOME_MEANING[order.outcome]}>{words(order.outcome)}</td>
                      <td className="num">{quantity(order.filled_qty, instrument)}</td>
                      <td className="num">{quantity(order.queue_ahead_at_entry, instrument)}</td>
                      <td className="num">{quantity(order.min_queue_ahead, instrument)}</td>
                      <td className="wrap">{order.reason || DASH}</td>
                    </tr>
                  ))}</tbody>
                </table>
              </div>
            )}
            <Pager offset={page.offset} limit={PAGE_SIZE} total={page.total} onOffset={setOffset} noun="orders" />
          </>
        ) : null}
      </div>
    </Panel>
  );
}

function FillsTable({ runId, instrument }: { runId: string; instrument: InstrumentMeta | null }) {
  const fills = useAsync(() => api.fills(runId), [runId]);
  const [offset, setOffset] = useState(0);
  const list = fills.data;
  // A reload may shrink the collection; never leave the user on an empty stale page.
  const visibleOffset = list ? Math.min(offset, Math.max(0, Math.ceil(list.length / PAGE_SIZE) - 1) * PAGE_SIZE) : offset;
  return (
    <Panel title={<>Simulated fills <BasisBadge basis="modeled" /></>} actions={<button type="button" onClick={fills.reload} disabled={fills.loading}>Reload fills</button>}>
      <div className="col">
        <LoadState result={fills} label="Loading simulated fills" />
        {list && list.length === 0 ? <Empty>No simulated fills in this run.</Empty> : null}
        {list && list.length > 0 ? (
          <>
            <div className="table-scroll">
              <table className="table">
                <caption>Fill time is exchange-clock time after dataset origin. Monetary amounts are quote currency; realized P&amp;L is before fees. Mid at fill is derived from historical data.</caption>
                <thead><tr><th scope="col">Time</th><th scope="col">Order</th><th scope="col">Side</th><th scope="col" className="num">Price</th><th scope="col" className="num">Qty</th><th scope="col">Liquidity</th><th scope="col" className="num">Fee</th><th scope="col" className="num">Value</th><th scope="col" className="num">Position after</th><th scope="col" className="num">Realized P&amp;L</th><th scope="col" className="num">Mid at fill <BasisBadge basis="derived" /></th></tr></thead>
                <tbody>{list.slice(visibleOffset, visibleOffset + PAGE_SIZE).map((fill) => (
                  <tr key={fill.seq}>
                    <td className="mono">{fmtClock(fill.ts)}</td>
                    <th scope="row"><Link className="mono" to={`/runs/${encodeURIComponent(runId)}/orders/${fill.order_id}`}>{fill.order_id}</Link></th>
                    <td><SideBadge side={fill.side} /></td>
                    <td className="num">{price(fill.price_tick, instrument)}</td>
                    <td className="num">{quantity(fill.qty, instrument)}</td>
                    <td>{fill.maker ? 'maker' : 'taker'}</td>
                    <td className="num">{fmtSigned(fill.fee)}</td>
                    <td className="num">{fmtNum(fill.value)}</td>
                    <td className="num">{quantity(fill.position_after, instrument)}</td>
                    <td className="num">{fmtSigned(fill.realized_pnl)}</td>
                    <td className="num">{price(fill.mid_tick, instrument)}</td>
                  </tr>
                ))}</tbody>
              </table>
            </div>
            <Pager offset={visibleOffset} limit={PAGE_SIZE} total={list.length} onOffset={setOffset} noun="fills" />
          </>
        ) : null}
      </div>
    </Panel>
  );
}
