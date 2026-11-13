import { useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';

import { api } from '../../api/client';
import type { DatasetManifest, Preset, RunConfig, RunSummary } from '../../api/types';
import { JsonBlock } from '../../components/data';
import { assumptionsLabel, strategyLabel } from '../../components/describe';
import { PageHeader, usePageTitle } from '../../components/page';
import { Badge, Empty, ErrorBox, Panel, Spinner, useAsync } from '../../components/ui';
import { FieldRow } from './FieldInput';
import { ScriptedOrdersEditor } from './ScriptedOrdersEditor';
import { buildSections, fieldProblem, startingConfig } from './fields';
import type { Section } from './fields';
import { configAdvisories, diffConfigs, validateRunConfig } from './validate';

type SubmitMode = 'run' | 'scenario_b';

/** Where the values currently in the form came from. Shown so nothing looks like a preset by accident. */
type Origin =
  | { type: 'defaults' }
  | { type: 'preset'; title: string; loaded: RunConfig }
  | { type: 'run'; runId: string; loaded: RunConfig };

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

function PresetList({
  presets,
  onApply,
}: {
  presets: Preset[];
  onApply: (p: Preset) => void;
}) {
  if (presets.length === 0) {
    return (
      <Empty>
        The server offers no presets. The form below starts from conservative defaults; edit every
        field as needed.
      </Empty>
    );
  }
  return (
    <div className="table-scroll">
      <table className="table">
        <thead>
          <tr>
            <th scope="col">Preset</th>
            <th scope="col">Strategy</th>
            <th scope="col">Demonstrates</th>
            <th scope="col">Execution assumptions</th>
            <th scope="col">
              <span className="visually-hidden">Apply</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {presets.map((p) => (
            <tr key={p.id}>
              <th scope="row" className="wrap">
                <span className="preset-title">{p.title}</span>
                <br />
                <span className="dim">{p.description}</span>
              </th>
              <td className="mono">{strategyLabel(p.config.strategy.kind)}</td>
              <td className="wrap">{p.demonstrates}</td>
              <td className="wrap mono">{assumptionsLabel(p.config)}</td>
              <td>
                <button type="button" onClick={() => onApply(p)}>
                  Load into form
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function SectionPanel({
  section,
  config,
  onChange,
  children,
}: {
  section: Section;
  config: RunConfig;
  onChange: (next: RunConfig) => void;
  children?: ReactNode;
}) {
  return (
    <Panel
      title={
        <>
          {section.title}
          {section.modeled ? (
            <Badge
              kind="modeled"
              title="An execution assumption. Results produced under it are estimates of what could have happened, not observations."
            >
              modeled
            </Badge>
          ) : null}
        </>
      }
    >
      <fieldset className="col">
        <legend className="note">{section.description}</legend>
        <div className="lab-fields">
          {section.fields.map((f) => (
            <FieldRow key={f.id} field={f} obj={config} onChange={onChange} scope="lab" />
          ))}
        </div>
        {children}
      </fieldset>
    </Panel>
  );
}

export default function LabPage() {
  usePageTitle('Strategy Lab');
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

  const fieldProblems = useMemo(() => {
    if (!config) return [];
    const out: string[] = [];
    for (const s of sections) {
      for (const f of s.fields) {
        const p = fieldProblem(f, config);
        if (p) out.push(`${f.label} (${f.id}): ${p}`);
      }
    }
    return out;
  }, [config, sections]);

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
    setOrigin({ type: 'preset', title: p.title, loaded: p.config });
  };

  const loadBaseRun = () => {
    if (!baseRun) return;
    setDatasetId(baseRun.manifest.config.dataset_id);
    edit(baseRun.manifest.config);
    setOrigin({ type: 'run', runId: baseRun.manifest.run_id, loaded: baseRun.manifest.config });
  };

  const blocked = fieldProblems.length > 0 || ruleProblems.length > 0;

  const submit = async (mode: SubmitMode) => {
    if (!config || blocked || submitting) return;
    setSubmitting(mode);
    setSubmitError(null);
    setSubmitNote(null);
    try {
      const trimmed = label.trim();
      const summary = await api.createRun({ config, label: trimmed === '' ? null : trimmed });
      const runId = summary.manifest.run_id;
      if (mode === 'run') {
        navigate(`/runs/${encodeURIComponent(runId)}`);
      } else if (runId === baseRunId) {
        setSubmitNote(
          `This configuration is the same experiment as scenario A (run ${runId}): identical inputs give an identical run. Change at least one assumption to get a scenario B.`,
        );
      } else {
        navigate(`/compare?a=${encodeURIComponent(baseRunId)}&b=${encodeURIComponent(runId)}`);
      }
    } catch (e) {
      setSubmitError(e);
    } finally {
      setSubmitting(null);
    }
  };

  const loadingInitial =
    (datasets.loading && !datasets.data) || fromLoading || (config === null && !datasets.error);

  return (
    <div className="col">
      <PageHeader
        title="Strategy Laboratory"
        lead="Configure one simulated run: a strategy, and the execution assumptions it is replayed under. Every assumption marked modeled is an estimate you choose, and comparing two choices is how you learn which results depend on it."
      />

      {datasets.error ? <ErrorBox error={datasets.error} /> : null}
      {fromRun.error ? <ErrorBox error={fromRun.error} /> : null}
      {datasets.data && datasets.data.length === 0 ? (
        <Empty>
          <p>No datasets are installed, so there is nothing to replay.</p>
          <p>
            Create one with <code>blackbox synth</code> or <code>blackbox ingest-tardis</code>; see{' '}
            <Link to="/datasets">Datasets</Link>.
          </p>
        </Empty>
      ) : null}
      {loadingInitial && !(datasets.data && datasets.data.length === 0) ? (
        <Spinner label="Loading datasets and presets" />
      ) : null}

      {config && datasets.data ? (
        <form
          className="col"
          noValidate
          onSubmit={(e) => {
            e.preventDefault();
            void submit('run');
          }}
        >
          <Panel title="1. Dataset">
            <div className="lab-fields">
              <div className="field">
                <label className="field-label" htmlFor="lab-dataset">
                  Dataset to replay
                </label>
                <div className="field-control">
                  <select
                    id="lab-dataset"
                    value={datasetId ?? ''}
                    onChange={(e) => changeDataset(e.target.value)}
                  >
                    {dataset === null && datasetId !== null ? (
                      <option value={datasetId}>{datasetId} (not installed)</option>
                    ) : null}
                    {datasets.data.map((d) => (
                      <option key={d.id} value={d.id}>
                        {d.title} ({d.id}){d.kind === 'synthetic' ? ' \u2014 synthetic' : ''}
                      </option>
                    ))}
                  </select>
                </div>
                <span className="field-help">
                  {dataset ? (
                    <>
                      {dataset.kind === 'synthetic' ? (
                        <Badge kind="warn">synthetic {'\u2014'} generated, not market data</Badge>
                      ) : (
                        <Badge kind="observed">recorded market data</Badge>
                      )}{' '}
                      tick size {dataset.instrument.tick_size}, lot size{' '}
                      {dataset.instrument.lot_size}.{' '}
                      <Link to={`/replay/${encodeURIComponent(dataset.id)}`}>Open market replay</Link>
                    </>
                  ) : (
                    'This dataset is not installed on the server; a run on it will be rejected.'
                  )}
                </span>
              </div>
              <div className="field">
                <label className="field-label" htmlFor="lab-label">
                  Run label <span className="field-unit">optional</span>
                </label>
                <div className="field-control">
                  <input
                    id="lab-label"
                    type="text"
                    value={label}
                    maxLength={120}
                    onChange={(e) => setLabel(e.target.value)}
                  />
                </div>
                <span className="field-help">
                  A name for the run list, for example the scenario. Not part of any hash.
                </span>
              </div>
            </div>
          </Panel>

          <Panel title="2. Presets">
            <div className="col">
              {presets.error ? <ErrorBox error={presets.error} /> : null}
              {presets.loading ? <Spinner label="Loading presets" /> : null}
              {presets.data ? <PresetList presets={presets.data} onApply={applyPreset} /> : null}
              <p className="note">{originText(origin, config)}</p>
            </div>
          </Panel>

          <h2 className="lab-heading">3. Configuration</h2>
          <div className="lab-sections">
            {sections.map((s) => (
              <SectionPanel key={s.id} section={s} config={config} onChange={edit}>
                {s.id === 'strategy' && config.strategy.kind.type === 'scripted' ? (
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
              </SectionPanel>
            ))}
          </div>

          <Panel title="4. Scenario A (optional baseline)">
            <div className="col">
              <p className="note">
                To ask a counterfactual question, pick an existing run as scenario A. &quot;Run as
                scenario B&quot; then simulates the configuration above and opens the comparison of
                A and B. A comparison reports which decisions and outcomes diverged; it does not
                attribute a P&amp;L difference to a single parameter.
              </p>
              {runs.error ? <ErrorBox error={runs.error} /> : null}
              <div className="row">
                <label htmlFor="lab-base-run">Scenario A</label>
                <select
                  id="lab-base-run"
                  className="lab-base-select"
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
                <button type="button" onClick={loadBaseRun} disabled={!baseRunMatches}>
                  Load A&apos;s configuration into the form
                </button>
                {runs.loading ? <Spinner label="Loading runs" /> : null}
              </div>
              {baseRunMatches && baseRun ? (
                <>
                  <p className="mono small">A: {assumptionsLabel(baseRun.manifest.config)}</p>
                  {changes.length === 0 ? (
                    <p className="notice">
                      The form is identical to scenario A. Running it would return A itself.
                    </p>
                  ) : (
                    <div className="table-scroll">
                      <table className="table">
                        <caption>
                          What scenario B changes relative to A ({changes.length} field
                          {changes.length === 1 ? '' : 's'})
                        </caption>
                        <thead>
                          <tr>
                            <th scope="col">Configuration path</th>
                            <th scope="col">A</th>
                            <th scope="col">B (this form)</th>
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
                  )}
                </>
              ) : null}
            </div>
          </Panel>

          <Panel title="5. Run">
            <div className="col">
              {blocked ? (
                <div className="error-box" role="alert">
                  <div className="error-box-head">
                    <Badge kind="error">invalid configuration</Badge>
                    <span>
                      {fieldProblems.length + ruleProblems.length} problem
                      {fieldProblems.length + ruleProblems.length === 1 ? '' : 's'} to fix before
                      this can run
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
              ) : null}
              {advisories.length > 0 ? (
                <div className="notice" role="note">
                  <Badge kind="warn">check</Badge>{' '}
                  {blocked ? 'Also worth checking:' : 'The configuration is valid, but:'}
                  <ul>
                    {advisories.map((a, i) => (
                      <li key={i}>{a}</li>
                    ))}
                  </ul>
                </div>
              ) : null}
              {submitError !== null ? <ErrorBox error={submitError} /> : null}
              {submitNote ? (
                <p className="notice" role="status">
                  {submitNote}
                </p>
              ) : null}
              <div className="row">
                <button type="submit" className="btn-primary" disabled={blocked || submitting !== null}>
                  {submitting === 'run' ? 'Simulating' : 'Run simulation'}
                </button>
                <button
                  type="button"
                  disabled={blocked || submitting !== null || baseRunId === ''}
                  onClick={() => void submit('scenario_b')}
                  title={baseRunId === '' ? 'Pick a scenario A run first.' : undefined}
                >
                  {submitting === 'scenario_b' ? 'Simulating' : 'Run as scenario B'}
                </button>
                {submitting !== null ? <Spinner label="Replaying the dataset" /> : null}
                {baseRunId === '' ? (
                  <span className="note">Scenario B needs a scenario A run selected above.</span>
                ) : (
                  <span className="note mono">A = {baseRunId}</span>
                )}
              </div>
              <p className="note">
                The run is synchronous and idempotent: a configuration already simulated on this
                dataset returns the stored run with the same run id and result hash. Results are
                simulated on historical replay and are never exchange executions.
              </p>
              {blocked ? null : (
                <JsonBlock summary="Request body (RunConfig as it will be sent)" value={config} />
              )}
            </div>
          </Panel>
        </form>
      ) : null}
    </div>
  );
}
