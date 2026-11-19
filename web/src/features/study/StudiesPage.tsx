// Execution study viewer (`/studies`, `/studies/:id`; docs/research/EXECUTION_STUDY_VIEWER.md).
// Reads a published, read-only study result (`/study-results/<id>/study-result.json`), checks its
// SHA-256 against the published hash file and its contract version before showing anything, and
// links a study run to a stored public demo run only when config and dataset hashes both match.
// Any matrix cell, chart row or run opens its detail (`?cell=`, Back/Forward restore it); on a
// phone the page is staged as Overview / Matrix / Cell / Uncertainty tabs (`?tab=`).
// Everything here is synthetic and modelled; no value is annualised.

import { Fragment, useEffect, useMemo, useRef } from 'react';
import type { ReactNode } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';

import { api, DEMO_WORKSPACE, describeError, isAbort, retryBusy } from '../../api/client';
import { usePhone } from '../../components/PaneTabs';
import { Tabs } from '../../components/Tabs';
import { Badge, Empty, HashText, Notice, Spinner, useAsync } from '../../components/ui';
import { useAnnounce } from '../../shell/announce';
import { Workspace } from '../../shell/Workspace';
import { CellPanel, selectionTitle } from './CellPanel';
import { ComparisonTags, DecompositionChart, IntervalChart, type Pick } from './charts';
import { STUDY_FILE, STUDY_SCHEMA, STUDY_SCHEMA_VERSION } from './contract';
import type { PublicRun, StudyCell, StudyDataset, StudyResult, VerifiedStudy } from './contract';
import { COMPARISON_LABEL, COMPARISON_MEANING, labelGroup, labelRun, resolveSelection, type ComparisonKind } from './explore';
import { PUBLISHED_STUDIES, loadStudyResult, runCellKey, type StudyFailureKind } from './load';
import { candidateRunIds, listedDemoRuns, matchRun, reproduceCommand, type ListedDemoRun, type RunLink, type StoredRunFacts } from './links';
import {
  DATA_CLASS_MEANING,
  DESIGN_LABEL,
  METRICS,
  NOT_CALIBRATED,
  allGroups,
  cancelDelay,
  decomposition,
  describeLatency,
  designSummary,
  fmtPoint,
  heldConstant,
  latencyComparison,
  latencyLabel,
  metricDef,
  queueComparison,
  queueLabel,
  regimeComparison,
  regimeLabel,
  sensitivityMatrices,
  type ChartRow,
  type MatrixView,
} from './model';
import { Chain, ClassBadge, LinkStatus, Reproduce, Section, SyntheticBadges } from './parts';
import { CancelDelaySection, FeedDelaySection, InventorySection, UncertaintySection } from './StudyViews';
import './study.css';

/** The verified study, or its failure as a value `useAsync` can hold. */
async function loadStudy(id: string, signal: AbortSignal): Promise<VerifiedStudy | { kind: StudyFailureKind; message: string }> {
  const r = await loadStudyResult(id, signal);
  return r.ok ? { study: r.study, sha256: r.sha256 } : { kind: r.kind, message: r.message };
}

interface LiveLinks {
  /** `null` when the demo list could not be read. */
  listed: ListedDemoRun[] | null;
  stored: Map<string, StoredRunFacts>;
  problem: string | null;
}

/** Reads the public demo list, then the stored facts of every study run a demo lists. */
async function loadLiveLinks(study: StudyResult, signal: AbortSignal): Promise<LiveLinks> {
  let listed: ListedDemoRun[];
  try {
    listed = listedDemoRuns(await api.demos(undefined, { signal }));
  } catch (e) {
    if (isAbort(e)) throw e;
    return { listed: null, stored: new Map(), problem: `The public demo runs could not be read (${describeError(e).message}).` };
  }
  const ids = candidateRunIds([...study.cells, ...study.demo_anchors.runs], listed);
  // The verified run list carries every fact a match checks, so no run is read by id when it is
  // listed: one parallel `GET /runs/{id}` per candidate competed for the heavy-read permit on a
  // cold server and was refused `503 busy` (the compare-set fix, PHASE6_RELIABILITY A1).
  const stored = new Map<string, StoredRunFacts>();
  let failed = 0;
  try {
    const list = await retryBusy(() => api.runs({ workspace: DEMO_WORKSPACE }, { signal }), signal);
    const wanted = new Set(ids);
    for (const r of list.items) if (wanted.has(r.run_id)) stored.set(r.run_id, r);
  } catch (e) {
    if (isAbort(e)) throw e;
  }
  // Anything not in the list is read by id one at a time, retrying a busy refusal.
  for (const id of ids) {
    if (stored.has(id)) continue;
    try {
      stored.set(id, await retryBusy(() => api.run(id, { signal }), signal));
    } catch (e) {
      if (isAbort(e)) throw e;
      failed += 1;
    }
  }
  return { listed, stored, problem: failed > 0 ? `${failed} of ${ids.length} stored runs could not be read; they are not linked.` : null };
}

export default function StudiesPage() {
  const { studyId } = useParams();
  if (studyId === undefined) return <StudyIndex />;
  const entry = PUBLISHED_STUDIES.find((s) => s.id === studyId);
  if (!entry) {
    return (
      <Workspace context={studyId} scroll>
        <Empty>
          <p>
            No published study <span className="mono">{studyId}</span>. <Link to="/studies">All studies</Link>.
          </p>
        </Empty>
      </Workspace>
    );
  }
  return <StudyPage id={entry.id} />;
}

function StudyIndex() {
  return (
    <Workspace context="Studies" scroll ownHeading>
      <div className="pad-3 col stc-page">
        <h1 className="stc-title">Execution studies</h1>
        <p className="note">
          Read-only results of local research studies, published as synthetic artefacts with a content hash. Each study is a
          deterministic simulation design on generated data, not an observation of a real market.
        </p>
        <ul className="col stc-index" data-testid="study-index">
          {PUBLISHED_STUDIES.map((s) => (
            <li key={s.id} className="stc-index-item">
              <Link to={`/studies/${s.id}`} className="stc-index-link">
                {s.title}
              </Link>
              <p className="note">{s.summary}</p>
              <SyntheticBadges />
            </li>
          ))}
        </ul>
      </div>
    </Workspace>
  );
}

type Stage = 'overview' | 'matrix' | 'cell' | 'uncertainty';

const STAGES: { id: Stage; label: string }[] = [
  { id: 'overview', label: 'Overview' },
  { id: 'matrix', label: 'Matrix' },
  { id: 'cell', label: 'Cell' },
  { id: 'uncertainty', label: 'Uncertainty' },
];

const COMPARISON_KINDS: ComparisonKind[] = ['paired', 'metrics_only', 'cross_dataset', 'multi_seed', 'not_attributable'];

const ERROR_HEAD: Record<StudyFailureKind, string> = {
  unavailable: 'Study unavailable',
  hash_mismatch: 'Content hash mismatch',
  unsupported_contract: 'Unsupported study contract version',
  invalid: 'Invalid study file',
};

function StudyPage({ id }: { id: string }) {
  const loaded = useAsync((signal) => loadStudy(id, signal), [id]);
  const failure = loaded.error
    ? { kind: 'unavailable' as const, message: loaded.error.message }
    : loaded.data && 'kind' in loaded.data
      ? loaded.data
      : null;
  if (failure) {
    return (
      <Workspace context={id} scroll>
        <div className="pad-3">
          <div className="error-box" role="alert" data-testid="study-error" data-kind={failure.kind}>
            <div className="error-box-head">{ERROR_HEAD[failure.kind]}</div>
            <div className="error-box-message">{failure.message}</div>
            <div className="error-box-message">
              Nothing from this file is shown. The viewer reads {STUDY_SCHEMA} version {STUDY_SCHEMA_VERSION} and verifies the SHA-256 in{' '}
              <span className="mono">{STUDY_FILE}.sha256</span> first.
            </div>
            <div className="error-box-actions">
              <button type="button" onClick={loaded.reload}>
                Retry
              </button>
            </div>
          </div>
        </div>
      </Workspace>
    );
  }
  if (!loaded.data || 'kind' in loaded.data) {
    return (
      <Workspace context={id} scroll>
        <div className="pad-3">
          <Spinner label="Downloading and verifying the study" />
        </div>
      </Workspace>
    );
  }
  return <StudyView verified={loaded.data} />;
}

function StudyView({ verified }: { verified: VerifiedStudy }) {
  const { study, sha256 } = verified;
  const [params, setParams] = useSearchParams();
  const phone = usePhone();
  const announce = useAnnounce();
  const metricParam = params.get('metric');
  const metric = metricParam !== null && METRICS.some((m) => m.key === metricParam) ? metricParam : 'net_pnl';
  const def = metricDef(metric);
  const rawCell = params.get('cell');
  const sel = useMemo(() => resolveSelection(study, rawCell), [study, rawCell]);
  const tabParam = params.get('tab');
  const stage: Stage = STAGES.find((s) => s.id === tabParam)?.id ?? 'overview';
  const live = useAsync((signal) => loadLiveLinks(study, signal), [study]);
  const linkOf = (run: PublicRun): RunLink =>
    live.data ? matchRun(run, live.data.listed, live.data.stored) : { kind: 'not_stored', reason: 'unchecked' };
  const allRuns = [...study.cells, ...study.demo_anchors.runs];
  const linkById = (runId: string): RunLink => {
    const run = allRuns.find((r) => r.run_id === runId);
    return run ? linkOf(run) : { kind: 'not_stored', reason: 'no_stored_run' };
  };

  // Focus follows the detail (B1, keyboard): a selection made where its control disappears (inside
  // the detail, or any selection on a phone, which switches to the Cell tab) moves focus to the new
  // detail's title; closing returns focus to the control that opened the detail, or to the
  // selected cell's own control when that control was re-rendered or the URL opened the detail.
  const opener = useRef<HTMLElement | null>(null);
  const pendingFocus = useRef<{ to: 'detail' } | { to: 'opener'; key: string } | null>(null);
  // A selection is a history entry (Back/Forward restore it); metric and tab changes replace.
  const select = (key: string) => {
    const next = resolveSelection(study, key);
    const active = document.activeElement;
    const fromDetail = active instanceof HTMLElement && active.closest('[data-testid="cell-panel"]') !== null;
    if (active instanceof HTMLElement && active !== document.body && !fromDetail) opener.current = active;
    if (fromDetail || phone) pendingFocus.current = { to: 'detail' };
    setParams((p) => {
      const q = new URLSearchParams(p);
      q.set('cell', key);
      if (phone) q.set('tab', 'cell');
      return q;
    });
    if (next) announce(`Selected ${selectionTitle(next)}`);
  };
  const close = () => {
    pendingFocus.current = { to: 'opener', key: sel?.key ?? '' };
    setParams((p) => {
      const q = new URLSearchParams(p);
      q.delete('cell');
      if (q.get('tab') === 'cell') q.set('tab', 'matrix');
      return q;
    });
  };
  const replaceParam = (name: 'tab' | 'metric', value: string) =>
    setParams(
      (p) => {
        const q = new URLSearchParams(p);
        q.set(name, value);
        return q;
      },
      { replace: true },
    );
  const pick: Pick = { selected: sel?.key ?? null, onSelect: select };
  // Phone: a new stage (or cell) starts at its top, not at the previous stage's scroll depth.
  const stagesRef = useRef<HTMLDivElement>(null);
  const asideRef = useRef<HTMLElement>(null);
  useEffect(() => {
    const el = stagesRef.current;
    const scroller = el?.closest('.ws-body-scroll');
    if (el && scroller) {
      const above = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      if (above < 0) scroller.scrollTop += above;
    }
    // Wide screens: a newly selected cell's detail starts at its title.
    asideRef.current?.scrollTo({ top: 0 });
  }, [stage, rawCell]);
  useEffect(() => {
    const want = pendingFocus.current;
    if (want === null) return;
    if (want.to === 'detail') {
      const title = document.getElementById('stc-cell-title');
      if (title === null) return;
      pendingFocus.current = null;
      title.focus();
      return;
    }
    if (rawCell !== null) return;
    pendingFocus.current = null;
    const back = opener.current;
    opener.current = null;
    const target =
      (back?.isConnected ? back : null) ??
      document.querySelector<HTMLElement>(`button[data-cell="${CSS.escape(want.key)}"]`) ??
      document.querySelector<HTMLElement>('.stc-stages [role="tab"][aria-selected="true"]') ??
      document.getElementById('main');
    target?.focus();
  }, [stage, rawCell]);
  const groupLabelOf = (r: ChartRow) => labelGroup(study, r.group);

  const synthetic = <SyntheticBadges />;
  const sensitivity = (
    <>
      <ClassBadge dataClass="sensitivity" />
      {synthetic}
    </>
  );

  const header = (
    <header className="col stc-head">
      <p className="small dim">
        <Link to="/studies">Studies</Link> / <span className="mono">{study.study_id}</span>
      </p>
      <h1 className="stc-title">{study.title}</h1>
      {synthetic}
      <Notice tag="Synthetic" tone="modeled">
        {NOT_CALIBRATED}
      </Notice>
    </header>
  );

  const identity = (
    <Section title="Study identity" id="stc-identity" key="identity">
      <dl className="kv" data-testid="study-identity">
        <dt>Study</dt>
        <dd className="mono">{study.study_id}</dd>
        <dt>Contract</dt>
        <dd>
          <span className="mono">
            {study.schema} v{study.schema_version}
          </span>{' '}
          <span className="dim">(this viewer reads v{STUDY_SCHEMA_VERSION})</span>
        </dd>
        <dt>Content SHA-256</dt>
        <dd data-testid="study-sha256">
          <HashText hash={sha256} chars={16} /> <Badge kind="ok">verified in this browser</Badge>
        </dd>
        <dt>Generator</dt>
        <dd>
          <span className="mono">blackbox {study.generator.blackbox_version}</span>, simulation revision{' '}
          <span className="mono">{study.generator.simulation_revision}</span>, engine{' '}
          <span className="mono">
            {study.generator.engine.name} {study.generator.engine.version}
          </span>
          , run contract v<span className="mono">{study.generator.run_contract_version}</span>
        </dd>
        <dt>Source commit</dt>
        <dd>
          {study.generator.source_commit ? (
            <>
              <span className="mono">{study.generator.source_commit}</span> <span className="dim">(declared at generation, not verified here)</span>
            </>
          ) : (
            <span className="dim">not declared</span>
          )}
        </dd>
        <dt>Platform class</dt>
        <dd className="mono">{study.generator.platform}</dd>
        <dt>Reproduce</dt>
        <dd>
          <code className="mono">{study.generator.command}</code>
        </dd>
      </dl>
    </Section>
  );

  const howToRead = (
    <Section title="How to read these numbers" id="stc-honesty" key="honesty">
      <ul className="stc-classes col" data-testid="study-data-classes">
        {(['deterministic', 'sensitivity', 'seed_variation'] as const).map((c) => (
          <li key={c}>
            <ClassBadge dataClass={c} /> <span className="note">{DATA_CLASS_MEANING[c]}</span>
          </li>
        ))}
        <li>
          <Badge kind="warn">Not calibrated to real markets</Badge> <span className="note">{NOT_CALIBRATED}</span>
        </li>
      </ul>
      <h3 className="stc-h3">Comparison labels</h3>
      <ul className="stc-classes col" data-testid="study-comparison-kinds">
        {COMPARISON_KINDS.map((k) => (
          <li key={k}>
            <span className={`stc-cmp stc-cmp-${k}`}>{COMPARISON_LABEL[k]}</span> <span className="note">{COMPARISON_MEANING[k]}</span>
          </li>
        ))}
      </ul>
      <p className="note">
        Each label is derived from the artefact: equal dataset SHA-256, the comparison&apos;s order-alignment flags, the configuration
        paths that differ and the number of seeds behind a value. Intervals are seed variation only (
        {study.uncertainty.applies_to_latencies.map(latencyLabel).join('; ')}); see Uncertainty for everything they leave out.
      </p>
      <ul className="note stc-caveats">
        {study.caveats.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ul>
    </Section>
  );

  const picker = (
    <div className="row stc-metric-pick" key="picker">
      <label className="row" htmlFor="stc-metric">
        <span>Measure for the comparisons below</span>
        <select id="stc-metric" value={metric} onChange={(e) => replaceParam('metric', e.target.value)}>
          {METRICS.map((m) => (
            <option key={m.key} value={m.key}>
              {m.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  );

  const matrix = (
    <Section title="Sensitivity matrix: queue model x latency, per regime" id="stc-matrix" labels={sensitivity} key="matrix">
      <p className="note">
        {def.label}. Rows: queue-position model; columns: order-latency shape (both at matched 1 ms medians). Each regime is its own stratum:
        regimes replay different synthetic events, so compare within a table, not across tables. The delta is against the table&apos;s{' '}
        {queueLabel(study.baseline.queue_model)} x {latencyLabel(study.baseline.latency)} cell. Select a cell for its detail.
      </p>
      {sensitivityMatrices(study, metric).map((m) => (
        <Matrix key={m.regime} study={study} view={m} fmt={def.fmt} pick={pick} />
      ))}
    </Section>
  );

  const charts = [
    <Section title="Queue-model comparison" id="stc-queue" key="queue">
      <IntervalChart
        testId="study-queue-chart"
        rows={queueComparison(study, metric)}
        fmt={def.fmt}
        caption={`${def.label} by queue model, ${regimeLabel(study.baseline.regime)}, constant 1 ms latency`}
        labels={sensitivity}
        pick={pick}
        labelOf={groupLabelOf}
      />
      <p className="note">
        {regimeLabel(study.baseline.regime)}, constant 1 ms latency, no feed offset. L2 data cannot reveal the true queue position; the spread
        across models is the sensitivity of the simulated outcome to that assumption.
      </p>
    </Section>,
    <Section title="Latency comparison" id="stc-latency" key="latency">
      <IntervalChart
        testId="study-latency-chart"
        rows={latencyComparison(study, metric)}
        fmt={def.fmt}
        caption={`${def.label} by latency model, ${regimeLabel(study.baseline.regime)}, baseline queue model`}
        labels={
          <>
            <ClassBadge dataClass="sensitivity" />
            <ClassBadge dataClass="seed_variation" />
            {synthetic}
          </>
        }
        pick={pick}
        labelOf={groupLabelOf}
      />
      <p className="note">
        Shapes are matched on their marginal 1 ms median; asymmetric, cancel-delay and feed-offset rows change one leg on purpose. Whiskers
        appear only for stochastic latency models and show seed variation over {study.design.latency_seeds.length} seeds.
      </p>
    </Section>,
    <InventorySection study={study} pick={pick} key="inventory" />,
    <CancelDelaySection study={study} pick={pick} key="cancel" />,
    <FeedDelaySection study={study} pick={pick} key="feed" />,
    <Section title="Fill rate across every group" id="stc-fill" key="fill">
      <IntervalChart
        testId="study-fill-chart"
        rows={allGroups(study, 'fill_rate')}
        fmt={metricDef('fill_rate').fmt}
        caption="Fill rate (filled / submitted orders) for every study group"
        labels={sensitivity}
        pick={pick}
        labelOf={groupLabelOf}
      />
      <p className="note">Filled orders over submitted orders, as simulated. Not an empirical fill probability.</p>
    </Section>,
    <Section title="Fee and P&L decomposition" id="stc-pnl" labels={sensitivity} key="pnl">
      <DecompositionChart rows={decomposition(study)} fmt={(v) => v.toFixed(4)} labels={null} pick={pick} />
      <p className="note">Session totals in quote currency for one simulated session per run; not a return rate and not annualised.</p>
    </Section>,
    <Section title="Regime comparison" id="stc-regime" key="regime">
      <IntervalChart
        testId="study-regime-chart"
        rows={regimeComparison(study, metric)}
        fmt={def.fmt}
        caption={`${def.label} of each regime's baseline run`}
        labels={
          <>
            <ClassBadge dataClass="deterministic" />
            {synthetic}
          </>
        }
        pick={pick}
        labelOf={groupLabelOf}
      />
      <p className="note">
        Cross-regime order alignment: {study.design.cross_regime_alignment}. Regimes are synthetic interventions, not reconstructions of
        historical events; deltas mix the regime with knock-on strategy effects.
      </p>
    </Section>,
  ];

  const uncertainty = <UncertaintySection study={study} metric={metric} pick={pick} key="uncertainty" />;

  const evidence = [
    <Section title="Evidence: stored public demo runs" id="stc-evidence" key="evidence">
      <EvidencePanel study={study} live={live.data} loading={live.loading} linkOf={linkOf} linkById={linkById} pick={pick} />
    </Section>,
    <Section title={`Every study run (${study.cells.length})`} id="stc-cells" key="cells">
      <CellsTable study={study} linkOf={linkOf} pick={pick} />
    </Section>,
  ];

  const unknownCell =
    rawCell !== null && !sel ? (
      <Notice tag="No such cell" tone="warn">
        <span data-testid="cell-unknown">
          The URL selects <span className="mono">{rawCell}</span>, which names no run or group of this study.
        </span>
      </Notice>
    ) : null;

  const panel = sel ? (
    <CellPanel study={study} sel={sel} linkOf={linkOf} linkById={linkById} checking={live.loading && !live.data} onSelect={select} onClose={close} />
  ) : null;

  if (phone) {
    const content: Record<Stage, ReactNode> = {
      overview: [identity, howToRead, <StudyDesign study={study} key="design" />, ...evidence],
      matrix: [picker, matrix, ...charts],
      cell: panel ?? (
        <div className="col stc-gap-1">
          {unknownCell}
          <p className="note" data-testid="cell-empty">
            No cell selected. Open a matrix cell or a chart row under Matrix, a seed under Uncertainty, or a run under Overview.
          </p>
        </div>
      ),
      uncertainty: [picker, uncertainty],
    };
    return (
      <Workspace context={study.title} scroll ownHeading>
        <div className="pad-3 col stc-page" data-testid="study-view" data-study-id={study.study_id} data-stage={stage}>
          {header}
          <div ref={stagesRef}>
            <Tabs<Stage>
              label="Study view"
              className="stc-stages"
              active={stage}
              onChange={(s) => replaceParam('tab', s)}
              tabs={STAGES.map((s) => ({ id: s.id, label: s.label, count: s.id === 'cell' && sel ? '1' : undefined }))}
            >
              <div className="col stc-stage">{content[stage]}</div>
            </Tabs>
          </div>
        </div>
      </Workspace>
    );
  }

  const open = sel !== null || unknownCell !== null;
  return (
    <Workspace context={study.title} scroll ownHeading>
      <div className={open ? 'stc-shell stc-shell-open' : 'stc-shell'} data-testid="study-view" data-study-id={study.study_id}>
        <div className="pad-3 col stc-page">
          {header}
          {identity}
          {howToRead}
          <StudyDesign study={study} />
          {picker}
          {matrix}
          {charts}
          {uncertainty}
          {evidence}
        </div>
        {open ? (
          <aside className="stc-aside" aria-label="Selected cell" ref={asideRef}>
            {panel ?? (
              <div className="col stc-panel">
                {unknownCell}
                <button type="button" className="btn-link" onClick={close}>
                  Close
                </button>
              </div>
            )}
          </aside>
        ) : null}
      </div>
    </Workspace>
  );
}

function datasetSeconds(d: StudyDataset): string {
  const ns = d.generator?.duration_ns;
  return typeof ns === 'number' ? `${ns / 1e9} s` : '\u2014';
}

function DatasetTable({ datasets, caption }: { datasets: StudyDataset[]; caption: string }) {
  return (
    <div className="table-scroll">
      <table className="table">
        <caption>{caption}</caption>
        <thead>
          <tr>
            <th scope="col">Dataset</th>
            <th scope="col">Regime</th>
            <th scope="col" className="num">
              Rows
            </th>
            <th scope="col" className="num">
              Session
            </th>
            <th scope="col">Events SHA-256</th>
          </tr>
        </thead>
        <tbody>
          {datasets.map((d) => (
            <tr key={d.id}>
              <th scope="row" className="mono">
                {d.id}
              </th>
              <td>{regimeLabel(d.regime)}</td>
              <td className="mono num">{d.counts.rows.toLocaleString('en-US')}</td>
              <td className="mono num">{datasetSeconds(d)}</td>
              <td>
                <HashText hash={d.events_sha256} chars={16} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function StudyDesign({ study }: { study: StudyResult }) {
  const baseLatency = study.latency_models.find((m) => m.name === study.baseline.latency);
  const stochastic = study.latency_models.filter((m) => m.stochastic);
  return (
    <Section title="Design: baseline, varied and held constant" id="stc-design">
      <div className="grid-2">
        <div className="col">
          <h3 className="stc-h3">Baseline</h3>
          <dl className="kv" data-testid="study-baseline">
            <dt>Preset</dt>
            <dd className="mono">{study.baseline.preset}</dd>
            <dt>Dataset / regime</dt>
            <dd>
              <span className="mono">{study.baseline.dataset.id}</span> ({regimeLabel(study.baseline.regime)})
            </dd>
            <dt>Queue model</dt>
            <dd>{queueLabel(study.baseline.queue_model)}</dd>
            <dt>Latency distribution</dt>
            <dd>{baseLatency ? describeLatency(baseLatency.config) : study.baseline.latency}</dd>
            <dt>Cancel delay</dt>
            <dd>{baseLatency ? cancelDelay(baseLatency.config) : '\u2014'}</dd>
            <dt>Feed latency</dt>
            <dd>generator feed delay plus offset {study.baseline.feed_offset_ns / 1e6} ms</dd>
            <dt>Seed ensemble</dt>
            <dd>
              seeds <span className="mono">{study.design.latency_seeds.join(', ')}</span> for stochastic latency models only; deterministic cells use
              seed <span className="mono">{study.design.latency_seeds[0]}</span>
            </dd>
            <dt>Baseline run</dt>
            <dd>
              <span className="mono">{study.baseline.run_id}</span> <span className="dim">config</span>{' '}
              <HashText hash={study.baseline.config_sha256} chars={16} />
            </dd>
          </dl>
        </div>
        <div className="col">
          <h3 className="stc-h3">Held constant in every run</h3>
          <dl className="kv" data-testid="study-held-constant">
            {heldConstant(study).map((h) => (
              <Fragment key={h.label}>
                <dt>{h.label}</dt>
                <dd>{h.value}</dd>
              </Fragment>
            ))}
          </dl>
        </div>
      </div>
      <h3 className="stc-h3">What each design block varies</h3>
      <div className="table-scroll">
        <table className="table" data-testid="study-design-blocks">
          <thead>
            <tr>
              <th scope="col">Block</th>
              <th scope="col">Varied variable</th>
              <th scope="col" className="num">
                Groups
              </th>
              <th scope="col" className="num">
                Runs
              </th>
            </tr>
          </thead>
          <tbody>
            {designSummary(study).map((d) => (
              <tr key={d.design}>
                <th scope="row">{d.label}</th>
                <td className="wrap">{d.varied}</td>
                <td className="mono num">{d.groups}</td>
                <td className="mono num">{d.runs}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <h3 className="stc-h3">Latency levels</h3>
      <div className="table-scroll">
        <table className="table">
          <thead>
            <tr>
              <th scope="col">Level</th>
              <th scope="col">Distribution</th>
              <th scope="col">Cancel delay</th>
              <th scope="col">Seeds</th>
            </tr>
          </thead>
          <tbody>
            {study.latency_models.map((m) => (
              <tr key={m.name}>
                <th scope="row" className="wrap">
                  {latencyLabel(m.name)}
                </th>
                <td className="wrap stc-wide">{describeLatency(m.config)}</td>
                <td className="wrap">{cancelDelay(m.config)}</td>
                <td className="wrap">{m.stochastic ? `${study.design.latency_seeds.length} (seed variation)` : '1 (deterministic)'}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="note">
        Queue models: {study.queue_models.map((m) => queueLabel(m.name)).join('; ')}. Stochastic latency models:{' '}
        {stochastic.map((m) => latencyLabel(m.name)).join('; ')}. Feed latency offset level: +20 ms on every local feed receipt.
      </p>
      <h3 className="stc-h3">Datasets</h3>
      <DatasetTable datasets={study.datasets} caption="Generated per regime by the study; synthetic, regenerated from the generator settings." />
    </Section>
  );
}

type MatrixEntry = MatrixView['cells'][number][number];

function MatrixCell({ study, cell, reference, fmt, pick, column }: { study: StudyResult; cell: MatrixEntry; reference: MatrixEntry; fmt: (v: number) => string; pick: Pick; column: string }) {
  if (cell === null) {
    return (
      <td className="dim" data-label={column}>
        not in design
      </td>
    );
  }
  const { point } = cell;
  const delta = point && reference?.point && cell.key !== reference.key ? point.value - reference.point.value : null;
  const selected = pick.selected === cell.key;
  return (
    <td className={selected ? 'mono num stc-mcell stc-selected' : 'mono num stc-mcell'} data-label={column}>
      <button type="button" className="stc-cellbtn" aria-pressed={selected} data-cell={cell.key} onClick={() => pick.onSelect(cell.key)}>
        <span className="stc-ci">{point ? fmt(point.value) : 'unavailable'}</span>
        {point?.interval ? (
          <span className="stc-ci dim small">
            [{fmt(point.interval[0])}, {fmt(point.interval[1])}]
          </span>
        ) : null}
        <span className="dim small stc-ci">
          {cell.key === reference?.key ? (
            'reference'
          ) : delta !== null ? (
            <>
              {'\u0394'} {delta >= 0 ? '+' : ''}
              {fmt(delta)}
            </>
          ) : (
            '\u0394 unavailable'
          )}
        </span>
      </button>
      {cell.key === reference?.key ? null : <ComparisonTags label={labelGroup(study, cell.group)} short />}
    </td>
  );
}

function Matrix({ study, view, fmt, pick }: { study: StudyResult; view: MatrixView; fmt: (v: number) => string; pick: Pick }) {
  const qi = view.queues.indexOf(study.baseline.queue_model);
  const li = view.latencies.indexOf(study.baseline.latency);
  const reference = qi >= 0 && li >= 0 ? view.cells[qi][li] : null;
  return (
    <div className="table-scroll">
      <table className="table stc-matrix stc-stack" data-testid={`study-matrix-${view.regime}`}>
        <caption>{regimeLabel(view.regime)}</caption>
        <thead>
          <tr>
            <th scope="col">Queue model \ latency</th>
            {view.latencies.map((l) => (
              <th key={l} scope="col" className="num">
                {latencyLabel(l)}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {view.queues.map((q, i) => (
            <tr key={q}>
              <th scope="row">{queueLabel(q)}</th>
              {view.latencies.map((l, j) => (
                <MatrixCell key={l} study={study} cell={view.cells[i][j]} reference={reference} fmt={fmt} pick={pick} column={latencyLabel(l)} />
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function DetailButton({ runId, pick }: { runId: string; pick: Pick }) {
  const key = runCellKey(runId);
  return (
    <button type="button" className="btn-link" aria-pressed={pick.selected === key} onClick={() => pick.onSelect(key)} aria-label={`Details of run ${runId}`}>
      Details
    </button>
  );
}

function EvidencePanel({
  study,
  live,
  loading,
  linkOf,
  linkById,
  pick,
}: {
  study: StudyResult;
  live: LiveLinks | null;
  loading: boolean;
  linkOf: (r: PublicRun) => RunLink;
  linkById: (id: string) => RunLink;
  pick: Pick;
}) {
  const anchors = study.demo_anchors;
  return (
    <div className="col">
      <p className="note">
        The study also recomputes the <span className="mono">{anchors.demo_id}</span> demo ({anchors.title}) on the demo&apos;s own 120 s synthetic
        datasets. A run links to the stored public run only when the API lists the same run id (the config SHA-256 prefix, which covers the
        engine, dataset hash and full configuration) and the stored run&apos;s dataset events SHA-256 equals the study&apos;s. Otherwise the
        local reproduction command is shown.
      </p>
      {loading && !live ? <Spinner label="Checking stored public runs" /> : null}
      {live?.problem ? (
        <Notice tag="Not linked" tone="warn">
          {live.problem}
        </Notice>
      ) : null}
      <div className="table-scroll">
        <table className="table" data-testid="study-anchors">
          <thead>
            <tr>
              <th scope="col">#</th>
              <th scope="col">Run (changed assumption)</th>
              <th scope="col">Run id</th>
              <th scope="col">Dataset</th>
              <th scope="col">Compared with</th>
              <th scope="col">Stored public run</th>
            </tr>
          </thead>
          <tbody>
            {anchors.runs.map((r) => {
              const link = linkOf(r);
              const ref = r.reference_index === null ? null : (anchors.runs.find((a) => a.index === r.reference_index) ?? null);
              return (
                <tr key={r.run_id} data-testid="study-anchor" data-run-id={r.run_id} data-link={link.kind}>
                  <td className="mono">{r.index}</td>
                  <th scope="row" className="wrap">
                    {r.label}
                  </th>
                  <td>
                    <span className="mono">{r.run_id}</span> <DetailButton runId={r.run_id} pick={pick} />
                  </td>
                  <td>
                    <span className="mono">{r.dataset.id}</span> <HashText hash={r.dataset.sha256} chars={8} />
                  </td>
                  <td className="wrap">
                    {ref ? <>#{ref.index} </> : null}
                    <ComparisonTags label={labelRun(study, r)} short />
                  </td>
                  <td className="wrap">
                    <div className="col stc-evidence">
                      <LinkStatus link={link} />
                      {link.kind === 'stored' ? (
                        <Chain run={r} link={link} linkById={linkById} />
                      ) : (
                        <Reproduce command={reproduceCommand(study, r, { anchorIndex: r.index })} />
                      )}
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <h3 className="stc-h3">Demo datasets</h3>
      <DatasetTable datasets={anchors.datasets} caption="The demo's own synthetic datasets, regenerated by the study." />
    </div>
  );
}

function CellsTable({ study, linkOf, pick }: { study: StudyResult; linkOf: (r: PublicRun) => RunLink; pick: Pick }) {
  const fillRate = metricDef('fill_rate').fmt;
  const net = metricDef('net_pnl').fmt;
  return (
    <details className="stc-cells">
      <summary>Show every run with its hashes and reproduction command</summary>
      <div className="table-scroll">
        <table className="table" data-testid="study-cells">
          <thead>
            <tr>
              <th scope="col">Regime</th>
              <th scope="col">Queue</th>
              <th scope="col">Latency</th>
              <th scope="col" className="num">
                Feed offset
              </th>
              <th scope="col" className="num">
                Seed
              </th>
              <th scope="col">Design</th>
              <th scope="col">Run id</th>
              <th scope="col">Comparison</th>
              <th scope="col" className="num">
                Fill rate
              </th>
              <th scope="col" className="num">
                Net P&amp;L
              </th>
              <th scope="col">Stored public run</th>
            </tr>
          </thead>
          <tbody>
            {study.cells.map((c: StudyCell) => {
              const link = linkOf(c);
              return (
                <tr key={c.run_id} data-link={link.kind}>
                  <td>{regimeLabel(c.regime)}</td>
                  <td>{queueLabel(c.queue_model)}</td>
                  <td>{latencyLabel(c.latency)}</td>
                  <td className="mono num">{c.feed_offset_ns / 1e6} ms</td>
                  <td className="mono num">{c.seed}</td>
                  <td>{DESIGN_LABEL[c.design] ?? c.design}</td>
                  <td>
                    <span className="mono">{c.run_id}</span> <DetailButton runId={c.run_id} pick={pick} />
                  </td>
                  <td>
                    <ComparisonTags label={labelRun(study, c)} short />
                  </td>
                  <td className="mono num">{c.measures.fill_rate === undefined ? '\u2014' : fillRate(c.measures.fill_rate)}</td>
                  <td className="mono num">{net(c.accounting.net_pnl)}</td>
                  <td className="wrap">
                    <div className="col stc-evidence">
                      <LinkStatus link={link} />
                      {link.kind === 'stored' ? null : <Reproduce command={reproduceCommand(study, c, { detail: c.detail })} />}
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </details>
  );
}
