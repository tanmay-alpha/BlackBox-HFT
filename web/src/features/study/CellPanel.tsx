// Detail of the selected study cell (`?cell=`, B1): exact parameters, the baseline, controlled
// differences, key results against the reference, run identity, order-level evidence (links only
// for stored public runs), statistical notes and how to reproduce it. Every value is simulated on
// synthetic data.

import { Fragment } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';

import { Badge } from '../../components/ui';
import { ComparisonTags } from './charts';
import type { AnchorRun, PublicRun, StudyCell, StudyResult } from './contract';
import { COMPARISON_LABEL, COMPARISON_MEANING, controlledDifferences, labelSelection, resolveSelection, type Selection } from './explore';
import { reproduceCommand, studyOutDir, type RunLink } from './links';
import {
  DESIGN_LABEL,
  METRICS,
  NOT_CALIBRATED,
  cancelDelay,
  describeLatency,
  fmtPoint,
  latencyLabel,
  metricDef,
  metricPoint,
  queueLabel,
  regimeLabel,
  type MetricPoint,
} from './model';
import { Chain, ClassBadge, LinkStatus, Reproduce } from './parts';

export interface CellPanelProps {
  study: StudyResult;
  sel: Selection;
  linkOf: (run: PublicRun) => RunLink;
  linkById: (runId: string) => RunLink;
  /** The stored public runs are still being checked. */
  checking: boolean;
  onSelect: (key: string) => void;
  onClose: () => void;
}

/** A one-line title of a selection. */
export function selectionTitle(sel: Selection): string {
  if (sel.kind === 'anchor') return `Demo run #${sel.run.index}: ${sel.run.label}`;
  const g = sel.kind === 'group' ? sel.group : sel.run;
  const feed = g.feed_offset_ns !== 0 ? ` \u00b7 feed +${g.feed_offset_ns / 1e6} ms` : '';
  const base = `${regimeLabel(g.regime)} \u00b7 ${queueLabel(g.queue_model)} \u00b7 ${latencyLabel(g.latency)}${feed}`;
  return sel.kind === 'run' ? `${base} \u00b7 seed ${sel.run.seed}` : base;
}

function Full({ value }: { value: string }) {
  return <span className="mono stc-full">{value}</span>;
}

function SelectButton({ cellKey, onSelect, children }: { cellKey: string; onSelect: (key: string) => void; children: ReactNode }) {
  return (
    <button type="button" className="btn-link" onClick={() => onSelect(cellKey)}>
      {children}
    </button>
  );
}

const ms = (ns: number) => `${ns / 1e6} ms`;

function Parameters({ study, sel }: { study: StudyResult; sel: Selection }) {
  const levels = sel.kind === 'group' ? sel.group : sel.run;
  const queue = study.queue_models.find((m) => m.name === levels.queue_model);
  const latency = study.latency_models.find((m) => m.name === levels.latency);
  const runs: PublicRun[] = sel.kind === 'group' ? sel.members : [sel.run];
  const dataset = runs[0]?.dataset;
  const regime = sel.kind === 'anchor' ? 'demo dataset' : regimeLabel(sel.kind === 'group' ? sel.group.regime : sel.run.regime);
  const design = sel.kind === 'anchor' ? 'demo anchor (the demo’s curated runs)' : sel.kind === 'group' ? sel.group.design : sel.run.design;
  return (
    <dl className="kv" data-testid="cell-parameters">
      <dt>Dataset</dt>
      <dd>
        {dataset ? (
          <>
            <span className="mono">{dataset.id}</span> ({regime}), {dataset.rows.toLocaleString('en-US')} rows, synthetic
          </>
        ) : (
          'no member run'
        )}
      </dd>
      <dt>Queue model</dt>
      <dd>
        {queueLabel(levels.queue_model)}
        {queue ? <div className="mono small dim stc-full">{JSON.stringify(queue.config)}</div> : null}
      </dd>
      <dt>Order latency</dt>
      <dd>
        {latencyLabel(levels.latency)}
        {latency ? <div className="small dim">{describeLatency(latency.config)}</div> : null}
      </dd>
      <dt>Cancel delay</dt>
      <dd>{latency ? cancelDelay(latency.config) : '\u2014'}</dd>
      <dt>Feed latency</dt>
      <dd>generator feed delay plus offset {ms(levels.feed_offset_ns)}</dd>
      <dt>{runs.length > 1 ? 'Seeds' : 'Seed'}</dt>
      <dd className="mono">{runs.map((r) => r.seed).join(', ')}</dd>
      <dt>Design block</dt>
      <dd>{DESIGN_LABEL[design] ?? design}</dd>
      <dt>Latency draws</dt>
      <dd>{latency ? (latency.stochastic ? `stochastic (${runs.length} seed${runs.length === 1 ? '' : 's'} here)` : 'deterministic') : '\u2014'}</dd>
    </dl>
  );
}

function Baseline({ study, onSelect }: { study: StudyResult; onSelect: (key: string) => void }) {
  const b = study.baseline;
  return (
    <>
      <dl className="kv" data-testid="cell-baseline">
        <dt>Preset</dt>
        <dd className="mono">{b.preset}</dd>
        <dt>Levels</dt>
        <dd>
          {regimeLabel(b.regime)}, {queueLabel(b.queue_model)}, {latencyLabel(b.latency)}, feed offset {ms(b.feed_offset_ns)}, seed{' '}
          <span className="mono">{String(b.config.seed ?? '?')}</span>
        </dd>
        <dt>Run</dt>
        <dd>
          <SelectButton cellKey={`run.${b.run_id}`} onSelect={onSelect}>
            <span className="mono">{b.run_id}</span>
          </SelectButton>
        </dd>
        <dt>Config SHA-256</dt>
        <dd>
          <Full value={b.config_sha256} />
        </dd>
        <dt>Dataset</dt>
        <dd>
          <span className="mono">{b.dataset.id}</span> <Full value={b.dataset.sha256} />
        </dd>
      </dl>
      <details className="stc-repro">
        <summary>Full baseline configuration (JSON)</summary>
        <pre className="mono small stc-pre">{JSON.stringify(b.config, null, 2)}</pre>
      </details>
    </>
  );
}

function Differences({ study, sel }: { study: StudyResult; sel: Selection }) {
  const set = controlledDifferences(study, sel);
  return (
    <div className="col stc-gap-1" data-testid="cell-differences">
      <p className="note small">Against {set.against}. Only fields that differ; hover a field for its configuration path.</p>
      {set.problem ? (
        <p className="note" role="note">
          Not derived: {set.problem}
        </p>
      ) : set.diffs.length === 0 ? (
        <p className="note">None: this is the baseline configuration.</p>
      ) : (
        <div className="table-scroll">
          <table className="table stc-difftable">
            <thead>
              <tr>
                <th scope="col">Field</th>
                <th scope="col">Baseline</th>
                <th scope="col">This cell</th>
              </tr>
            </thead>
            <tbody>
              {set.diffs.map((d) => (
                <tr key={d.path} data-path={d.path}>
                  <th scope="row" className="wrap" title={d.path}>
                    {d.label}
                    <span className="visually-hidden"> ({d.path})</span>
                  </th>
                  <td className="wrap mono small">{d.baseline}</td>
                  <td className="wrap mono small">{d.value}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

interface ResultRow {
  label: string;
  value: string;
  reference: string | null;
  delta: string | null;
}

function signed(fmt: (v: number) => string, d: number): string {
  return `${d >= 0 ? '+' : ''}${fmt(d)}`;
}

/** Measures of a run against its reference run. */
function runResults(run: PublicRun, ref: PublicRun | null): ResultRow[] {
  const rows: ResultRow[] = METRICS.map((m) => {
    const v = run.measures[m.key];
    const r = ref?.measures[m.key];
    return {
      label: m.label,
      value: v === undefined ? 'unavailable' : m.fmt(v),
      reference: r === undefined ? null : m.fmt(r),
      delta: v !== undefined && r !== undefined ? signed(m.fmt, v - r) : null,
    };
  });
  const money = metricDef('net_pnl').fmt;
  const count = (v: number) => v.toLocaleString('en-US');
  const extra: [string, number, number | undefined, (v: number) => string][] = [
    ['Gross P&L (realized + unrealized)', run.accounting.gross_pnl, ref?.accounting.gross_pnl, money],
    ['Max absolute inventory', run.measures.max_abs_position, ref?.measures.max_abs_position, (v) => v.toFixed(5)],
    ['Submitted orders', run.orders.submitted, ref?.orders.submitted, count],
    ['Filled orders', run.orders.filled, ref?.orders.filled, count],
    ['Cancelled orders', run.orders.canceled, ref?.orders.canceled, count],
  ];
  for (const [label, v, r, fmt] of extra) {
    if (v === undefined) continue;
    rows.push({ label, value: fmt(v), reference: r === undefined ? null : fmt(r), delta: r === undefined ? null : signed(fmt, v - r) });
  }
  return rows;
}

/** Group measures (seed means with intervals) against the reference group. */
function groupResults(sel: Extract<Selection, { kind: 'group' }>, ref: Selection | null): ResultRow[] {
  const refGroup = ref?.kind === 'group' ? ref.group : null;
  return METRICS.map((m) => {
    const p: MetricPoint | null = metricPoint(sel.group.metrics[m.key]);
    const r = refGroup ? metricPoint(refGroup.metrics[m.key]) : null;
    return { label: m.label, value: fmtPoint(p, m.fmt), reference: r ? fmtPoint(r, m.fmt) : null, delta: p && r ? signed(m.fmt, p.value - r.value) : null };
  });
}

function Results({ study, sel, onSelect }: { study: StudyResult; sel: Selection; onSelect: (key: string) => void }) {
  const label = labelSelection(study, sel);
  const ref = label?.referenceKey ? resolveSelection(study, label.referenceKey) : null;
  const rows = sel.kind === 'group' ? groupResults(sel, ref) : runResults(sel.run, ref && ref.kind !== 'group' ? ref.run : null);
  return (
    <div className="col stc-gap-1">
      <p className="note small">
        Simulated session totals and rates on synthetic data; not a return rate, not annualised.{' '}
        {label ? (
          <>
            Reference: {label.against}
            {label.referenceKey ? (
              <>
                {' '}
                (
                <SelectButton cellKey={label.referenceKey} onSelect={onSelect}>
                  open
                </SelectButton>
                )
              </>
            ) : null}
            . The difference is labelled <ComparisonTags label={label} short />.
          </>
        ) : (
          'This is the reference itself.'
        )}
      </p>
      <div className="table-scroll">
        <table className="table stc-results stc-stack" data-testid="cell-results">
          <thead>
            <tr>
              <th scope="col">Measure</th>
              <th scope="col" className="num">
                This cell
              </th>
              <th scope="col" className="num">
                Reference
              </th>
              <th scope="col" className="num">
                {'\u0394'}
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.label}>
                <th scope="row" className="wrap">
                  {r.label}
                </th>
                <td className="mono num wrap" data-label="This cell">
                  {r.value}
                </td>
                <td className="mono num wrap" data-label="Reference">
                  {r.reference ?? '\u2014'}
                </td>
                <td className="mono num wrap" data-label={'\u0394'}>
                  {r.delta ?? '\u2014'}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

function Identity({ sel, onSelect }: { sel: Selection; onSelect: (key: string) => void }) {
  if (sel.kind !== 'group') {
    const r = sel.run;
    return (
      <dl className="kv" data-testid="cell-identity">
        <dt>Run id</dt>
        <dd className="mono">{r.run_id}</dd>
        <dt>Config SHA-256</dt>
        <dd>
          <Full value={r.config_sha256} />
        </dd>
        <dt>Result SHA-256</dt>
        <dd>
          <Full value={r.result_sha256} />
        </dd>
        <dt>Dataset SHA-256</dt>
        <dd>
          <span className="mono">{r.dataset.id}</span> <Full value={r.dataset.sha256} />
        </dd>
      </dl>
    );
  }
  const datasets = new Set(sel.members.map((m) => m.dataset.sha256));
  const shared = datasets.size === 1 ? sel.members[0].dataset : null;
  return (
    <div className="col stc-gap-1" data-testid="cell-identity">
      {shared ? (
        <dl className="kv">
          <dt>Dataset SHA-256</dt>
          <dd>
            <span className="mono">{shared.id}</span> <Full value={shared.sha256} />
          </dd>
        </dl>
      ) : null}
      <ul className="stc-plain stc-members">
        {sel.members.map((m) => (
          <li key={m.run_id}>
            <dl className="kv">
              <dt>Seed {m.seed}</dt>
              <dd>
                <SelectButton cellKey={`run.${m.run_id}`} onSelect={onSelect}>
                  <span className="mono">{m.run_id}</span>
                </SelectButton>
              </dd>
              <dt>Config SHA-256</dt>
              <dd>
                <Full value={m.config_sha256} />
              </dd>
              <dt>Result SHA-256</dt>
              <dd>
                <Full value={m.result_sha256} />
              </dd>
              {shared ? null : (
                <>
                  <dt>Dataset SHA-256</dt>
                  <dd>
                    <span className="mono">{m.dataset.id}</span> <Full value={m.dataset.sha256} />
                  </dd>
                </>
              )}
            </dl>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Divergence({ run }: { run: PublicRun }) {
  const c = run.comparison;
  if (!c || c.reference_run_id === run.run_id) return null;
  const pair = (title: string, p: typeof c.first_divergence) =>
    p ? (
      <>
        <dt>{title}</dt>
        <dd>
          order <span className="mono">{p.order_id}</span>, decision tick <span className="mono">{p.tick_no}</span> (
          <span className="mono">{String(p.decision_ts_ns)}</span> ns), {p.class.replace(/_/g, ' ')}
          {p.differences.length > 0 ? <span className="dim">: {p.differences.join(', ')}</span> : null}
        </dd>
      </>
    ) : null;
  return (
    <>
      <p className="note small">
        From the artefact&apos;s order-by-order comparison with run <span className="mono">{c.reference_run_id}</span> (
        {c.aligned ? 'aligned' : 'not aligned: metrics only'}). Simulated orders, not recorded executions.
      </p>
      <dl className="kv">
        {pair('First divergence', c.first_divergence)}
        {pair('First outcome divergence', c.first_outcome_divergence)}
        {c.lifecycle_counts ? (
          <>
            <dt>Order pairs by class</dt>
            <dd className="small">
              {Object.entries(c.lifecycle_counts).map(([k, v], i) => (
                <Fragment key={k}>
                  {i > 0 ? '; ' : ''}
                  {k.replace(/_/g, ' ')} <span className="mono">{v}</span>
                </Fragment>
              ))}
            </dd>
          </>
        ) : null}
      </dl>
    </>
  );
}

function RunEvidence({ study, run, link, linkById, checking }: { study: StudyResult; run: StudyCell | AnchorRun; link: RunLink; linkById: (id: string) => RunLink; checking: boolean }) {
  const where = 'detail' in run ? { detail: run.detail } : { anchorIndex: run.index };
  return (
    <div className="col stc-gap-1" data-testid="cell-evidence" data-link={link.kind}>
      {link.kind === 'stored' ? (
        <>
          <LinkStatus link={link} />
          <Chain run={run} link={link} linkById={linkById} />
        </>
      ) : (
        <>
          <p>
            <strong>Not stored: reproduce locally.</strong>{' '}
            <span className="small dim">{checking ? 'Checking the stored public runs.' : <LinkStatus link={link} />}</span>
          </p>
          <pre className="mono small stc-pre" data-testid="cell-reproduce-inline">
            {reproduceCommand(study, run, where)}
          </pre>
        </>
      )}
      <Divergence run={run} />
    </div>
  );
}

function groupReproduce(study: StudyResult, members: StudyCell[]): string {
  const out = studyOutDir(study);
  return [
    study.generator.command.replace('<dir>', out),
    ...members.map((m) => `# seed ${m.seed}: ${out}/${m.detail}\n#   run ${m.run_id}, config_sha256 ${m.config_sha256}, result_sha256 ${m.result_sha256}`),
    `# result hashes hold on platform class ${study.generator.platform}`,
  ].join('\n');
}

function Notes({ study, sel }: { study: StudyResult; sel: Selection }) {
  const label = labelSelection(study, sel);
  const seeds = sel.kind === 'group' ? sel.members.length : 1;
  const u = study.uncertainty;
  return (
    <ul className="note stc-caveats" data-testid="cell-notes">
      {seeds > 1 ? (
        <li>
          Values are means over {seeds} latency seeds; intervals are the {u.level * 100}% {u.method} ({u.replicates} resamples, bootstrap seed{' '}
          {u.seed}). They measure seed variation of the latency model only; {seeds} seeds give imprecise intervals.
        </li>
      ) : (
        <li>One deterministic simulated run: rerunning it gives the identical value; it has no sampling error and no interval.</li>
      )}
      {label?.kinds.map((k) => (
        <li key={k}>
          <strong>{COMPARISON_LABEL[k]}</strong>: {COMPARISON_MEANING[k]}
        </li>
      ))}
      <li>Excluded from every interval: {u.excludes}.</li>
      <li>{NOT_CALIBRATED}</li>
    </ul>
  );
}

function H3({ children }: { children: ReactNode }) {
  return <h3 className="stc-h3">{children}</h3>;
}

export function CellPanel({ study, sel, linkOf, linkById, checking, onSelect, onClose }: CellPanelProps) {
  const label = labelSelection(study, sel);
  const deterministic = sel.kind !== 'group' || sel.members.length === 1;
  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onClose();
    }
  };
  return (
    <section className="stc-panel col" aria-labelledby="stc-cell-title" data-testid="cell-panel" data-cell={sel.key} onKeyDown={onKeyDown}>
      <header className="col stc-panel-head">
        <div className="row stc-panel-top">
          <span className="small dim">{sel.kind === 'group' ? `Group of ${sel.members.length} run${sel.members.length === 1 ? '' : 's'}` : sel.kind === 'anchor' ? 'Demo anchor run' : 'Single run'}</span>
          <button type="button" className="btn-link" onClick={onClose} aria-label="Close cell detail">
            Close
          </button>
        </div>
        <h2 id="stc-cell-title" className="stc-h2">
          {selectionTitle(sel)}
        </h2>
        <code className="mono small dim stc-full">cell={sel.key}</code>
        <span className="row stc-badges">
          <Badge kind="neutral" title="Generated data, not recorded from an exchange">
            Synthetic dataset
          </Badge>
          <Badge kind="modeled" title="Fills, queue position and latency are simulated">
            Simulated fills
          </Badge>
          <ClassBadge dataClass={deterministic ? 'deterministic' : 'seed_variation'} />
        </span>
        <ComparisonTags label={label} />
      </header>
      <H3>Exact parameters</H3>
      <Parameters study={study} sel={sel} />
      <H3>Baseline configuration</H3>
      <Baseline study={study} onSelect={onSelect} />
      <H3>Controlled differences vs baseline</H3>
      <Differences study={study} sel={sel} />
      <H3>Key results</H3>
      <Results study={study} sel={sel} onSelect={onSelect} />
      <H3>Run identity</H3>
      <Identity sel={sel} onSelect={onSelect} />
      <H3>Order-level evidence</H3>
      {sel.kind === 'group' ? (
        sel.members.length === 1 ? (
          <RunEvidence study={study} run={sel.members[0]} link={linkOf(sel.members[0])} linkById={linkById} checking={checking} />
        ) : (
          <div className="col stc-gap-1" data-testid="cell-evidence" data-link="per-run">
            <p className="note small">Order-level evidence is per run. Select a seed run:</p>
            <ul className="stc-plain">
              {sel.members.map((m) => (
                <li key={m.run_id}>
                  <SelectButton cellKey={`run.${m.run_id}`} onSelect={onSelect}>
                    seed {m.seed} <span className="mono">{m.run_id}</span>
                  </SelectButton>{' '}
                  <LinkStatus link={linkOf(m)} />
                </li>
              ))}
            </ul>
          </div>
        )
      ) : (
        <RunEvidence study={study} run={sel.run} link={linkOf(sel.run)} linkById={linkById} checking={checking} />
      )}
      <H3>Statistical notes</H3>
      <Notes study={study} sel={sel} />
      <H3>Reproduction</H3>
      <Reproduce
        open
        summary="Commands"
        command={
          sel.kind === 'group'
            ? groupReproduce(study, sel.members)
            : reproduceCommand(study, sel.run, sel.kind === 'run' ? { detail: sel.run.detail } : { anchorIndex: sel.run.index })
        }
      />
      <p className="note small">
        Regenerates every synthetic dataset and run of the study locally (no download), then compare the config and result hashes above.
      </p>
    </section>
  );
}
