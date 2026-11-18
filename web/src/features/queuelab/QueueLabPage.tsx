// Queue Model & Fill Uncertainty Lab (`/queue-lab`, Phase 7 D4;
// docs/research/QUEUE_SHADOW_EXPERIMENT.md). Reads the published synthetic `blackbox.queue-shadow`
// v1 artefact, verifies its SHA-256 and contract before anything renders, and shows the shadow-order
// comparison of the reconstructed L3 queue with nine L2 queue models: fill fractions with the
// artefact's Wilson and Newcombe intervals, queue-ahead error at checkpoints, cancelled volume
// ahead, Kaplan-Meier medians and the pre-registered hypotheses. Model, side, latency L, horizon
// tau and checkpoint live in the URL (Back restores them); a phone gets staged tabs. No value is
// recomputed; real NASDAQ results are quoted from the document, never shown as data.

import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import { Link, useSearchParams } from 'react-router-dom';

import { usePhone } from '../../components/PaneTabs';
import { Tabs } from '../../components/Tabs';
import { Badge, HashText, Notice, Spinner, useAsync } from '../../components/ui';
import { useAnnounce } from '../../shell/announce';
import { paths } from '../../shell/paths';
import { Workspace } from '../../shell/Workspace';
import { CANCEL_AHEAD_EXAMPLE, WORKBENCH_DATASET } from '../research/investigations';
import { STUDY_FILE, type StudyErrorKind } from '../study/contract';
import { Section, SyntheticBadges } from '../study/parts';
import { MarkChart, markDomain, type MarkRow } from './charts';
import { QUEUE_SCHEMA, QUEUE_SCHEMA_VERSION, REFERENCE_MODEL, type Interval, type QueueShadowResult, type Value } from './contract';
import { loadQueueShadow, QUEUE_STUDY_ID } from './load';
import {
  EVIDENCE_LABEL,
  EVIDENCE_MEANING,
  FROZEN_DESIGN_SHA256,
  OUTCOME_TEXT,
  STAGES,
  bounds,
  cellsAt,
  checkpointModels,
  checkpointSeries,
  docHref,
  fmtCount,
  fmtDiff,
  fmtMs,
  fmtProb,
  fmtShares,
  fmtSignedShares,
  fmtSpan,
  gridRows,
  hbPart2Rows,
  hypothesisRows,
  l2Models,
  labQuery,
  labelOf,
  modelLabel,
  num,
  outcomeSentence,
  outcomeSummary,
  parseLabState,
  reason,
  volumeAt,
  type EvidenceClass,
  type LabState,
  type Stage,
} from './model';
import '../study/study.css';
import './queuelab.css';

const ERROR_HEAD: Record<StudyErrorKind, string> = {
  unavailable: 'Study unavailable',
  hash_mismatch: 'Content hash mismatch',
  unsupported_contract: 'Unsupported study contract version',
  invalid: 'Invalid study file',
};

export default function QueueLabPage() {
  const loaded = useAsync((signal) => loadQueueShadow(signal), []);
  const failure = loaded.error
    ? { kind: 'unavailable' as const, message: loaded.error.message }
    : loaded.data && !loaded.data.ok
      ? loaded.data
      : null;
  if (failure) {
    return (
      <Workspace context={QUEUE_STUDY_ID} scroll>
        <div className="pad-3">
          <div className="error-box" role="alert" data-testid="ql-error" data-kind={failure.kind}>
            <div className="error-box-head">{ERROR_HEAD[failure.kind]}</div>
            <div className="error-box-message">{failure.message}</div>
            <div className="error-box-message">
              Nothing from this file is shown. The lab reads {QUEUE_SCHEMA} version {QUEUE_SCHEMA_VERSION} and verifies the SHA-256 in{' '}
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
  if (!loaded.data?.ok) {
    return (
      <Workspace context={QUEUE_STUDY_ID} scroll>
        <div className="pad-3">
          <Spinner label="Downloading and verifying the queue study" />
        </div>
      </Workspace>
    );
  }
  return <Lab study={loaded.data.study} sha256={loaded.data.sha256} />;
}

// ----- small pieces ----------------------------------------------------------------------------

function Ev({ c }: { c: EvidenceClass }) {
  return (
    <span className={`ql-ev ql-ev-${c}`} title={EVIDENCE_MEANING[c]}>
      {EVIDENCE_LABEL[c]}
    </span>
  );
}

function Evs({ classes }: { classes: EvidenceClass[] }) {
  return (
    <span className="row stc-badges">
      {classes.map((c) => (
        <Ev key={c} c={c} />
      ))}
    </span>
  );
}

/**
 * A point estimate and its nominal 95% interval exactly as the artefact holds them (formatted, never
 * recomputed); the raw numbers ride along as `data-*` attributes.
 */
function Est({ value, interval, fmt, extra }: { value: Value; interval?: Interval; fmt: (x: number | null) => string; extra?: ReactNode }) {
  const v = num(value);
  if (v === null) {
    return (
      <span className="dim ql-na" data-testid="ql-est" data-status="unavailable">
        unavailable: {reason(value)}
      </span>
    );
  }
  const b = interval ? bounds(interval) : null;
  return (
    <span data-testid="ql-est" data-status="available" data-value={String(v)} data-lower={b ? String(b[0]) : undefined} data-upper={b ? String(b[1]) : undefined}>
      <span className="stc-ci">{fmt(v)}</span>
      {b ? (
        <>
          {' '}
          <span className="stc-ci" aria-label="nominal 95% interval">
            [{fmt(b[0])}, {fmt(b[1])}]
          </span>
        </>
      ) : interval ? (
        <span className="dim stc-ci"> interval unavailable: {reason(interval.lower)}</span>
      ) : null}
      {extra ? (
        <>
          {' '}
          <span className="dim">{typeof extra === 'string' ? <span className="stc-ci">{extra}</span> : extra}</span>
        </>
      ) : null}
    </span>
  );
}

function Select<T extends string | number>({
  id,
  label,
  value,
  options,
  onChange,
}: {
  id: string;
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <label className="ql-control" htmlFor={id}>
      <span className="small">{label}</span>
      <select
        id={id}
        value={String(value)}
        onChange={(e) => {
          const next = options.find((o) => String(o.value) === e.target.value);
          if (next) onChange(next.value);
        }}
      >
        {options.map((o) => (
          <option key={String(o.value)} value={String(o.value)}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

// ----- the lab ---------------------------------------------------------------------------------

function Lab({ study, sha256 }: { study: QueueShadowResult; sha256: string }) {
  const [params, setParams] = useSearchParams();
  const phone = usePhone();
  const announce = useAnnounce();
  const { state, ignored } = parseLabState(params, study);
  const { symbol, model, latency, tau, checkpoint, stage } = state;

  // A view change is a history entry (Back restores it); a phone stage change replaces.
  const update = (patch: Partial<LabState>, replace = false) => {
    const next = { ...state, ...patch };
    setParams(new URLSearchParams(labQuery(next)), { replace });
    if (!replace) announce(`${labelOf(study, next.model)}, L ${fmtSpan(next.latency)}, \u03c4 ${fmtSpan(next.tau)}, checkpoint ${fmtSpan(next.checkpoint)}`);
  };

  const stagesRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = stagesRef.current;
    const scroller = el?.closest('.ws-body-scroll');
    if (el && scroller) {
      const above = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      if (above < 0) scroller.scrollTop += above;
    }
  }, [stage]);

  const placement = study.results.symbols.find((s) => s.symbol === symbol)!;
  const sessionSeconds = (study.results.tracking_end_ns - study.design.window_start_ns) / 1e9;

  const header = (
    <header className="col stc-head">
      <p className="small dim">
        <Link to={paths.studies()}>Studies</Link> / <span className="mono">{study.study_id}</span>
      </p>
      <h1 className="stc-title">Queue model and fill uncertainty lab</h1>
      <p className="note">{study.title}</p>
      <SyntheticBadges />
      <p className="small row ql-verified" data-testid="ql-sha256">
        <span className="mono">
          {study.schema} v{study.schema_version}
        </span>
        <HashText hash={sha256} chars={16} />
        <Badge kind="ok">SHA-256 verified in this browser</Badge>
      </p>
      <p className="ql-question" data-testid="ql-question">
        How far do the L2 queue models&apos; estimates of the shares ahead, and the fills they imply, sit from the same hypothetical 100-share
        orders tracked through the reconstructed L3 queue, given that a reconstructed L3 position of real orders is not ground truth for a
        hypothetical order and that every shadow fill assumes the market did not react to it?
      </p>
    </header>
  );

  const controls = (
    <div className="col ql-controls-wrap">
      <div className="ql-controls" data-testid="ql-controls" role="group" aria-label="Lab view">
        {study.results.symbols.length > 1 ? (
          <Select
            id="ql-symbol"
            label="Symbol"
            value={symbol}
            options={study.results.symbols.map((s) => ({ value: s.symbol, label: s.symbol }))}
            onChange={(v) => update({ symbol: v })}
          />
        ) : (
          <div className="ql-control">
            <span className="small">Symbol</span>
            <span className="mono ql-fixed">{symbol}</span>
          </div>
        )}
        <Select id="ql-model" label="L2 model in focus" value={model} options={l2Models(study).map((m) => ({ value: m.id, label: modelLabel(m) }))} onChange={(v) => update({ model: v })} />
        <div className="ql-control" data-testid="ql-side">
          <span className="small">Side</span>
          <span className="ql-fixed" title="Contract v1 pools the shadows of both sides (even k buys, odd k sells); it holds no per-side value.">
            buys and sells pooled
          </span>
        </div>
        <Select id="ql-latency" label="Latency L (assumed)" value={latency} options={study.design.latencies_ns.map((v) => ({ value: v, label: fmtSpan(v) }))} onChange={(v) => update({ latency: v })} />
        <Select id="ql-tau" label={'Horizon \u03c4'} value={tau} options={study.design.horizons_ns.map((v) => ({ value: v, label: fmtSpan(v) }))} onChange={(v) => update({ tau: v })} />
        <Select
          id="ql-checkpoint"
          label="Checkpoint"
          value={checkpoint}
          options={study.design.checkpoints_ns.map((v) => ({ value: v, label: fmtSpan(v) }))}
          onChange={(v) => update({ checkpoint: v })}
        />
      </div>
      {ignored.length > 0 ? (
        <Notice tag="URL" tone="warn">
          <span data-testid="ql-ignored">{ignored.join(' ')}</span>
        </Notice>
      ) : null}
    </div>
  );

  const identity = (
    <Section title="Artefact identity" id="ql-identity-h" key="identity">
      <dl className="kv" data-testid="ql-identity">
        <dt>Contract</dt>
        <dd>
          <span className="mono">
            {study.schema} v{study.schema_version}
          </span>{' '}
          <span className="dim">(this lab reads v{QUEUE_SCHEMA_VERSION})</span>
        </dd>
        <dt>Content SHA-256</dt>
        <dd>
          <HashText hash={sha256} chars={16} /> <Badge kind="ok">verified in this browser</Badge>
        </dd>
        <dt>Design SHA-256</dt>
        <dd>
          <HashText hash={study.design_sha256} chars={16} />{' '}
          {study.design_sha256 === FROZEN_DESIGN_SHA256 ? (
            <span className="dim">equals the pre-registered design hash</span>
          ) : (
            <Badge kind="warn">differs from the pre-registered design hash</Badge>
          )}
        </dd>
        <dt>Data class</dt>
        <dd>
          <span className="mono">{study.data_class}</span>: {String(study.source.generator ?? study.source.kind)}
          {study.source.config ? (
            <>
              , seed <span className="mono">{String(study.source.config.seed)}</span>, <span className="mono">{String(study.source.config.events)}</span> events, symbol{' '}
              <span className="mono">{String(study.source.config.symbol)}</span>
            </>
          ) : null}
        </dd>
        <dt>Input stream</dt>
        <dd>
          <HashText hash={study.source.sha256} chars={16} /> <span className="dim">{fmtCount(study.source.bytes)} bytes, {fmtCount(study.input.frames_read)} frames, stop {study.input.stop.reason}</span>
        </dd>
        <dt>Generator</dt>
        <dd>
          <span className="mono">blackbox {study.generator.blackbox_version}</span>
          {study.generator.source_commit ? (
            <>
              {' '}
              at <span className="mono">{study.generator.source_commit.slice(0, 12)}</span> <span className="dim">(declared, not verified here)</span>
            </>
          ) : null}
        </dd>
        {study.engine ? (
          <>
            <dt>Engine implementation</dt>
            <dd data-testid="ql-engine">
              <span className="mono">{study.engine.revision}</span>
              <p className="note">{study.engine.deadline_semantics}</p>
              <p className="note">{study.engine.checkpoint_semantics}</p>
            </dd>
          </>
        ) : null}
        <dt>Reproduce</dt>
        <dd>
          <code className="mono">{study.generator.command}</code>
        </dd>
        <dt>Placement ({symbol})</dt>
        <dd data-testid="ql-placement">
          {fmtCount(placement.placed)} placed of {fmtCount(placement.scheduled)} scheduled; {fmtCount(placement.skipped_empty_side)} on an empty side,{' '}
          {fmtCount(placement.skipped_locked_or_crossed)} locked or crossed, {fmtCount(placement.not_arrived)} never arrived (tracking ended{' '}
          {study.results.stream_reached_window_end ? 'at the window end' : `at the end of input, ${sessionSeconds.toFixed(2)} s after the window start`})
        </dd>
        <dt>Design</dt>
        <dd>
          one {study.design.shares}-share shadow every {fmtSpan(study.design.interval_ns)}; L {study.design.latencies_ns.map(fmtSpan).join(', ')};{' '}
          {'\u03c4'} {study.design.horizons_ns.map(fmtSpan).join(', ')}; checkpoints {study.design.checkpoints_ns.map(fmtSpan).join(', ')}
        </dd>
      </dl>
      <h3 className="stc-h3">Evidence classes on this page</h3>
      <ul className="stc-classes col" data-testid="ql-evidence">
        {(Object.keys(EVIDENCE_LABEL) as EvidenceClass[]).map((c) => (
          <li key={c}>
            <Ev c={c} /> <span className="note">{EVIDENCE_MEANING[c]}</span>
          </li>
        ))}
      </ul>
      <h3 className="stc-h3">Labels recorded in the artefact</h3>
      <ul className="note stc-caveats" data-testid="ql-labels">
        {study.labels.map((l) => (
          <li key={l}>{l}</li>
        ))}
      </ul>
    </Section>
  );

  // ----- (b) fill fraction and difference against L3 --------------------------------------------
  const cells = cellsAt(study, symbol, latency, tau);
  const ref = cells[0];
  const fillRows: MarkRow[] = cells.map((c) => ({
    key: c.model,
    label: labelOf(study, c.model),
    value: num(c.fill_fraction),
    interval: bounds(c.fill_fraction_95),
    reference: c.model === REFERENCE_MODEL,
    selected: c.model === model,
    onSelect: c.model === REFERENCE_MODEL ? undefined : () => update({ model: c.model }),
    text: <Est value={c.fill_fraction} interval={c.fill_fraction_95} fmt={fmtProb} extra={`${c.filled}/${c.eligible}`} />,
    data: { 'data-model': c.model },
  }));
  const diffRows: MarkRow[] = cells.slice(1).map((c) => ({
    key: c.model,
    label: labelOf(study, c.model),
    value: c.vs_l3 ? num(c.vs_l3.difference) : null,
    interval: c.vs_l3 ? bounds(c.vs_l3.difference_95) : null,
    selected: c.model === model,
    onSelect: () => update({ model: c.model }),
    text: c.vs_l3 ? (
      <Est
        value={c.vs_l3.difference}
        interval={c.vs_l3.difference_95}
        fmt={fmtDiff}
        extra={c.vs_l3.excludes_zero ? <strong className="ql-flag">H0 nominally rejected</strong> : undefined}
      />
    ) : (
      'no comparison'
    ),
    data: { 'data-model': c.model, 'data-excludes-zero': String(c.vs_l3?.excludes_zero ?? null) },
  }));
  const grid = gridRows(study, symbol, model);

  const fills = (
    <Section title={'Fill fraction within \u03c4: L2 models against the L3 reference'} id="ql-fills-h" key="fills" testId="ql-fills" labels={<Evs classes={['counterfactual', 'reconstructed', 'derived']} />}>
      <p className="note">
        {symbol}, L {fmtSpan(latency)}, {'\u03c4'} {fmtSpan(tau)}: {fmtCount(ref.eligible)} eligible of {fmtCount(ref.placed)} placed shadows (a shadow is eligible when
        arrival + {'\u03c4'} is at or before the end of tracking). A fill is the first counterfactual execution of any part of the shadow. Select a model
        to put it in focus.
      </p>
      <div className="grid-2">
        <MarkChart
          testId="ql-fill-chart"
          rows={fillRows}
          domain={markDomain(fillRows, { fixed: [0, 1] })}
          caption={`Fill fraction within ${fmtSpan(tau)} per path with its nominal 95% Wilson interval`}
          legend={<>Scale 0 to 1. Dots: fill fraction (filled / eligible); whiskers: nominal 95% Wilson score interval; dashed rule and ring: L3 reference.</>}
        />
        <MarkChart
          testId="ql-diff-chart"
          rows={diffRows}
          domain={markDomain(diffRows, { zero: true })}
          caption="Model minus L3 fill fraction with its nominal 95% Newcombe paired interval"
          legend={<>{'\u0394'} = model {'\u2212'} L3 on the same shadows; whiskers: nominal 95% Newcombe (1998) method 10 paired interval; solid rule: zero. H0 nominally rejected = interval excludes 0; exploratory, unadjusted for multiplicity.</>}
        />
      </div>
      <details className="stc-cells">
        <summary>Paired outcomes and fill causes at this L and {'\u03c4'}</summary>
        <div className="table-scroll">
          <table className="table stc-stack" data-testid="ql-paired">
            <thead>
              <tr>
                <th scope="col">Path</th>
                <th scope="col" className="num">Both filled</th>
                <th scope="col" className="num">Model only</th>
                <th scope="col" className="num">L3 only</th>
                <th scope="col" className="num">Neither</th>
                <th scope="col" className="num">Queue</th>
                <th scope="col" className="num">Trade-through</th>
                <th scope="col" className="num">Book cross</th>
              </tr>
            </thead>
            <tbody>
              {cells.map((c) => (
                <tr key={c.model}>
                  <th scope="row">{labelOf(study, c.model)}</th>
                  {(c.vs_l3 ? [c.vs_l3.both, c.vs_l3.model_only, c.vs_l3.l3_only, c.vs_l3.neither] : [null, null, null, null]).map((v, i) => (
                    <td key={i} className="mono num" data-label={['Both filled', 'Model only', 'L3 only', 'Neither'][i]}>
                      {v === null ? 'reference' : fmtCount(v)}
                    </td>
                  ))}
                  <td className="mono num" data-label="Fills from the queue">
                    {fmtCount(c.fill_causes.queue)}
                  </td>
                  <td className="mono num" data-label="Fills by trade-through">
                    {fmtCount(c.fill_causes.trade_through)}
                  </td>
                  <td className="mono num" data-label="Fills by book cross">
                    {fmtCount(c.fill_causes.book_cross)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <p className="note">
          A trade-through or book cross fills every path at the same moment whatever its queue position, so those fills cannot separate the
          models.
        </p>
      </details>
      <h3 className="stc-h3">
        {labelOf(study, model)} against L3 at every L and {'\u03c4'}
      </h3>
      <div className="table-scroll">
        <table className="table stc-stack" data-testid="ql-grid">
          <thead>
            <tr>
              <th scope="col">L, {'\u03c4'}</th>
              <th scope="col" className="num">Eligible</th>
              <th scope="col" className="num">L3 fill [nominal 95% Wilson]</th>
              <th scope="col" className="num">{labelOf(study, model)} fill [nominal 95% Wilson]</th>
              <th scope="col" className="num">{'\u0394'} [nominal 95% Newcombe]</th>
            </tr>
          </thead>
          <tbody>
            {grid.map((g) => {
              const selected = g.latency === latency && g.tau === tau;
              return (
                <tr key={`${g.latency}/${g.tau}`} className={selected ? 'selected' : undefined} data-latency={g.latency} data-tau={g.tau}>
                  <th scope="row">
                    <button type="button" className="stc-pick" aria-pressed={selected} onClick={() => update({ latency: g.latency, tau: g.tau })}>
                      L {fmtSpan(g.latency)}, {'\u03c4'} {fmtSpan(g.tau)}
                    </button>
                  </th>
                  <td className="mono num" data-label="Eligible">
                    {fmtCount(g.reference.eligible)}
                  </td>
                  <td className="mono num" data-label="L3 fill [nominal 95% Wilson]">
                    <Est value={g.reference.fill_fraction} interval={g.reference.fill_fraction_95} fmt={fmtProb} />
                  </td>
                  <td className="mono num" data-label={`${labelOf(study, model)} fill [nominal 95% Wilson]`}>
                    <Est value={g.model.fill_fraction} interval={g.model.fill_fraction_95} fmt={fmtProb} />
                  </td>
                  <td className="mono num" data-label={'\u0394 [nominal 95% Newcombe]'}>
                    {g.model.vs_l3 ? (
                      <Est
                        value={g.model.vs_l3.difference}
                        interval={g.model.vs_l3.difference_95}
                        fmt={fmtDiff}
                        extra={g.model.vs_l3.excludes_zero ? <strong className="ql-flag">H0 nominally rejected</strong> : undefined}
                      />
                    ) : null}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <ul className="note stc-caveats">
        <li>{study.interval_methods.fill_fraction_95}.</li>
        <li>{study.interval_methods.difference_95}.</li>
      </ul>
    </Section>
  );

  // ----- (c) queue-ahead error at the checkpoints ------------------------------------------------
  const atCheckpoint = checkpointModels(study, symbol, latency, checkpoint);
  const l3Ahead = atCheckpoint.find((c) => c.model === 'risk_adverse')?.mean_l3_ahead_shares ?? atCheckpoint[0].mean_l3_ahead_shares;
  const errorRows: MarkRow[] = atCheckpoint.map((c) => ({
    key: c.model,
    label: labelOf(study, c.model),
    value: num(c.mean_abs_error_shares),
    interval: null,
    selected: c.model === model,
    onSelect: () => update({ model: c.model }),
    text: (
      <Est
        value={c.mean_abs_error_shares}
        fmt={fmtShares}
        extra={
          <>
            {num(c.mean_signed_error_shares) === null ? null : (
              <>
                <span className="stc-ci">signed {fmtSignedShares(num(c.mean_signed_error_shares))},</span>{' '}
              </>
            )}
            <span className="stc-ci">{c.pairs} pairs</span>
          </>
        }
      />
    ),
    data: { 'data-model': c.model },
  }));
  const series = checkpointSeries(study, symbol, latency, model);

  const queueError = (
    <Section title={'Queue-ahead error: |front \u2212 ahead_L3| per model'} id="ql-queue-h" key="queue" testId="ql-queue-error" labels={<Evs classes={['estimated', 'reconstructed', 'derived']} />}>
      <p className="note">
        This is where the models differ. <span className="mono">front</span> is the model&apos;s estimate of the shares ahead of the shadow;{' '}
        <span className="mono">ahead_L3</span> the shares ahead in the reconstructed queue. Means over shadows still unfilled under both paths at the
        checkpoint, so the compared population shrinks with time. {symbol}, L {fmtSpan(latency)}, checkpoint {fmtSpan(checkpoint)} after arrival; mean
        L3 shares ahead over RiskAdverse&apos;s pairs, for scale: <Est value={l3Ahead} fmt={fmtShares} />.
      </p>
      <MarkChart
        testId="ql-checkpoint-chart"
        rows={errorRows}
        domain={markDomain(errorRows, { zero: true })}
        caption={`Mean absolute queue-ahead error in shares at ${fmtSpan(checkpoint)}`}
        legend={<>Dots: mean |front {'\u2212'} ahead_L3| in shares, from 0. Signed mean: positive = the model counts more shares ahead than L3 (undercredits progress). No interval: {study.interval_methods.checkpoints}.</>}
      />
      <h3 className="stc-h3">{labelOf(study, model)} at every checkpoint</h3>
      <div className="table-scroll">
        <table className="table stc-stack" data-testid="ql-checkpoints">
          <thead>
            <tr>
              <th scope="col">Checkpoint</th>
              <th scope="col" className="num">Pairs</th>
              <th scope="col" className="num">Mean L3 ahead</th>
              <th scope="col" className="num">Mean |front {'\u2212'} ahead_L3|</th>
              <th scope="col" className="num">Mean signed</th>
            </tr>
          </thead>
          <tbody>
            {series.map((c) => {
              const selected = c.checkpoint_ns === checkpoint;
              return (
                <tr key={c.checkpoint_ns} className={selected ? 'selected' : undefined} data-checkpoint={c.checkpoint_ns}>
                  <th scope="row">
                    <button type="button" className="stc-pick" aria-pressed={selected} onClick={() => update({ checkpoint: c.checkpoint_ns })}>
                      {fmtSpan(c.checkpoint_ns)}
                    </button>
                  </th>
                  <td className="mono num" data-label="Pairs">
                    {fmtCount(c.pairs)}
                  </td>
                  <td className="mono num" data-label="Mean L3 ahead (shares)">
                    <Est value={c.mean_l3_ahead_shares} fmt={fmtShares} />
                  </td>
                  <td className="mono num" data-label={'Mean |front \u2212 ahead_L3| (shares)'}>
                    <Est value={c.mean_abs_error_shares} fmt={fmtShares} />
                  </td>
                  <td className="mono num" data-label="Mean signed (shares)">
                    <Est value={c.mean_signed_error_shares} fmt={fmtSignedShares} />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </Section>
  );

  // ----- (d) cancelled volume ahead ------------------------------------------------------------
  const cancelled = (
    <Section title="Cancelled volume ahead of the shadow" id="ql-cancel-h" key="cancelled" testId="ql-cancelled" labels={<Evs classes={['reconstructed', 'derived']} />}>
      <p className="note">
        Shares cancelled, deleted or replaced away at the shadow&apos;s price until the L3 fill, {'\u03c4'} or the end of tracking: the share that was
        ahead of the shadow (ahead / (ahead + behind), pooled over shadows). Only cancellations ahead move the L3 queue; the probability models
        must guess this split from aggregated depth. {study.interval_methods.cancelled_volume}.
      </p>
      <div className="table-scroll">
        <table className="table stc-stack">
          <caption>{symbol}: share of cancelled volume ahead (ahead / total shares)</caption>
          <thead>
            <tr>
              <th scope="col">Latency L</th>
              {study.design.horizons_ns.map((h) => (
                <th key={h} scope="col" className="num">
                  {'\u03c4'} {fmtSpan(h)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {study.design.latencies_ns.map((l) => (
              <tr key={l}>
                <th scope="row">L {fmtSpan(l)}</th>
                {study.design.horizons_ns.map((h) => {
                  const v = volumeAt(study, symbol, l, h);
                  const selected = l === latency && h === tau;
                  return (
                    <td key={h} className={selected ? 'mono num stc-selected' : 'mono num'} data-label={`\u03c4 ${fmtSpan(h)}`} data-latency={l} data-tau={h}>
                      <Est value={v.share_ahead} fmt={fmtProb} extra={`(${fmtCount(v.ahead_shares)} / ${fmtCount(v.ahead_shares + v.behind_shares)})`} />
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Section>
  );

  // ----- (e) Kaplan-Meier median time to fill ----------------------------------------------------
  const kmRows: MarkRow[] = cells.map((c) => {
    const median = num(c.km_median_ns);
    return {
      key: c.model,
      label: labelOf(study, c.model),
      value: median === null ? null : median / 1e6,
      interval: null,
      reference: c.model === REFERENCE_MODEL,
      selected: c.model === model,
      onSelect: c.model === REFERENCE_MODEL ? undefined : () => update({ model: c.model }),
      text: (
        <span className="col ql-km-text">
          <Est value={c.km_median_ns} fmt={fmtMs} />
          <span className="dim small">
            P(fill by {'\u03c4'}) <Est value={c.km_fill_probability} interval={c.km_fill_probability_95} fmt={fmtProb} />; {c.km_censored} censored of{' '}
            {c.km_median_ns.n}
          </span>
        </span>
      ),
      data: { 'data-model': c.model },
    };
  });

  const km = (
    <Section title="Kaplan-Meier median time to fill" id="ql-km-h" key="km" testId="ql-km" labels={<Evs classes={['counterfactual', 'derived']} />}>
      <p className="note">
        {symbol}, L {fmtSpan(latency)}, {'\u03c4'} {fmtSpan(tau)}. {study.interval_methods.km}. A median reported here is the first time the estimated
        share of unfilled shadows reaches one half.
      </p>
      <MarkChart
        testId="ql-km-chart"
        rows={kmRows}
        domain={markDomain(kmRows, { zero: true })}
        caption={`Kaplan-Meier median time to the first counterfactual fill, milliseconds, censored at ${fmtSpan(tau)}`}
        legend={<>Dots: median time to fill in ms from 0; dashed rule and ring: L3 reference. Below each value: the fill probability by {'\u03c4'} with its nominal 95% Greenwood interval.</>}
      />
    </Section>
  );

  // ----- (f) one shadow order ---------------------------------------------------------------------
  const explain = (
    <Section title="One shadow order, step by step" id="ql-explain-h" key="explain" testId="ql-explain" labels={<Evs classes={['reconstructed', 'decision']} />}>
      <p className="note">
        Contract <span className="mono">{study.schema}</span> v{study.schema_version} stores aggregates only (cells, checkpoints, cancelled volume and
        hypothesis flags); it holds no per-shadow trace, so this page cannot step through one shadow order and does not invent one.
      </p>
      <p>
        <Link to={paths.marketDataset(WORKBENCH_DATASET, { tab: 'book', example: CANCEL_AHEAD_EXAMPLE })} data-testid="ql-workbench-link">
          A cancel ahead in the reconstructed L3 queue (Market Data Workbench)
        </Link>{' '}
        <span className="ql-illustration">illustration on the same synthetic generator, not the same shadow order</span>
      </p>
      <p className="note">
        The workbench example deletes an order in front of others at the best price: every order behind it moves up, which the L3 reference sees
        and an L2 model only infers from the level&apos;s total.
      </p>
    </Section>
  );

  // ----- (g) hypotheses ----------------------------------------------------------------------
  const hRows = hypothesisRows(study);
  const hb2 = hbPart2Rows(study, hRows);
  const summary = outcomeSummary(study);
  const symbols = study.results.symbols.map((s) => s.symbol);

  const hypotheses = (
    <Section title="Pre-registered hypotheses" id="ql-hyp-h" key="hypotheses" testId="ql-hypotheses" labels={<Evs classes={['derived']} />}>
      <p className="note">Decisions below are nominal and exploratory, not multiplicity-adjusted or calibrated for overlapping, serially dependent shadows.</p>
      <dl className="kv" data-testid="ql-definitions">
        <dt>H-A</dt>
        <dd>
          RiskAdverse never credits cancellations ahead, so it understates the fill fraction relative to L3. Test: {study.hypotheses.definitions.h_a}.
        </dd>
        <dt>H-B</dt>
        <dd>
          The probability models are closer to L3 than RiskAdverse (part 1: {study.hypotheses.definitions.h_b_closer}); no single model is within
          the interval on every symbol (part 2: {study.hypotheses.definitions.h_b_no_single_model}).
        </dd>
        <dt>H0</dt>
        <dd>Each model&apos;s fill fraction equals L3&apos;s. Test: {study.hypotheses.definitions.h0}.</dd>
      </dl>
      <p className="ql-outcome" data-testid="ql-outcome">
        <strong>Synthetic outcome.</strong> {outcomeSentence(summary)}
      </p>
      <div className="table-scroll">
        <table className="table stc-stack" data-testid="ql-hyp-table">
          <caption>Nominal/exploratory decisions per symbol, L and {'\u03c4'}, from the artefact&apos;s hypothesis flags</caption>
          <thead>
            <tr>
              <th scope="col">Cell</th>
              <th scope="col" className="num">Eligible</th>
              <th scope="col" className="num">RiskAdverse {'\u0394'} [nominal 95%]</th>
              <th scope="col">H-A (nominal)</th>
              <th scope="col">H-B part 1: closer than RiskAdverse (exploratory)</th>
              <th scope="col">H0 nominally rejected</th>
            </tr>
          </thead>
          <tbody>
            {hRows.map((r) => {
              const [lo, hi] = r.cell.risk_adverse_difference_95;
              const selected = r.cell.symbol === symbol && r.cell.latency_ns === latency && r.cell.horizon_ns === tau;
              return (
                <tr
                  key={`${r.cell.symbol}/${r.cell.latency_ns}/${r.cell.horizon_ns}`}
                  className={selected ? 'selected' : undefined}
                  data-latency={r.cell.latency_ns}
                  data-tau={r.cell.horizon_ns}
                  data-ha={r.ha}
                >
                  <th scope="row">
                    {r.cell.symbol}, L {fmtSpan(r.cell.latency_ns)}, {'\u03c4'} {fmtSpan(r.cell.horizon_ns)}
                  </th>
                  <td className="mono num" data-label="Eligible">
                    {fmtCount(r.eligible)}
                  </td>
                  <td className="mono num" data-label={'RiskAdverse \u0394 [nominal 95%]'}>
                    {r.cell.risk_adverse_difference === null ? (
                      <span className="dim">unavailable</span>
                    ) : (
                      <span className="stc-ci">
                        {fmtDiff(r.cell.risk_adverse_difference)}{' '}
                        {lo !== null && hi !== null ? `[${fmtDiff(lo)}, ${fmtDiff(hi)}]` : null}
                      </span>
                    )}
                  </td>
                  <td className="wrap" data-label="H-A (nominal)">
                    {r.testable ? OUTCOME_TEXT[r.ha] : 'not testable: no eligible shadow'}
                  </td>
                  <td className="wrap" data-label="H-B part 1 (exploratory)">
                    {r.testable ? (
                      <>
                        {r.closer.length} of {r.probabilityModels}
                        {r.closer.length > 0 ? <span className="dim">: {r.closer.map((m) => labelOf(study, m)).join(', ')}</span> : null}
                      </>
                    ) : (
                      'not testable'
                    )}
                  </td>
                  <td className="wrap" data-label="H0 nominally rejected">
                    {r.testable ? (
                      r.h0Rejected.length === 0 ? (
                        `none of ${l2Models(study).length}`
                      ) : (
                        <strong className="ql-flag">{r.h0Rejected.map((m) => labelOf(study, m)).join(', ')}</strong>
                      )
                    ) : (
                      'not testable'
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <h3 className="stc-h3">H-B part 2 (nominal/exploratory): models whose nominal 95% difference intervals contain zero on every studied symbol ({symbols.join(', ')})</h3>
      <div className="table-scroll">
        <table className="table stc-stack" data-testid="ql-hb2">
          <thead>
            <tr>
              <th scope="col">L, {'\u03c4'}</th>
              <th scope="col">Models within</th>
              <th scope="col">H-B part 2 (nominal/exploratory)</th>
            </tr>
          </thead>
          <tbody>
            {hb2.map((r) => (
              <tr key={`${r.latency}/${r.tau}`}>
                <th scope="row">
                  L {fmtSpan(r.latency)}, {'\u03c4'} {fmtSpan(r.tau)}
                </th>
                <td className="wrap" data-label="Models within">
                  {r.testable ? `${r.models.length} of ${l2Models(study).length}` : 'none: no eligible shadow'}
                </td>
                <td className="wrap" data-label="H-B part 2 (nominal/exploratory)">
                  {r.testable ? (r.outcome === 'supported' ? 'holds' : 'not supported') : 'not testable'}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {symbols.length === 1 ? (
        <p className="note">With one synthetic symbol, &quot;every studied symbol&quot; is {symbols[0]} alone; the plan&apos;s part 2 compares QQQ and AAPL.</p>
      ) : null}
      <aside className="ql-quote" data-testid="ql-real-data" aria-label="Real-data outcomes quoted from the document">
        <p className="small">
          <Badge kind="neutral">Quoted text, not data</Badge> Real-data outcomes (NASDAQ ITCH sample days of 2019, the opening minutes). The results
          files are not published, so this page neither shows nor verifies these numbers;{' '}
          <a href={docHref('7-hypothesis-outcomes')} target="_blank" rel="noreferrer">
            read §7 of the experiment document
          </a>
          .
        </p>
        <blockquote cite={docHref('7-hypothesis-outcomes')} className="note">
          <p>
            Only 2019-10-30 (QQQ, AAPL and AMD; OOS-A) is a clean out-of-sample day. December 30 QQQ/AAPL had already been inspected on November
            14/15, before the freeze; AMZN is a held-out symbol on that previously decoded day, not an independent held-out day.
            The boundary implementation was corrected after outcomes were reviewed; this is not an untouched-engine validation.
          </p>
          <p>
            The fixed-engine OOS-A recomputation has 243 model-versus-L3 comparisons and maximum absolute fill-fraction difference 0.0166976.
            H-A has zero nominally supported cells. Depending on horizon, 7{'\u2013'}9 models have nominal 95% difference intervals containing zero
            on every symbol. There are 9 nominal H0 rejections: 3 contrasts repeated across L, not 9 independent discoveries. These exploratory
            decisions are unadjusted for multiplicity, and overlapping shadows share market events.
          </p>
          <p>
            At L = 0 and {'\u03c4'} = 1 s, book-cross and trade-through fills together exceed queue fills for each OOS-A symbol. These shared
            fill mechanisms limit how strongly the fill-fraction comparisons can distinguish queue models.
          </p>
        </blockquote>
      </aside>
    </Section>
  );

  // ----- (h) limitations -----------------------------------------------------------------------
  const maxTau = Math.max(...study.design.horizons_ns);
  const comparisons = symbols.length * study.design.latencies_ns.length * study.design.horizons_ns.length * l2Models(study).length;
  const limitations = (
    <Section title="Limitations" id="ql-limits-h" key="limits" testId="ql-limitations">
      <ul className="col ql-limits">
        <li data-limit="dependence">
          <strong>Nominal 95% intervals do not account for overlapping shadows.</strong> One shadow arrives every {fmtSpan(study.design.interval_ns)} and
          horizons reach {fmtSpan(maxTau)}, so neighbouring shadows overlap and share the same market events. Wilson, Newcombe and Greenwood
          intervals do not adjust for this serial dependence; their nominal coverage is not calibrated and they may be too narrow.
          Hypothesis decisions are exploratory and unadjusted for the {comparisons} model-versus-L3 comparisons.
        </li>
        <li data-limit="latency">
          <strong>No latency calibration.</strong> L ({study.design.latencies_ns.map(fmtSpan).join(', ')}) is a swept assumption; there is no receipt
          clock ({study.clock.local_clock}).
        </li>
        <li data-limit="counterfactual">
          <strong>Counterfactual fills.</strong> A shadow fill is a model output under no market impact: nobody saw the order or reacted to it. It is
          never a recorded execution, and its bias for real orders is unknown in size and direction.
        </li>
        <li data-limit="synthetic">
          <strong>Synthetic and small.</strong> The generator&apos;s session ends {sessionSeconds.toFixed(2)} s after the window start, so{' '}
          {fmtCount(placement.placed)} shadows are placed for {symbol}; these numbers demonstrate the pipeline and the contract and say nothing about real
          markets.{' '}
          <a href={docHref('9-limitations')} target="_blank" rel="noreferrer">
            Full limitations (§9)
          </a>
          .
        </li>
      </ul>
      {study.interval_note ? <p className="note" data-testid="ql-interval-note">{study.interval_note}</p> : null}
      <h3 className="stc-h3">Caveats recorded in the artefact</h3>
      <ul className="note stc-caveats" data-testid="ql-artifact-caveats">
        {study.caveats.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ul>
    </Section>
  );

  if (phone) {
    const content: Record<Stage, ReactNode> = {
      overview: [identity, explain, limitations],
      fills: [fills, km],
      queue: [queueError, cancelled],
      hypotheses: [hypotheses],
    };
    return (
      <Workspace context={study.title} scroll ownHeading>
        <div className="pad-3 col stc-page" data-testid="ql-view" data-study-id={study.study_id} data-stage={stage}>
          {header}
          {controls}
          <div ref={stagesRef}>
            <Tabs<Stage> label="Lab view" className="stc-stages" active={stage} onChange={(s) => update({ stage: s }, true)} tabs={STAGES}>
              <div className="col stc-stage">{content[stage]}</div>
            </Tabs>
          </div>
        </div>
      </Workspace>
    );
  }

  return (
    <Workspace context={study.title} scroll ownHeading>
      <div className="pad-3 col stc-page" data-testid="ql-view" data-study-id={study.study_id}>
        {header}
        {controls}
        {fills}
        {queueError}
        {cancelled}
        {km}
        {explain}
        {hypotheses}
        {limitations}
        {identity}
      </div>
    </Workspace>
  );
}
