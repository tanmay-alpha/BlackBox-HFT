// Strategy Lab (UI_DESIGN_SPEC.md §11.4): presets on the left, the configuration in tabs (one per
// section of `fields.ts`) in the centre, validation, the diff against the loaded values, the
// optional baseline run ("Compare with…") and the run request on the right.

import { useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';

import { api, isStaticViewer, STATIC_VIEWER_MESSAGE } from '../../api/client';
import type { DatasetManifest, Preset, RunConfig, RunSummary } from '../../api/types';
import { DataTable } from '../../components/DataTable';
import type { Column } from '../../components/DataTable';
import { JsonBlock } from '../../components/data';
import { assumptionsLabel, strategyLabel } from '../../components/describe';
import { SplitPane } from '../../components/SplitPane';
import { StaticViewerNote } from '../../components/staticViewer';
import { Tabs } from '../../components/Tabs';
import { Badge, Empty, ErrorBox, Panel, Pill, Spinner, useAsync } from '../../components/ui';
import { useInspector } from '../../shell/inspector';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { FieldRow } from './FieldInput';
import { ScriptedOrdersEditor } from './ScriptedOrdersEditor';
import { buildSections, fieldProblem, startingConfig } from './fields';
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

function runOptionLabel(r: RunSummary): string {
  const name = r.label ? `${r.label} ` : '';
  return `${name}${r.manifest.run_id} \u00b7 ${strategyLabel(r.manifest.config.strategy.kind)} \u00b7 ${r.manifest.dataset.id}`;
}

const PRESET_COLUMNS: Column<Preset>[] = [
  { id: 'title', header: 'Preset', rowHeader: true, cell: (p) => p.title, sortValue: (p) => p.title },
  {
    id: 'strategy',
    header: 'Strategy',
    cell: (p) => <span className="mono">{strategyLabel(p.config.strategy.kind)}</span>,
    sortValue: (p) => strategyLabel(p.config.strategy.kind),
  },
];

const MODELED_TITLE =
  'An execution assumption. Results produced under it are estimates of what could have happened, not observations.';

function PresetInspector({ preset, onApply }: { preset: Preset; onApply: (p: Preset) => void }) {
  useInspector(
    `Preset ${preset.title}`,
    <div className="col">
      <p className="prose">{preset.description}</p>
      <dl className="kv">
        <dt>Strategy</dt>
        <dd className="mono">{strategyLabel(preset.config.strategy.kind)}</dd>
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

function ChangesTable({ caption, a, b, changes }: { caption: string; a: string; b: string; changes: ConfigChange[] }) {
  return (
    <div className="table-scroll">
      <table className="table">
        <caption>{caption}</caption>
        <thead>
          <tr>
            <th scope="col">Configuration path</th>
            <th scope="col">{a}</th>
            <th scope="col">{b}</th>
          </tr>
        </thead>
        <tbody>
          {changes.map((c) => (
            <tr key={c.path}>
              <th scope="row" className="mono">
                {c.path}
              </th>
              <td className="mono">{c.a}</td>
              <td className="mono">{c.b}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function LabPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const datasetParam = searchParams.get('dataset');
  const fromParam = searchParams.get('from');

  const datasets = useAsync(() => api.datasets(), []);
  const runs = useAsync(() => api.runs(), []);
  const fromRun = useAsync<RunSummary | null>(
    () => (fromParam ? api.run(fromParam) : Promise.resolve(null)),
    [fromParam],
  );

  const [datasetId, setDatasetId] = useState<string | null>(null);
  const [config, setConfig] = useState<RunConfig | null>(null);
  const [origin, setOrigin] = useState<Origin>({ type: 'defaults' });
  const [baseRunId, setBaseRunId] = useState('');
  const [label, setLabel] = useState('');
  const [submitting, setSubmitting] = useState<SubmitMode | null>(null);
  const [submitError, setSubmitError] = useState<unknown>(null);
  const [submitNote, setSubmitNote] = useState<string | null>(null);
  const [activeSection, setActiveSection] = useState('strategy');
  const [selectedPresetId, setSelectedPresetId] = useState<string | null>(null);

  const presets = useAsync<Preset[]>(
    () => (datasetId ? api.presets(datasetId) : Promise.resolve([])),
    [datasetId],
  );

  const dataset: DatasetManifest | null =
    datasets.data?.find((d) => d.id === datasetId) ?? null;
  const instrument = dataset ? dataset.instrument : null;

  // Initial form contents, decided once: the run named by ?from=, else the dataset named by
  // ?dataset= (or the first installed one) with starting values.
  const fromLoading = fromParam !== null && fromRun.loading;
  useEffect(() => {
    if (config !== null || fromLoading) return;
    if (fromRun.data) {
      const run = fromRun.data;
      setDatasetId(run.manifest.config.dataset_id);
      setConfig(run.manifest.config);
      setOrigin({ type: 'run', runId: run.manifest.run_id, loaded: run.manifest.config });
      setBaseRunId(run.manifest.run_id);
      return;
    }
    const list = datasets.data;
    if (!list || list.length === 0) return;
    const chosen = list.find((d) => d.id === datasetParam) ?? list[0];
    setDatasetId(chosen.id);
    setConfig(startingConfig(chosen.id, chosen.instrument));
    setOrigin({ type: 'defaults' });
  }, [config, fromLoading, fromRun.data, datasets.data, datasetParam]);

  const sections = useMemo(
    () => (config ? buildSections(config, { instrument, presets: presets.data ?? [] }) : []),
    [config, instrument, presets.data],
  );

  const problemsBySection = useMemo(() => {
    const out = new Map<string, string[]>();
    if (!config) return out;
    for (const s of sections) {
      const list: string[] = [];
      for (const f of s.fields) {
        const p = fieldProblem(f, config);
        if (p) list.push(`${f.label} (${f.id}): ${p}`);
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

  const baseRun = runs.data?.find((r) => r.manifest.run_id === baseRunId) ?? fromRun.data ?? null;
  const baseRunMatches = baseRun !== null && baseRun.manifest.run_id === baseRunId;
  const changes = useMemo(
    () => (config && baseRunMatches && baseRun ? diffConfigs(baseRun.manifest.config, config) : []),
    [config, baseRun, baseRunMatches],
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
    setConfig((c) => (c ? { ...c, dataset_id: id } : startingConfig(id, ds ? ds.instrument : null)));
  };

  const applyPreset = (p: Preset) => {
    // `api.presets(datasetId)` already rebound the preset to the selected dataset.
    edit(p.config);
    setOrigin({ type: 'preset', id: p.id, title: p.title, loaded: p.config });
    setSelectedPresetId(p.id);
  };

  const loadBaseRun = () => {
    if (!baseRun) return;
    setDatasetId(baseRun.manifest.config.dataset_id);
    edit(baseRun.manifest.config);
    setOrigin({ type: 'run', runId: baseRun.manifest.run_id, loaded: baseRun.manifest.config });
  };

  const blocked = fieldProblems.length > 0 || ruleProblems.length > 0;
  // The hosted read-only viewer cannot simulate; the button title and the run panel say so.
  const readOnly = isStaticViewer();

  const submit = async (mode: SubmitMode) => {
    if (!config || blocked || readOnly || submitting) return;
    setSubmitting(mode);
    setSubmitError(null);
    setSubmitNote(null);
    try {
      const trimmed = label.trim();
      const summary = await api.createRun({ config, label: trimmed === '' ? null : trimmed });
      const runId = summary.manifest.run_id;
      if (mode === 'run') {
        navigate(paths.run(runId));
      } else if (runId === baseRunId) {
        setSubmitNote(
          `This configuration is the same experiment as scenario A (run ${runId}): identical inputs give an identical run. Change at least one assumption to get a scenario B.`,
        );
      } else {
        navigate(paths.comparePair(baseRunId, runId));
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

  const runTitle = readOnly
    ? STATIC_VIEWER_MESSAGE
    : blocked
      ? 'Fix the invalid configuration first.'
      : 'Simulate this configuration (Ctrl+Enter)';

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
      <button
        type="submit"
        form={FORM_ID}
        className="btn-primary"
        disabled={!ready || blocked || readOnly || submitting !== null}
        title={runTitle}
      >
        {submitting === 'run' ? 'Simulating' : 'Run simulation'}
      </button>
      {submitting !== null ? <Spinner label="Replaying the dataset" /> : null}
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
                tick size <span className="mono">{dataset.instrument.tick_size}</span>, lot size{' '}
                <span className="mono">{dataset.instrument.lot_size}</span>.{' '}
                <Link to={paths.bookDataset(dataset.id)}>Open in Order Book Lab</Link>
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
                {baseRunId !== '' && !runs.data?.some((r) => r.manifest.run_id === baseRunId) ? (
                  <option value={baseRunId}>{baseRunId}</option>
                ) : null}
                {(runs.data ?? []).map((r) => (
                  <option key={r.manifest.run_id} value={r.manifest.run_id}>
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
            {runs.loading ? <Spinner label="Loading runs" /> : null}
          </div>
          {baseRunMatches && baseRun ? (
            <>
              <p className="mono small wrap-anywhere">A: {assumptionsLabel(baseRun.manifest.config)}</p>
              {changes.length === 0 ? (
                <p className="notice">The form is identical to scenario A. Running it would return A itself.</p>
              ) : (
                <ChangesTable
                  caption={`What scenario B changes relative to A (${changes.length} field${changes.length === 1 ? '' : 's'})`}
                  a="A"
                  b="B (this form)"
                  changes={changes}
                />
              )}
            </>
          ) : null}
          <div className="row">
            <button
              type="button"
              disabled={blocked || readOnly || submitting !== null || baseRunId === ''}
              onClick={() => void submit('scenario_b')}
              title={readOnly ? STATIC_VIEWER_MESSAGE : baseRunId === '' ? 'Pick a scenario A run first.' : undefined}
            >
              {submitting === 'scenario_b' ? 'Simulating' : 'Run as scenario B'}
            </button>
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
            Seed: <span className="mono">{config.seed}</span> (edit it in the Seed tab).
          </p>
          {submitError !== null ? <ErrorBox error={submitError} /> : null}
          {submitNote ? (
            <p className="notice" role="status">
              {submitNote}
            </p>
          ) : null}
          <StaticViewerNote />
          <p className="note">
            The run is synchronous and idempotent: a configuration already simulated on this dataset
            returns the stored run with the same run id and result hash. Results are simulated on
            historical replay and are never exchange executions.
          </p>
          {blocked ? null : <JsonBlock summary="Request body (RunConfig as it will be sent)" value={config} />}
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
          <Link to={paths.datasets()}>Market Data Explorer</Link>.
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
