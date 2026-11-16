// bb-analytics reports of a run (`GET /api/v1/runs/{id}/analytics`), shown verbatim: every value
// carries its unit and the number of contributing and excluded observations. An unavailable value
// is a dash with the server's reason as its title, and each table states its reasons once in a
// notice above it (the same reason repeated in every cell drowned the numbers). Nothing is
// recomputed here. Caveats are the server's own, printed in full.

import type { ReactNode } from 'react';

import type { AnalyticsValue, ConfidenceBand, Distribution, ExecutionReport, KaplanMeier, PnlDecomposition, PriceMetric, QueueBucket, RiskReport, SpreadMetrics } from '../../api/model';
import { DataTable, type Column } from '../../components/DataTable';
import { DASH, fmtDuration, fmtInt, fmtStat } from '../../components/fmt';
import { unitLabel } from '../../components/labels';
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
    <span className="dim" title={`Unavailable: ${v.reason}`}>
      {DASH}
      <span className="visually-hidden"> unavailable: {v.reason}</span>
    </span>
  );
}

/**
 * The reasons behind the dashes of one table, once: a single sentence when every unavailable
 * value shares its reason, else each distinct reason with how many values it explains.
 */
function UnavailableNotice({ values }: { values: AnalyticsValue[] }) {
  const reasons = new Map<string, number>();
  for (const v of values) if (v.status === 'unavailable') reasons.set(v.reason, (reasons.get(v.reason) ?? 0) + 1);
  if (reasons.size === 0) return null;
  const list = [...reasons];
  return (
    <div className="risk-unavailable">
      <Notice tag="UNAVAILABLE" tone="info">
        {list.length === 1
          ? `${DASH} marks a value the analytics engine could not compute: ${list[0][0]}.`
          : `${DASH} marks a value the analytics engine could not compute (each dash names its reason on hover): ${list.map(([r, n]) => `${r} (${fmtInt(n)})`).join('; ')}.`}
      </Notice>
    </div>
  );
}

/** Tables shorter than this take their full height in the page instead of scrolling inside it. */
const SHORT_TABLE = 30;

/** A table inside the scrolling Risk page: short ones drop the DataTable's inner height cap. */
export function PageTable({ rows, children }: { rows: number; children: ReactNode }) {
  return <div className={rows < SHORT_TABLE ? 'risk-table-full' : undefined}>{children}</div>;
}

const counts = (v: AnalyticsValue) => `n ${fmtInt(v.n)}${v.excluded > 0 ? ` \u00b7 excl. ${fmtInt(v.excluded)}` : ''}`;

interface ValueRow {
  key: string;
  label: ReactNode;
  value: AnalyticsValue;
  note?: string;
}

const VALUE_COLUMNS: Column<ValueRow>[] = [
  { id: 'label', header: 'Metric', rowHeader: true, cell: (r) => r.label },
  { id: 'value', header: 'Value', numeric: true, cell: (r) => <ValueCell v={r.value} />, raw: (r) => (r.value.status === 'available' ? String(r.value.value) : r.value.reason) },
  { id: 'unit', header: 'Unit', cell: (r) => <span className="dim">{r.value.unit === 'ns' ? 'duration' : unitLabel(r.value.unit)}</span>, raw: (r) => r.value.unit },
  { id: 'n', header: 'Observations', cell: (r) => <span className="mono dim">{counts(r.value)}</span> },
  { id: 'note', header: 'Note', className: () => 'wrap', cell: (r) => (r.note ? <span className="note">{r.note}</span> : DASH) },
];

function ValueTable({ label, rows }: { label: string; rows: ValueRow[] }) {
  return (
    <>
      <UnavailableNotice values={rows.map((r) => r.value)} />
      <PageTable rows={rows.length}>
        <DataTable label={label} columns={VALUE_COLUMNS} rows={rows} rowKey={(r) => r.key} />
      </PageTable>
    </>
  );
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

const bandAvailable = (b: ConfidenceBand) => b.lower.status === 'available' && b.upper.status === 'available';

const band = (b: ConfidenceBand) => (bandAvailable(b) ? `[${fmtValue(b.lower)}, ${fmtValue(b.upper)}] (${b.method})` : `unavailable (${b.method})`);

function BandCell({ b }: { b: ConfidenceBand }) {
  return bandAvailable(b) ? (
    <span className="mono">{band(b)}</span>
  ) : (
    <span className="dim" title={`Unavailable: ${b.method}`}>
      {DASH}
      <span className="visually-hidden"> unavailable: {b.method}</span>
    </span>
  );
}

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
    <Panel
      title="Risk analytics"
      basis="derived"
      flush
      caption={
        <span title={`bb-analytics ${risk.analytics_version}`}>
          Computed by the analytics engine (version {risk.analytics_version}). Path statistics use the equity series&apos; mark: {risk.mark}.
        </span>
      }
    >
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
  { id: 'note', header: 'Note', className: () => 'wrap', cell: (r) => (r.note ? <span className="note">{r.note}</span> : DASH) },
];

function SpreadTable({ side, s }: { side: 'all' | 'maker' | 'taker'; s: SpreadMetrics }) {
  const rows = spreadRows(s);
  return (
    <section className="col">
      <h3 className="run-subhead">
        {side} fills {'\u00b7'} filled quantity <ValueCell v={s.filled_quantity} />
      </h3>
      <UnavailableNotice values={[s.filled_quantity, ...rows.flatMap((r) => [r.m.price, r.m.ticks, r.m.bps])]} />
      <PageTable rows={rows.length}>
        <DataTable label={`Spread metrics, ${side} fills`} columns={PRICE_COLUMNS} rows={rows} rowKey={(r) => r.key} />
      </PageTable>
    </section>
  );
}

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
      label: `${label}: modeled fill probability (Kaplan\u2013Meier, simulated)`,
      value: km.fill_probability,
      note: `Kaplan\u2013Meier horizon estimate over simulated orders; ${fmtInt(km.n)} orders, ${fmtInt(km.censored)} censored, ${fmtInt(km.excluded)} excluded; 95% band ${band(km.probability_95)}. It differs from the raw fill ratio because orders cancelled or still open at the end are censored, not counted as unfilled.`,
    },
    { key: `${prefix}-m`, label: `${label}: median time`, value: km.median_ns },
  ];
}

function pnlRows(side: string, p: PnlDecomposition): ValueRow[] {
  return [
    { key: `${side}-edge`, label: `${side}: edge`, value: p.edge },
    { key: `${side}-drift`, label: `${side}: drift`, value: p.drift },
    { key: `${side}-fees`, label: `${side}: fees`, value: p.fees },
    { key: `${side}-net`, label: `${side}: net at exchange end mid`, value: p.net },
  ];
}

type QueueRow = QueueBucket & { key: string };

const QUEUE_COLUMNS: Column<QueueRow>[] = [
  {
    id: 'bucket',
    header: 'Queue ahead at entry',
    rowHeader: true,
    cell: (b) => `${b.lower == null ? '\u2212\u221e' : fmtStat(b.lower)} \u2264 x < ${b.upper == null ? '\u221e' : fmtStat(b.upper)} ${b.covariate_unit}`,
  },
  { id: 'n', header: 'Orders', numeric: true, cell: (b) => fmtInt(b.estimate.n) },
  { id: 'p', header: 'Modeled fill probability', numeric: true, cell: (b) => <ValueCell v={b.estimate.fill_probability} /> },
  { id: 'ci', header: '95% band', cell: (b) => <BandCell b={b.estimate.probability_95} /> },
  { id: 'censored', header: 'Censored', numeric: true, cell: (b) => fmtInt(b.estimate.censored) },
];

function QueueTable({ id, title, caption, buckets }: { id: string; title: string; caption: string; buckets: QueueBucket[] }) {
  const rows: QueueRow[] = buckets.map((b, i) => ({ ...b, key: `${id}${i}` }));
  return (
    <section className="col" aria-labelledby={`risk-queue-${id}`}>
      <h3 id={`risk-queue-${id}`} className="run-subhead">
        {title}
      </h3>
      <p className="note risk-table-caption">{caption}</p>
      <UnavailableNotice
        values={buckets.flatMap((b) => {
          const { lower, upper } = b.estimate.probability_95;
          return [b.estimate.fill_probability, ...[lower, upper].filter((v) => v.status === 'unavailable').slice(0, 1)];
        })}
      />
      <PageTable rows={rows.length}>
        <DataTable label={title} columns={QUEUE_COLUMNS} rows={rows} rowKey={(b) => b.key} />
      </PageTable>
    </section>
  );
}

export function ExecutionAnalytics({ execution: e }: { execution: ExecutionReport }) {
  const opportunities: ValueRow[] = e.opportunities.flatMap(([h, d]) => distributionRows(`opp-${h}`, `Opportunity cost @ ${fmtDuration(h)}`, d));
  return (
    <div className="col risk-analytics">
      <Panel
        title="Execution analytics: spreads and markouts"
        basis="derived"
        flush
        caption={
          <span title={`bb-analytics ${e.analytics_version}`}>
            Computed by the analytics engine (version {e.analytics_version}); {fmtInt(e.order_count)} orders, {fmtInt(e.unassociated_fills)} fills not associated
            with an order. Fills and queue position are MODELED.
          </span>
        }
      >
        {(['all', 'maker', 'taker'] as const).map((side) => (
          <SpreadTable key={side} side={side} s={e.spreads[side]} />
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
      <Panel
        title="P&L decomposition"
        basis="derived"
        flush
        caption={`Edge + drift − fees at exchange end mid (${fmtValue(e.exchange_end_mid)}); may differ from run Net P&L marked at local mid.`}
      >
        <ValueTable label="P&L decomposition" rows={[...pnlRows('all', e.pnl.all), ...pnlRows('maker', e.pnl.maker), ...pnlRows('taker', e.pnl.taker)]} />
      </Panel>
      <Panel title="Fill timing and cancels" basis="modeled" flush>
        <ValueTable
          label="Fill timing and cancels"
          rows={[
            ...distributionRows('ff', 'Time to first simulated fill (uncensored)', e.first_fill.observed_ns),
            ...kmRows('ffkm', 'First fill', e.first_fill.km),
            ...distributionRows('cf', 'Time to complete simulated fill (uncensored)', e.complete_fill.observed_ns),
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
        <QueueTable
          id="r"
          title="By relative queue position"
          caption="Modeled queue ahead at entry divided by the quantity displayed at the order's price level (a ratio)."
          buckets={e.queue.relative}
        />
        <QueueTable id="a" title="By absolute queue ahead (lots)" caption="Modeled queue ahead at entry, in lots of the instrument." buckets={e.queue.absolute_lots} />
      </Panel>
      {e.order_details_streamed ? (
        <Notice tag="NOT SERVED" tone="info">
          Per-order execution details are not retained by the API; only the aggregates above are served.
        </Notice>
      ) : null}
      <Caveats list={e.caveats} />
    </div>
  );
}
