// Research home (`/research`): five investigations, each one compact row: the research question,
// the dataset with its provenance class, the method, one small figure and the numerical result
// computed now from its source, the exact link into the workspace holding the evidence, the
// limitation and the reproduction command. Sources: the verified study result (study feature's
// public loader) and the Market Data Workbench API; stored demo runs via `GET /demos` and
// `GET /runs/{id}`. A source that fails shows its exact state; no number is cached or invented.
// Identity and lineage rules: docs/engineering/RESEARCH_IDENTITY.md.

import { Fragment } from 'react';
import type { ReactNode } from 'react';
import { Link } from 'react-router-dom';

import { api, describeError, isAbort } from '../../api/client';
import type { Demo } from '../../api/model';
import { fmtInt, fmtNum, fmtPct, fmtPnl, fmtPp, fmtSigned } from '../../components/fmt';
import { Badge, HashText, Spinner, useAsync, type BadgeKind } from '../../components/ui';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { marketApi, type EventDetail, type MarketSummary } from '../market/api';
import { fmtPrice4 } from '../market/model';
import type { StudyResult } from '../study/contract';
import { candidateRunIds, listedDemoRuns, matchRun, studyOutDir, type ListedDemoRun, type StoredRunFacts } from '../study/links';
import { groupCellKey, loadStudyResult, runCellKey, type StudyFailureKind, type StudyLoadResult } from '../study/load';
import { latencyLabel, regimeLabel } from '../study/model';
import {
  CANCEL_AHEAD_EXAMPLE,
  SourceProblem,
  WORKBENCH_DATASET,
  cancelAhead,
  feeEffect,
  latencyEffect,
  regimeEffect,
  sensitivityRanking,
  workbenchCommand,
  type CancelAhead,
  type FeeEffect,
  type LatencyEffect,
  type RegimeEffect,
  type SensitivityRanking,
} from './investigations';
import './research.css';

const STUDY_ID = 'exec-validation';

// ----- evidence classes ------------------------------------------------------------------------

type EvidenceClass = 'synthetic_event' | 'reconstructed_state' | 'modelled_decision' | 'simulated_fill' | 'derived_metric' | 'unobservable';

const EVIDENCE: Record<EvidenceClass, { label: string; kind: BadgeKind; meaning: string }> = {
  synthetic_event: {
    label: 'Synthetic generated event',
    kind: 'neutral',
    meaning: 'A message written by a seeded generator. Not historical and not observed market data.',
  },
  reconstructed_state: {
    label: 'Reconstructed state',
    kind: 'derived',
    meaning: 'Book and FIFO queue state rebuilt by replaying the source messages in order.',
  },
  modelled_decision: {
    label: 'Modelled decision',
    kind: 'modeled',
    meaning: 'What the simulated strategy decided under the configured latency and queue model.',
  },
  simulated_fill: {
    label: 'Simulated fill',
    kind: 'modeled',
    meaning: 'A fill produced by the execution model against replayed data, never a recorded execution.',
  },
  derived_metric: {
    label: 'Derived metric',
    kind: 'derived',
    meaning: 'Arithmetic over the values above (differences, rates, rankings), computed as stated.',
  },
  unobservable: {
    label: 'Unobservable counterfactual',
    kind: 'unobservable',
    meaning: 'What would have happened under other conditions in a real market; no data here observes it.',
  },
};

function Ev({ c }: { c: EvidenceClass }) {
  return (
    <Badge kind={EVIDENCE[c].kind} title={EVIDENCE[c].meaning}>
      {EVIDENCE[c].label}
    </Badge>
  );
}

/** Provenance class of a dataset from its declared `data_class`. */
function Provenance({ dataClass }: { dataClass: string }) {
  return dataClass === 'synthetic' ? (
    <Badge kind="neutral" title={EVIDENCE.synthetic_event.meaning}>
      Synthetic generated events
    </Badge>
  ) : (
    <Badge kind="warn" title="Declared data class of this dataset">
      Data class {dataClass}
    </Badge>
  );
}

// ----- sources ---------------------------------------------------------------------------------

interface Workbench {
  summary: MarketSummary;
  detail: EventDetail;
}

/** The workbench demo's summary, its curated cancel-ahead example and that event's detail. */
async function loadWorkbench(signal: AbortSignal): Promise<Workbench> {
  const [summary, examples] = await Promise.all([marketApi.dataset(WORKBENCH_DATASET, { signal }), marketApi.examples(WORKBENCH_DATASET, { signal })]);
  const example = examples.items.find((x) => x.id === CANCEL_AHEAD_EXAMPLE);
  if (!example) throw new SourceProblem(`The workbench lists no "${CANCEL_AHEAD_EXAMPLE}" example for ${WORKBENCH_DATASET}.`);
  const detail = await marketApi.eventDetail(WORKBENCH_DATASET, example.event_index, { orders_per_level: 100 }, { signal });
  return { summary, detail };
}

interface Stored {
  demos: Demo[];
  listed: ListedDemoRun[];
  facts: Map<string, StoredRunFacts>;
}

/** The public demo list and the stored facts of every study anchor run a demo lists. */
async function loadStored(study: StudyResult, signal: AbortSignal): Promise<Stored> {
  const demos = await api.demos(undefined, { signal });
  const listed = listedDemoRuns(demos);
  const ids = candidateRunIds(study.demo_anchors.runs, listed);
  const settled = await Promise.allSettled(ids.map((id) => api.run(id, { signal })));
  const facts = new Map<string, StoredRunFacts>();
  for (const s of settled) if (s.status === 'fulfilled') facts.set(s.value.run_id, s.value);
  return { demos, listed, facts };
}

type View<T> = { state: 'loading' } | { state: 'failed'; kind: string; head: string; message: string } | { state: 'ready'; value: T };

function failedFrom(e: unknown, source: string): View<never> {
  if (e instanceof SourceProblem) return { state: 'failed', kind: 'inconsistent', head: `${source}: unexpected content`, message: e.message };
  const v = describeError(e);
  return { state: 'failed', kind: v.state, head: `${source}: ${v.head}`, message: v.message };
}

/** Runs a derivation over a source that answered; a `SourceProblem` becomes the failure shown. */
function attempt<T>(source: string, derive: () => T): View<T> {
  try {
    return { state: 'ready', value: derive() };
  } catch (e) {
    return failedFrom(e, source);
  }
}

const STUDY_FAILURE: Record<StudyFailureKind, string> = {
  unavailable: 'unavailable',
  hash_mismatch: 'content hash mismatch',
  unsupported_contract: 'unsupported contract version',
  invalid: 'does not match its contract',
};

/** Applies `derive` to a verified study, or carries the study's own failure. */
function fromStudy<T>(loaded: StudyLoadResult | null, error: Error | null, derive: (s: StudyResult) => T): View<T> {
  if (error && !isAbort(error)) return failedFrom(error, `Study result ${STUDY_ID}`);
  if (!loaded) return { state: 'loading' };
  if (!loaded.ok) {
    return { state: 'failed', kind: loaded.kind, head: `Study result ${STUDY_ID}: ${STUDY_FAILURE[loaded.kind]}; nothing from it is shown`, message: loaded.message };
  }
  return attempt(`Study result ${STUDY_ID}`, () => derive(loaded.study));
}

// ----- page ------------------------------------------------------------------------------------

export default function ResearchPage() {
  const study = useAsync((signal) => loadStudyResult(STUDY_ID, signal), []);
  const verified = study.data?.ok ? study.data.study : null;
  const stored = useAsync((signal) => (verified ? loadStored(verified, signal) : Promise.resolve(null)), [verified]);
  const workbench = useAsync(loadWorkbench, []);

  const wbSource = `Workbench ${WORKBENCH_DATASET}`;
  const wbData = workbench.data;
  const wb: View<{ summary: MarketSummary; result: CancelAhead }> = workbench.error
    ? failedFrom(workbench.error, wbSource)
    : wbData
      ? attempt(wbSource, () => ({ summary: wbData.summary, result: cancelAhead(wbData.detail) }))
      : { state: 'loading' };
  const storedView: StoredView = stored.error
    ? { state: 'failed', message: describeError(stored.error).message }
    : stored.data
      ? { state: 'ready', value: stored.data }
      : { state: 'loading' };

  return (
    <Workspace context={null} scroll ownHeading>
      <div className="pad-3 col rh" data-testid="research-home">
        <header className="col rh-head">
          <h1 className="rh-title">Research home</h1>
          <p className="note rh-lede">
            Five questions, each answered from its source when this page loads: market data, L3 reconstruction, the L2 view, execution assumptions,
            simulation and sensitivity. Every row names its dataset and how the number was computed, links to the exact view holding the evidence, and
            says what it cannot show. All data here is synthetic; no value is a forecast or a recorded execution.
          </p>
          <details className="rh-classes">
            <summary>
              <span className="rh-label">Evidence classes</span>
              {(Object.keys(EVIDENCE) as EvidenceClass[]).map((c) => (
                <Ev key={c} c={c} />
              ))}
            </summary>
            <dl className="rh-facts">
              {(Object.keys(EVIDENCE) as EvidenceClass[]).map((c) => (
                <Fragment key={c}>
                  <dt>{EVIDENCE[c].label}</dt>
                  <dd>{EVIDENCE[c].meaning}</dd>
                </Fragment>
              ))}
            </dl>
          </details>
        </header>
        <ol className="rh-list">
          <QueueInvestigation view={wb} />
          <FeeInvestigation view={fromStudy(study.data, study.error, feeEffect)} study={verified} stored={storedView} />
          <LatencyInvestigation view={fromStudy(study.data, study.error, latencyEffect)} study={verified} stored={storedView} />
          <RegimeInvestigation view={fromStudy(study.data, study.error, regimeEffect)} study={verified} stored={storedView} />
          <RankingInvestigation view={fromStudy(study.data, study.error, (s) => sensitivityRanking(s, 'net_pnl'))} study={verified} />
        </ol>
      </div>
    </Workspace>
  );
}

type StoredView = { state: 'loading' } | { state: 'failed'; message: string } | { state: 'ready'; value: Stored };

// ----- row frame -------------------------------------------------------------------------------

interface RowProps<T> {
  id: string;
  question: string;
  dataset: ReactNode;
  method: ReactNode;
  view: View<T>;
  figure: (v: T) => ReactNode;
  result: (v: T) => ReactNode;
  evidence: (v: T) => ReactNode;
  limitation: ReactNode;
  reproduce: string | null;
  /** Raw values for checks, as `data-*` attributes of the row. */
  data?: (v: T) => Record<string, string | number>;
}

function Investigation<T>({ id, question, dataset, method, view, figure, result, evidence, limitation, reproduce, data }: RowProps<T>) {
  const attrs = view.state === 'ready' && data ? Object.fromEntries(Object.entries(data(view.value)).map(([k, v]) => [`data-${k}`, String(v)])) : {};
  return (
    <li className="rh-row" id={id} data-testid={`rh-${id}`} data-state={view.state} aria-labelledby={`${id}-q`} {...attrs}>
      <div className="rh-ask col">
        <h2 className="rh-q" id={`${id}-q`}>
          {question}
        </h2>
        <dl className="rh-facts">
          <dt>Dataset</dt>
          <dd>{dataset}</dd>
          <dt>Method</dt>
          <dd>{method}</dd>
        </dl>
      </div>
      <div className="rh-answer col">
        {view.state === 'loading' ? <Spinner label="Reading the source" /> : null}
        {view.state === 'failed' ? (
          <div className="error-box" role="alert" data-testid="rh-source-error" data-kind={view.kind}>
            <div className="error-box-head">{view.head}</div>
            <div className="error-box-message">{view.message}</div>
          </div>
        ) : null}
        {view.state === 'ready' ? (
          <>
            <figure className="rh-figure">{figure(view.value)}</figure>
            <div className="rh-result" data-testid="rh-result">
              {result(view.value)}
            </div>
          </>
        ) : null}
      </div>
      <div className="rh-trail col">
        <div className="rh-evidence" data-testid="rh-evidence">
          <span className="rh-label">Evidence</span>
          {view.state === 'ready' ? evidence(view.value) : <span className="note">Shown once the source is read.</span>}
        </div>
        <p className="note rh-limit">
          <span className="rh-label">Limitation</span> {limitation}
        </p>
        {reproduce ? (
          <details className="rh-repro">
            <summary>Reproduce</summary>
            <pre className="mono small rh-pre" data-testid="rh-reproduce">
              {reproduce}
            </pre>
          </details>
        ) : null}
      </div>
    </li>
  );
}

function StudyDataset({ study, regimes }: { study: StudyResult | null; regimes?: boolean }) {
  if (!study) return <span className="note">Execution study {STUDY_ID} (read when verified)</span>;
  const base = study.datasets.find((d) => d.id === study.baseline.dataset.id);
  return (
    <span className="rh-ds">
      <Provenance dataClass={study.data_class} />{' '}
      {regimes ? (
        <>
          <span className="mono">{study.datasets.length}</span> study datasets, one generated per regime ({study.datasets.map((d) => regimeLabel(d.regime)).join(', ')})
        </>
      ) : (
        <>
          <span className="mono">{study.baseline.dataset.id}</span> ({regimeLabel(study.baseline.regime)}) events SHA-256 <HashText hash={study.baseline.dataset.sha256} />
        </>
      )}
      {base?.generator?.duration_ns ? <>, {base.generator.duration_ns / 1e9} s session</> : null}; study {STUDY_ID}, contract {study.schema} v{study.schema_version}
    </span>
  );
}

function studyRepro(study: StudyResult | null, lines: string[]): string | null {
  if (!study) return null;
  const out = studyOutDir(study);
  return [study.generator.command.replace('<dir>', out), ...lines.map((l) => l.replace('<out>', out))].join('\n');
}

/** A stored demo that answers a related question on its own dataset: never "the same data". */
function RelatedDemo({ stored, demoId }: { stored: StoredView; demoId: string }) {
  if (stored.state === 'loading') return null;
  if (stored.state === 'failed') return <span className="note">Stored demos not checked: {stored.message}</span>;
  const demo = stored.value?.demos.find((d) => d.id === demoId);
  if (!demo) return <span className="note">The stored demo {demoId} is not published by this API.</span>;
  const runs = demo.runs.map((r) => r.run_id).filter((x): x is string => typeof x === 'string' && x !== '');
  if (runs.length !== demo.runs.length) return <span className="note">The demo {demo.title} has no stored runs yet.</span>;
  const datasets = [...new Set(demo.runs.map((r) => r.config.dataset_id))].join(', ');
  return (
    <span>
      Related question, different data: <Link to={paths.compareSet({ runs, demo: demo.id })}>{demo.title}</Link>{' '}
      <span className="note">
        (stored demo on <span className="mono">{datasets}</span>; not the study's dataset)
      </span>
    </span>
  );
}

const pctBar = (v: number, max: number) => `${max > 0 ? Math.max(0, Math.min(100, (v / max) * 100)) : 0}%`;

// ----- (a) L2 vs L3 ----------------------------------------------------------------------------

function QueueInvestigation({ view }: { view: View<{ summary: MarketSummary; result: CancelAhead }> }) {
  const summary = view.state === 'ready' ? view.value.summary : null;
  return (
    <Investigation
      id="l2-queue"
      question="Why does L2 hide queue information?"
      dataset={
        summary ? (
          <span className="rh-ds">
            <Provenance dataClass={summary.data_class} /> <span className="mono">{summary.dataset_id}</span>, {summary.symbol} on {summary.venue},{' '}
            {fmtInt(summary.events)} source frames, source SHA-256 <HashText hash={summary.dataset_version} />
          </span>
        ) : (
          <span className="note">Market Data Workbench dataset {WORKBENCH_DATASET} (read from the API)</span>
        )
      }
      method={
        <>
          The workbench's curated <span className="mono">{CANCEL_AHEAD_EXAMPLE}</span> event (first accepted delete inside a touch queue). From
          its event detail: the level total before and after (what L2 shows) against each listed order's queue position and shares ahead (what
          L3 shows), matched by order reference. <Ev c="synthetic_event" /> <Ev c="reconstructed_state" /> <Ev c="derived_metric" />
        </>
      }
      view={view}
      data={({ result: r }) => ({
        'event-index': r.eventIndex,
        'l2-delta': r.l2.delta,
        'deleted-shares': r.deletedShares,
        position: r.position,
        'level-orders': r.levelOrders,
        'next-ahead-before': r.nextBehind?.before.shares_ahead ?? '',
        'next-ahead-after': r.nextBehind?.after.shares_ahead ?? '',
      })}
      figure={({ result: r }) => <CancelFigure r={r} />}
      result={({ result: r }) => (
        <>
          <p>
            <strong>L2</strong>: {r.side === 'buy' ? 'bid' : 'ask'} {fmtPrice4(r.price4)} went from <span className="num">{fmtInt(r.l2.before)}</span> to{' '}
            <span className="num">{fmtInt(r.l2.after)}</span> shares (<span className="num" data-testid="rh-l2-delta">{fmtSigned(r.l2.delta, 0)}</span>), one
            order fewer. Nothing says where in the queue it left.
          </p>
          <p>
            <strong>L3</strong>: order <span className="mono">{r.orderRef}</span> was position <span className="num">{r.position}</span> of{' '}
            <span className="num">{r.levelOrders}</span>. The <span className="num">{r.ordersAhead}</span> orders ahead kept their place
            {r.aheadUnchanged ? '' : ' (not confirmed for every listed order)'}; the <span className="num">{r.ordersBehind}</span> behind moved up one
            position and <span className="num">{fmtInt(r.deletedShares)}</span> shares
            {r.behindConsistent ? '' : ' (not every listed order matches; see the event)'}.
            {r.nextBehind ? (
              <>
                {' '}
                Next in line, <span className="mono">{r.nextBehind.order_ref}</span>: shares ahead{' '}
                <span className="num" data-testid="rh-ahead-change">
                  {fmtInt(r.nextBehind.before.shares_ahead)} → {fmtInt(r.nextBehind.after.shares_ahead)}
                </span>
                .
              </>
            ) : null}
          </p>
        </>
      )}
      evidence={({ result: r }) => (
        <Link to={paths.marketDataset(WORKBENCH_DATASET, { tab: 'book', event: r.eventIndex, example: CANCEL_AHEAD_EXAMPLE })} data-testid="rh-link-primary">
          Event #{fmtInt(r.eventIndex)} in the workbench, before/after book
        </Link>
      )}
      limitation={
        <>
          Synthetic messages from a seeded generator, not exchange data. The queue is reconstructed from full order-level messages; with real L2
          data a queue position can only be estimated by a model. One curated event, not a frequency.
        </>
      }
      reproduce={summary ? workbenchCommand(summary.generator, summary.dataset_version) : null}
    />
  );
}

function CancelFigure({ r }: { r: CancelAhead }) {
  const ahead = r.sharesAheadOfDeleted;
  const behindBefore = r.l2.before - ahead - r.deletedShares;
  const max = r.l2.before;
  const seg = (cls: string, v: number, label: string) =>
    v > 0 ? <span className={`rh-seg ${cls}`} style={{ width: pctBar(v, max) }} title={`${label}: ${fmtInt(v)} shares`} /> : null;
  return (
    <>
      <div className="rh-bars" role="img" aria-label={`Level ${fmtInt(r.l2.before)} shares before, ${fmtInt(r.l2.after)} after; ${fmtInt(ahead)} shares ahead of the deleted order unchanged`}>
        <span className="rh-bar-label">Before</span>
        <span className="rh-track">
          {seg('rh-seg-ahead', ahead, 'Ahead of the deleted order')}
          {seg('rh-seg-gone', r.deletedShares, 'Deleted order')}
          {seg('rh-seg-behind', behindBefore, 'Behind the deleted order')}
        </span>
        <span className="num">{fmtInt(r.l2.before)}</span>
        <span className="rh-bar-label">After</span>
        <span className="rh-track">
          {seg('rh-seg-ahead', ahead, 'Ahead (unchanged)')}
          {seg('rh-seg-behind', r.l2.after - ahead, 'Moved up')}
        </span>
        <span className="num">{fmtInt(r.l2.after)}</span>
      </div>
      <figcaption className="note small">
        Level shares on one scale. <span className="rh-key rh-seg-ahead" /> ahead <span className="rh-key rh-seg-gone" /> deleted{' '}
        <span className="rh-key rh-seg-behind" /> behind. L2 sees only the bar length.
      </figcaption>
    </>
  );
}

// ----- (b) fees ----------------------------------------------------------------------------------

function FeeInvestigation({ view, study, stored }: { view: View<FeeEffect>; study: StudyResult | null; stored: StoredView }) {
  return (
    <Investigation
      id="fees"
      question="How do fees affect market-making P&L?"
      dataset={<StudyDataset study={study} />}
      method={
        <>
          The study's baseline run (<span className="mono">maker_baseline</span>, PowerProb3 queue, constant 1 ms latency): its accounting of gross
          P&L (realized + unrealized), fees and net, checked against gross − fees = net, and the same split for every regime's baseline run.{' '}
          <Ev c="simulated_fill" /> <Ev c="derived_metric" />
        </>
      }
      view={view}
      data={(v) => ({ gross: v.baseline.gross, fees: v.baseline.fees, net: v.baseline.net, 'run-id': v.baseline.runId })}
      figure={(v) => <FeeFigure v={v} />}
      result={(v) => (
        <p>
          Gross <span className="num">{fmtPnl(v.baseline.gross)}</span>, fees paid <span className="num">{fmtNum(v.baseline.fees, 4)}</span>, net{' '}
          <span className="num" data-testid="rh-net">
            {fmtPnl(v.baseline.net)}
          </span>{' '}
          (quote currency, one session, {fmtInt(v.baseline.fills)} simulated fills
          {v.feeRates ? `, maker ${bp(v.feeRates.maker)} / taker ${bp(v.feeRates.taker)}` : ''}). Fees exceed the absolute gross P&L in{' '}
          <span className="num">{v.feesExceedGross}</span> of <span className="num">{v.regimes.length}</span> regime baselines.
        </p>
      )}
      evidence={(v) => (
        <>
          <Link to={paths.study(STUDY_ID, { metric: 'fees', cell: runCellKey(v.baseline.runId) })} data-testid="rh-link-primary">
            Baseline run {v.baseline.runId} in the study
          </Link>
          <RelatedDemo stored={stored} demoId="gross_vs_net" />
        </>
      )}
      limitation={
        <>
          Gross is the same simulated fills before fees, not a separately simulated zero-fee run. Fills are modelled on synthetic data; a session
          total, not a return or a forecast.
        </>
      }
      reproduce={studyRepro(study, [`# baseline run: <out>/cells/${view.state === 'ready' ? view.value.baseline.runId : '<run>'}.json (accounting)`])}
    />
  );
}

const bp = (fraction: number) => `${+(fraction * 1e4).toFixed(2)} bp`;

function FeeFigure({ v }: { v: FeeEffect }) {
  const items = [
    { label: 'Gross', value: v.baseline.gross },
    { label: 'Fees deducted', value: -v.baseline.fees },
    { label: 'Net', value: v.baseline.net },
  ];
  const max = Math.max(...items.map((i) => Math.abs(i.value)));
  return (
    <>
      <div className="rh-signed" role="img" aria-label={items.map((i) => `${i.label} ${fmtPnl(i.value)}`).join(', ')}>
        {items.map((i) => (
          <SignedBar key={i.label} label={i.label} value={i.value} max={max} text={fmtPnl(i.value)} />
        ))}
      </div>
      <figcaption className="note small">Quote currency on one scale around zero; baseline run {v.baseline.runId}.</figcaption>
    </>
  );
}

function SignedBar({ label, value, max, text }: { label: string; value: number; max: number; text: string }) {
  const w = max > 0 ? (Math.abs(value) / max) * 50 : 0;
  return (
    <>
      <span className="rh-bar-label" title={label}>
        {label}
      </span>
      <span className="rh-axis">
        <span className={value < 0 ? 'rh-sbar rh-sbar-neg' : 'rh-sbar rh-sbar-pos'} style={value < 0 ? { right: '50%', width: `${w}%` } : { left: '50%', width: `${w}%` }} />
      </span>
      <span className="num">{text}</span>
    </>
  );
}

// ----- (c) latency -------------------------------------------------------------------------------

function LatencyInvestigation({ view, study, stored }: { view: View<LatencyEffect>; study: StudyResult | null; stored: StoredView }) {
  return (
    <Investigation
      id="latency"
      question="What changes when orders arrive late?"
      dataset={<StudyDataset study={study} />}
      method={
        <>
          Each order-latency level of the study, and a feed delay, with every other assumption at the baseline: fill rate (filled / submitted
          orders) and net P&L against constant 1 ms. Stochastic levels are means over latency seeds. <Ev c="modelled_decision" />{' '}
          <Ev c="simulated_fill" /> <Ev c="derived_metric" />
        </>
      }
      view={view}
      data={(v) => ({
        'ref-fill-rate': v.reference.fillRate.value,
        'slow-fill-rate': v.slowEntry?.fillRate.value ?? '',
        'slow-net': v.slowEntry?.net.value ?? '',
        'feed-fill-rate': v.feedDelay?.fillRate.value ?? '',
      })}
      figure={(v) => <LatencyFigure v={v} />}
      result={(v) => (
        <p>
          {v.slowEntry ? (
            <>
              Entry latency 10 ms: fill rate <span className="num">{fmtPct(v.reference.fillRate.value)}</span> →{' '}
              <span className="num" data-testid="rh-slow-fill">
                {fmtPct(v.slowEntry.fillRate.value)}
              </span>{' '}
              (<span className="num">{fmtPp(v.slowEntry.fillRate.value - v.reference.fillRate.value)}</span>), net P&L{' '}
              <span className="num">{fmtPnl(v.reference.net.value)}</span> → <span className="num">{fmtPnl(v.slowEntry.net.value)}</span>, fees{' '}
              <span className="num">{fmtNum(v.reference.fees.value, 4)}</span> → <span className="num">{fmtNum(v.slowEntry.fees.value, 4)}</span> paid.{' '}
              {v.slowEntry.net.value > v.reference.net.value && v.slowEntry.fees.value < v.reference.fees.value
                ? 'Net P&L rose while fills fell: fewer fills paid fewer fees. '
                : null}
            </>
          ) : null}
          {v.feedDelay ? (
            <>
              {v.feedDelay.label}: fill rate <span className="num">{fmtPct(v.feedDelay.fillRate.value)}</span> (
              <span className="num">{fmtPp(v.feedDelay.fillRate.value - v.reference.fillRate.value)}</span>).{' '}
            </>
          ) : null}
          {v.rows.some((r) => r.fillRate.interval) ? 'Stochastic levels are seed means with 95% intervals in the figure.' : null}
        </p>
      )}
      evidence={(v) => (
        <>
          <Link
            to={paths.study(STUDY_ID, { metric: 'fill_rate', cell: groupCellKey(v.slowEntry?.group ?? v.rows[v.rows.length - 1].group) })}
            data-testid="rh-link-primary"
          >
            {v.slowEntry ? latencyLabel(v.slowEntry.group.latency) : v.rows[v.rows.length - 1].label} in the study
          </Link>
          <RelatedDemo stored={stored} demoId="cost_of_latency" />
        </>
      )}
      limitation={
        <>
          Latency shapes are assumptions, not measured venue latencies. Each level replays one 30 s synthetic market; seed intervals cover only latency
          draws. What a real order would have received is an unobservable counterfactual.
        </>
      }
      reproduce={studyRepro(study, ['# latency levels: aggregates with regime normal_liquidity, queue power_prob3 in <out>/public/study-result.json'])}
    />
  );
}

function LatencyFigure({ v }: { v: LatencyEffect }) {
  const max = Math.max(...v.rows.map((r) => r.fillRate.value));
  return (
    <>
      <div className="rh-dots" role="img" aria-label={v.rows.map((r) => `${r.label} ${fmtPct(r.fillRate.value)}`).join(', ')}>
        {v.rows.map((r) => (
          <DotRow key={`${r.group.latency}|${r.group.feed_offset_ns}`} label={r.label} value={r.fillRate.value} max={max} text={fmtPct(r.fillRate.value)} reference={r.reference} interval={r.fillRate.interval} />
        ))}
      </div>
      <figcaption className="note small">
        Fill rate, 0 to {fmtPct(max)}; the square and bold label mark the reference; bars mark 95% seed intervals where the level is stochastic.
      </figcaption>
    </>
  );
}

function DotRow({ label, value, max, text, reference, interval }: { label: string; value: number; max: number; text: string; reference: boolean; interval?: [number, number] | null }) {
  return (
    <>
      <span className={reference ? 'rh-bar-label rh-strong' : 'rh-bar-label'} title={label}>
        {label}
      </span>
      <span className="rh-axis rh-axis-plain">
        {interval ? <span className="rh-ci" style={{ left: pctBar(interval[0], max), width: `calc(${pctBar(interval[1], max)} - ${pctBar(interval[0], max)})` }} /> : null}
        <span className={reference ? 'rh-dot rh-dot-ref' : 'rh-dot'} style={{ left: pctBar(value, max) }} />
      </span>
      <span className="num">{text}</span>
    </>
  );
}

// ----- (d) liquidity shock ----------------------------------------------------------------------

function RegimeInvestigation({ view, study, stored }: { view: View<RegimeEffect>; study: StudyResult | null; stored: StoredView }) {
  return (
    <Investigation
      id="regime"
      question="How does a liquidity shock affect fills?"
      dataset={<StudyDataset study={study} regimes />}
      method={
        <>
          Each regime's baseline run (same strategy and execution assumptions) on its own generated market: fill rate and net P&L, with liquidity
          depletion and widening spreads against normal liquidity. Regimes replay different events, so this is a metrics-only comparison, not
          like-for-like. <Ev c="synthetic_event" /> <Ev c="simulated_fill" /> <Ev c="derived_metric" />
        </>
      }
      view={view}
      data={(v) => {
        const at = (regime: string) => v.rows.find((r) => r.regime === regime)?.fillRate ?? '';
        return { 'ref-fill-rate': v.reference.fillRate, 'depletion-fill-rate': at('liquidity_depletion'), 'wide-fill-rate': at('widening_spreads') };
      }}
      figure={(v) => <RegimeFigure v={v} />}
      result={(v) => (
        <p>
          {v.rows
            .filter((r) => r.shock)
            .map((r) => (
              <span key={r.regime} data-testid={`rh-regime-${r.regime}`}>
                {r.label}: fill rate <span className="num">{fmtPct(r.fillRate)}</span> (<span className="num">{fmtPp(r.fillRate - v.reference.fillRate)}</span>{' '}
                vs {fmtPct(v.reference.fillRate)}), net <span className="num">{fmtPnl(r.net)}</span>.{' '}
              </span>
            ))}
          Normal liquidity net <span className="num">{fmtPnl(v.reference.net)}</span>.
        </p>
      )}
      evidence={(v) => {
        const depletion = v.rows.find((r) => r.regime === 'liquidity_depletion') ?? v.rows.find((r) => r.shock);
        return (
          <>
            {depletion ? (
              <Link to={paths.study(STUDY_ID, { metric: 'fill_rate', cell: runCellKey(depletion.runId) })} data-testid="rh-link-primary">
                {depletion.label} baseline run in the study
              </Link>
            ) : null}
            <AnchorLinks v={v} stored={stored} />
          </>
        );
      }}
      limitation={
        <>
          The regimes are generator interventions, not historical crash reconstructions. Different events per regime: differences are not
          attributable to the regime alone. No market impact of the strategy's own orders.
        </>
      }
      reproduce={studyRepro(study, ['# regime baselines: cells with design regime_baseline in <out>/public/study-result.json'])}
    />
  );
}

/** Stored demo comparisons of a normal and a shocked anchor run, linked only when both match by config and dataset hash. */
function AnchorLinks({ v, stored }: { v: RegimeEffect; stored: StoredView }) {
  if (stored.state === 'loading') return null;
  if (stored.state === 'failed') return <span className="note">Stored runs not checked: {stored.message}</span>;
  const s = stored.value;
  if (!s) return null;
  return (
    <>
      {v.anchorPairs.map((p) => {
        const a = matchRun(p.reference, s.listed, s.facts);
        const b = matchRun(p.shocked, s.listed, s.facts);
        if (a.kind === 'stored' && b.kind === 'stored') {
          return (
            <span key={p.shocked.run_id}>
              Stored demo pair: <Link to={paths.comparePair(a.runId, b.runId)}>{regimeLabel(p.regime)} vs normal, comparison</Link>{' '}
              <span className="note">
                (<span className="mono">{p.reference.dataset.id}</span> vs <span className="mono">{p.shocked.dataset.id}</span>, the demo's own datasets)
              </span>
            </span>
          );
        }
        const why = a.kind === 'not_stored' ? a.reason : b.kind === 'not_stored' ? b.reason : '';
        return (
          <span key={p.shocked.run_id} className="note">
            {regimeLabel(p.regime)} demo pair not linked: {why === 'dataset_mismatch' ? 'stored dataset hash differs' : why === 'unchecked' ? 'stored run not readable' : 'no stored run with this config hash'}.
          </span>
        );
      })}
    </>
  );
}

function RegimeFigure({ v }: { v: RegimeEffect }) {
  const max = Math.max(...v.rows.map((r) => r.fillRate));
  return (
    <>
      <div className="rh-dots" role="img" aria-label={v.rows.map((r) => `${r.label} ${fmtPct(r.fillRate)}`).join(', ')}>
        {v.rows.map((r) => (
          <DotRow key={r.regime} label={r.label} value={r.fillRate} max={max} text={fmtPct(r.fillRate)} reference={r.reference} />
        ))}
      </div>
      <figcaption className="note small">
        Fill rate of each regime's baseline run, 0 to {fmtPct(max)}; the square marks the reference ({v.reference.label}).
      </figcaption>
    </>
  );
}

// ----- (e) sensitivity ranking ------------------------------------------------------------------

const RANK_SHOWN = 5;

function RankingInvestigation({ view, study }: { view: View<SensitivityRanking>; study: StudyResult | null }) {
  return (
    <Investigation
      id="ranking"
      question="Which execution assumptions change the result most?"
      dataset={<StudyDataset study={study} />}
      method={
        <>
          Computed here: every one-factor group of the study on the baseline regime (one assumption changed: queue model, latency level or feed
          delay) minus the baseline group in net P&L, ranked by absolute change; stochastic levels use the seed mean. Regimes (other events) and
          factorial groups (two changes) are not ranked. <Ev c="derived_metric" />
        </>
      }
      view={view}
      data={(v) => ({ 'top-delta': v.ranked[0].delta, 'top-cell': groupCellKey(v.ranked[0].group), ranked: v.ranked.length, baseline: v.baseline.value })}
      figure={(v) => <RankFigure v={v} />}
      result={(v) => (
        <p>
          Largest change: <strong>{v.ranked[0].label}</strong>, net P&L{' '}
          <span className="num" data-testid="rh-top-delta">
            {fmtPnl(v.ranked[0].delta)}
          </span>{' '}
          against <span className="num">{fmtPnl(v.baseline.value)}</span>
          {v.ranked[0].fillRateDelta !== null ? (
            <>
              {' '}
              (fill rate <span className="num">{fmtPp(v.ranked[0].fillRateDelta)}</span>)
            </>
          ) : null}
          . <span className="num">{v.ranked.length}</span> assumptions ranked
          {v.unchanged.length ? (
            <>
              ; <span className="num">{v.unchanged.length}</span> leave net P&L unchanged to rounding ({v.unchanged.join('; ')})
            </>
          ) : null}
          {v.excluded.length ? `; left out without a value: ${v.excluded.join(', ')}` : ''}.
        </p>
      )}
      evidence={(v) => (
        <Link to={paths.study(STUDY_ID, { metric: 'net_pnl', cell: groupCellKey(v.ranked[0].group) })} data-testid="rh-link-primary">
          {v.ranked[0].label} in the study
        </Link>
      )}
      limitation={
        <>
          A ranking of model sensitivity on one synthetic market, not of which assumption is true. Changes are against one baseline; interactions
          appear only in the factorial cells.
        </>
      }
      reproduce={studyRepro(study, ['# one-factor groups: aggregates with design one_factor in <out>/public/study-result.json'])}
    />
  );
}

function RankFigure({ v }: { v: SensitivityRanking }) {
  const shown = v.ranked.slice(0, RANK_SHOWN);
  const max = Math.max(...shown.map((r) => Math.abs(r.delta)));
  return (
    <>
      <ol className="rh-rank" aria-label="Assumptions ranked by absolute change in net P&L">
        {shown.map((r, i) => (
          <li key={groupCellKey(r.group)} className="rh-rank-row" data-testid="rh-rank-row">
            <span className="num rh-rank-n">{i + 1}</span>
            <SignedBar label={r.label} value={r.delta} max={max} text={fmtPnl(r.delta)} />
          </li>
        ))}
      </ol>
      <figcaption className="note small">
        Change in net P&L against the baseline ({fmtPnl(v.baseline.value)}), top {shown.length} of {v.ranked.length}, on one scale around zero.
      </figcaption>
    </>
  );
}
