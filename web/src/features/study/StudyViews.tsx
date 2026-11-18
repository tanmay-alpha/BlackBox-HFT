// The study's focused views (B2: inventory impact, cancel delay, feed delay, seed variation) and
// its uncertainty breakdown (B4). A view draws only when `views.ts` validated its fields; otherwise
// it prints why it is omitted. Every value is simulated on synthetic data; nothing is annualised.

import type { ReactNode } from 'react';

import { ComparisonTags, IntervalChart, SeedStripChart, type Pick } from './charts';
import type { StudyResult } from './contract';
import { labelGroup } from './explore';
import { fmtPoint, latencyLabel, metricDef, type ChartRow } from './model';
import { ClassBadge, Section, SyntheticBadges } from './parts';
import { cancelDelayViews, feedDelayViews, inventoryView, seedStrips, spreads, type PairedDelta, type SpreadRow, type View } from './views';

function Omitted({ reason, testId }: { reason: string; testId: string }) {
  return (
    <p className="note stc-omitted" data-testid={testId} data-omitted="true">
      Omitted: {reason}.
    </p>
  );
}

function ViewOr<T>({ view, testId, children }: { view: View<T>; testId: string; children: (data: T) => ReactNode }) {
  return view.ok ? <>{children(view.data)}</> : <Omitted reason={view.reason} testId={`${testId}-omitted`} />;
}

export function InventorySection({ study, pick }: { study: StudyResult; pick: Pick }) {
  const view = inventoryView(study);
  const fmt = metricDef('mean_abs_position').fmt;
  return (
    <Section title="Inventory impact" id="stc-inventory" testId="study-inventory">
      <ViewOr view={view} testId="study-inventory">
        {(data) => {
          const rows: ChartRow[] = data.rows.map((r) => ({ key: r.key, group: r.group, label: r.label, point: r.mean, reference: r.reference }));
          return (
            <>
              <IntervalChart
                testId="study-inventory-chart"
                rows={rows}
                fmt={fmt}
                caption="Mean absolute inventory per group, baseline regime"
                labels={
                  <>
                    <ClassBadge dataClass="sensitivity" />
                    <SyntheticBadges />
                  </>
                }
                pick={pick}
                labelOf={(r) => labelGroup(study, r.group)}
              />
              <div className="table-scroll">
                <table className="table stc-stack" data-testid="study-inventory-max">
                  <caption className="note small">
                    Largest absolute inventory over each group&apos;s runs
                    {data.limit !== null ? `; the strategy's position limit is ${data.limit}` : ''}.
                  </caption>
                  <thead>
                    <tr>
                      <th scope="col">Group</th>
                      <th scope="col" className="num">
                        Mean |inventory|
                      </th>
                      <th scope="col" className="num">
                        Max |inventory|
                      </th>
                      <th scope="col" className="num">
                        Runs
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.rows.map((r) => (
                      <tr key={r.key}>
                        <th scope="row" className="wrap">
                          {r.label}
                        </th>
                        <td className="mono num wrap" data-label="Mean |inventory|">
                          {fmtPoint(r.mean, fmt)}
                        </td>
                        <td className="mono num" data-label="Max |inventory|">
                          {fmt(r.max)}
                        </td>
                        <td className="mono num" data-label="Runs">
                          {r.runs}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          );
        }}
      </ViewOr>
      <p className="note">
        Absolute position in base units, sampled over the simulated session. Inventory follows from simulated fills; it is not an
        observed position.
      </p>
    </Section>
  );
}

function DeltaTable({ d, pick, testId }: { d: PairedDelta; pick: Pick; testId: string }) {
  return (
    <div className="col stc-gap-1" data-testid={testId} data-cell={d.key}>
      <div className="row stc-delta-head">
        <h3 className="stc-h3">{d.title}</h3>
        <ComparisonTags label={d.label} />
      </div>
      <p className="note small">
        Changed: {d.change}. Reference: {d.label.against}.{' '}
        <button type="button" className="btn-link" aria-pressed={pick.selected === d.key} onClick={() => pick.onSelect(d.key)}>
          Open this cell
        </button>
        <span className="dim"> {'\u00b7'} </span>
        <button type="button" className="btn-link" aria-pressed={pick.selected === d.referenceKey} onClick={() => pick.onSelect(d.referenceKey)}>
          Open the reference
        </button>
      </p>
      <div className="table-scroll">
        <table className="table stc-stack">
          <thead>
            <tr>
              <th scope="col">Measure</th>
              <th scope="col" className="num">
                Reference
              </th>
              <th scope="col" className="num">
                Changed
              </th>
              <th scope="col" className="num">
                {'\u0394'}
              </th>
            </tr>
          </thead>
          <tbody>
            {d.rows.map((r) => (
              <tr key={r.metric}>
                <th scope="row" className="wrap">
                  {r.label}
                </th>
                <td className="mono num wrap" data-label="Reference">
                  {fmtPoint(r.reference, r.fmt)}
                </td>
                <td className="mono num wrap" data-label="Changed">
                  {fmtPoint(r.changed, r.fmt)}
                </td>
                <td className="mono num" data-label={'\u0394'}>
                  {r.delta >= 0 ? '+' : ''}
                  {r.fmt(r.delta)}
                </td>
              </tr>
            ))}
            <tr>
              <th scope="row">Cancelled orders (mean per run)</th>
              <td className="mono num" data-label="Reference">
                {d.canceled.reference.toLocaleString('en-US')}
              </td>
              <td className="mono num" data-label="Changed">
                {d.canceled.changed.toLocaleString('en-US')}
              </td>
              <td className="mono num" data-label={'\u0394'}>
                {d.canceled.changed - d.canceled.reference >= 0 ? '+' : ''}
                {(d.canceled.changed - d.canceled.reference).toLocaleString('en-US')}
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  );
}

export function CancelDelaySection({ study, pick }: { study: StudyResult; pick: Pick }) {
  return (
    <Section title="Cancellation delay" id="stc-cancel" testId="study-cancel">
      <ViewOr view={cancelDelayViews(study)} testId="study-cancel">
        {(list) => list.map((d) => <DeltaTable key={d.key} d={d} pick={pick} testId="study-cancel-delta" />)}
      </ViewOr>
      <p className="note">
        A slower cancel leaves resting orders in the modelled queue longer. Order entry and response stay at the baseline 1 ms; only
        the cancel-entry latency changes (its config representation changes with it, see the cell&apos;s controlled differences).
      </p>
    </Section>
  );
}

export function FeedDelaySection({ study, pick }: { study: StudyResult; pick: Pick }) {
  return (
    <Section title="Feed delay" id="stc-feed" testId="study-feed">
      <ViewOr view={feedDelayViews(study)} testId="study-feed">
        {(list) => list.map((d) => <DeltaTable key={d.key} d={d} pick={pick} testId="study-feed-delta" />)}
      </ViewOr>
      <p className="note">
        The strategy sees every market-data update later by the offset; orders, latency and queue model stay at the baseline. Synthetic
        feed timing, not a measured venue feed.
      </p>
    </Section>
  );
}

/** B4: what the study's uncertainty is and is not, with the seed strips and the sensitivity spreads. */
export function UncertaintySection({ study, metric, pick }: { study: StudyResult; metric: string; pick: Pick }) {
  const def = metricDef(metric);
  const u = study.uncertainty;
  const d = study.design;
  const strips = seedStrips(study, metric);
  const spread = spreads(study, metric);
  const param = spread.filter((s) => s.across === 'parameter');
  const data = spread.filter((s) => s.across === 'dataset');
  return (
    <Section title="Uncertainty" id="stc-uncertainty" testId="study-uncertainty">
      <p className="note">
        Six separate sources, here for {def.label}. Only the first is quantified as an interval. No figure is annualised and
        none is a profitability claim: every P&amp;L is a simulated session total on synthetic data.
      </p>

      <h3 className="stc-h3">1. Seed variability (the intervals shown)</h3>
      <p className="note" data-testid="uncertainty-seed">
        {u.level * 100}% {u.method}, {u.replicates} resamples, bootstrap seed {u.seed}, over latency seeds{' '}
        <span className="mono">{d.latency_seeds.join(', ')}</span>. Applies only to the stochastic latency models (
        {u.applies_to_latencies.map(latencyLabel).join('; ')}), conditional on one generated market per regime. With{' '}
        {d.latency_seeds.length} seeds the intervals are imprecise. Every seed value:
      </p>
      <ViewOr view={strips} testId="study-seed-strip">
        {(s) => (
          <>
            <SeedStripChart strips={s.strips} fmt={def.fmt} pick={pick} testId="study-seed-strip" />
            {s.omitted.length > 0 ? (
              <ul className="note small stc-caveats">
                {s.omitted.map((o) => (
                  <li key={o.label}>
                    Omitted {o.label}: {o.reason}.
                  </li>
                ))}
              </ul>
            ) : null}
          </>
        )}
      </ViewOr>

      <h3 className="stc-h3">2. Parameter sensitivity (model assumptions, paired)</h3>
      <SpreadTable rows={param} fmt={def.fmt} testId="uncertainty-parameter" />
      <p className="note">
        How far the value moves when one modelling assumption changes on the same dataset and schedule. A spread, not a confidence
        interval: it says how much the result depends on the assumption, not which assumption is right.
      </p>

      <h3 className="stc-h3">3. Dataset variation (cross-dataset)</h3>
      <SpreadTable rows={data} fmt={def.fmt} testId="uncertainty-dataset" />
      <p className="note">
        One generated session of {d.duration_ns / 1e9} s per regime, generator seed {d.generator_seed}. Regimes replay different events
        (cross-regime alignment: {d.cross_regime_alignment}); variation across generator seeds of the same regime is not estimated.
      </p>

      <h3 className="stc-h3">4. Sampling error</h3>
      <p className="note" data-testid="uncertainty-sampling">
        Not estimated here. Each regime is a single synthetic session, so there is no sample of market days to estimate it from.
      </p>

      <h3 className="stc-h3">5. Model uncertainty</h3>
      <p className="note" data-testid="uncertainty-model">
        Not addressed. The queue-position and latency models tested are a finite family ({study.queue_models.length} queue models,{' '}
        {study.latency_models.length} latency levels); their spread above is not a bound on the error of the model itself. Queue
        position is unobservable in L2 data and every fill is simulated.
      </p>

      <h3 className="stc-h3">6. Real-world calibration uncertainty</h3>
      <p className="note" data-testid="uncertainty-calibration">
        Not addressed: the data are synthetic and nothing is calibrated to a real venue, instrument or period. No value here transfers to
        live trading.
      </p>
    </Section>
  );
}

function SpreadTable({ rows, fmt, testId }: { rows: SpreadRow[]; fmt: (v: number) => string; testId: string }) {
  if (rows.length === 0) return <Omitted reason="fewer than two levels carry this measure" testId={`${testId}-omitted`} />;
  return (
    <div className="table-scroll">
      <table className="table stc-stack" data-testid={testId}>
        <thead>
          <tr>
            <th scope="col">What varies</th>
            <th scope="col" className="num">
              Levels
            </th>
            <th scope="col" className="num">
              Min
            </th>
            <th scope="col" className="num">
              Max
            </th>
            <th scope="col" className="num">
              Range
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.factor}>
              <th scope="row" className="wrap">
                {r.factor}
              </th>
              <td className="mono num" data-label="Levels">
                {r.levels}
              </td>
              <td className="mono num" data-label="Min">
                {fmt(r.min)}
              </td>
              <td className="mono num" data-label="Max">
                {fmt(r.max)}
              </td>
              <td className="mono num" data-label="Range">
                {fmt(r.range)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
