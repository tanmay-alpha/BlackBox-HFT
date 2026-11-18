// Strategy Lab (UI_DESIGN_SPEC.md §11.4): one page in four steps. 1 Choose the experiment
// (dataset, strategy, a preset as starting point); 2 Configure assumptions (Guided shows the
// essentials, Advanced every section of `fields.ts`; both edit the same `RunConfig`); 3 Review
// (validation, what differs from the starting point and why it matters, a size estimate);
// 4 Run (label, the single submit, the optional counterfactual "Compare with…" scenario B, then
// links along Job → Results → Replay → Forensics → Risk → Compare). Wide screens keep a sticky
// summary beside the steps. Submitting enqueues an experiment job (`POST /api/v1/experiments`);
// the same configuration returns the live job or the stored run instead of a duplicate.

import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { Check, Layers, ListOrdered, TriangleAlert, Zap } from 'lucide-react';

import { api } from '../../api/client';
import { toWire } from '../../api/normalize';
import type { DatasetSummary, Run, RunDetail, SubmitExperimentResult } from '../../api/model';
import type { Preset, RunConfig } from '../../api/types';
import { JsonBlock } from '../../components/data';
import { assumptionsLabel, queueModelLabel, STRATEGY_INFO } from '../../components/describe';
import { fmtBytes, fmtDuration, fmtInt } from '../../components/fmt';
import { pathLabel } from '../../components/labels';
import { Tabs } from '../../components/Tabs';
import { Badge, Empty, ErrorBox, Notice, Pill, Spinner, useAsync } from '../../components/ui';
import { GatedButton, useAccess } from '../../shell/access';
import { useAnnounce } from '../../shell/announce';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { FieldRow, fieldDomId } from './FieldInput';
import { ScriptedOrdersEditor } from './ScriptedOrdersEditor';
import { buildSections, startingConfig } from './fields';
import type { Field, Section } from './fields';
import { collectProblems } from './problems';
import { jobStages, jobView, stepStates } from './progress';
import type { Job, JobView, StepId } from './progress';
import {
  ROLE_LABEL,
  assumptionChips,
  assumptionRows,
  changeDelta,
  changeValue,
  presetChips,
  presetShortTitle,
  sizeEstimate,
  strategyRole,
  whyItMatters,
} from './review';
import type { Chip, StrategyRole } from './review';
import { configAdvisories, diffConfigs } from './validate';
import type { ConfigChange } from './validate';
import './lab.css';

type SubmitMode = 'run' | 'scenario_b';
type FormMode = 'guided' | 'advanced';

/** Where the values currently in the form came from. Shown so nothing looks like a preset by accident. */
type Origin =
  | { type: 'defaults' }
  | { type: 'preset'; id: string; title: string; loaded: RunConfig }
  | { type: 'run'; runId: string; loaded: RunConfig };

/** The last accepted submission, kept on the page so the next steps stay one click away. */
interface Submission {
  result: SubmitExperimentResult;
  /** Scenario A of a counterfactual submission. */
  scenarioA: string | null;
  /** The submitted configuration, serialized: the Run step reports the job only while the form still holds it. */
  configKey: string;
}

const MODE_STORAGE_KEY = 'bb.lab.mode';
const SCOPE = 'lab';
const STRATEGY_FIELD = 'strategy.kind.type';
const TERMINAL: Record<string, true> = { completed: true, failed: true, cancelled: true, expired: true };

const STEPS: readonly { id: StepId; short: string }[] = [
  { id: 'lab-step-choose', short: 'Choose' },
  { id: 'lab-step-configure', short: 'Configure' },
  { id: 'lab-step-review', short: 'Review' },
  { id: 'lab-step-run', short: 'Run' },
];

const ROLE_ICON: Record<StrategyRole, typeof Layers> = { maker: Layers, taker: Zap, scripted: ListOrdered };

const MODELED_TITLE =
  'An execution assumption. Results produced under it are estimates of what could have happened, not observations.';

/**
 * Guided mode: the essentials, each with a one-line plain explanation. The fields are the same
 * descriptors as the Advanced form (same ids, getters and setters on the same `RunConfig`); only
 * the explanation is shorter. A field the selected strategy or latency model lacks is left out.
 */
const GUIDED_GROUPS: {
  title: string;
  modeled: boolean;
  fields: Record<string, string>;
  absent: (c: RunConfig, latencyModel: string) => string;
}[] = [
  {
    title: 'Order size',
    modeled: false,
    fields: { 'strategy.kind.order_qty': 'How much each order asks to buy or sell.' },
    absent: () => 'A scripted strategy places a fixed list of orders with their own sizes; edit the list in Advanced \u203a Strategy.',
  },
  {
    title: 'Latency',
    modeled: true,
    fields: {
      'execution.order_latency.entry_ns': 'How long an order takes to reach the simulated exchange.',
      'execution.order_latency.response_ns': 'How long the exchange\u2019s reply takes to come back.',
    },
    absent: (_c, model) => `This configuration uses the ${model} latency model; edit its parameters in Advanced \u203a Order latency.`,
  },
  {
    title: 'Fees',
    modeled: true,
    fields: {
      'execution.fees.maker': 'Paid when an order that waited in the book is filled; negative is a rebate.',
      'execution.fees.taker': 'Paid when an order trades immediately against the book.',
    },
    absent: () => '',
  },
  {
    title: 'Risk limit',
    modeled: false,
    fields: {
      'risk.max_position': 'The harness rejects any order that could take the absolute position above this, counting working orders.',
    },
    absent: () => '',
  },
];

/** The plain name of a strategy kind; unknown kinds (newer servers) show as sent. */
function strategyName(type: string | null): string {
  return type !== null && type in STRATEGY_INFO ? STRATEGY_INFO[type as keyof typeof STRATEGY_INFO].label : (type ?? 'unknown strategy');
}

function runOptionLabel(r: Run): string {
  const name = r.label ? `${r.label} ` : '';
  return `${name}${r.run_id} \u00b7 ${strategyName(r.strategy_kind)} \u00b7 ${r.dataset.id}`;
}

/** A field's current value in display units, for the summary. */
function shownValue(field: Field<RunConfig> | undefined, c: RunConfig): string | null {
  if (!field) return null;
  return changeValue(field, JSON.stringify(field.get(c)) ?? 'null');
}

function scrollToStep(id: string): void {
  const el = document.getElementById(id);
  if (!el) return;
  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  el.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'start' });
  el.querySelector<HTMLElement>('h2')?.focus({ preventScroll: true });
}

function StepHead({ n, id, title, lede, aside }: { n: number; id: string; title: string; lede: ReactNode; aside?: ReactNode }) {
  return (
    <header className="lab-step-head">
      <span className="lab-step-num" aria-hidden="true">
        {n}
      </span>
      <div className="lab-step-titles">
        <h2 id={`${id}-title`} tabIndex={-1}>
          <span className="visually-hidden">{`Step ${n} of 4: `}</span>
          {title}
        </h2>
        <p className="lab-lede">{lede}</p>
      </div>
      {aside ? <div className="lab-step-aside">{aside}</div> : null}
    </header>
  );
}

/** Changed settings as a list (readable on a phone): label, old → new (signed difference) in form units, why it matters. */
function ChangeList({
  caption,
  a,
  b,
  changes,
  fields,
}: {
  caption: string;
  a: string;
  b: string;
  changes: ConfigChange[];
  fields: Map<string, Field<RunConfig>>;
}) {
  return (
    <div className="col">
      <p className="lab-changes-caption">{caption}</p>
      <ul className="lab-changes">
        {changes.map((c) => {
          const f = fields.get(c.path);
          const delta = changeDelta(f, c.a, c.b);
          return (
            <li key={c.path} className="lab-change">
              <div className="lab-change-head">
                <span className="lab-change-label" title={c.path}>{f ? f.label : pathLabel(c.path)}</span>
                {c.path.startsWith('execution.') ? (
                  <Badge kind="modeled" title={MODELED_TITLE}>
                    modeled
                  </Badge>
                ) : null}
                <span className="lab-change-values">
                  <span className="visually-hidden">{`${a}: `}</span>
                  <span className="mono">{changeValue(f, c.a, c.path)}</span>
                  <span aria-hidden="true"> {'\u2192'} </span>
                  <span className="visually-hidden">{`, ${b}: `}</span>
                  <span className="mono lab-change-new">{changeValue(f, c.b, c.path)}</span>
                  {delta ? <span className="mono lab-change-delta"> ({delta})</span> : null}
                </span>
              </div>
              <p className="lab-change-why">
                {whyItMatters(c.path)}
                {f ? <span className="lab-change-path dim" title={c.path}> ({pathLabel(c.path)})</span> : null}
              </p>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** A card's key assumptions as compact chips; one a variant changes is marked in text, not colour only. */
function ChipList({ id, chips }: { id: string; chips: Chip[] }) {
  if (chips.length === 0) return null;
  return (
    <span className="lab-chips" id={id}>
      <span className="visually-hidden">Key assumptions: </span>
      {chips.map((c) => (
        <span
          key={c.key}
          className={c.differs ? 'lab-chip lab-chip-diff' : 'lab-chip'}
          title={c.differs ? 'Differs from the value most presets share' : undefined}
        >
          {c.differs ? <span className="lab-chip-mark" aria-hidden="true">{'\u2260'}</span> : null}
          {c.text}
          {c.differs ? <span className="visually-hidden"> (differs from most presets)</span> : null}
          <span className="visually-hidden">;</span>
        </span>
      ))}
    </span>
  );
}

const STAGE_WORD = { done: 'done', current: 'current', todo: 'not yet', error: 'ended', skipped: 'did not start' } as const;

/** After a submission: Queued → Running → Completed with honest wording, and the job's page. */
function JobProgress({ view }: { view: JobView }) {
  const stages = jobStages(view);
  const job = view.job;
  return (
    <div className="lab-job">
      {stages.length > 0 ? (
        <ol className="lab-job-stages" aria-label="Job progress">
          {stages.map((s) => (
            <li key={s.label} className={`lab-stage lab-stage-${s.state}`} aria-current={s.state === 'current' ? 'step' : undefined}>
              <span className="lab-stage-mark" aria-hidden="true">
                {s.state === 'done' ? <Check size={12} /> : s.state === 'error' ? <TriangleAlert size={12} /> : null}
              </span>
              <span>{s.label}</span>
              <span className="visually-hidden">{`: ${STAGE_WORD[s.state]}`}</span>
            </li>
          ))}
        </ol>
      ) : null}
      <p className="lab-lede" role="status" data-testid="lab-job-status">
        <strong className="lab-job-headline">{view.headline}.</strong> {view.detail}
      </p>
      {job ? (
        <p className="lab-lede">
          <Link to={paths.job(job.id)} className="lab-job-link" data-testid="lab-next-job">
            Job <span className="mono">{job.id}</span>
          </Link>{' '}
          <Pill tone={job.state === 'completed' ? 'ok' : view.phase === 'failed' ? 'error' : 'info'}>{job.state}</Pill>{' '}
          {TERMINAL[job.state] ? 'its page keeps the timings and any error.' : 'its page shows progress and can cancel it.'}
        </p>
      ) : null}
    </div>
  );
}

/** Report → Replay → Forensics → Risk → Compare for the submitted run; links once it exists. */
function RunLinks({ submission, view }: { submission: Submission | null; view: JobView | null }) {
  const runId = submission?.result.experiment.run_id ?? null;
  const ready = view?.ready === true;
  const pending = view === null ? null : view.phase === 'failed' || view.phase === 'none' ? 'not available: no run was produced' : 'opens when the job completes';
  const items: { key: string; label: string; what: string; to: string | null }[] = [
    { key: 'report', label: 'Report', what: 'measured outcomes and the reproducibility manifest', to: runId ? paths.report(runId) : null },
    { key: 'replay', label: 'Replay', what: 'the order book with the simulated orders on it', to: runId ? paths.bookRun(runId) : null },
    { key: 'forensics', label: 'Forensics', what: 'the order blotter; each order explains its fill or non-fill', to: runId ? paths.run(runId) : null },
    { key: 'risk', label: 'Risk', what: 'P&L, inventory and drawdown over the run', to: runId ? paths.riskRun(runId) : null },
    {
      key: 'compare',
      label: 'Compare',
      what: submission?.scenarioA ? `scenario A (${submission.scenarioA}) against this run` : 'this run against another, decision by decision',
      to: runId ? paths.comparePair(submission?.scenarioA ?? runId, submission?.scenarioA ? runId : undefined) : null,
    },
  ];
  return (
    <ul className="lab-links" aria-label="Views of the run">
      {items.map((it) => (
        <li key={it.key} className="lab-link-item">
          {ready && it.to ? (
            <Link to={it.to} className="btn lab-link" data-testid={`lab-next-${it.key}`}>
              {it.label}
            </Link>
          ) : (
            <span className="lab-link lab-link-off">{it.label}</span>
          )}
          <span className="lab-link-what">
            {it.what}
            {pending !== null && !ready ? <span className="lab-next-pending"> ({pending})</span> : null}
          </span>
        </li>
      ))}
    </ul>
  );
}

export default function LabPage() {
  const announce = useAnnounce();
  const access = useAccess();
  const [searchParams, setSearchParams] = useSearchParams();
  const datasetParam = searchParams.get('dataset');
  const fromParam = searchParams.get('from');
  const uid = useId();

  const datasets = useAsync((signal) => api.datasets({ signal }).then((c) => c.items), []);
  const runs = useAsync((signal) => api.runs({}, { signal }), []);
  const fromRun = useAsync<RunDetail | null>(
    (signal) => (fromParam ? api.run(fromParam, { signal }) : Promise.resolve(null)),
    [fromParam],
  );

  const [datasetId, setDatasetId] = useState<string | null>(null);
  const [config, setConfig] = useState<RunConfig | null>(null);
  const [origin, setOrigin] = useState<Origin>({ type: 'defaults' });
  const [baseRunId, setBaseRunId] = useState('');
  const [label, setLabel] = useState('');
  const [submitting, setSubmitting] = useState<SubmitMode | null>(null);
  const [submitError, setSubmitError] = useState<unknown>(null);
  const [submitNote, setSubmitNote] = useState<ReactNode>(null);
  const [submission, setSubmission] = useState<Submission | null>(null);
  const [job, setJob] = useState<Job | null>(null);
  const [activeSection, setActiveSection] = useState('strategy');
  const [mode, setModeState] = useState<FormMode>(() =>
    window.localStorage.getItem(MODE_STORAGE_KEY) === 'advanced' ? 'advanced' : 'guided',
  );
  const setMode = (m: FormMode) => {
    setModeState(m);
    window.localStorage.setItem(MODE_STORAGE_KEY, m);
  };
  // A field to focus once the form shows it; a fresh object per request re-runs the effect.
  const [focusRequest, setFocusRequest] = useState<{ fieldId: string } | null>(null);
  const resultRef = useRef<HTMLHeadingElement>(null);

  const presets = useAsync<Preset[]>(
    (signal) => (datasetId ? api.presets(datasetId, { signal }) : Promise.resolve([])),
    [datasetId],
  );

  const dataset: DatasetSummary | null = datasets.data?.find((d) => d.id === datasetId) ?? null;
  const instrument = dataset?.current_version?.instrument ?? null;

  // Initial form contents, decided once: the run named by ?from=, else the dataset named by
  // ?dataset= (or the first installed one) with starting values.
  const fromLoading = fromParam !== null && fromRun.loading;
  useEffect(() => {
    if (config !== null || fromLoading) return;
    if (fromRun.data) {
      const run = fromRun.data;
      setDatasetId(run.config.dataset_id);
      setConfig(run.config);
      setOrigin({ type: 'run', runId: run.run_id, loaded: run.config });
      setBaseRunId(run.run_id);
      return;
    }
    const list = datasets.data;
    if (!list || list.length === 0) return;
    const chosen = list.find((d) => d.id === datasetParam) ?? list[0];
    setDatasetId(chosen.id);
    setConfig(startingConfig(chosen.id, chosen.current_version?.instrument ?? null));
    setOrigin({ type: 'defaults' });
  }, [config, fromLoading, fromRun.data, datasets.data, datasetParam]);

  const sections = useMemo(
    () => (config ? buildSections(config, { instrument, presets: presets.data ?? [] }) : []),
    [config, instrument, presets.data],
  );
  const fieldsById = useMemo(() => new Map(sections.flatMap((s) => s.fields.map((f) => [f.id, f] as const))), [sections]);
  const sectionOf = useMemo(() => new Map(sections.flatMap((s) => s.fields.map((f) => [f.id, s.id] as const))), [sections]);
  // The strategy kind is chosen in step 1; the Advanced Strategy tab holds its parameters.
  const advancedSections: Section[] = useMemo(
    () => sections.map((s) => (s.id === 'strategy' ? { ...s, fields: s.fields.filter((f) => f.id !== STRATEGY_FIELD) } : s)),
    [sections],
  );

  // One problem per field; a server rule about a flagged field is attached to it (D3).
  const problems = useMemo(() => (config ? collectProblems(sections, config) : null), [config, sections]);
  const fieldProblems = problems?.fields ?? [];
  const ruleProblems = problems?.rules ?? [];
  const invalidBySection = useMemo(() => {
    const m = new Map<string, number>();
    for (const p of problems?.fields ?? []) m.set(p.sectionId, (m.get(p.sectionId) ?? 0) + 1);
    return m;
  }, [problems]);
  const advisories = useMemo(() => (config ? configAdvisories(config, instrument) : []), [config, instrument]);

  // List items carry no configuration; scenario A's comes from `GET /runs/{id}`.
  const baseRunDetail = useAsync<RunDetail | null>(
    (signal) =>
      baseRunId === '' ? Promise.resolve(null) : fromRun.data?.run_id === baseRunId ? Promise.resolve(fromRun.data) : api.run(baseRunId, { signal }),
    [baseRunId, fromRun.data],
  );
  const baseRun = baseRunDetail.data?.run_id === baseRunId ? baseRunDetail.data : null;
  const changesVsA = useMemo(() => (config && baseRun ? diffConfigs(baseRun.config, config) : []), [config, baseRun]);
  // With no preset loaded, the review compares against the starting values of the dataset.
  const startConfig = useMemo(() => (datasetId ? startingConfig(datasetId, instrument) : null), [datasetId, instrument]);
  const reference = origin.type !== 'defaults' ? origin.loaded : startConfig;
  const originChanges = useMemo(() => (config && reference ? diffConfigs(reference, config) : []), [config, reference]);
  const estimate = useMemo(() => (config ? sizeEstimate(config, dataset) : null), [config, dataset]);

  // Follow the submitted job until it ends, so the run's views become links when it completes.
  useEffect(() => {
    const first = submission?.result.job ?? null;
    setJob(first);
    if (!first || TERMINAL[first.state]) return;
    const ctrl = new AbortController();
    let timer: number | undefined;
    const poll = async () => {
      try {
        const j = await api.job(first.id, { signal: ctrl.signal });
        setJob(j);
        if (TERMINAL[j.state]) {
          announce(`Job ${j.id} ${j.state}.`);
          return;
        }
        timer = window.setTimeout(() => void poll(), 2000);
      } catch {
        if (!ctrl.signal.aborted) timer = window.setTimeout(() => void poll(), 5000);
      }
    };
    timer = window.setTimeout(() => void poll(), 1500);
    return () => {
      ctrl.abort();
      window.clearTimeout(timer);
    };
  }, [submission, announce]);

  useEffect(() => {
    if (focusRequest === null) return;
    const el = document.getElementById(fieldDomId(SCOPE, focusRequest.fieldId));
    el?.scrollIntoView({ block: 'center' });
    el?.focus({ preventScroll: true });
  }, [focusRequest]);

  // The step in view, for the step bar: the last step whose top has passed the upper third of
  // the scroll area (the last step once the area is scrolled to its end).
  const [currentStep, setCurrentStep] = useState<StepId>('lab-step-choose');
  const hasConfig = config !== null;
  useEffect(() => {
    if (!hasConfig) return;
    const first = document.getElementById(STEPS[0].id);
    const scroller = first?.closest<HTMLElement>('.ws-body-scroll');
    if (!scroller) return;
    let frame = 0;
    const update = () => {
      frame = 0;
      const top = scroller.getBoundingClientRect().top + scroller.clientHeight / 3;
      let id: StepId = STEPS[0].id;
      if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 4) id = STEPS[STEPS.length - 1].id;
      else for (const s of STEPS) if ((document.getElementById(s.id)?.getBoundingClientRect().top ?? Infinity) <= top) id = s.id;
      setCurrentStep(id);
    };
    const onScroll = () => {
      if (frame === 0) frame = window.requestAnimationFrame(update);
    };
    update();
    scroller.addEventListener('scroll', onScroll, { passive: true });
    window.addEventListener('resize', onScroll);
    return () => {
      scroller.removeEventListener('scroll', onScroll);
      window.removeEventListener('resize', onScroll);
      window.cancelAnimationFrame(frame);
    };
  }, [hasConfig]);

  /** Shows a field (Advanced and its section unless Guided or step 1 has it) and focuses it. */
  const focusField = (fieldId: string) => {
    const guided = GUIDED_GROUPS.some((g) => fieldId in g.fields);
    if (fieldId !== STRATEGY_FIELD && !(mode === 'guided' && guided)) {
      setMode('advanced');
      setActiveSection(sectionOf.get(fieldId) ?? 'strategy');
    }
    setFocusRequest({ fieldId });
  };

  const edit = (next: RunConfig) => {
    setConfig(next);
    setSubmitError(null);
    setSubmitNote(null);
  };

  const changeDataset = (id: string) => {
    setDatasetId(id);
    setSubmitError(null);
    setSubmitNote(null);
    const ds = datasets.data?.find((d) => d.id === id) ?? null;
    setConfig((c) => (c ? { ...c, dataset_id: id } : startingConfig(id, ds?.current_version?.instrument ?? null)));
    setSearchParams({ dataset: id }, { replace: true });
  };

  const applyPreset = (p: Preset) => {
    // `api.presets(datasetId)` already rebound the preset to the selected dataset.
    edit(p.config);
    setOrigin({ type: 'preset', id: p.id, title: p.title, loaded: p.config });
  };

  const applyDefaults = () => {
    if (!datasetId) return;
    edit(startingConfig(datasetId, instrument));
    setOrigin({ type: 'defaults' });
  };

  const loadBaseRun = () => {
    if (!baseRun) return;
    setDatasetId(baseRun.config.dataset_id);
    edit(baseRun.config);
    setOrigin({ type: 'run', runId: baseRun.run_id, loaded: baseRun.config });
  };

  const problemCount = problems?.count ?? 0;
  const blocked = problemCount > 0;
  /** Jump to the first problem: its field when it has one, else the Review step's list. */
  const jumpToProblem = () => {
    const first = fieldProblems[0];
    if (first) focusField(first.fieldId);
    else scrollToStep('lab-step-review');
  };

  /** What the submission returned, in words (UI_DESIGN_SPEC §11.4: idempotence reported honestly). */
  const outcomeText = (r: SubmitExperimentResult): string => {
    if (r.run) return `Already simulated \u2014 stored run ${r.run.run_id} returned.`;
    if (r.job && !r.created) return `The same experiment is already ${r.job.state}: existing job ${r.job.id} returned (first label wins).`;
    if (r.job) return `Job ${r.job.id} ${r.job.state}${r.job.queue_position !== null ? `, position #${r.job.queue_position}` : ''}.`;
    return `Experiment ${r.experiment.id} is ${r.experiment.status}.`;
  };

  const submit = async (how: SubmitMode) => {
    if (!config || blocked || submitting) return;
    if (!access.hasScope('run')) {
      if (access.mode !== 'research') access.openKeyDialog();
      return;
    }
    setSubmitting(how);
    setSubmitError(null);
    setSubmitNote(null);
    try {
      const trimmed = label.trim();
      const result = await api.submitExperiment(config, trimmed === '' ? null : trimmed);
      const runId = result.experiment.run_id;
      announce(outcomeText(result));
      if (how === 'scenario_b' && runId === baseRunId) {
        setSubmitNote(
          `This configuration is the same experiment as scenario A (run ${runId}): identical inputs give an identical run. Change at least one assumption to get a scenario B.`,
        );
      } else {
        setSubmission({ result, scenarioA: how === 'scenario_b' ? baseRunId : null, configKey: JSON.stringify(config) });
        window.requestAnimationFrame(() => {
          resultRef.current?.scrollIntoView({ block: 'nearest' });
          resultRef.current?.focus({ preventScroll: true });
        });
      }
    } catch (e) {
      setSubmitError(e);
    } finally {
      setSubmitting(null);
    }
  };

  // Ctrl+Enter submits from anywhere in the workspace; `submit` refuses an invalid configuration.
  const submitRef = useRef(submit);
  submitRef.current = submit;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        void submitRef.current('run');
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // The preset cards decide the layout of step 1; rendering the steps before the first preset list
  // arrives made the whole page jump when the cards appeared (CLS 0.20, PHASE32_PERFORMANCE).
  const presetsPending = presets.loading && !presets.data && !presets.error;
  const loadingInitial = (datasets.loading && !datasets.data) || fromLoading || (config === null && !datasets.error) || presetsPending;
  const noDatasets = datasets.data !== null && datasets.data !== undefined && datasets.data.length === 0;
  const ready = config !== null && datasets.data;
  const notReady = !ready ? 'Loading\u2026' : blocked ? 'Fix the invalid configuration first (step 3 lists it).' : submitting !== null ? 'Submitting\u2026' : null;

  const view = submission ? jobView(submission.result, job) : null;
  // The Run step reports the job only while the form still holds the submitted configuration.
  const steps = stepStates({
    origin: origin.type,
    edits: originChanges.length,
    problems: problemCount,
    submitted: view && submission?.configKey === JSON.stringify(config) ? view.phase : null,
  });

  const toolbar = (
    <nav className="lab-stepper" aria-label="Lab steps">
      <ol>
        {STEPS.map((s, i) => {
          const st = steps[s.id];
          return (
            <li key={s.id}>
              <button
                type="button"
                className={`lab-stepper-btn lab-stepper-${st.state}`}
                aria-current={ready && currentStep === s.id ? 'step' : undefined}
                onClick={() => scrollToStep(s.id)}
                disabled={!ready}
              >
                <span className="lab-stepper-num" aria-hidden="true">
                  {!ready ? i + 1 : st.state === 'done' ? <Check size={12} strokeWidth={3} /> : st.state === 'error' ? <TriangleAlert size={12} /> : i + 1}
                </span>
                <span>{s.short}</span>
                {ready ? <span className="lab-stepper-status">{st.status}</span> : null}
              </button>
            </li>
          );
        })}
      </ol>
      {ready && blocked ? (
        <button type="button" className="lab-fix" onClick={jumpToProblem} data-testid="lab-fix" title="Go to the first problem">
          <TriangleAlert size={14} aria-hidden="true" />
          <span>
            Fix {problemCount} problem{problemCount === 1 ? '' : 's'}
          </span>
        </button>
      ) : null}
    </nav>
  );

  const active = advancedSections.find((s) => s.id === activeSection) ?? advancedSections[0] ?? null;
  const latencyModel = changeValue(fieldsById.get('execution.order_latency.type'), JSON.stringify(config?.execution.order_latency.type ?? null));
  const strategyField = fieldsById.get(STRATEGY_FIELD);
  const synthetic = dataset?.kind === 'synthetic';
  const startName = origin.type === 'preset' ? `preset \u201c${origin.title}\u201d` : origin.type === 'run' ? `run ${origin.runId}` : 'starting values';
  const presetList = presets.data ?? [];
  const chips = useMemo(() => presetChips((presets.data ?? []).map((p) => p.config)), [presets.data]);
  const startChips = startConfig ? assumptionChips(startConfig).map((c) => ({ ...c, differs: false })) : [];

  // ----- step 1 --------------------------------------------------------------------------------

  const presetName = `${uid}-preset`;
  const stepChoose = config ? (
    <section className="lab-step" id="lab-step-choose" aria-labelledby="lab-step-choose-title">
      <StepHead
        n={1}
        id="lab-step-choose"
        title="Choose the experiment"
        lede="Pick the market data to replay and a starting point. A preset is a complete, valid configuration that asks one question; you can change any of it in step 2."
      />
      <div className="lab-fields">
        <div className="field">
          <label className="field-label" htmlFor="lab-dataset">
            Dataset
          </label>
          <div className="field-control">
            <select id="lab-dataset" value={datasetId ?? ''} onChange={(e) => changeDataset(e.target.value)}>
              {datasetId === null ? <option value="">none</option> : null}
              {dataset === null && datasetId !== null ? <option value={datasetId}>{datasetId} (not installed)</option> : null}
              {(datasets.data ?? []).map((d) => (
                <option key={d.id} value={d.id}>
                  {d.title} ({d.id}){d.kind === 'synthetic' ? ' \u2014 synthetic' : ''}
                </option>
              ))}
            </select>
          </div>
          <span className="field-help">
            {dataset ? (
              <>
                {synthetic ? (
                  <Badge kind="warn">synthetic {'\u2014'} generated, not market data</Badge>
                ) : (
                  <Badge kind="observed">recorded market data</Badge>
                )}{' '}
                {dataset.current_version ? (
                  <>
                    v{dataset.current_version.version}, tick <span className="mono">{dataset.current_version.instrument.tick_size}</span>, lot{' '}
                    <span className="mono">{dataset.current_version.instrument.lot_size}</span>.{' '}
                  </>
                ) : (
                  'No current version. '
                )}
                <Link to={paths.bookDataset(dataset.id)} className="lab-ds-link">
                  Open in Order Book Replay
                </Link>
              </>
            ) : (
              'This dataset is not installed on the server; a run on it will be rejected.'
            )}
          </span>
        </div>
        {strategyField ? <FieldRow field={strategyField} obj={config} onChange={edit} scope={SCOPE} /> : null}
      </div>

      <fieldset className="lab-presets">
        <legend className="lab-group-title">Starting point</legend>
        {presets.error ? <ErrorBox error={presets.error} onRetry={presets.reload} /> : null}
        {presets.loading ? <Spinner label="Loading presets" /> : null}
        <div className="lab-cards">
          {presetList.map((p, i) => {
            const checked = origin.type === 'preset' && origin.id === p.id;
            const t = `${uid}-p-${p.id}`;
            const kind = p.config.strategy.kind.type;
            const role = strategyRole(kind);
            const Icon = role ? ROLE_ICON[role] : null;
            return (
              <div key={p.id} className={checked ? 'lab-preset lab-preset-on' : 'lab-preset'} data-role={role ?? undefined}>
                <label className="lab-card" title={p.title}>
                  <span className="lab-card-top">
                    <input
                      type="radio"
                      name={presetName}
                      value={p.id}
                      checked={checked}
                      onChange={() => applyPreset(p)}
                      aria-labelledby={`${t}-t ${t}-k`}
                      aria-describedby={`${t}-q ${t}-a`}
                    />
                    <span className="lab-card-title" id={`${t}-t`}>
                      {presetShortTitle(p.title, kind)}
                    </span>
                    {checked ? <span className="lab-card-state">loaded</span> : null}
                  </span>
                  <span className="lab-card-kind" id={`${t}-k`}>
                    {Icon ? <Icon size={14} aria-hidden="true" /> : null}
                    <span>{strategyName(kind)}</span>
                    {role ? <span className="lab-card-role">{ROLE_LABEL[role]}</span> : null}
                  </span>
                  <span className="lab-card-q" id={`${t}-q`}>
                    <span className="lab-card-q-tag">Question</span> {p.demonstrates}
                  </span>
                  <ChipList id={`${t}-a`} chips={chips[i] ?? []} />
                </label>
                <details className="lab-card-more">
                  <summary>Details</summary>
                  <p className="lab-lede">{p.description}</p>
                  <p className="mono small wrap-anywhere">{assumptionsLabel(p.config)}</p>
                </details>
              </div>
            );
          })}
          <div className={origin.type === 'defaults' ? 'lab-preset lab-preset-on' : 'lab-preset'}>
            <label className="lab-card">
              <span className="lab-card-top">
                <input
                  type="radio"
                  name={presetName}
                  value=""
                  checked={origin.type === 'defaults'}
                  onChange={applyDefaults}
                  aria-labelledby={`${uid}-def-t`}
                  aria-describedby={`${uid}-def-d ${uid}-def-a`}
                />
                <span className="lab-card-title" id={`${uid}-def-t`}>
                  Starting values
                </span>
                {origin.type === 'defaults' ? <span className="lab-card-state">loaded</span> : null}
              </span>
              <span className="lab-card-kind">any strategy {'\u00b7'} not a preset</span>
              <span className="lab-card-q" id={`${uid}-def-d`}>
                Smallest valid sizes for the instrument and the most conservative execution assumptions.
              </span>
              <ChipList id={`${uid}-def-a`} chips={startChips} />
            </label>
          </div>
          {origin.type === 'run' ? (
            <div className="lab-preset lab-preset-on">
              <label className="lab-card">
                <span className="lab-card-top">
                  <input type="radio" name={presetName} value={origin.runId} checked readOnly aria-labelledby={`${uid}-run-t`} />
                  <span className="lab-card-title" id={`${uid}-run-t`}>
                    Run <span className="mono">{origin.runId}</span>
                  </span>
                  <span className="lab-card-state">loaded</span>
                </span>
                <span className="lab-card-q">The configuration of an earlier run, to change and run again.</span>
              </label>
            </div>
          ) : null}
        </div>
        {presets.data && presets.data.length === 0 ? (
          <p className="lab-lede">The server offers no presets for this dataset; start from the starting values and edit as needed.</p>
        ) : null}
      </fieldset>
    </section>
  ) : null;

  // ----- step 2 --------------------------------------------------------------------------------

  const guidedForm = config ? (
    <div className="lab-guided">
      {GUIDED_GROUPS.map((g) => {
        const rows = Object.entries(g.fields).flatMap(([id, help]) => {
          const f = fieldsById.get(id);
          return f ? [{ ...f, help }] : [];
        });
        const absent = rows.length === 0 ? g.absent(config, latencyModel) : '';
        return (
          <fieldset key={g.title} className="lab-group">
            <legend className="lab-group-title">
              <span>{g.title}</span>
              {g.modeled ? (
                <Badge kind="modeled" title={MODELED_TITLE}>
                  modeled
                </Badge>
              ) : null}
            </legend>
            <div className="lab-fields">
              {rows.map((f) => (
                <FieldRow key={f.id} field={f} obj={config} onChange={edit} scope={SCOPE} />
              ))}
            </div>
            {absent ? <p className="lab-lede">{absent}</p> : null}
          </fieldset>
        );
      })}
    </div>
  ) : null;

  const stepConfigure =
    config && active ? (
      <section className="lab-step" id="lab-step-configure" aria-labelledby="lab-step-configure-title">
        <StepHead
          n={2}
          id="lab-step-configure"
          title="Configure assumptions"
          lede={
            mode === 'guided'
              ? 'The settings that most change a result. Everything else keeps its current value; Advanced shows every setting of the same configuration.'
              : 'Every setting of the run configuration, by section. Guided shows the same values.'
          }
          aside={
            <div className="lab-mode" role="group" aria-label="Form mode">
              <button type="button" aria-pressed={mode === 'guided'} onClick={() => setMode('guided')}>
                Guided
              </button>
              <button type="button" aria-pressed={mode === 'advanced'} onClick={() => setMode('advanced')}>
                Advanced
              </button>
            </div>
          }
        />
        {mode === 'guided' ? (
          guidedForm
        ) : (
          <Tabs
            label="Configuration sections"
            className="lab-tabs"
            active={active.id}
            onChange={setActiveSection}
            tabs={advancedSections.map((s) => {
              const n = invalidBySection.get(s.id) ?? 0;
              return { id: s.id, label: s.title, count: n > 0 ? `${n} invalid` : undefined };
            })}
          >
            <fieldset className="lab-group">
              <legend className="lab-group-title">
                <span>{active.title}</span>
                {active.modeled ? (
                  <Badge kind="modeled" title={MODELED_TITLE}>
                    modeled
                  </Badge>
                ) : null}
              </legend>
              <p className="lab-lede">{active.description}</p>
              {active.id === 'strategy' ? (
                <p className="lab-lede">
                  Strategy: <strong>{strategyName(config.strategy.kind.type)}</strong> (chosen in step 1).
                </p>
              ) : null}
              <div className="lab-fields">
                {active.fields.map((f) => (
                  <FieldRow key={f.id} field={f} obj={config} onChange={edit} scope={SCOPE} />
                ))}
              </div>
              {active.id === 'strategy' && config.strategy.kind.type === 'scripted' ? (
                <ScriptedOrdersEditor
                  orders={config.strategy.kind.orders}
                  onChange={(orders) => edit({ ...config, strategy: { ...config.strategy, kind: { type: 'scripted', orders } } })}
                />
              ) : null}
            </fieldset>
          </Tabs>
        )}
      </section>
    ) : null;

  // ----- step 3 --------------------------------------------------------------------------------

  const referenceName = origin.type === 'preset' ? 'Preset' : origin.type === 'run' ? `Run ${origin.runId}` : 'Starting values';
  const stepReview = config ? (
    <section className="lab-step" id="lab-step-review" aria-labelledby="lab-step-review-title">
      <StepHead
        n={3}
        id="lab-step-review"
        title="Review changes"
        lede="Check what this experiment changes and that the configuration is valid. The browser applies the server's rules; the server checks again on submission."
      />
      <section className="lab-block" aria-label="Validation">
        <h3 className="lab-group-title">Validation</h3>
        {blocked ? (
          <div className="error-box" role="alert">
            <div className="error-box-head">
              <Badge kind="error">invalid configuration</Badge>
              <span>
                {problemCount} problem{problemCount === 1 ? '' : 's'} to fix before this can run
              </span>
            </div>
            <ul className="lab-problems">
              {fieldProblems.map((p) => (
                <li key={`f-${p.fieldId}`}>
                  <button type="button" className="btn-link" onClick={() => focusField(p.fieldId)}>
                    {p.text}
                  </button>
                  {problems?.rawByField.has(p.fieldId) ? (
                    <details className="lab-problem-raw">
                      <summary>Server rule</summary>
                      {problems.rawByField.get(p.fieldId)?.map((t) => (
                        <p key={t} className="mono">
                          {t}
                        </p>
                      ))}
                    </details>
                  ) : null}
                </li>
              ))}
              {ruleProblems.map((p, i) => (
                <li key={`r-${i}`}>{p}</li>
              ))}
            </ul>
          </div>
        ) : (
          <div className="row">
            <Pill tone="ok">valid</Pill>
            <span className="lab-lede">No errors. The server re-validates on submission.</span>
          </div>
        )}
        {advisories.length > 0 ? (
          <div className="notice" role="note">
            <span className="notice-tag">check</span>
            <div>
              {blocked ? 'Also worth checking:' : 'The configuration is valid, but:'}
              <ul>
                {advisories.map((a, i) => (
                  <li key={i}>{a}</li>
                ))}
              </ul>
            </div>
          </div>
        ) : null}
      </section>

      <section className="lab-block" aria-label="Changed vs preset">
        <h3 className="lab-group-title">
          {origin.type === 'preset' ? 'Changed vs preset' : origin.type === 'run' ? `Changed vs run ${origin.runId}` : 'Changed vs starting values'}
        </h3>
        {originChanges.length === 0 ? (
          <p className="lab-lede">
            No setting differs from the {startName}.{origin.type === 'preset' ? ' Running it as is reproduces the preset.' : ''}
          </p>
        ) : (
          <ChangeList
            caption={`${originChanges.length} setting${originChanges.length === 1 ? ' differs' : 's differ'} from the ${startName}.`}
            a={referenceName}
            b="This experiment"
            changes={originChanges}
            fields={fieldsById}
          />
        )}
      </section>

      <section className="lab-block" aria-label="Modeling assumptions">
        <h3 className="lab-group-title">
          <span>Modeling assumptions</span>
          <Badge kind="modeled" title={MODELED_TITLE}>
            modeled
          </Badge>
        </h3>
        <p className="lab-lede">
          How the simulator turns the strategy&apos;s orders into fills and costs. They are assumptions, not observations: every fill and fee
          in the results is an estimate under them.
        </p>
        <dl className="lab-assume">
          {assumptionRows(config, reference).map((r) => (
            <div key={r.key} className="lab-assume-row">
              <dt>
                {r.label}
                {r.changed ? (
                  <>
                    {' '}
                    <Badge kind="neutral" title={`Differs from the ${startName}`}>
                      changed
                    </Badge>
                  </>
                ) : null}
              </dt>
              <dd>
                <span className={r.key === 'queue' || r.key === 'fill' ? 'lab-assume-value' : 'mono'}>{r.value}</span>
                <span className="lab-assume-help">{r.help}</span>
              </dd>
            </div>
          ))}
        </dl>
      </section>

      <section className="lab-block" aria-label="Size estimate">
        <h3 className="lab-group-title">
          <span>Size estimate</span>
          <Badge kind="derived" title="Arithmetic over the dataset span and the evaluation interval; not a measurement.">
            derived
          </Badge>
        </h3>
        {estimate ? (
          <p className="lab-lede">
            About <span className="mono">{fmtInt(estimate.ticks)}</span> strategy evaluations, one every{' '}
            <span className="mono">{fmtDuration(config.strategy.interval_ns)}</span> across{' '}
            <span className="mono">{fmtDuration(estimate.spanNs)}</span> of data; the engine replays{' '}
            <span className="mono">{fmtInt(estimate.rows)}</span> recorded rows. The server returns its own estimate with the job.
          </p>
        ) : (
          <p className="lab-lede">Not available: the dataset has no current version or the interval is invalid.</p>
        )}
      </section>
    </section>
  ) : null;

  // ----- step 4 --------------------------------------------------------------------------------

  const est = submission?.result.experiment.estimate;
  const stepRun = config ? (
    <section className="lab-step" id="lab-step-run" aria-labelledby="lab-step-run-title">
      <StepHead n={4} id="lab-step-run" title="Run" lede="Name the run if you like, then submit it to the simulation queue." />
      <Notice tag="simulated">
        Results are simulated on a replay of the selected dataset: orders never reach an exchange and the simulation has no market impact.
        {synthetic ? ' This dataset is synthetic, not market data.' : ''}
      </Notice>
      <form
        className="lab-run"
        noValidate
        onSubmit={(e) => {
          e.preventDefault();
          void submit('run');
        }}
      >
        <div className="field lab-label-field">
          <label className="field-label" htmlFor="lab-label">
            Run label <span className="field-unit">optional</span>
          </label>
          <div className="field-control">
            <input id="lab-label" type="text" value={label} maxLength={120} onChange={(e) => setLabel(e.target.value)} />
          </div>
          <span className="field-help">A name for the run list, for example the scenario. Not part of any hash.</span>
        </div>
        <p className="lab-lede">
          Seed <span className="mono">{config.seed}</span>: the same seed, configuration and dataset reproduce the same result.{' '}
          <button type="button" className="btn-link" onClick={() => focusField('seed')}>
            Change seed
          </button>
        </p>
        <div className="lab-submit-row">
          <GatedButton
            className="btn-primary lab-submit"
            onClick={() => void submit('run')}
            blockedReason={notReady}
            title="Submit this configuration as an experiment job (Ctrl+Enter)"
            data-testid="lab-submit"
          >
            {submitting === 'run' ? 'Submitting' : 'Run simulation'}
          </GatedButton>
          {submitting !== null ? <Spinner label="Submitting the experiment" /> : null}
          <span className="lab-lede">
            {access.reasonFor('run') ?? notReady ?? 'Ctrl+Enter also submits. The same configuration returns the existing job or run instead of simulating twice.'}
          </span>
        </div>
      </form>
      {submitError !== null ? <ErrorBox error={submitError} /> : null}
      {submitNote ? (
        <p className="notice" role="status">
          {submitNote}
        </p>
      ) : null}

      <section className="lab-block" aria-label="Job and results">
        <h3 className="lab-group-title" ref={resultRef} tabIndex={-1}>
          {view ? 'Job and results' : 'What happens next'}
        </h3>
        {view && submission ? (
          <>
            <JobProgress view={view} />
            {submission.configKey !== JSON.stringify(config) ? (
              <p className="lab-lede">The form has changed since this submission; the job and links here are for the configuration that was submitted.</p>
            ) : null}
            <p className="lab-lede">
              Run id <span className="mono">{submission.result.experiment.run_id}</span>.
              {est ? (
                <>
                  {' '}
                  Server estimate: <span className="mono">{fmtInt(est.ticks)}</span> evaluations, at most{' '}
                  <span className="mono">{fmtInt(est.tape_events_upper)}</span> tape events (<span className="mono">{fmtBytes(est.output_bytes_upper)}</span>).
                </>
              ) : null}
            </p>
          </>
        ) : (
          <p className="lab-lede">
            Submitting queues a job for the worker: queued, then running, then completed. Once the run exists, its views open from here.
          </p>
        )}
        <RunLinks submission={submission} view={view} />
      </section>

      <details className="lab-compare" open={fromParam !== null || undefined}>
        <summary>Compare with{'\u2026'} an existing run (counterfactual)</summary>
        <section className="col" aria-label="Compare with">
          <p className="lab-lede">
            Pick an existing run as scenario A. &quot;Run as scenario B&quot; simulates this configuration so the two can be compared
            decision by decision. A comparison reports which decisions and outcomes diverged; it does not attribute a P&amp;L
            difference to a single parameter.
          </p>
          {runs.error ? <ErrorBox error={runs.error} onRetry={runs.reload} /> : null}
          <div className="field">
            <label className="field-label" htmlFor="lab-base-run">
              Scenario A
            </label>
            <div className="field-control">
              <select
                id="lab-base-run"
                value={baseRunId}
                onChange={(e) => {
                  setBaseRunId(e.target.value);
                  setSubmitNote(null);
                }}
              >
                <option value="">none</option>
                {baseRunId !== '' && !runs.data?.items.some((r) => r.run_id === baseRunId) ? <option value={baseRunId}>{baseRunId}</option> : null}
                {(runs.data?.items ?? []).map((r) => (
                  <option key={r.run_id} value={r.run_id}>
                    {runOptionLabel(r)}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="row">
            <button type="button" onClick={loadBaseRun} disabled={baseRun === null}>
              Load A&apos;s configuration into the form
            </button>
            {runs.loading || baseRunDetail.loading ? <Spinner label="Loading runs" /> : null}
            {runs.data?.incomplete ? <span className="lab-lede">first {runs.data.items.length} runs listed</span> : null}
          </div>
          <ErrorBox error={baseRunDetail.error} onRetry={baseRunDetail.reload} />
          {baseRun ? (
            <>
              <p className="mono small wrap-anywhere">A: {assumptionsLabel(baseRun.config)}</p>
              {changesVsA.length === 0 ? (
                <p className="notice">The form is identical to scenario A. Running it would return A itself.</p>
              ) : (
                <ChangeList
                  caption={`What scenario B changes relative to A (${changesVsA.length} setting${changesVsA.length === 1 ? '' : 's'}).`}
                  a="A"
                  b="B (this form)"
                  changes={changesVsA}
                  fields={fieldsById}
                />
              )}
            </>
          ) : null}
          <div className="row">
            <GatedButton
              onClick={() => void submit('scenario_b')}
              blockedReason={baseRunId === '' ? 'Pick a scenario A run first.' : notReady}
              data-testid="lab-submit-b"
            >
              {submitting === 'scenario_b' ? 'Submitting' : 'Run as scenario B'}
            </GatedButton>
            {baseRunId === '' ? (
              <span className="lab-lede">Scenario B needs a scenario A run selected above.</span>
            ) : (
              <span className="lab-lede mono">A = {baseRunId}</span>
            )}
          </div>
        </section>
      </details>

      {blocked ? null : (
        <JsonBlock
          summary="Request body (POST /api/v1/experiments, ns values and seed as decimal strings)"
          value={{ config: toWire.config(config), label: label.trim() === '' ? null : label.trim() }}
        />
      )}
    </section>
  ) : null;

  // ----- summary -------------------------------------------------------------------------------

  const latencySummary = config
    ? fieldsById.has('execution.order_latency.entry_ns')
      ? `${shownValue(fieldsById.get('execution.order_latency.entry_ns'), config)} / ${shownValue(fieldsById.get('execution.order_latency.response_ns'), config)}`
      : latencyModel
    : '';
  const summary = config ? (
    <aside className="lab-summary" aria-label="Experiment summary">
      <h2 className="lab-group-title">Experiment summary</h2>
      <dl className="lab-summary-list">
        <dt>Dataset</dt>
        <dd>
          {dataset?.title ?? datasetId}
          {synthetic ? (
            <>
              {' '}
              <Badge kind="warn">synthetic</Badge>
            </>
          ) : null}
        </dd>
        <dt>Strategy</dt>
        <dd>{strategyName(config.strategy.kind.type)}</dd>
        <dt>Starting point</dt>
        <dd>{origin.type === 'preset' ? origin.title : origin.type === 'run' ? <span className="mono">{origin.runId}</span> : 'Starting values'}</dd>
        <dt>{origin.type === 'preset' ? 'Changed vs preset' : origin.type === 'run' ? 'Changed vs run' : 'Changed vs starting values'}</dt>
        <dd>
          {originChanges.length === 0 ? (
            'nothing'
          ) : (
            <ul className="lab-summary-changes">
              {originChanges.slice(0, 4).map((c) => {
                const f = fieldsById.get(c.path);
                return (
                  <li key={c.path}>
                    <span title={c.path}>{f ? f.label : pathLabel(c.path)}</span>{' '}
                    <span className="mono">
                      {changeValue(f, c.a, c.path)} {'\u2192'} {changeValue(f, c.b, c.path)}
                    </span>
                  </li>
                );
              })}
              {originChanges.length > 4 ? <li>{`+${originChanges.length - 4} more (step 3)`}</li> : null}
            </ul>
          )}
        </dd>
        {fieldsById.has('strategy.kind.order_qty') ? (
          <>
            <dt>Order quantity</dt>
            <dd className="mono">{shownValue(fieldsById.get('strategy.kind.order_qty'), config)}</dd>
          </>
        ) : null}
        <dt>Latency entry / response</dt>
        <dd className="mono">{latencySummary}</dd>
        <dt>Queue model</dt>
        <dd>{queueModelLabel(config.execution.queue_model)}</dd>
        <dt>Fees maker / taker</dt>
        <dd className="mono">
          {shownValue(fieldsById.get('execution.fees.maker'), config)} / {shownValue(fieldsById.get('execution.fees.taker'), config)}
        </dd>
        <dt>Seed</dt>
        <dd className="mono">{config.seed}</dd>
      </dl>
      <p className="lab-summary-state">
        {blocked ? (
          <>
            <Badge kind="error">invalid</Badge> {problemCount} to fix
          </>
        ) : (
          <>
            <Pill tone="ok">valid</Pill> ready to run
          </>
        )}
      </p>
      <button type="button" className="btn-primary" onClick={blocked ? jumpToProblem : () => scrollToStep('lab-step-run')}>
        {blocked ? 'Fix first problem' : 'Go to Run'}
      </button>
      {view && submission ? (
        <p className="lab-lede">
          Last submission: run <span className="mono">{submission.result.experiment.run_id}</span> {'\u00b7'} {view.headline.toLowerCase()}
        </p>
      ) : null}
    </aside>
  ) : null;

  let body: ReactNode;
  if (datasets.error || fromRun.error) {
    body = (
      <div className="col pad">
        {datasets.error ? <ErrorBox error={datasets.error} onRetry={datasets.reload} /> : null}
        {fromRun.error ? <ErrorBox error={fromRun.error} onRetry={fromRun.reload} /> : null}
      </div>
    );
  } else if (noDatasets) {
    body = (
      <Empty>
        <p>No datasets are installed, so there is nothing to replay.</p>
        <p>
          Create one with <code>blackbox synth</code> or <code>blackbox ingest-tardis</code>; see <Link to={paths.datasets()}>Datasets</Link>.
        </p>
      </Empty>
    );
  } else if (loadingInitial || !ready) {
    body = <Spinner label="Loading datasets and presets" />;
  } else {
    body = (
      <div className="lab">
        <div className="lab-main">
          {stepChoose}
          {stepConfigure}
          {stepReview}
          {stepRun}
        </div>
        {summary}
      </div>
    );
  }

  return (
    <Workspace context={datasetId} toolbar={toolbar} scroll>
      {body}
    </Workspace>
  );
}
