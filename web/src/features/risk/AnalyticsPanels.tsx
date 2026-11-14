// bb-analytics reports of a run (`GET /api/v1/runs/{id}/analytics`), shown verbatim: every value
// carries its unit and the number of contributing and excluded observations, and an unavailable
// value shows the server's reason instead of a number. Nothing is recomputed here. Caveats are the
// server's own, printed in full.

import type { ReactNode } from 'react';

import type { AnalyticsValue, ConfidenceBand, Distribution, ExecutionReport, KaplanMeier, PnlDecomposition, PriceMetric, QueueBucket, RiskReport, SpreadMetrics } from '../../api/model';
import { DataTable, type Column } from '../../components/DataTable';
import { DASH, fmtDuration, fmtInt, fmtStat } from '../../components/fmt';
import { Notice, Panel } from '../../components/ui';

/** An available value in its unit; a duration statistic in ns is shown as a duration. */
export function fmtValue(v: AnalyticsValue): string {
  if (v.status === 'unavailable') return 'unavailable';
  if (v.unit === 'ns') return fmtDuration(v.value);
  return fmtStat(v.value);
}

function ValueCell({ v }: { v: AnalyticsValue }) {
  return v.status === 'available' ? (
    <span className="mono">{fmtValue(v)}</span>
  ) : (
    <span className="dim" title={v.reason}>
      unavailable: {v.reason}
    </span>
  );
}

const counts = (v: AnalyticsValue) => `n ${fmtInt(v.n)}${v.excluded > 0 ? ` \u00b7 excl. ${fmtInt(v.excluded)}` : ''}`;
const unitOf = (v: AnalyticsValue) => (v.unit === 'ns' ? 'duration' : v.unit);

interface ValueRow {
  key: string;
  label: ReactNode;
  value: AnalyticsValue;
  note?: string;
}

const VALUE_COLUMNS: Column<ValueRow>[] = [
  { id: 'label', header: 'Metric', rowHeader: true, cell: (r) => r.label },
  { id: 'value', header: 'Value', numeric: true, cell: (r) => <ValueCell v={r.value} />, raw: (r) => (r.value.status === 'available' ? String(r.value.value) : r.value.reason) },
  { id: 'unit', header: 'Unit', cell: (r) => <span className="dim">{unitOf(r.value)}</span> },
  { id: 'n', header: 'Observations', cell: (r) => <span className="mono dim">{counts(r.value)}</span> },
  { id: 'note', header: 'Note', cell: (r) => (r.note ? <span className="note">{r.note}</span> : DASH) },
];

function ValueTable({ label, rows }: { label: string; rows: ValueRow[] }) {
  return <DataTable label={label} columns={VALUE_COLUMNS} rows={rows} rowKey={(r) => r.key} />;
}

function Caveats({ list }: { list: string[] }) {
  if (list.length === 0) return null;
  return (
    <ul className="risk-caveats">
      {list.map((c) => (
        <li key={c}>{c}</li>
      ))}
    </ul>
  );
}

const band = (b: ConfidenceBand) =>
  b.lower.status === 'available' && b.upper.status === 'available' ? `[${fmtValue(b.lower)}, ${fmtValue(b.upper)}] (${b.method})` : `unavailable (${b.method})`;

// ----- risk ------------------------------------------------------------------------------------

export function RiskAnalytics({ risk }: { risk: RiskReport }) {
  const horizon = risk.bucket_horizon_ns.status === 'available' ? fmtValue(risk.bucket_horizon_ns) : 'unknown horizon';
  const rows: ValueRow[] = [
    { key: 'net', label: 'Net P&L', value: risk.net_pnl, note: `mark: ${risk.mark}` },
    { key: 'turnover', label: 'Turnover', value: risk.turnover },
    { key: 'ppt', label: 'Net P&L per turnover', value: risk.pnl_per_turnover_bps, note: 'Fee-dominated when the edge is small.' },
    { key: 'dd', label: 'Maximum drawdown', value: risk.max_drawdown, note: 'Path-dependent; one realisation; not a risk forecast.' },
    { key: 'tuw', label: 'Time under water', value: risk.time_under_water_ns },
    { key: 'map', label: 'Mean |position|', value: risk.mean_absolute_position },
    { key: 'tal', label: 'Time at the position limit', value: risk.time_at_position_limit_ns },
    { key: 'long', label: 'Max long', value: risk.max_long },
    { key: 'short', label: 'Max short', value: risk.max_short },
    {
      key: 'sr',
      label: `Bucket Sharpe ratio (${horizon} buckets, not annualised)`,
      value: risk.bucket_sharpe,
      note: `Mean / sd of non-overlapping bucket P&L; needs \u2265 30 buckets. Selected from ${fmtInt(risk.trials)} trial${risk.trials === 1 ? '' : 's'}.`,
    },
    { key: 'se', label: 'Sharpe HAC standard error', value: risk.sharpe_hac_se, note: 'Newey\u2013West, delta method.' },
    { key: 'sample', label: 'Equity sample interval', value: risk.sample_interval_ns },
  ];
  return (
    <Panel title="Risk analytics" basis="derived" flush caption={`bb-analytics ${risk.analytics_version}. Path statistics use the equity series' mark: ${risk.mark}.`}>
      <ValueTable label="Risk analytics" rows={rows} />
      <Caveats list={risk.caveats} />
    </Panel>
  );
}

// ----- execution -------------------------------------------------------------------------------

interface PriceRow {
  key: string;
  label: string;
  m: PriceMetric;
  note?: string;
}

const PRICE_COLUMNS: Column<PriceRow>[] = [
  { id: 'label', header: 'Metric', rowHeader: true, cell: (r) => r.label },
  { id: 'price', header: 'Quote', numeric: true, cell: (r) => <ValueCell v={r.m.price} /> },
  { id: 'ticks', header: 'Ticks', numeric: true, cell: (r) => <ValueCell v={r.m.ticks} /> },
  { id: 'bps', header: 'bp', numeric: true, cell: (r) => <ValueCell v={r.m.bps} /> },
  { id: 'n', header: 'Observations', cell: (r) => <span className="mono dim">{counts(r.m.price)}</span> },
  { id: 'note', header: 'Note', cell: (r) => (r.note ? <span className="note">{r.note}</span> : DASH) },
];

function spreadRows(s: SpreadMetrics): PriceRow[] {
  const rows: PriceRow[] = [
    { key: 'eh', label: 'Effective half-spread', m: s.effective_half, note: 'Against the pre-fill mid M(t\u207b).' },
    { key: 'ef', label: 'Effective spread', m: s.effective_full },
    { key: 'arr', label: 'Slippage vs arrival mid', m: s.arrival_slippage },
    { key: 'dec', label: 'Slippage vs decision price', m: s.decision_slippage },
    { key: 'lat', label: 'Latency drift (decision \u2192 arrival)', m: s.latency_drift },
  ];
  for (const h of s.horizons) {
    const at = fmtDuration(h.horizon_ns);
    rows.push(
      { key: `rs-${h.horizon_ns}`, label: `Realised half-spread @ ${at}`, m: h.realised_half },
      { key: `mo-${h.horizon_ns}`, label: `Markout @ ${at}`, m: h.markout },
      { key: `as-${h.horizon_ns}`, label: `Adverse selection @ ${at}`, m: h.adverse_selection, note: `95% band ${band(h.adverse_selection_95)}` },
      { key: `me-${h.horizon_ns}`, label: `Matched effective half-spread @ ${at}`, m: h.matched_effective_half, note: 'Same cohort as RS and AS.' },
    );
  }
  return rows;
}

function distributionRows(prefix: string, label: string, d: Distribution): ValueRow[] {
  return [
    { key: `${prefix}-total`, label: `${label}: total`, value: d.total },
    { key: `${prefix}-median`, label: `${label}: median`, value: d.median },
    { key: `${prefix}-p05`, label: `${label}: p05`, value: d.p05 },
    { key: `${prefix}-p95`, label: `${label}: p95`, value: d.p95 },
  ];
}

function kmRows(prefix: string, label: string, km: KaplanMeier): ValueRow[] {
  return [
    {
      key: `${prefix}-p`,
      label: `${label}: fill probability`,
      value: km.fill_probability,
      note: `Kaplan\u2013Meier; ${fmtInt(km.n)} orders, ${fmtInt(km.censored)} censored, ${fmtInt(km.excluded)} excluded; 95% band ${band(km.probability_95)}.`,
    },
    { key: `${prefix}-m`, label: `${label}: median time`, value: km.median_ns },
  ];
}

function pnlRows(side: string, p: PnlDecomposition): ValueRow[] {
  return [
    { key: `${side}-edge`, label: `${side}: edge`, value: p.edge },
    { key: `${side}-drift`, label: `${side}: drift`, value: p.drift },
    { key: `${side}-fees`, label: `${side}: fees`, value: p.fees },
    { key: `${side}-net`, label: `${side}: net`, value: p.net },
  ];
}

const QUEUE_COLUMNS: Column<QueueBucket & { key: string }>[] = [
  {
    id: 'bucket',
    header: 'Queue ahead at entry',
    rowHeader: true,
    cell: (b) => `${b.lower === null ? '\u2212\u221e' : fmtStat(b.lower)} \u2264 x < ${b.upper === null ? '\u221e' : fmtStat(b.upper)} ${b.covariate_unit}`,
  },
  { id: 'n', header: 'Orders', numeric: true, cell: (b) => fmtInt(b.estimate.n) },
  { id: 'p', header: 'Fill probability', numeric: true, cell: (b) => <ValueCell v={b.estimate.fill_probability} /> },
  { id: 'ci', header: '95% band', cell: (b) => <span className="mono">{band(b.estimate.probability_95)}</span> },
  { id: 'censored', header: 'Censored', numeric: true, cell: (b) => fmtInt(b.estimate.censored) },
];

export function ExecutionAnalytics({ execution: e }: { execution: ExecutionReport }) {
  const opportunities: ValueRow[] = e.opportunities.flatMap(([h, d]) => distributionRows(`opp-${h}`, `Opportunity cost @ ${fmtDuration(h)}`, d));
  return (
    <div className="col risk-analytics">
      <Panel
        title="Execution analytics: spreads and markouts"
        basis="derived"
        flush
        caption={`bb-analytics ${e.analytics_version}; ${fmtInt(e.order_count)} orders, ${fmtInt(e.unassociated_fills)} fills not associated with an order. Fills and queue position are MODELED.`}
      >
        {(['all', 'maker', 'taker'] as const).map((side) => (
          <section key={side} className="col">
            <h3 className="run-subhead">
              {side} fills {'\u00b7'} filled quantity <ValueCell v={e.spreads[side].filled_quantity} />
            </h3>
            <DataTable label={`Spread metrics, ${side} fills`} columns={PRICE_COLUMNS} rows={spreadRows(e.spreads[side])} rowKey={(r) => r.key} />
          </section>
        ))}
      </Panel>
      <Panel title="Implementation shortfall and opportunity cost" basis="derived" flush caption="Only explicitly directional orders are eligible.">
        <ValueTable
          label="Shortfall"
          rows={[
            ...distributionRows('is', 'Shortfall', e.shortfall),
            ...distributionRows('is-ex', 'Execution component', e.shortfall_components.execution),
            ...distributionRows('is-de', 'Delay component', e.shortfall_components.delay),
            ...distributionRows('is-tr', 'Trading component', e.shortfall_components.trading),
            ...distributionRows('is-op', 'Opportunity component', e.shortfall_components.opportunity),
            ...distributionRows('is-fe', 'Fees component', e.shortfall_components.fees),
            ...opportunities,
          ]}
        />
      </Panel>
      <Panel title="P&L decomposition" basis="derived" flush caption={`Edge, drift and fees on one mark; exchange end mid ${fmtValue(e.exchange_end_mid)}.`}>
        <ValueTable label="P&L decomposition" rows={[...pnlRows('all', e.pnl.all), ...pnlRows('maker', e.pnl.maker), ...pnlRows('taker', e.pnl.taker)]} />
      </Panel>
      <Panel title="Fill timing and cancels" basis="modeled" flush>
        <ValueTable
          label="Fill timing and cancels"
          rows={[
            ...distributionRows('ff', 'Time to first fill (observed)', e.first_fill.observed_ns),
            ...kmRows('ffkm', 'First fill', e.first_fill.km),
            ...distributionRows('cf', 'Time to complete fill (observed)', e.complete_fill.observed_ns),
            ...kmRows('cfkm', 'Complete fill', e.complete_fill.km),
            ...distributionRows('cx', 'Cancel to exchange', e.cancels.to_exchange_ns),
            ...distributionRows('crt', 'Cancel round trip', e.cancels.round_trip_ns),
            ...distributionRows('cq', 'In-flight quantity at cancel', e.cancels.in_flight_quantity),
          ]}
        />
        <p className="note">
          Cancels: {fmtInt(e.cancels.attempts)} attempts, {fmtInt(e.cancels.late_cancels)} late, {fmtInt(e.cancels.exchange_censored)} censored before
          the exchange, {fmtInt(e.cancels.acknowledgement_censored)} censored before acknowledgement.
        </p>
      </Panel>
      <Panel
        title={`Fill probability by modeled queue position (within ${fmtDuration(e.queue.horizon_ns)})`}
        basis="modeled"
        flush
        caption={`${e.queue.modeled ? 'Queue position is the engine model\u2019s estimate, not an observation.' : 'Queue position as recorded.'} Excluded: ${fmtInt(e.queue.excluded_relative)} (relative), ${fmtInt(e.queue.excluded_absolute)} (absolute).`}
      >
        <DataTable label="Relative queue buckets" columns={QUEUE_COLUMNS} rows={e.queue.relative.map((b, i) => ({ ...b, key: `r${i}` }))} rowKey={(b) => b.key} />
        <DataTable label="Absolute queue buckets (lots)" columns={QUEUE_COLUMNS} rows={e.queue.absolute_lots.map((b, i) => ({ ...b, key: `a${i}` }))} rowKey={(b) => b.key} />
      </Panel>
      {e.order_details_streamed ? (
        <Notice tag="NOT SERVED" tone="info">
          Per-order execution details are not retained by the API (order_details_streamed); only the aggregates above are served.
        </Notice>
      ) : null}
      <Caveats list={e.caveats} />
    </div>
  );
}
