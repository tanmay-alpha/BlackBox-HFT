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

import { api } from '../../api/client';
import { toWire } from '../../api/normalize';
import type { DatasetSummary, Run, RunDetail, SubmitExperimentResult } from '../../api/model';
import type { Preset, RunConfig } from '../../api/types';
import { JsonBlock } from '../../components/data';
import { assumptionsLabel, STRATEGY_INFO } from '../../components/describe';
import { fmtBytes, fmtDuration, fmtInt } from '../../components/fmt';
import { Tabs } from '../../components/Tabs';
import { Badge, Empty, ErrorBox, Notice, Pill, Spinner, useAsync } from '../../components/ui';
import { GatedButton, useAccess } from '../../shell/access';
import { useAnnounce } from '../../shell/announce';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { FieldRow, fieldDomId } from './FieldInput';
import { ScriptedOrdersEditor } from './ScriptedOrdersEditor';
import { buildSections, fieldProblem, startingConfig } from './fields';
import type { Field, Section } from './fields';
import { firstSentence, sizeEstimate, whyItMatters } from './review';
import { configAdvisories, diffConfigs, ruleFieldPaths, validateRunConfig } from './validate';
import type { ConfigChange } from './validate';
import './lab.css';

type SubmitMode = 'run' | 'scenario_b';
type FormMode = 'guided' | 'advanced';
type Job = NonNullable<SubmitExperimentResult['job']>;

/** Where the values currently in the form came from. Shown so nothing looks like a preset by accident. */
type Origin =
  | { type: 'defaults' }
  | { type: 'preset'; id: string; title: string; loaded: RunConfig }
  | { type: 'run'; runId: string; loaded: RunConfig };

/** The last accepted submission, kept on the page so the next steps stay one click away. */
interface Submission {
  result: SubmitExperimentResult;
  text: string;
  /** Scenario A of a counterfactual submission. */
  scenarioA: string | null;
}

const MODE_STORAGE_KEY = 'bb.lab.mode';
const SCOPE = 'lab';
const STRATEGY_FIELD = 'strategy.kind.type';
const TERMINAL: Record<string, true> = { completed: true, failed: true, cancelled: true, expired: true };

const STEPS = [
  { id: 'lab-step-choose', short: 'Choose' },
  { id: 'lab-step-configure', short: 'Configure' },
  { id: 'lab-step-review', short: 'Review' },
  { id: 'lab-step-run', short: 'Run' },
] as const;

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

/** A changed value in the unit the form uses (ms, bp, option names), or as stored when unknown. */
function changeValue(field: Field<RunConfig> | undefined, raw: string, path = ''): string {
  if (raw === '(absent)') return raw;
  if (!field) {
    const n = Number(raw);
    return path.endsWith('_ns') && Number.isFinite(n) ? fmtDuration(n) : raw;
  }
  if (field.kind === 'select') {
    const v: unknown = JSON.parse(raw);
    return field.options.find((o) => o.value === v)?.label ?? raw;
  }
  if (field.kind === 'number' || field.kind === 'optional_number') {
    const n = Number(raw);
    if (raw === 'null' || !Number.isFinite(n)) return raw !== 'null' ? raw : field.kind === 'optional_number' ? 'off' : 'blank';
    const shown = field.kind === 'number' && field.convert ? Number(field.convert.toDisplay(n).toPrecision(12)) : n;
    return field.unit ? `${shown} ${field.unit}` : String(shown);
  }
  return raw;
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

/** Changed settings as a list (readable on a phone): label, old → new in plain units, why it matters. */
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
          return (
            <li key={c.path} className="lab-change">
              <div className="lab-change-head">
                <span className="lab-change-label">{f ? f.label : c.path}</span>
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
                </span>
              </div>
              <p className="lab-change-why">{whyItMatters(c.path)}</p>
              {f ? <span className="lab-change-path mono">{c.path}</span> : null}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/** Job → Results → Replay → Forensics → Risk → Compare; links once a run exists. */
function NextSteps({ submission, job: polled }: { submission: Submission | null; job: Job | null }) {
  // The submit response names the job; the poll only refreshes it (until the first poll lands,
  // or while an earlier submission's job is still in state, the response's own job is shown).
  const submitted = submission?.result.job ?? null;
  const job = submitted && polled?.id === submitted.id ? polled : submitted;
  const runId = submission?.result.experiment.run_id ?? null;
  const ready = submission !== null && (submission.result.run !== null || job?.state === 'completed');
  const failed = job !== null && TERMINAL[job.state] === true && job.state !== 'completed';
  const pending = failed ? `not available: the job ${job?.state ?? 'ended'}` : 'available when the job completes';
  const items: { key: string; label: string; what: string; to: string | null }[] = [
    { key: 'results', label: 'Results', what: 'the run\u2019s report: measured outcomes and the reproducibility manifest', to: runId ? paths.report(runId) : null },
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
    <ol className="lab-next">
      <li className="lab-next-item">
        <span className="lab-next-name">Job</span>
        {submission === null ? (
          <span className="lab-next-what">queued on the worker; its page shows progress and can cancel it</span>
        ) : job ? (
          <span className="lab-next-what">
            <Link to={paths.job(job.id)} data-testid="lab-next-job">
              Job <span className="mono">{job.id}</span>
            </Link>{' '}
            <Pill tone={job.state === 'completed' ? 'ok' : failed ? 'error' : 'info'}>{job.state}</Pill>
            {job.queue_position !== null && !TERMINAL[job.state] ? <span className="mono"> #{job.queue_position} in queue</span> : null}
          </span>
        ) : submission.result.run !== null ? (
          <span className="lab-next-what">not needed: this configuration was already simulated</span>
        ) : (
          <span className="lab-next-what">no job was returned for this submission</span>
        )}
      </li>
      {items.map((it) => (
        <li key={it.key} className="lab-next-item">
          <span className="lab-next-name">{it.label}</span>
          <span className="lab-next-what">
            {ready && it.to ? (
              <Link to={it.to} data-testid={`lab-next-${it.key}`}>
                Open {it.label.toLowerCase()}
              </Link>
            ) : null}{' '}
            {it.what}
            {submission !== null && !ready ? <span className="lab-next-pending"> ({pending})</span> : null}
          </span>
        </li>
      ))}
    </ol>
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

  const fieldProblems = useMemo(() => {
    const out: { sectionId: string; fieldId: string; text: string }[] = [];
    if (!config) return out;
    for (const s of sections) {
      for (const f of s.fields) {
        const p = fieldProblem(f, config);
        if (p) out.push({ sectionId: s.id, fieldId: f.id, text: `${s.title} \u203a ${f.label}: ${p}` });
      }
    }
    return out;
  }, [config, sections]);
  const invalidBySection = useMemo(() => {
    const m = new Map<string, number>();
    for (const p of fieldProblems) m.set(p.sectionId, (m.get(p.sectionId) ?? 0) + 1);
    return m;
  }, [fieldProblems]);

  // A server rule about a field that already shows its own problem is the same mistake: keep the
  // field's message (display units) and attach the rule's raw text to it instead of counting twice.
  const { ruleProblems, rawByField } = useMemo(() => {
    const rules: string[] = [];
    const raw = new Map<string, string[]>();
    const flagged = new Set(fieldProblems.map((p) => p.fieldId));
    for (const text of config ? validateRunConfig(config) : []) {
      const hit = ruleFieldPaths(text).filter((id) => flagged.has(id));
      if (hit.length === 0) rules.push(text);
      for (const id of hit) raw.set(id, [...(raw.get(id) ?? []), text]);
    }
    return { ruleProblems: rules, rawByField: raw };
  }, [config, fieldProblems]);
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
  const reference = useMemo(
    () => (origin.type !== 'defaults' ? origin.loaded : datasetId ? startingConfig(datasetId, instrument) : null),
    [origin, datasetId, instrument],
  );
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

  const blocked = fieldProblems.length > 0 || ruleProblems.length > 0;
  const problemCount = fieldProblems.length + ruleProblems.length;

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
      const text = outcomeText(result);
      announce(text);
      if (how === 'scenario_b' && runId === baseRunId) {
        setSubmitNote(
          `This configuration is the same experiment as scenario A (run ${runId}): identical inputs give an identical run. Change at least one assumption to get a scenario B.`,
        );
      } else {
        setSubmission({ result, text, scenarioA: how === 'scenario_b' ? baseRunId : null });
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

  const loadingInitial = (datasets.loading && !datasets.data) || fromLoading || (config === null && !datasets.error);
  const noDatasets = datasets.data !== null && datasets.data !== undefined && datasets.data.length === 0;
  const ready = config !== null && datasets.data;
  const notReady = !ready ? 'Loading\u2026' : blocked ? 'Fix the invalid configuration first (step 3 lists it).' : submitting !== null ? 'Submitting\u2026' : null;

  const stepStatus: Record<(typeof STEPS)[number]['id'], ReactNode> = {
    'lab-step-choose': origin.type === 'preset' ? 'preset' : origin.type === 'run' ? 'from run' : 'defaults',
    'lab-step-configure': `${originChanges.length} edit${originChanges.length === 1 ? '' : 's'}`,
    'lab-step-review': blocked ? `${problemCount} to fix` : 'valid',
    'lab-step-run': submission ? 'submitted' : blocked ? 'blocked' : 'ready',
  };

  const toolbar = (
    <nav className="lab-stepper" aria-label="Lab steps">
      <ol>
        {STEPS.map((s, i) => (
          <li key={s.id}>
            <button
              type="button"
              className={s.id === 'lab-step-review' && blocked ? 'lab-stepper-btn lab-stepper-bad' : 'lab-stepper-btn'}
              onClick={() => scrollToStep(s.id)}
              disabled={!ready}
            >
              <span className="lab-stepper-num" aria-hidden="true">
                {i + 1}
              </span>
              <span>{s.short}</span>
              {ready ? <span className="lab-stepper-status">{stepStatus[s.id]}</span> : null}
            </button>
          </li>
        ))}
      </ol>
    </nav>
  );

  const selectedPreset = origin.type === 'preset' ? (presets.data?.find((p) => p.id === origin.id) ?? null) : null;
  const active = advancedSections.find((s) => s.id === activeSection) ?? advancedSections[0] ?? null;
  const latencyModel = changeValue(fieldsById.get('execution.order_latency.type'), JSON.stringify(config?.execution.order_latency.type ?? null));
  const strategyField = fieldsById.get(STRATEGY_FIELD);
  const synthetic = dataset?.kind === 'synthetic';
  const startName = origin.type === 'preset' ? `preset \u201c${origin.title}\u201d` : origin.type === 'run' ? `run ${origin.runId}` : 'starting values';

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
                <Link to={paths.bookDataset(dataset.id)}>Open in Order Book Replay</Link>
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
          {(presets.data ?? []).map((p) => {
            const checked = origin.type === 'preset' && origin.id === p.id;
            const t = `${uid}-p-${p.id}`;
            return (
              <label key={p.id} className={checked ? 'lab-card lab-card-on' : 'lab-card'}>
                <span className="lab-card-top">
                  <input
                    type="radio"
                    name={presetName}
                    value={p.id}
                    checked={checked}
                    onChange={() => applyPreset(p)}
                    aria-labelledby={`${t}-t`}
                    aria-describedby={`${t}-d ${t}-q`}
                  />
                  <span className="lab-card-title" id={`${t}-t`}>
                    {p.title}
                  </span>
                  {checked ? <span className="lab-card-state">loaded</span> : null}
                </span>
                <span className="lab-card-kind">{strategyName(p.config.strategy.kind.type)}</span>
                <span className="lab-card-desc" id={`${t}-d`}>
                  {firstSentence(p.description)}
                </span>
                <span className="lab-card-q" id={`${t}-q`}>
                  <span className="lab-card-q-tag">Question</span> {p.demonstrates}
                </span>
              </label>
            );
          })}
          <label className={origin.type === 'defaults' ? 'lab-card lab-card-on' : 'lab-card'}>
            <span className="lab-card-top">
              <input
                type="radio"
                name={presetName}
                value=""
                checked={origin.type === 'defaults'}
                onChange={applyDefaults}
                aria-labelledby={`${uid}-def-t`}
                aria-describedby={`${uid}-def-d`}
              />
              <span className="lab-card-title" id={`${uid}-def-t`}>
                Starting values
              </span>
              {origin.type === 'defaults' ? <span className="lab-card-state">loaded</span> : null}
            </span>
            <span className="lab-card-kind">any strategy</span>
            <span className="lab-card-desc" id={`${uid}-def-d`}>
              Smallest valid sizes for the instrument and the most conservative execution assumptions; not a preset.
            </span>
          </label>
          {origin.type === 'run' ? (
            <label className="lab-card lab-card-on">
              <span className="lab-card-top">
                <input type="radio" name={presetName} value={origin.runId} checked readOnly aria-labelledby={`${uid}-run-t`} />
                <span className="lab-card-title" id={`${uid}-run-t`}>
                  Run <span className="mono">{origin.runId}</span>
                </span>
                <span className="lab-card-state">loaded</span>
              </span>
              <span className="lab-card-desc">The configuration of an earlier run, to change and run again.</span>
            </label>
          ) : null}
        </div>
        {presets.data && presets.data.length === 0 ? (
          <p className="lab-lede">The server offers no presets for this dataset; start from the starting values and edit as needed.</p>
        ) : null}
        {selectedPreset ? (
          <details className="lab-preset-more">
            <summary>About {selectedPreset.title}</summary>
            <p className="prose">{selectedPreset.description}</p>
            <p className="mono small wrap-anywhere">{assumptionsLabel(selectedPreset.config)}</p>
          </details>
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
                  {rawByField.has(p.fieldId) ? (
                    <details className="lab-problem-raw">
                      <summary>Server rule</summary>
                      {rawByField.get(p.fieldId)?.map((t) => (
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
        Results are simulated on historical replay: orders never reach an exchange and the simulation has no market impact.
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

      <section className="lab-block" aria-label="Next steps">
        <h3 className="lab-group-title" ref={resultRef} tabIndex={-1}>
          {submission ? 'Submitted' : 'What happens next'}
        </h3>
        {submission ? (
          <p className="lab-lede" role="status">
            At submission: {submission.text}
            {submission.result.run ? null : (
              <>
                {' '}
                Run id <span className="mono">{submission.result.experiment.run_id}</span>.
              </>
            )}
            {est ? (
              <>
                {' '}
                Server estimate: <span className="mono">{fmtInt(est.ticks)}</span> evaluations, at most{' '}
                <span className="mono">{fmtInt(est.tape_events_upper)}</span> tape events (<span className="mono">{fmtBytes(est.output_bytes_upper)}</span>).
              </>
            ) : null}
          </p>
        ) : null}
        <NextSteps submission={submission} job={job} />
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
        <dt>Edits</dt>
        <dd className="mono">{originChanges.length}</dd>
        {fieldsById.has('strategy.kind.order_qty') ? (
          <>
            <dt>Order quantity</dt>
            <dd className="mono">{shownValue(fieldsById.get('strategy.kind.order_qty'), config)}</dd>
          </>
        ) : null}
        <dt>Latency entry / response</dt>
        <dd className="mono">{latencySummary}</dd>
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
      <button type="button" className="btn-primary" onClick={() => scrollToStep(blocked ? 'lab-step-review' : 'lab-step-run')}>
        {blocked ? 'Review problems' : 'Go to Run'}
      </button>
      {submission ? (
        <p className="lab-lede">
          Last submission: run <span className="mono">{submission.result.experiment.run_id}</span>
          {job ? ` \u00b7 job ${job.state}` : ''}
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
