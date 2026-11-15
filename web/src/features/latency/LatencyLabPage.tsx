// Experiment Comparison -> Set (UI_DESIGN_SPEC.md §11.8): several runs side by side, the first as
// baseline. Every number is a stored run metric or a row of a comparison the server stored
// (`/api/v1/comparisons`); every interpretive sentence is the server's own interpretation note,
// rendered verbatim with its evidence basis. Missing comparisons are created only on request,
// through a gated control; nothing is compared in the browser.

import { useState } from 'react';
import type { ReactNode } from 'react';
import { Link, useSearchParams } from 'react-router-dom';

import { api, isAbort } from '../../api/client';
import type { Comparison, Demo, RunDetail } from '../../api/model';
import type { DatasetManifest, InstrumentMeta, MetricRow, OrderPair, PairClass } from '../../api/types';
import { ConfigValue, metricLabel } from '../../components/configValue';
import { DataTable, type Column } from '../../components/DataTable';
import { assumptionsLabel, strategyLabel } from '../../components/describe';
import { DASH, fmtDuration, fmtInt, fmtMoney, fmtNum, fmtPct, fmtPnl, fmtPp, fmtPrice, fmtQty, fmtSigned, fmtTicks, words } from '../../components/fmt';
import type { GlossaryKey } from '../../components/glossary';
import { digitsForUnit } from '../../components/ns';
import { SplitPane, type Pane } from '../../components/SplitPane';
import { ToolbarDivider } from '../../components/Toolbar';
import { Term } from '../../components/Tooltip';
import { Badge, BasisBadge, ClockText, Empty, ErrorBox, HashText, Notice, Panel, SideBadge, Spinner, TimeDigitsContext, useAsync } from '../../components/ui';
import { GatedButton } from '../../shell/access';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { CompareTabs } from '../experiments/CompareTabs';
import { CLASS_LABEL as COMPARE_CLASS_LABEL, EligibilityNotice, PAIR_CLASSES } from '../compare/classes';
import { fmtSignedDuration, outcomeKind } from '../forensics/labels';
// Shared comparison styles: the eligibility notice (`EligibilityNotice`) is styled there.
import '../compare/compare.css';
import './latency.css';

/** Comparison metric keys shown, in display order. Rows the server does not return are skipped. */
const KEY_METRICS = [
  'orders_submitted',
  'fill_count',
  'fill_ratio_orders',
  'time_to_first_fill_ns',
  'round_trip_ns',
  'cancels_sent',
  'cancels_rejected',
  'fees',
  'spread_capture',
  'spread_capture_ticks',
  'markout_100ms',
  'markout_1000ms',
  'markout_5000ms',
  'markout_30000ms',
  'realized_pnl',
  'unrealized_pnl',
  'net_pnl',
  'max_drawdown',
];

/** Signed money: P&L and fees carry an explicit sign (§7.1). */
const SIGNED_QUOTE: Record<string, true> = { fees: true, realized_pnl: true, unrealized_pnl: true, net_pnl: true };

const METRIC_TERM: Record<string, GlossaryKey> = {
  fill_ratio_orders: 'fill_ratio',
  round_trip_ns: 'round_trip',
  fees: 'fees',
  spread_capture: 'spread_capture',
  spread_capture_ticks: 'spread_capture',
  max_drawdown: 'drawdown',
};

/** The comparison's class names, with the two one-sided classes named for this page's runs. */
const CLASS_LABEL: Record<PairClass, string> = {
  ...COMPARE_CLASS_LABEL,
  only_a: 'Only in baseline',
  only_b: 'Only in this run',
};

/** Unit column of the mixed-unit metrics table (§6.1). */
const UNIT_LABEL: Record<string, string> = {
  quote: 'quote',
  qty: 'base',
  count: 'count',
  ratio: '% (delta pp)',
  ns: 'time',
  ticks: 'ticks',
};

function fmtMetric(v: number | null, row: MetricRow): string {
  if (v === null) return DASH;
  switch (row.unit) {
    case 'count':
      return fmtInt(v);
    case 'ratio':
      return fmtPct(v);
    case 'ns':
      return fmtDuration(v);
    case 'ticks':
      return fmtTicks(v);
    case 'quote':
      return SIGNED_QUOTE[row.key] ? fmtPnl(v, 4) : fmtMoney(v, 4);
    default:
      return fmtNum(v, 4);
  }
}

function fmtDelta(v: number | null, unit: string): string {
  if (v === null) return DASH;
  switch (unit) {
    case 'count':
      return fmtSigned(v, 0);
    case 'ratio':
      // A difference of two ratios is in percentage points, not percent.
      return fmtPp(v);
    case 'ns':
      return fmtSignedDuration(v);
    case 'ticks':
      return fmtSigned(v, 3);
    default:
      return fmtSigned(v, 4);
  }
}

function signClass(v: number | null): string | undefined {
  if (v === null || v === 0) return undefined;
  return v > 0 ? 'up' : 'down';
}

function parseRuns(raw: string | null): string[] {
  if (!raw) return [];
  const seen = new Set<string>();
  for (const part of raw.split(',')) {
    const id = part.trim();
    if (id !== '') seen.add(id);
  }
  return [...seen];
}

type Settled<T> = { ok: true; value: T } | { ok: false; error: unknown };

function settle<T>(p: Promise<T>): Promise<Settled<T>> {
  return p.then(
    (value) => ({ ok: true, value }),
    (error: unknown) => {
      if (isAbort(error)) throw error;
      return { ok: false, error };
    },
  );
}

/** The stored comparison of the baseline (A) with one run (B), as the server holds it. */
type PairEntry =
  | { kind: 'ok'; cmp: Comparison }
  /** None stored with A = baseline; `reversedId` names one stored the other way round. */
  | { kind: 'missing'; reversedId: string | null }
  | { kind: 'error'; error: unknown };

interface PairLookup {
  entries: PairEntry[];
  /** The comparison list was capped: an existing comparison may not have been seen. */
  incomplete: boolean;
  listed: number;
}

function runLabel(runs: Settled<RunDetail>[], ids: string[], i: number): string {
  const r = runs[i];
  return r && r.ok && r.value.label ? r.value.label : ids[i];
}

export default function LatencyLabPage() {
  const [params, setParams] = useSearchParams();
  const listed = parseRuns(params.get('runs'));
  const demoId = params.get('demo');

  // With `runs`, the demo is looked up on the baseline's dataset; with a demo alone, its stored
  // runs (`run_id`) are the set. Demo runs are matched by `run_id` only.
  const baseListed = useAsync(
    (signal) => (listed.length === 0 ? Promise.resolve(null) : api.run(listed[0], { signal })),
    [listed[0] ?? null],
  );
  const demoDataset = listed.length === 0 ? undefined : (baseListed.data?.dataset.id ?? null);
  const demos = useAsync(
    (signal) => (demoId === null || demoDataset === null ? Promise.resolve(null) : api.demos(demoDataset, { signal })),
    [demoId, demoDataset],
  );
  const demo = demos.data?.find((d) => d.id === demoId) ?? null;
  const ids = listed.length > 0 ? listed : (demo?.runs.flatMap((r) => (r.run_id === null ? [] : [r.run_id])) ?? []);
  const key = ids.join(',');

  const setIds = (next: string[]) => {
    const p = new URLSearchParams(params);
    if (next.length === 0) p.delete('runs');
    else p.set('runs', next.join(','));
    setParams(p);
  };

  const all = useAsync((signal) => api.runs({}, { signal }), []);
  const runs = useAsync((signal) => Promise.all(ids.map((id) => settle(api.run(id, { signal })))), [key]);
  const [createdIds, setCreatedIds] = useState<Record<string, string>>({});
  const comparisons = useAsync(async (signal): Promise<PairLookup | null> => {
    if (ids.length < 2) return null;
    const base = ids[0];
    const list = await api.readableComparisons({ signal });
    const entries = await Promise.all(
      ids.slice(1).map(async (other): Promise<PairEntry> => {
        const id = createdIds[`${base}|${other}`] ?? list.items.find((c) => c.run_a === base && c.run_b === other)?.id;
        if (id === undefined) {
          return { kind: 'missing', reversedId: list.items.find((c) => c.run_a === other && c.run_b === base)?.id ?? null };
        }
        // The list omits `alignment`; the resource carries it.
        const cmp = await settle(api.comparison(id, { signal }));
        return cmp.ok ? { kind: 'ok', cmp: cmp.value } : { kind: 'error', error: cmp.error };
      }),
    );
    return { entries, incomplete: list.incomplete, listed: list.items.length };
  }, [key, createdIds]);
  const missing = (comparisons.data?.entries ?? []).flatMap((e, i) => (e.kind === 'missing' ? [ids[i + 1]] : []));
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<unknown>(null);
  const createMissing = async () => {
    const base = ids[0];
    if (base === undefined || missing.length === 0) return;
    setCreating(true);
    setCreateError(null);
    const next = { ...createdIds };
    try {
      for (const other of missing) {
        const cmp = await api.createComparison(base, other);
        next[`${base}|${other}`] = cmp.id;
      }
    } catch (e) {
      setCreateError(e);
    } finally {
      setCreatedIds(next);
      setCreating(false);
    }
  };

  const baseRun = runs.data?.[0]?.ok ? runs.data[0].value : null;
  const baseDataset = baseRun?.dataset.id ?? null;
  const dataset = useAsync(
    (signal) => (baseDataset === null ? Promise.resolve(null) : api.dataset(baseDataset, { signal })),
    [baseDataset],
  );
  // Prices print with the instrument of the very dataset version the baseline replayed.
  const manifest: DatasetManifest | null =
    dataset.data?.current_version && baseRun && dataset.data.current_version.version === baseRun.dataset.version
      ? dataset.data.current_version.manifest
      : null;

  const [pick, setPick] = useState('');
  const available = (all.data?.items ?? []).filter((r) => !ids.includes(r.run_id));

  const toolbar = (
    <>
      <CompareTabs />
      <ToolbarDivider />
      <form
        className="row set-add"
        onSubmit={(e) => {
          e.preventDefault();
          if (pick !== '') {
            setIds([...ids, pick]);
            setPick('');
          }
        }}
      >
        <label htmlFor="set-add">Add run</label>
        <select id="set-add" value={pick} onChange={(e) => setPick(e.target.value)} className="set-select">
          <option value="">{all.loading ? 'Loading runs\u2026' : 'Choose a stored run'}</option>
          {available.map((r) => (
            <option key={r.run_id} value={r.run_id}>
              {r.label || 'unlabelled'} · {r.run_id} · {r.dataset.id}
            </option>
          ))}
        </select>
        <button type="submit" disabled={pick === ''}>
          Add
        </button>
      </form>
    </>
  );

  // Body of every pair panel until the comparisons are available.
  let pairState: ReactNode = null;
  if (ids.length === 0) {
    pairState =
      demoId !== null && (demos.loading || baseListed.loading) ? (
        <Spinner label="Resolving the experiment's stored runs" />
      ) : (
        <Empty>
          Choose at least two runs with Add run, or open one of the canonical experiments from the{' '}
          <Link to={paths.overview()}>Overview</Link>.
        </Empty>
      );
  } else if (ids.length === 1) {
    pairState = <Empty>Add a second run to compare against the baseline.</Empty>;
  } else if (comparisons.loading && !comparisons.data) {
    pairState = <Spinner label="Looking up the stored comparisons with the baseline" />;
  } else if (comparisons.error) {
    pairState = <ErrorBox error={comparisons.error} onRetry={comparisons.reload} />;
  }
  const pair: PairPanelProps | null =
    pairState === null && comparisons.data && runs.data
      ? { ids, runs: runs.data, comparisons: comparisons.data.entries, inst: manifest?.instrument ?? null }
      : null;

  const comparisonsNote =
    comparisons.data && (missing.length > 0 || comparisons.data.incomplete || createError) ? (
      <div className="col">
        {comparisons.data.incomplete ? (
          <Notice tag="List incomplete">
            First {fmtInt(comparisons.data.listed)} stored comparisons searched; an older comparison of these runs may
            exist beyond them.
          </Notice>
        ) : null}
        {missing.length > 0 ? (
          <Notice tag="Not compared" tone="info">
            {missing.length} run{missing.length === 1 ? ' has' : 's have'} no stored comparison with the baseline (A =
            baseline). The server computes comparisons; this page does not.{' '}
            <GatedButton scope="run" onClick={() => void createMissing()} blockedReason={creating ? 'Creating comparisons\u2026' : null}>
              {creating ? 'Creating\u2026' : `Create ${missing.length} missing comparison${missing.length === 1 ? '' : 's'}`}
            </GatedButton>
          </Notice>
        ) : null}
        {createError ? <ErrorBox error={createError} /> : null}
      </div>
    ) : null;

  const runSet = (
    <RunSetPanel
      ids={ids}
      runs={runs.data}
      loading={runs.loading}
      error={runs.error}
      onChange={setIds}
      listError={all.error}
      listIncomplete={all.data?.incomplete ? all.data.items.length : null}
      demo={demo}
      note={comparisonsNote}
    />
  );

  const top: ReactNode =
    demoId !== null ? (
      <SplitPane
        id="compare.set.top"
        panes={[
          { id: 'runs', label: 'Run set', defaultSize: '55%', minSize: '25%', content: runSet },
          {
            id: 'experiment',
            label: 'Experiment',
            defaultSize: '45%',
            minSize: '20%',
            content: (
              <DemoPanel
                demoId={demoId}
                demo={demo}
                ids={ids}
                loading={demos.loading || baseListed.loading}
                error={demos.error ?? baseListed.error}
              />
            ),
          },
        ]}
      />
    ) : (
      runSet
    );

  const bottom: Pane[] = [
    {
      id: 'alignment',
      label: 'Decision alignment',
      defaultSize: demoId === 'gross_vs_net' ? '34%' : '50%',
      minSize: '20%',
      content: <AlignmentPanel pair={pair} state={pairState} />,
    },
    {
      id: 'interpretation',
      label: 'Interpretation notes',
      defaultSize: demoId === 'gross_vs_net' ? '33%' : '50%',
      minSize: '15%',
      content: <InterpretationPanel pair={pair} state={pairState} />,
    },
  ];
  if (demoId === 'gross_vs_net') {
    bottom.push({
      id: 'waterfall',
      label: 'Gross to net',
      defaultSize: '33%',
      minSize: '15%',
      content: <GrossToNet ids={ids} runs={runs.data} loading={runs.loading} />,
    });
  }

  const context = demo ? demo.title : ids.length > 0 ? `${ids[0].slice(0, 12)} + ${ids.length - 1}` : null;

  return (
    <Workspace context={context} toolbar={toolbar} layout>
      <TimeDigitsContext.Provider value={manifest ? digitsForUnit(manifest.timestamps.input_unit) : 9}>
        <SplitPane
          id="compare.set.main"
          orientation="vertical"
          panes={[
            { id: 'top', label: 'Run set and experiment', defaultSize: '30%', minSize: '12%', content: top },
            {
              id: 'middle',
              label: 'Configuration and metrics',
              defaultSize: '38%',
              minSize: '12%',
              content: (
                <SplitPane
                  id="compare.set.middle"
                  panes={[
                    {
                      id: 'variables',
                      label: 'Varied versus controlled',
                      defaultSize: '45%',
                      minSize: '20%',
                      content: <VariablesPanel pair={pair} state={pairState} />,
                    },
                    {
                      id: 'metrics',
                      label: 'Key metrics',
                      defaultSize: '55%',
                      minSize: '20%',
                      content: <MetricsPanel pair={pair} state={pairState} />,
                    },
                  ]}
                />
              ),
            },
            {
              id: 'bottom',
              label: 'Alignment and interpretation',
              defaultSize: '32%',
              minSize: '12%',
              content: <SplitPane id={demoId === 'gross_vs_net' ? 'compare.set.bottom3' : 'compare.set.bottom'} panes={bottom} />,
            },
          ]}
        />
      </TimeDigitsContext.Provider>
    </Workspace>
  );
}

// ----- run set --------------------------------------------------------------------------------

interface SetRow {
  id: string;
  index: number;
  run: Settled<RunDetail> | undefined;
  /** The demo run with this `run_id`, when the set is a canonical experiment. */
  demoLabel: string | null;
}

function RunSetPanel({
  ids,
  runs,
  loading,
  error,
  onChange,
  listError,
  listIncomplete,
  demo,
  note,
}: {
  ids: string[];
  runs: Settled<RunDetail>[] | null;
  loading: boolean;
  error: Error | null;
  onChange: (ids: string[]) => void;
  listError: Error | null;
  /** Runs shown in Add run when the run list was capped. */
  listIncomplete: number | null;
  demo: Demo | null;
  note: ReactNode;
}) {
  const rows: SetRow[] = ids.map((id, index) => ({
    id,
    index,
    run: runs?.[index],
    demoLabel: demo?.runs.find((r) => r.run_id === id)?.label ?? null,
  }));
  const failed = rows.filter((r) => r.run && !r.run.ok);
  const columns: Column<SetRow>[] = [
    {
      id: 'role',
      header: 'Role',
      cell: (r) => <Badge kind={r.index === 0 ? 'neutral' : 'modeled'}>{r.index === 0 ? 'baseline' : `run ${r.index + 1}`}</Badge>,
      raw: (r) => (r.index === 0 ? 'baseline' : `run ${r.index + 1}`),
    },
    { id: 'run', header: 'Run', rowHeader: true, cell: (r) => <HashText hash={r.id} />, raw: (r) => r.id },
    ...(demo
      ? [
          {
            id: 'demo',
            header: 'Experiment run',
            cell: (r: SetRow) => (r.demoLabel === null ? <span className="dim">not in the experiment</span> : r.demoLabel),
            raw: (r: SetRow) => r.demoLabel ?? '',
          },
        ]
      : []),
    {
      id: 'label',
      header: 'Label',
      cell: (r) =>
        !r.run ? (
          <span className="dim">{DASH}</span>
        ) : r.run.ok ? (
          r.run.value.label ? (
            <span title={r.run.value.label}>{r.run.value.label}</span>
          ) : (
            <span className="dim">unlabelled</span>
          )
        ) : (
          <span className="set-failed">ERROR not loaded</span>
        ),
    },
    {
      id: 'strategy',
      header: 'Strategy',
      cell: (r) => <span className="mono">{r.run?.ok ? strategyLabel(r.run.value.config.strategy.kind) : DASH}</span>,
    },
    { id: 'dataset', header: 'Dataset', cell: (r) => <span className="mono">{r.run?.ok ? `${r.run.value.dataset.id} v${r.run.value.dataset.version}` : DASH}</span> },
    {
      id: 'assumptions',
      header: 'Execution assumptions',
      cell: (r) => {
        if (!r.run?.ok) return DASH;
        const text = assumptionsLabel(r.run.value.config);
        return <span title={text}>{text}</span>;
      },
    },
    {
      id: 'open',
      header: 'Open',
      cell: (r) => (
        <span className="row set-actions">
          <Link to={paths.run(r.id)}>Forensics</Link>
          <Link to={paths.bookRun(r.id)}>Replay Terminal</Link>
        </span>
      ),
      raw: () => '',
    },
    {
      id: 'set',
      header: 'Set',
      cell: (r) => (
        <span className="row set-actions">
          {r.index > 0 ? (
            <button type="button" onClick={() => onChange([r.id, ...ids.filter((x) => x !== r.id)])}>
              Make baseline
            </button>
          ) : null}
          <button type="button" onClick={() => onChange(ids.filter((x) => x !== r.id))}>
            Remove
          </button>
        </span>
      ),
      raw: () => '',
    },
  ];
  return (
    <Panel
      title="Run set"
      caption="Runs side by side on the same historical replay, the first as baseline. Differences are between simulations under different assumptions; they are not forecasts and say nothing about how the market would have reacted."
      flush
    >
      {loading || error || listError || listIncomplete !== null || failed.length > 0 || note ? (
        <div className="set-status col">
          {loading ? <Spinner label="Loading runs" /> : null}
          {error ? <ErrorBox error={error} /> : null}
          {listError ? <ErrorBox error={listError} /> : null}
          {listIncomplete !== null ? (
            <Notice tag="List incomplete">First {fmtInt(listIncomplete)} runs offered in Add run; older runs are not listed.</Notice>
          ) : null}
          {failed.map((r) => (
            <div key={r.id} className="col">
              <p className="note">
                Run <span className="mono">{r.id}</span> could not be loaded:
              </p>
              <ErrorBox error={r.run && !r.run.ok ? r.run.error : null} />
            </div>
          ))}
          {note}
        </div>
      ) : null}
      <DataTable
        label="Run set"
        rows={rows}
        rowKey={(r) => r.id}
        columns={columns}
        empty="No runs in the set. Add stored runs with Add run in the toolbar."
        fill
      />
    </Panel>
  );
}

// ----- experiment -----------------------------------------------------------------------------

function DemoPanel({
  demoId,
  demo,
  ids,
  loading,
  error,
}: {
  demoId: string;
  demo: Demo | null;
  ids: string[];
  loading: boolean;
  error: Error | null;
}) {
  return (
    <Panel title={`Experiment ${demo ? demo.title : demoId}`}>
      {loading ? <Spinner label="Loading experiment description" /> : null}
      {error ? <ErrorBox error={error} /> : null}
      {!loading && !error && !demo ? (
        <p className="note">
          The server lists no experiment <span className="mono">{demoId}</span> for the baseline run's dataset.
        </p>
      ) : null}
      {demo ? (
        <>
          <p className="prose">{demo.question}</p>
          <dl className="kv">
            <dt>Varied</dt>
            <dd>
              {demo.varied.length === 0 ? (
                'nothing'
              ) : (
                <ul className="set-list">
                  {demo.varied.map((v) => (
                    <li key={v}>{v}</li>
                  ))}
                </ul>
              )}
            </dd>
            <dt>Controlled</dt>
            <dd className="prose">
              <ul className="set-list">
                {demo.controlled.map((c) => (
                  <li key={c}>{c}</li>
                ))}
              </ul>
            </dd>
          </dl>
          <p className="note">{demo.how_to_read}</p>
          <table className="table">
            <caption>The experiment's runs, by the run id the server reports for each.</caption>
            <thead>
              <tr>
                <th scope="col">Experiment run</th>
                <th scope="col">Stored run</th>
                <th scope="col">In this set</th>
              </tr>
            </thead>
            <tbody>
              {demo.runs.map((r) => (
                <tr key={r.label}>
                  <th scope="row">{r.label}</th>
                  <td>
                    {r.run_id === null ? (
                      <span className="dim">not stored yet</span>
                    ) : (
                      <Link to={paths.run(r.run_id)}>
                        <HashText hash={r.run_id} />
                      </Link>
                    )}
                  </td>
                  <td>{r.run_id !== null && ids.includes(r.run_id) ? 'yes' : 'no'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      ) : null}
    </Panel>
  );
}

// ----- pair panels ----------------------------------------------------------------------------

interface PairPanelProps {
  ids: string[];
  runs: Settled<RunDetail>[];
  /** One entry per run after the baseline. */
  comparisons: PairEntry[];
  /** Instrument of the baseline's dataset, for prices and quantities. */
  inst: InstrumentMeta | null;
}

function ok(pair: PairPanelProps): (Comparison | null)[] {
  return pair.comparisons.map((c) => (c.kind === 'ok' ? c.cmp : null));
}

function VariablesPanel({ pair, state }: { pair: PairPanelProps | null; state: ReactNode }) {
  return (
    <Panel title="Varied versus controlled">
      {state}
      {pair
        ? pair.comparisons.map((c, i) => {
            const other = pair.ids[i + 1];
            const name = runLabel(pair.runs, pair.ids, i + 1);
            if (c.kind === 'error') {
              return (
                <section key={other} className="col set-pair">
                  <p className="note">
                    The stored comparison of the baseline with <span className="mono">{other}</span> could not be loaded:
                  </p>
                  <ErrorBox error={c.error} />
                </section>
              );
            }
            if (c.kind === 'missing') {
              return (
                <section key={other} className="col set-pair">
                  <h3 className="set-pair-title">Baseline vs {name}</h3>
                  <p className="note">
                    No stored comparison with the baseline as A.
                    {c.reversedId !== null ? (
                      <>
                        {' '}
                        One exists the other way round (<Link to={paths.comparePair(other, pair.ids[0])}>open it as a pair</Link>);
                        its deltas are baseline minus this run.
                      </>
                    ) : null}{' '}
                    Create the missing comparisons from the Run set panel.
                  </p>
                </section>
              );
            }
            const cmp = c.cmp;
            return (
              <section key={other} className="col set-pair">
                <h3 className="set-pair-title row">
                  <span>Baseline vs {name}</span>
                  <Link className="small" to={paths.comparePair(pair.ids[0], other)}>
                    Full comparison
                  </Link>
                </h3>
                <EligibilityNotice cmp={cmp} />
                {cmp.config_diffs.length === 0 ? (
                  <p className="note">The configurations are identical.</p>
                ) : (
                  <div className="table-scroll">
                    <table className="table">
                      <caption>
                        Varied: every configuration path that differs. Everything else in the run configuration is
                        identical (controlled).
                      </caption>
                      <thead>
                        <tr>
                          <th scope="col">Configuration path</th>
                          <th scope="col">Baseline</th>
                          <th scope="col">This run</th>
                        </tr>
                      </thead>
                      <tbody>
                        {cmp.config_diffs.map((d) => (
                          <tr key={d.path}>
                            <th scope="row" className="mono">
                              {d.path}
                            </th>
                            <ConfigValue path={d.path} value={d.a} />
                            <ConfigValue path={d.path} value={d.b} />
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </section>
            );
          })
        : null}
    </Panel>
  );
}

function MetricLabel({ row }: { row: MetricRow }) {
  const label = metricLabel(row);
  const term: GlossaryKey | undefined = row.key.startsWith('markout_') ? 'markout' : METRIC_TERM[row.key];
  return term ? <Term term={term}>{label}</Term> : <>{label}</>;
}

function MetricsPanel({ pair, state }: { pair: PairPanelProps | null; state: ReactNode }) {
  const comparisons = pair ? ok(pair) : [];
  const first = comparisons.find((c) => c !== null) ?? null;
  const rowsByKey = comparisons.map((c) => new Map<string, MetricRow>((c?.metrics ?? []).map((r) => [r.key, r])));
  const keys = KEY_METRICS.filter((k) => rowsByKey.some((m) => m.has(k)));
  return (
    <Panel
      title="Key metrics"
      basis="derived"
      flush
      caption="Values and deltas exactly as returned by the comparison of each run with the baseline. Delta is this run minus baseline. A dash means the value is undefined for that run."
    >
      {state ? <div className="set-status">{state}</div> : null}
      {pair && !first ? <Empty>No stored comparison with the baseline is available yet.</Empty> : null}
      {pair && first ? (
        <div className="table-scroll set-fill">
          <table className="table set-metrics">
            <thead>
              <tr>
                <th scope="col">Metric</th>
                <th scope="col">Unit</th>
                <th scope="col" className="num">
                  Baseline
                </th>
                {pair.ids.slice(1).map((id, i) => (
                  <th key={id} scope="col" className="num" colSpan={2} title={id}>
                    {runLabel(pair.runs, pair.ids, i + 1)}
                    <span className="unit">value · delta</span>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {keys.map((k) => {
                const any = rowsByKey.map((m) => m.get(k)).find((r) => r !== undefined) as MetricRow;
                return (
                  <tr key={k}>
                    <th scope="row" title={k}>
                      <MetricLabel row={any} />
                    </th>
                    <td className="dim">{UNIT_LABEL[any.unit] ?? any.unit}</td>
                    <td className="num">{fmtMetric(any.a, any)}</td>
                    {rowsByKey.map((m, i) => {
                      const r = m.get(k);
                      return r ? (
                        <MetricCells key={pair.ids[i + 1]} row={r} />
                      ) : (
                        <td key={pair.ids[i + 1]} colSpan={2} className="num dim">
                          {pair.comparisons[i]?.kind === 'ok' ? DASH : 'not compared'}
                        </td>
                      );
                    })}
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : null}
    </Panel>
  );
}

function MetricCells({ row }: { row: MetricRow }) {
  return (
    <>
      <td className="num">{fmtMetric(row.b, row)}</td>
      <td className={['num', signClass(row.delta)].filter(Boolean).join(' ')}>{fmtDelta(row.delta, row.unit)}</td>
    </>
  );
}

function DivergenceSide({ label, pair, side, inst }: { label: string; pair: OrderPair; side: 'a' | 'b'; inst: InstrumentMeta | null }) {
  const o = pair[side];
  if (!o) return <p className="small">{label}: no such decision.</p>;
  return (
    <p className="small row set-side">
      <span>{label}:</span>
      <SideBadge side={o.side} />
      <span className="mono">
        {inst ? fmtQty(o.qty, inst) : o.qty} @ {inst ? fmtPrice(o.price_tick, inst) : `tick ${o.price_tick}`}
      </span>
      <span>
        decided <ClockText ns={o.decision_ts} />
      </span>
      <Badge kind={outcomeKind(o.outcome)}>{words(o.outcome)}</Badge>
      <span>
        filled <span className="mono">{inst ? fmtQty(o.filled_qty, inst) : o.filled_qty}</span>
      </span>
      <span>
        arrival <ClockText ns={o.exch_arrival_ts} />
      </span>
      <span>
        first fill <ClockText ns={o.first_fill_ts} />
      </span>
    </p>
  );
}

function AlignmentPanel({ pair, state }: { pair: PairPanelProps | null; state: ReactNode }) {
  return (
    <Panel
      title="Decision alignment and first divergence"
      caption="Orders are aligned by order id, which encodes the strategy tick and slot of the decision, only when both runs share dataset, instrument, strategy kind, decision grid and simulator revision. A pair is identical only when its timing, every fill and its recorded lifecycle match too."
    >
      {state}
      {pair
        ? ok(pair).map((cmp, i) => {
            if (!cmp) return null;
            const ids = pair.ids;
            const al = cmp.status === 'aligned' ? cmp.alignment : null;
            const other = ids[i + 1];
            if (al === null) {
              return (
                <section key={other} className="col set-pair">
                  <h3 className="set-pair-title">Baseline vs {runLabel(pair.runs, ids, i + 1)}</h3>
                  <p className="note">
                    Status <span className="mono">{cmp.status}</span>: no per-order alignment; the runs differ in{' '}
                    {cmp.eligibility.reasons
                      .filter((r) => r.effect === 'no_alignment')
                      .map((r) => r.field)
                      .join(', ')}
                    .
                  </p>
                </section>
              );
            }
            const total = PAIR_CLASSES.reduce((s, c) => s + al[c], 0);
            const fd = al.first_divergence;
            const fo = al.first_outcome_divergence;
            const arrival = al.timing.exch_arrival;
            const firstFill = al.timing.first_fill;
            return (
              <section key={other} className="col set-pair">
                <h3 className="set-pair-title">Baseline vs {runLabel(pair.runs, ids, i + 1)}</h3>
                {total > 0 ? (
                  <div className="set-bar" aria-hidden="true">
                    {PAIR_CLASSES.map((c) =>
                      al[c] > 0 ? (
                        <span key={c} className={`set-seg set-seg-${c}`} style={{ flexGrow: al[c] }} title={`${CLASS_LABEL[c]}: ${fmtInt(al[c])}`} />
                      ) : null,
                    )}
                  </div>
                ) : null}
                <ul className="row set-classes small">
                  {PAIR_CLASSES.map((c) => (
                    <li key={c}>
                      <span className={`set-swatch set-seg-${c}`} aria-hidden="true" /> {CLASS_LABEL[c]}:{' '}
                      <span className="mono">{fmtInt(al[c])}</span>
                    </li>
                  ))}
                </ul>
                <p className="small">
                  Exchange arrival delta (this run minus baseline): median{' '}
                  <span className="mono">{arrival.median === null ? DASH : fmtSignedDuration(Math.round(arrival.median))}</span> over{' '}
                  <span className="mono">{fmtInt(arrival.count)}</span> pairs · first fill delta: median{' '}
                  <span className="mono">{firstFill.median === null ? DASH : fmtSignedDuration(Math.round(firstFill.median))}</span> over{' '}
                  <span className="mono">{fmtInt(firstFill.count)}</span> pairs
                </p>
                {fd ? (
                  <div className="set-divergence col">
                    <p>
                      First divergence: order <span className="mono">{fd.order_id}</span> (tick{' '}
                      <span className="mono">{fd.tick_no}</span>), decided at <ClockText ns={fd.decision_ts} /> ·{' '}
                      <Badge kind="neutral">{CLASS_LABEL[fd.class]}</Badge>
                      {fd.differences.length > 0 ? <span className="small"> · differs in {fd.differences.join(', ')}</span> : null}
                    </p>
                    <p className="small">Every pair before it is identical, timing included.</p>
                    <DivergenceSide label="Baseline" pair={fd} side="a" inst={pair.inst} />
                    <DivergenceSide label="This run" pair={fd} side="b" inst={pair.inst} />
                    {fo && fo.order_id !== fd.order_id ? (
                      <p className="small">
                        First difference in a decision or its fills: order <span className="mono">{fo.order_id}</span> (tick{' '}
                        <span className="mono">{fo.tick_no}</span>) · <Badge kind="neutral">{CLASS_LABEL[fo.class]}</Badge>
                      </p>
                    ) : null}
                    <div className="row small">
                      <Link to={paths.bookRun(ids[0], { t: fd.decision_ts, order: fd.order_id })}>Baseline in Replay Terminal</Link>
                      <Link to={paths.bookRun(other, { t: fd.decision_ts, order: fd.order_id })}>This run in Replay Terminal</Link>
                      {fd.a ? <Link to={paths.order(ids[0], fd.order_id)}>Baseline forensics</Link> : null}
                      {fd.b ? <Link to={paths.order(other, fd.order_id)}>This run forensics</Link> : null}
                      <Link to={paths.comparePair(ids[0], other)}>Full comparison</Link>
                    </div>
                  </div>
                ) : (
                  <p className="note">
                    No divergence: every aligned decision is identical, timing, fees and recorded lifecycle included.
                  </p>
                )}
              </section>
            );
          })
        : null}
    </Panel>
  );
}

function InterpretationPanel({ pair, state }: { pair: PairPanelProps | null; state: ReactNode }) {
  return (
    <Panel title="Interpretation notes (from the comparison, verbatim)">
      {state}
      {pair
        ? ok(pair).map((cmp, i) =>
            cmp ? (
              <section key={pair.ids[i + 1]} className="col set-pair">
                <h3 className="set-pair-title">Baseline vs {runLabel(pair.runs, pair.ids, i + 1)}</h3>
                {cmp.interpretation.length === 0 ? (
                  <p className="note">The comparison returned no interpretation notes.</p>
                ) : (
                  <ul className="set-notes">
                    {cmp.interpretation.map((n, k) => (
                      <li key={k}>
                        <BasisBadge basis={n.basis} />
                        <span>{n.statement}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            ) : null,
          )
        : null}
    </Panel>
  );
}

// ----- gross to net ---------------------------------------------------------------------------

interface WaterfallStep {
  label: string;
  from: number;
  to: number;
  total?: boolean;
}

function GrossToNet({ ids, runs, loading }: { ids: string[]; runs: Settled<RunDetail>[] | null; loading: boolean }) {
  const rows = (runs ?? []).flatMap((r, i) => {
    if (!r || !r.ok) return [];
    const m = r.value.metrics;
    const gross = m.realized_pnl + m.unrealized_pnl;
    const steps: WaterfallStep[] = [
      { label: 'Realized P&L', from: 0, to: m.realized_pnl },
      { label: 'Unrealized P&L', from: m.realized_pnl, to: gross },
      { label: 'Gross (realized + unrealized)', from: 0, to: gross, total: true },
      { label: 'Minus fees (a rebate adds)', from: gross, to: gross - m.fees },
      { label: 'Net P&L (reported)', from: 0, to: m.net_pnl, total: true },
    ];
    const gap = m.net_pnl - (gross - m.fees);
    return [{ id: ids[i], label: runLabel(runs ?? [], ids, i), m, steps, gap }];
  });
  const points = rows.flatMap((r) => [...r.steps.flatMap((s) => [s.from, s.to]), r.m.spread_capture, 0]);
  const lo = points.length > 0 ? Math.min(...points) : 0;
  const hi = points.length > 0 ? Math.max(...points) : 0;
  const span = hi - lo || 1;
  const pct = (v: number) => ((v - lo) / span) * 100;
  return (
    <Panel
      title="Gross to net, per run"
      basis="derived"
      caption="Built only from each run's reported metrics: realized and unrealized P&L, fees and net P&L (net = realized + unrealized − fees at the last account sample). Spread capture is shown separately for reference; it is a measure over maker fills, not a step of the sum. All bars share one scale."
    >
      {loading && !runs ? <Spinner label="Loading runs" /> : null}
      {runs && rows.length === 0 ? <Empty>No run of the set could be loaded.</Empty> : null}
      {rows.map((r) => (
        <section key={r.id} className="col set-waterfall" aria-label={`Gross to net of ${r.label}`}>
          <div className="set-wf-row set-wf-head">
            <h3 className="set-pair-title">{r.label}</h3>
            <span />
            <span className="unit">quote</span>
          </div>
          {r.steps.map((s) => {
            const a = Math.min(s.from, s.to);
            const b = Math.max(s.from, s.to);
            const dir = s.total ? 'total' : s.to >= s.from ? 'up' : 'down';
            const value = s.total ? s.to : s.to - s.from;
            return (
              <div key={s.label} className="set-wf-row">
                <span className="set-wf-label">{s.label}</span>
                <span className="set-wf-track" aria-hidden="true">
                  <span className="set-wf-zero" style={{ left: `${pct(0)}%` }} />
                  <span className={`set-wf-bar set-wf-${dir}`} style={{ left: `${pct(a)}%`, width: `max(1px, ${pct(b) - pct(a)}%)` }} />
                </span>
                <span className={['num', s.total ? undefined : signClass(value)].filter(Boolean).join(' ')}>{fmtPnl(value, 4)}</span>
              </div>
            );
          })}
          <div className="set-wf-row">
            <span className="set-wf-label">Spread capture (reference)</span>
            <span className="set-wf-track" aria-hidden="true">
              <span className="set-wf-zero" style={{ left: `${pct(0)}%` }} />
              <span
                className="set-wf-bar set-wf-refbar"
                style={{
                  left: `${pct(Math.min(0, r.m.spread_capture))}%`,
                  width: `max(1px, ${Math.abs(pct(r.m.spread_capture) - pct(0))}%)`,
                }}
              />
            </span>
            <span className="num">{fmtPnl(r.m.spread_capture, 4)}</span>
          </div>
          {Math.abs(r.gap) > 1e-9 * Math.max(1, Math.abs(r.m.net_pnl)) ? (
            <Notice tag="WARN" tone="warn">
              Reported net P&amp;L differs from realized + unrealized − fees by{' '}
              <span className="mono">{fmtSigned(r.gap, 8)}</span> (quote).
            </Notice>
          ) : null}
        </section>
      ))}
    </Panel>
  );
}
