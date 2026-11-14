// Strategy Lab (UI_DESIGN_SPEC.md §11.4): presets on the left, the configuration in tabs (one per
// section of `fields.ts`) in the centre, validation, the diff against the loaded values, the
// optional baseline run ("Compare with…") and the submission on the right. Submitting enqueues an
// experiment job (`POST /api/v1/experiments`) and opens it in Jobs & Sweeps; the same
// configuration returns the live job or the stored run instead of a duplicate.

import { useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';

import { api } from '../../api/client';
import { toWire } from '../../api/normalize';
import type { DatasetSummary, Run, RunDetail, SubmitExperimentResult } from '../../api/model';
import type { Preset, RunConfig } from '../../api/types';
import { DataTable } from '../../components/DataTable';
import type { Column } from '../../components/DataTable';
import { JsonBlock } from '../../components/data';
import { assumptionsLabel, STRATEGY_INFO } from '../../components/describe';
import { SplitPane } from '../../components/SplitPane';
import { Tabs } from '../../components/Tabs';
import { Badge, Empty, ErrorBox, Panel, Pill, Spinner, useAsync } from '../../components/ui';
import { GatedButton, useAccess } from '../../shell/access';
import { useAnnounce } from '../../shell/announce';
import { useInspector } from '../../shell/inspector';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { FieldRow } from './FieldInput';
import { ScriptedOrdersEditor } from './ScriptedOrdersEditor';
import { buildSections, fieldProblem, startingConfig } from './fields';
import type { Field } from './fields';
import { configAdvisories, diffConfigs, validateRunConfig } from './validate';
import type { ConfigChange } from './validate';
import './lab.css';

type SubmitMode = 'run' | 'scenario_b';

/** Where the values currently in the form came from. Shown so nothing looks like a preset by accident. */
type Origin =
  | { type: 'defaults' }
  | { type: 'preset'; id: string; title: string; loaded: RunConfig }
  | { type: 'run'; runId: string; loaded: RunConfig };

const FORM_ID = 'lab-form';

function originText(o: Origin, current: RunConfig | null): string {
  if (o.type === 'defaults') {
    return 'Starting values: smallest valid sizes for the instrument and conservative execution assumptions. They are form defaults to edit, not a preset.';
  }
  const source = o.type === 'preset' ? `preset "${o.title}"` : `run ${o.runId}`;
  const edits = current ? diffConfigs(o.loaded, current).length : 0;
  if (edits === 0) return `Values loaded from ${source}, unchanged.`;
  return `Values loaded from ${source}, then edited by you (${edits} field${edits === 1 ? ' differs' : 's differ'}).`;
}

/** The plain name of a strategy kind; unknown kinds (newer servers) show as sent. */
function strategyName(type: string | null): string {
  return type !== null && type in STRATEGY_INFO ? STRATEGY_INFO[type as keyof typeof STRATEGY_INFO].label : (type ?? 'unknown strategy');
}

function runOptionLabel(r: Run): string {
  const name = r.label ? `${r.label} ` : '';
  return `${name}${r.run_id} \u00b7 ${strategyName(r.strategy_kind)} \u00b7 ${r.dataset.id}`;
}

const PRESET_COLUMNS: Column<Preset>[] = [
  { id: 'title', header: 'Preset', rowHeader: true, cell: (p) => p.title, sortValue: (p) => p.title },
  {
    id: 'strategy',
    header: 'Strategy',
    cell: (p) => strategyName(p.config.strategy.kind.type),
    sortValue: (p) => strategyName(p.config.strategy.kind.type),
  },
];

type FormMode = 'guided' | 'advanced';
const MODE_STORAGE_KEY = 'bb.lab.mode';

/**
 * Guided mode: the essentials, each with a one-line plain explanation. The fields are the same
 * descriptors as the Advanced form (same ids, getters and setters on the same `RunConfig`); only
 * the explanation is shorter.
 */
const GUIDED_GROUPS: { title: string; modeled: boolean; fields: Record<string, string> }[] = [
  {
    title: 'Strategy',
    modeled: false,
    fields: {
      'strategy.kind.type': 'The rule that decides when and at what price to place orders.',
      'strategy.kind.order_qty': 'How much each order asks to buy or sell.',
    },
  },
  {
    title: 'Latency',
    modeled: true,
    fields: {
      'execution.order_latency.entry_ns': 'How long an order takes to reach the simulated exchange.',
      'execution.order_latency.response_ns': 'How long the exchange\u2019s reply takes to come back.',
    },
  },
  {
    title: 'Fees',
    modeled: true,
    fields: {
      'execution.fees.maker': 'Paid when an order that waited in the book is filled; negative is a rebate.',
      'execution.fees.taker': 'Paid when an order trades immediately against the book.',
    },
  },
];

/** A changed value in the unit the form uses (ms, bp, option names), or as stored when unknown. */
function changeValue(field: Field<RunConfig> | undefined, raw: string): string {
  if (!field || raw === '(absent)') return raw;
  if (field.kind === 'select') {
    const v: unknown = JSON.parse(raw);
    return field.options.find((o) => o.value === v)?.label ?? raw;
  }
  if (field.kind === 'number' || field.kind === 'optional_number') {
    const n = Number(raw);
    if (raw === 'null' || !Number.isFinite(n)) return raw === 'null' ? 'off' : raw;
    const shown = field.kind === 'number' && field.convert ? Number(field.convert.toDisplay(n).toPrecision(12)) : n;
    return field.unit ? `${shown} ${field.unit}` : String(shown);
  }
  return raw;
}

const MODELED_TITLE =
  'An execution assumption. Results produced under it are estimates of what could have happened, not observations.';

function PresetInspector({ preset, onApply }: { preset: Preset; onApply: (p: Preset) => void }) {
  useInspector(
    `Preset ${preset.title}`,
    <div className="col">
      <p className="prose">{preset.description}</p>
      <dl className="kv">
        <dt>Strategy</dt>
        <dd>{strategyName(preset.config.strategy.kind.type)}</dd>
        <dt>Demonstrates</dt>
        <dd className="prose">{preset.demonstrates}</dd>
        <dt>Execution assumptions</dt>
        <dd className="mono wrap-anywhere">{assumptionsLabel(preset.config)}</dd>
      </dl>
      <div className="row">
        <button type="button" onClick={() => onApply(preset)}>
          Load into form
        </button>
      </div>
    </div>,
    [preset, onApply],
  );
  return null;
}

function ChangesTable({
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
    <div className="table-scroll">
      <table className="table">
        <caption>{caption}</caption>
        <thead>
          <tr>
            <th scope="col">Setting</th>
            <th scope="col">{a}</th>
            <th scope="col">{b}</th>
          </tr>
        </thead>
        <tbody>
          {changes.map((c) => {
            const f = fields.get(c.path);
            return (
              <tr key={c.path}>
                <th scope="row">
                  {f ? (
                    <>
                      {f.label} <span className="note mono">{c.path}</span>
                    </>
                  ) : (
                    <span className="mono">{c.path}</span>
                  )}
                </th>
                <td className="mono">{changeValue(f, c.a)}</td>
                <td className="mono">{changeValue(f, c.b)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export default function LabPage() {
  const navigate = useNavigate();
  const announce = useAnnounce();
  const access = useAccess();
  const [searchParams] = useSearchParams();
  const datasetParam = searchParams.get('dataset');
  const fromParam = searchParams.get('from');

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
  const [activeSection, setActiveSection] = useState('strategy');
  const [selectedPresetId, setSelectedPresetId] = useState<string | null>(null);
  const [mode, setModeState] = useState<FormMode>(() =>
    window.localStorage.getItem(MODE_STORAGE_KEY) === 'advanced' ? 'advanced' : 'guided',
  );
  const setMode = (m: FormMode) => {
    setModeState(m);
    window.localStorage.setItem(MODE_STORAGE_KEY, m);
  };

  const presets = useAsync<Preset[]>(
    (signal) => (datasetId ? api.presets(datasetId, { signal }) : Promise.resolve([])),
    [datasetId],
  );

  const dataset: DatasetSummary | null =
    datasets.data?.find((d) => d.id === datasetId) ?? null;
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

  const problemsBySection = useMemo(() => {
    const out = new Map<string, string[]>();
    if (!config) return out;
    for (const s of sections) {
      const list: string[] = [];
      for (const f of s.fields) {
        const p = fieldProblem(f, config);
        if (p) list.push(`${s.title} \u203a ${f.label}: ${p}`);
      }
      out.set(s.id, list);
    }
    return out;
  }, [config, sections]);
  const fieldProblems = useMemo(() => [...problemsBySection.values()].flat(), [problemsBySection]);

  const ruleProblems = useMemo(() => (config ? validateRunConfig(config) : []), [config]);
  const advisories = useMemo(
    () => (config ? configAdvisories(config, instrument) : []),
    [config, instrument],
  );

  // List items carry no configuration; scenario A's comes from `GET /runs/{id}`.
  const baseRunDetail = useAsync<RunDetail | null>(
    (signal) =>
      baseRunId === '' ? Promise.resolve(null) : fromRun.data?.run_id === baseRunId ? Promise.resolve(fromRun.data) : api.run(baseRunId, { signal }),
    [baseRunId, fromRun.data],
  );
  const baseRun = baseRunDetail.data?.run_id === baseRunId ? baseRunDetail.data : null;
  const baseRunMatches = baseRun !== null;
  const changes = useMemo(
    () => (config && baseRun ? diffConfigs(baseRun.config, config) : []),
    [config, baseRun],
  );
  const originChanges = useMemo(
    () => (config && origin.type !== 'defaults' ? diffConfigs(origin.loaded, config) : []),
    [config, origin],
  );

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
  };

  const applyPreset = (p: Preset) => {
    // `api.presets(datasetId)` already rebound the preset to the selected dataset.
    edit(p.config);
    setOrigin({ type: 'preset', id: p.id, title: p.title, loaded: p.config });
    setSelectedPresetId(p.id);
  };

  const loadBaseRun = () => {
    if (!baseRun) return;
    setDatasetId(baseRun.config.dataset_id);
    edit(baseRun.config);
    setOrigin({ type: 'run', runId: baseRun.run_id, loaded: baseRun.config });
  };

  const blocked = fieldProblems.length > 0 || ruleProblems.length > 0;

  /** What the submission returned, in words (UI_DESIGN_SPEC §11.4: idempotence reported honestly). */
  const outcomeText = (r: SubmitExperimentResult): string => {
    if (r.run) return `Already simulated \u2014 stored run ${r.run.run_id} returned.`;
    if (r.job && !r.created) return `The same experiment is already ${r.job.state}: existing job ${r.job.id} returned (first label wins).`;
    if (r.job) return `Job ${r.job.id} ${r.job.state}${r.job.queue_position !== null ? `, position #${r.job.queue_position}` : ''}.`;
    return `Experiment ${r.experiment.id} is ${r.experiment.status}.`;
  };

  const submit = async (mode: SubmitMode) => {
    if (!config || blocked || submitting) return;
    if (!access.hasScope('run')) {
      if (access.mode !== 'research') access.openKeyDialog();
      return;
    }
    setSubmitting(mode);
    setSubmitError(null);
    setSubmitNote(null);
    try {
      const trimmed = label.trim();
      const result = await api.submitExperiment(config, trimmed === '' ? null : trimmed);
      const runId = result.experiment.run_id;
      const text = outcomeText(result);
      announce(text);
      if (mode === 'scenario_b' && runId === baseRunId) {
        setSubmitNote(
          `This configuration is the same experiment as scenario A (run ${runId}): identical inputs give an identical run. Change at least one assumption to get a scenario B.`,
        );
      } else if (mode === 'scenario_b' && result.run) {
        navigate(paths.comparePair(baseRunId, runId));
      } else if (mode === 'scenario_b' && result.job) {
        setSubmitNote(
          <>
            {text} Scenario B is run {runId}; when <Link to={paths.job(result.job.id)}>its job</Link> completes,{' '}
            <Link to={paths.comparePair(baseRunId, runId)}>compare A and B</Link>.
          </>,
        );
      } else if (result.job) {
        navigate(paths.job(result.job.id), { state: { submitted: text } });
      } else {
        navigate(paths.run(runId));
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

  const loadingInitial =
    (datasets.loading && !datasets.data) || fromLoading || (config === null && !datasets.error);
  const noDatasets = datasets.data !== null && datasets.data !== undefined && datasets.data.length === 0;
  const ready = config !== null && datasets.data;

  const notReady = !ready ? 'Loading\u2026' : blocked ? 'Fix the invalid configuration first.' : submitting !== null ? 'Submitting\u2026' : null;

  const toolbar = (
    <>
      <label className="lab-toolbar-label" htmlFor="lab-dataset">
        Dataset
      </label>
      <select
        id="lab-dataset"
        value={datasetId ?? ''}
        disabled={!ready}
        onChange={(e) => changeDataset(e.target.value)}
      >
        {datasetId === null ? <option value="">none</option> : null}
        {dataset === null && datasetId !== null ? <option value={datasetId}>{datasetId} (not installed)</option> : null}
        {(datasets.data ?? []).map((d) => (
          <option key={d.id} value={d.id}>
            {d.title} ({d.id}){d.kind === 'synthetic' ? ' \u2014 synthetic' : ''}
          </option>
        ))}
      </select>
      <label className="lab-toolbar-label" htmlFor="lab-preset">
        Preset
      </label>
      <select
        id="lab-preset"
        value={origin.type === 'preset' ? origin.id : ''}
        disabled={!ready || !presets.data || presets.data.length === 0}
        onChange={(e) => {
          const p = presets.data?.find((x) => x.id === e.target.value);
          if (p) applyPreset(p);
        }}
      >
        <option value="" disabled>
          {origin.type === 'run' ? `loaded from run ${origin.runId}` : 'none (starting values)'}
        </option>
        {(presets.data ?? []).map((p) => (
          <option key={p.id} value={p.id}>
            {p.title}
          </option>
        ))}
      </select>
      <GatedButton className="btn-primary" onClick={() => void submit('run')} blockedReason={notReady} title="Submit this configuration as an experiment job (Ctrl+Enter)" data-testid="lab-submit">
        {submitting === 'run' ? 'Submitting' : 'Run simulation'}
      </GatedButton>
      {submitting !== null ? <Spinner label="Submitting the experiment" /> : null}
    </>
  );

  const selectedPreset = presets.data?.find((p) => p.id === selectedPresetId) ?? null;
  const active = sections.find((s) => s.id === activeSection) ?? sections[0] ?? null;

  const presetsPane = (
    <Panel title="Presets" fill flush caption={config ? originText(origin, config) : undefined}>
      {presets.error ? <ErrorBox error={presets.error} onRetry={presets.reload} /> : null}
      {presets.loading ? <Spinner label="Loading presets" /> : null}
      {presets.data && presets.data.length === 0 ? (
        <Empty>
          The server offers no presets. The form starts from conservative defaults; edit every field as
          needed.
        </Empty>
      ) : null}
      {presets.data && presets.data.length > 0 ? (
        <DataTable
          label="Presets"
          fill
          rows={presets.data}
          rowKey={(p) => p.id}
          columns={PRESET_COLUMNS}
          selectedKey={selectedPresetId}
          onSelect={(p) => setSelectedPresetId(p.id)}
          onActivate={applyPreset}
          footer={<span className="note">Select a preset for its description; Enter or double click loads it into the form.</span>}
        />
      ) : null}
      {selectedPreset ? <PresetInspector preset={selectedPreset} onApply={applyPreset} /> : null}
    </Panel>
  );

  const modeToggle = (
    <div className="row lab-mode" role="group" aria-label="Form mode">
      <button type="button" aria-pressed={mode === 'guided'} className={mode === 'guided' ? 'btn-primary' : undefined} onClick={() => setMode('guided')}>
        Guided
      </button>
      <button type="button" aria-pressed={mode === 'advanced'} className={mode === 'advanced' ? 'btn-primary' : undefined} onClick={() => setMode('advanced')}>
        Advanced
      </button>
      <span className="note">
        {mode === 'guided'
          ? 'The essentials only. Every other setting keeps its current value; Advanced shows all of them.'
          : 'Every setting of the run configuration, by section.'}
      </span>
    </div>
  );

  const guidedForm = config ? (
    <div className="col">
      {GUIDED_GROUPS.map((g) => {
        const rows = Object.entries(g.fields).flatMap(([id, help]) => {
          const f = fieldsById.get(id);
          return f ? [{ ...f, help }] : [];
        });
        return (
          <fieldset key={g.title} className="col lab-section">
            <legend className="lab-section-head">
              <span>{g.title}</span>
              {g.modeled ? (
                <Badge kind="modeled" title={MODELED_TITLE}>
                  modeled
                </Badge>
              ) : null}
            </legend>
            <div className="lab-fields">
              {rows.map((f) => (
                <FieldRow key={f.id} field={f} obj={config} onChange={edit} scope="lab" />
              ))}
            </div>
            {g.title === 'Latency' && rows.length === 0 ? (
              <p className="note">
                This configuration uses the {changeValue(fieldsById.get('execution.order_latency.type'), JSON.stringify(config.execution.order_latency.type))} latency
                model; edit its parameters in Advanced.
              </p>
            ) : null}
            {g.title === 'Strategy' && config.strategy.kind.type === 'scripted' ? (
              <p className="note">A scripted strategy places a fixed list of orders; edit the list in Advanced.</p>
            ) : null}
          </fieldset>
        );
      })}
      {fieldProblems.length > 0 || ruleProblems.length > 0 ? (
        <p className="note">Some settings need fixing before this can run; the Validation panel lists them.</p>
      ) : null}
    </div>
  ) : null;

  const configPane = (
    <Panel title="Configuration" fill>
      {config && active ? (
        <form
          id={FORM_ID}
          className="lab-form"
          noValidate
          onSubmit={(e) => {
            e.preventDefault();
            void submit('run');
          }}
        >
          {modeToggle}
          {mode === 'guided' ? (
            guidedForm
          ) : (
            <Tabs
              label="Configuration sections"
              active={active.id}
              onChange={setActiveSection}
              tabs={sections.map((s) => {
                const n = problemsBySection.get(s.id)?.length ?? 0;
                return { id: s.id, label: s.title, count: n > 0 ? `${n} invalid` : undefined };
              })}
            >
              <fieldset className="col lab-section">
                <legend className="lab-section-head">
                  <span>{active.title}</span>
                  {active.modeled ? (
                    <Badge kind="modeled" title={MODELED_TITLE}>
                      modeled
                    </Badge>
                  ) : null}
                </legend>
                <p className="note">{active.description}</p>
                <div className="lab-fields">
                  {active.fields.map((f) => (
                    <FieldRow key={f.id} field={f} obj={config} onChange={edit} scope="lab" />
                  ))}
                </div>
                {active.id === 'strategy' && config.strategy.kind.type === 'scripted' ? (
                  <ScriptedOrdersEditor
                    orders={config.strategy.kind.orders}
                    onChange={(orders) =>
                      edit({
                        ...config,
                        strategy: { ...config.strategy, kind: { type: 'scripted', orders } },
                      })
                    }
                  />
                ) : null}
              </fieldset>
            </Tabs>
          )}
        </form>
      ) : null}
    </Panel>
  );

  const problemCount = fieldProblems.length + ruleProblems.length;
  const validationPane = config ? (
    <Panel title="Validation / diff" fill>
      <div className="col">
        <section className="col" aria-label="Validation">
          {blocked ? (
            <div className="error-box" role="alert">
              <div className="error-box-head">
                <Badge kind="error">invalid configuration</Badge>
                <span>
                  {problemCount} problem{problemCount === 1 ? '' : 's'} to fix before this can run
                </span>
              </div>
              <ul className="error-box-message">
                {fieldProblems.map((p) => (
                  <li key={`f-${p}`}>{p}</li>
                ))}
                {ruleProblems.map((p, i) => (
                  <li key={`r-${i}`}>{p}</li>
                ))}
              </ul>
            </div>
          ) : (
            <div className="row">
              <Pill tone="ok">valid</Pill>
              <span className="note">0 errors (checked in the browser; the server re-validates).</span>
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

        <section className="col" aria-label="Dataset">
          <h3>Dataset</h3>
          <p className="note">
            {dataset ? (
              <>
                {dataset.kind === 'synthetic' ? (
                  <Badge kind="warn">synthetic {'\u2014'} generated, not market data</Badge>
                ) : (
                  <Badge kind="observed">recorded market data</Badge>
                )}{' '}
                {dataset.current_version ? (
                  <>
                    v{dataset.current_version.version}, tick size <span className="mono">{dataset.current_version.instrument.tick_size}</span>, lot
                    size <span className="mono">{dataset.current_version.instrument.lot_size}</span>.{' '}
                  </>
                ) : (
                  'No current version. '
                )}
                <Link to={paths.bookDataset(dataset.id)}>Open in Order Book Replay</Link>
              </>
            ) : (
              'This dataset is not installed on the server; a run on it will be rejected.'
            )}
          </p>
        </section>

        <section className="col" aria-label="Changed vs preset">
          <h3>{origin.type === 'run' ? `Changed vs run ${origin.runId}` : 'Changed vs preset'}</h3>
          {origin.type === 'defaults' ? (
            <p className="note">No preset is loaded; the form holds starting values.</p>
          ) : originChanges.length === 0 ? (
            <p className="note">No field differs from the loaded values.</p>
          ) : (
            <ChangesTable
              caption={`${originChanges.length} field${originChanges.length === 1 ? '' : 's'} edited`}
              a="Loaded"
              b="This form"
              changes={originChanges}
              fields={fieldsById}
            />
          )}
        </section>

        <section className="col" aria-label="Compare with">
          <h3>Compare with{'\u2026'}</h3>
          <p className="note">
            To ask a counterfactual question, pick an existing run as scenario A. &quot;Run as scenario
            B&quot; then simulates this configuration and opens the comparison of A and B. A comparison
            reports which decisions and outcomes diverged; it does not attribute a P&amp;L difference to a
            single parameter.
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
                {baseRunId !== '' && !runs.data?.items.some((r) => r.run_id === baseRunId) ? (
                  <option value={baseRunId}>{baseRunId}</option>
                ) : null}
                {(runs.data?.items ?? []).map((r) => (
                  <option key={r.run_id} value={r.run_id}>
                    {runOptionLabel(r)}
                  </option>
                ))}
              </select>
            </div>
          </div>
          <div className="row">
            <button type="button" onClick={loadBaseRun} disabled={!baseRunMatches}>
              Load A&apos;s configuration into the form
            </button>
            {runs.loading || baseRunDetail.loading ? <Spinner label="Loading runs" /> : null}
            {runs.data?.incomplete ? <span className="note">first {runs.data.items.length} runs listed</span> : null}
          </div>
          <ErrorBox error={baseRunDetail.error} onRetry={baseRunDetail.reload} />
          {baseRunMatches && baseRun ? (
            <>
              <p className="mono small wrap-anywhere">A: {assumptionsLabel(baseRun.config)}</p>
              {changes.length === 0 ? (
                <p className="notice">The form is identical to scenario A. Running it would return A itself.</p>
              ) : (
                <ChangesTable
                  caption={`What scenario B changes relative to A (${changes.length} field${changes.length === 1 ? '' : 's'})`}
                  a="A"
                  b="B (this form)"
                  changes={changes}
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
              <span className="note">Scenario B needs a scenario A run selected above.</span>
            ) : (
              <span className="note mono">A = {baseRunId}</span>
            )}
          </div>
        </section>

        <section className="col" aria-label="Run">
          <h3>Run</h3>
          <div className="field">
            <label className="field-label" htmlFor="lab-label">
              Run label <span className="field-unit">optional</span>
            </label>
            <div className="field-control">
              <input
                id="lab-label"
                form={FORM_ID}
                type="text"
                value={label}
                maxLength={120}
                onChange={(e) => setLabel(e.target.value)}
              />
            </div>
            <span className="field-help">A name for the run list, for example the scenario. Not part of any hash.</span>
          </div>
          <p className="note">
            Seed: <span className="mono">{config.seed}</span> (edit it in Advanced, Seed tab).
          </p>
          {submitError !== null ? <ErrorBox error={submitError} /> : null}
          {submitNote ? (
            <p className="notice" role="status">
              {submitNote}
            </p>
          ) : null}
          <p className="note">
            Submitting enqueues an experiment job and opens it in Jobs &amp; Sweeps. It is idempotent: the
            same configuration returns the live job, or the stored run with the same run id and result hash, instead
            of simulating twice. Results are simulated on historical replay and are never exchange executions.
          </p>
          {blocked ? null : (
            <JsonBlock summary="Request body (POST /api/v1/experiments, ns values and seed as decimal strings)" value={{ config: toWire.config(config), label: label.trim() === '' ? null : label.trim() }} />
          )}
        </section>
      </div>
    </Panel>
  ) : (
    <Panel title="Validation / diff" fill />
  );

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
          Create one with <code>blackbox synth</code> or <code>blackbox ingest-tardis</code>; see{' '}
          <Link to={paths.datasets()}>Datasets</Link>.
        </p>
      </Empty>
    );
  } else if (loadingInitial || !ready) {
    body = <Spinner label="Loading datasets and presets" />;
  } else {
    body = (
      <SplitPane
        id="strategy.main"
        panes={[
          { id: 'presets', label: 'Presets', defaultSize: '22%', minSize: '14%', collapsible: true, content: presetsPane },
          { id: 'config', label: 'Configuration', defaultSize: '48%', minSize: '30%', content: configPane },
          { id: 'validation', label: 'Validation / diff', defaultSize: '30%', minSize: '18%', collapsible: true, content: validationPane },
        ]}
      />
    );
  }

  return (
    <Workspace context={datasetId} toolbar={toolbar} layout>
      {body}
    </Workspace>
  );
}
