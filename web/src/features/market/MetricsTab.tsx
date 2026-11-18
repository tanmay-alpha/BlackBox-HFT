// Step 5, Research metrics: series over the session sharing the selected event, the L3-native
// counts and distributions, the L3 -> L2 information-loss summary and the existing L2 metrics
// (M1-M9). Every figure is the server's; the only arithmetic here is spread (ask - bid), share
// imbalance and percentages, each labelled derived.

import { useMemo } from 'react';
import type { ReactNode } from 'react';

import { DataTable, type Column } from '../../components/DataTable';
import { fmtDuration, fmtInt, fmtStat } from '../../components/fmt';
import { BasisBadge, ErrorBox, Notice, Panel, Spinner, useAsync } from '../../components/ui';
import type { Basis } from '../../api/types';
import { fmtValue } from '../risk/AnalyticsPanels';
import { marketApi, type MarketComparison, type MarketMetrics, type MarketReport, type MetricPoint } from './api';
import { MetricChart, type ChartLine } from './MetricChart';
import { fmtPrice4, fmtShare, fmtShares, ratio, sampleAtOrBefore, shareImbalance } from './model';

/** Samples requested for the session series (the server allows 2..500). */
const MAX_POINTS = 300;

interface ChartSpec {
  id: string;
  title: string;
  basis: Basis;
  definition: ReactNode;
  lines: ChartLine[];
  fmt: (v: number) => string;
  domain?: [number, number];
  zero?: boolean;
}

const price = (v: number) => fmtPrice4(Math.round(v));
const shares = (v: number) => fmtShares(Math.round(v));
const fraction = (v: number) => v.toFixed(2);

function buildCharts(series: MetricPoint[], defs: Record<string, string>): ChartSpec[] {
  const pick = (f: (p: MetricPoint) => number | null) => series.map(f);
  const server = (...keys: string[]) =>
    keys
      .map((k) => defs[k])
      .filter((d): d is string => typeof d === 'string')
      .join(' ');
  return [
    {
      id: 'spread',
      title: 'Quoted spread',
      basis: 'derived',
      definition: (
        <>
          Best ask minus best bid after the sampled event, in price units (quote currency). Empty where either side is empty.{' '}
          {server('spread', 'best_bid_price4', 'best_ask_price4')}
        </>
      ),
      lines: [
        {
          id: 'spread',
          label: 'spread',
          color: 'var(--derived)',
          values: pick((p) => (p.best_bid_price4 === null || p.best_ask_price4 === null ? null : p.best_ask_price4 - p.best_bid_price4)),
        },
      ],
      fmt: price,
      zero: true,
    },
    {
      id: 'touch',
      title: 'Best bid and ask',
      basis: 'observed',
      definition: <>Best bid and best ask of the reconstructed book after the sampled event, in price units.</>,
      lines: [
        { id: 'bid', label: 'best bid', color: 'var(--bid)', values: pick((p) => p.best_bid_price4) },
        { id: 'ask', label: 'best ask', color: 'var(--ask)', values: pick((p) => p.best_ask_price4) },
      ],
      fmt: price,
    },
    {
      id: 'depth',
      title: 'Book depth',
      basis: 'observed',
      definition: <>Total resting shares on each side of the whole book of this symbol (all levels), after the sampled event. {server('bid_shares', 'ask_shares')}</>,
      lines: [
        { id: 'bid', label: 'bid shares', color: 'var(--bid)', values: pick((p) => p.bid_shares) },
        { id: 'ask', label: 'ask shares', color: 'var(--ask)', values: pick((p) => p.ask_shares) },
      ],
      fmt: shares,
      zero: true,
    },
    {
      id: 'imbalance',
      title: 'Depth imbalance',
      basis: 'derived',
      definition: (
        <>
          Share imbalance = (bid shares {'\u2212'} ask shares) / (bid shares + ask shares), whole book, dimensionless in [{'\u2212'}1, 1].
          Order-count imbalance is computed by the server: {server('order_count_imbalance') || 'see the definitions below.'}
        </>
      ),
      lines: [
        { id: 'shares', label: 'share imbalance', color: 'var(--derived)', values: pick((p) => shareImbalance(p.bid_shares, p.ask_shares)) },
        { id: 'orders', label: 'order-count imbalance', color: 'var(--accent)', values: pick((p) => p.order_count_imbalance), dashed: true },
      ],
      fmt: fraction,
      domain: [-1, 1],
      zero: true,
    },
    {
      id: 'flow',
      title: 'Order flow at the sampled event',
      basis: 'observed',
      definition: (
        <>
          Signed shares added, cancelled and executed by the sampled event alone (buy positive, sell negative), not a total over the
          bucket: most events between samples are not drawn. Totals are in the counts table.{' '}
          {server('signed_added_shares', 'signed_cancelled_shares', 'signed_executed_shares')}
        </>
      ),
      lines: [
        { id: 'add', label: 'added', color: 'var(--bid)', values: pick((p) => p.signed_added_shares) },
        { id: 'cancel', label: 'cancelled', color: 'var(--ask)', values: pick((p) => p.signed_cancelled_shares), dashed: true },
        { id: 'exec', label: 'executed', color: 'var(--accent)', values: pick((p) => p.signed_executed_shares) },
      ],
      fmt: shares,
      zero: true,
    },
    {
      id: 'touchflow',
      title: 'Touch depletion and replenishment',
      basis: 'observed',
      definition: (
        <>
          Shares removed from and added to the best price as it stood before the sampled event, by that event alone (a move of the
          touch itself is not counted). {server('touch_depleted_shares', 'touch_replenished_shares')}
        </>
      ),
      lines: [
        { id: 'dep', label: 'depleted', color: 'var(--ask)', values: pick((p) => p.touch_depleted_shares) },
        { id: 'rep', label: 'replenished', color: 'var(--bid)', values: pick((p) => p.touch_replenished_shares), dashed: true },
      ],
      fmt: shares,
      zero: true,
    },
    {
      id: 'live',
      title: 'Live orders',
      basis: 'observed',
      definition: <>Resting orders of this symbol after the sampled event (count). {server('live_orders')}</>,
      lines: [{ id: 'live', label: 'live orders', color: 'var(--observed)', values: pick((p) => p.live_orders) }],
      fmt: (v) => fmtInt(Math.round(v)),
      zero: true,
    },
  ];
}

function Readout({ spec, series, selected }: { spec: ChartSpec; series: MetricPoint[]; selected: number | null }) {
  if (selected === null) return <p className="note">Select an event (timeline, J/K, or click a chart) to read values here.</p>;
  const i = sampleAtOrBefore(series, selected);
  if (i < 0) return <p className="note">No sample at or before event #{fmtInt(selected)}.</p>;
  const at = series[i].event_index;
  return (
    <p className="note">
      {at === selected ? `At event #${fmtInt(at)}: ` : `At the last sample at or before #${fmtInt(selected)} (event #${fmtInt(at)}): `}
      {spec.lines.map((l, k) => {
        const v = l.values[i];
        return (
          <span key={l.id}>
            {k > 0 ? ' \u00b7 ' : ''}
            {l.label} <span className="mono">{v === null || v === undefined ? '\u2014' : spec.fmt(v)}</span>
          </span>
        );
      })}
    </p>
  );
}

interface CountRow {
  key: string;
  count: number;
  rate: number | null | undefined;
}

const COUNT_COLUMNS: Column<CountRow>[] = [
  { id: 'key', header: 'Measure', rowHeader: true, cell: (r) => r.key.replaceAll('_', ' ') },
  { id: 'count', header: 'Total', unit: 'count', numeric: true, cell: (r) => fmtInt(r.count), sortValue: (r) => r.count },
  {
    id: 'rate',
    header: 'Rate',
    unit: 'per second',
    numeric: true,
    cell: (r) => (r.rate === null || r.rate === undefined ? '\u2014' : fmtStat(r.rate)),
    sortValue: (r) => r.rate ?? null,
  },
];

interface LossRow {
  measure: string;
  value: number;
  of: string;
  share: number | null;
}

const LOSS_COLUMNS: Column<LossRow>[] = [
  { id: 'm', header: 'Measure', rowHeader: true, cell: (r) => r.measure },
  { id: 'v', header: 'Count', unit: 'count', numeric: true, cell: (r) => fmtInt(r.value) },
  { id: 's', header: 'Share', unit: '%', numeric: true, cell: (r) => (r.share === null ? '\u2014' : `${fmtShare(r.share)} ${r.of}`) },
];

function lossRows(c: MarketComparison): LossRow[] {
  const cancels = c.cancel_messages;
  return [
    { measure: 'Events naming an individual order', value: c.order_identity_observations, of: 'of events', share: ratio(c.order_identity_observations, c.events) },
    { measure: 'Queue positions read at the touch', value: c.touch_queue_observations, of: '', share: null },
    { measure: 'Execution messages', value: c.execution_messages, of: '', share: null },
    {
      measure: 'Executions attributable to a specific order',
      value: c.executions_with_order_identity,
      of: 'of executions',
      share: ratio(c.executions_with_order_identity, c.execution_messages),
    },
    { measure: 'Cancellations (X and D)', value: cancels, of: '', share: null },
    { measure: 'Cancels ahead of the reference order', value: c.cancels_ahead_of_reference, of: 'of cancels', share: ratio(c.cancels_ahead_of_reference, cancels) },
    { measure: 'Cancels of the reference order', value: c.cancels_at_reference, of: 'of cancels', share: ratio(c.cancels_at_reference, cancels) },
    { measure: 'Cancels behind the reference order', value: c.cancels_behind_reference, of: 'of cancels', share: ratio(c.cancels_behind_reference, cancels) },
    { measure: 'Same-timestamp add + cancel at one level', value: c.same_timestamp_add_cancel_level_groups, of: '', share: null },
  ];
}

type Summary = MarketReport['metrics'][number];

const L2_COLUMNS: Column<Summary>[] = [
  { id: 'name', header: 'Metric', rowHeader: true, cell: (m) => m.name.replaceAll('_', ' ') },
  { id: 'unit', header: 'Unit', cell: (m) => m.unit },
  { id: 'event', header: 'Event mean', numeric: true, cell: (m) => fmtValue(m.event_mean) },
  { id: 'tw', header: 'Time-weighted mean', numeric: true, cell: (m) => fmtValue(m.time_weighted_mean) },
  { id: 'min', header: 'Min', numeric: true, cell: (m) => fmtValue(m.event_min) },
  { id: 'max', header: 'Max', numeric: true, cell: (m) => fmtValue(m.event_max) },
];

function Definitions({ defs }: { defs: Record<string, string> }) {
  const keys = Object.keys(defs).sort();
  if (keys.length === 0) return null;
  return (
    <details>
      <summary>Definitions as the server states them ({fmtInt(keys.length)})</summary>
      <dl className="kv mk-defs">
        {keys.map((k) => (
          <div key={k} className="mk-def">
            <dt className="mono">{k}</dt>
            <dd>{defs[k]}</dd>
          </div>
        ))}
      </dl>
    </details>
  );
}

function SeriesPanels({ metrics, selected, onSelect }: { metrics: MarketMetrics; selected: number | null; onSelect: (i: number) => void }) {
  const series = metrics.series;
  const xs = useMemo(() => series.map((p) => p.event_index), [series]);
  const charts = useMemo(() => buildCharts(series, metrics.definitions), [series, metrics.definitions]);
  if (series.length === 0) return <Notice tag="No series">The server returned no metric samples for this dataset.</Notice>;
  const first = xs[0];
  const last = xs[xs.length - 1];
  return (
    <>
      <Notice tag="Sampling" tone="info">
        {fmtInt(series.length)} samples over events #{fmtInt(first)}{'\u2013'}#{fmtInt(last)}: each is the last event of an equal
        event-index bucket, so the x axis is event index, not clock time, and values hold until the next sample. A sample never uses a
        later event. The guide marks the selected event; click a chart to select the sample at or before that point.
      </Notice>
      <div className="grid-2 mk-charts">
        {charts.map((c) => (
          <Panel key={c.id} title={c.title} basis={c.basis} caption={c.definition}>
            <MetricChart
              xs={xs}
              lines={c.lines}
              selected={selected}
              onSelect={onSelect}
              fmt={c.fmt}
              domain={c.domain}
              zero={c.zero}
              ariaLabel={`${c.title} over event index ${first} to ${last}: ${c.lines.map((l) => l.label).join(', ')}.`}
            />
            <Readout spec={c} series={series} selected={selected} />
          </Panel>
        ))}
      </div>
    </>
  );
}

export function MetricsTab({ datasetId, selected, onSelect }: { datasetId: string; selected: number | null; onSelect: (i: number) => void }) {
  const metrics = useAsync((signal) => marketApi.metrics(datasetId, { max_points: MAX_POINTS }, { signal }), [datasetId]);
  const comparison = useAsync((signal) => marketApi.comparison(datasetId, { signal }), [datasetId]);
  const m = metrics.data;
  const c = comparison.data;

  const countRows: CountRow[] = m ? Object.entries(m.counts).map(([key, count]) => ({ key, count, rate: m.rates_per_second[key] })) : [];
  const lifetime = m?.order_lifetime_ns;
  const queue = m?.queue_position_at_execution;

  return (
    <div className="mk-scroll">
      <div className="col mk-stack">
        <Notice tag="Labels" tone="info">
          <BasisBadge basis="observed" /> values are read off the reconstructed book and message stream;{' '}
          <BasisBadge basis="derived" /> values are arithmetic over them. Nothing here is modeled, and with synthetic data nothing describes a
          real market.
        </Notice>
        {metrics.error ? (
          <ErrorBox error={metrics.error} onRetry={metrics.reload} />
        ) : !m ? (
          <Spinner label="Loading metric series" />
        ) : (
          <>
            <SeriesPanels metrics={m} selected={selected} onSelect={onSelect} />
            <div className="grid-2">
              <Panel title="Message counts and rates" basis="observed" flush caption={`${m.basis}. Totals and per-second rates as the server computed them.`}>
                <DataTable label="Message counts and rates" columns={COUNT_COLUMNS} rows={countRows} rowKey={(r) => r.key} empty="No counts." />
              </Panel>
              <Panel title="Order lifetime and queue position at execution" basis="derived">
                <dl className="kv">
                  <dt>Order lifetime</dt>
                  <dd className="mono">
                    {lifetime && lifetime.n > 0
                      ? `n ${fmtInt(lifetime.n)} \u00b7 min ${fmtDuration(lifetime.min)} \u00b7 p50 ${fmtDuration(lifetime.p50)} \u00b7 p90 ${fmtDuration(lifetime.p90)} \u00b7 max ${fmtDuration(lifetime.max)}`
                      : 'no fully removed order'}
                  </dd>
                  <dt>Queue position at execution</dt>
                  <dd className="mono">
                    {queue && queue.n > 0
                      ? `n ${fmtInt(queue.n)} \u00b7 min ${fmtInt(queue.min)} \u00b7 p50 ${fmtInt(queue.p50)} \u00b7 p90 ${fmtInt(queue.p90)} \u00b7 max ${fmtInt(queue.max)}`
                      : 'no execution'}
                  </dd>
                </dl>
                <p className="note">
                  Lifetime covers fully removed orders only; orders still resting at the end are right-censored and left out of this
                  sample. Queue position is the 1-based FIFO position at the order{'\u2019'}s price. Exact definitions are listed below.
                </p>
              </Panel>
            </div>
          </>
        )}
        <Panel
          title={'L3 \u2192 L2 information loss'}
          basis="derived"
          caption="What the order-level stream shows that a price-level (L2) view of the same book cannot."
        >
          {comparison.error ? (
            <ErrorBox error={comparison.error} onRetry={comparison.reload} />
          ) : !c ? (
            <Spinner label="Loading the L3/L2 comparison" />
          ) : (
            <div className="col">
              <p className="note">
                {c.basis} Over {fmtInt(c.events)} events. The reference order of a cancellation is the middle resting order at the cancelled
                order{'\u2019'}s price just before it (zero-based position floor((n {'\u2212'} 1) / 2)); an L2 observer sees the level shrink but
                not where in the queue.
              </p>
              <DataTable label="L3 to L2 information loss" columns={LOSS_COLUMNS} rows={lossRows(c)} rowKey={(r) => r.measure} />
              {c.l2_unobservable.length > 0 ? (
                <div>
                  <p>
                    Not observable in L2 <BasisBadge basis="unobservable" />
                  </p>
                  <ul className="mk-list">
                    {c.l2_unobservable.map((x) => (
                      <li key={x}>{x}</li>
                    ))}
                  </ul>
                </div>
              ) : null}
              <Definitions defs={c.definitions} />
            </div>
          )}
        </Panel>
        {m ? (
          <Panel
            title={'L2 microstructure metrics (M1\u2013M9)'}
            basis="derived"
            flush={m.l2 !== null}
            caption="The existing bb-analytics market report over the Level-2 rows reconstructed from this source. M6 (Stoikov micro-price) needs an independent calibration and M10 (liquidity recovery) needs explicit shock windows; neither is inferred here."
          >
            {m.l2 === null ? (
              <p className="note">The server returned no L2 report for this dataset.</p>
            ) : (
              <DataTable label="L2 microstructure metrics" columns={L2_COLUMNS} rows={m.l2.metrics} rowKey={(r) => r.name} empty="No L2 metric summaries." />
            )}
          </Panel>
        ) : null}
        {m ? <Definitions defs={m.definitions} /> : null}
      </div>
    </div>
  );
}
