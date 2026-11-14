// Content of the four guided steps. Each step reads stored results only and drives the shared
// workspace state (selected order, replay time) through callbacks.

import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';

import type { OrderDetail } from '../../../api/model';
import type { DecisionOutcome, InstrumentMeta, OrderPair, PairClass } from '../../../api/types';
import { EXCHANGE_MODEL_INFO, QUEUE_MODEL_INFO, STRATEGY_INFO, fractionToBps, latencyLabel } from '../../../components/describe';
import { DASH, fmtDuration, fmtDurationExact, fmtInt, fmtNum, fmtPrice, fmtQty, fmtSigned, humanizeNsText, words } from '../../../components/fmt';
import { BasisBadge, ClockText, ErrorBox, HashText, Notice, Spinner, type AsyncResult } from '../../../components/ui';
import { paths } from '../../../shell/paths';
import { divergences, PNL_IDENTITY_TOLERANCE, scenarioMetrics, type LatencyStory } from '../story';
import {
  METRIC_GROUPS,
  TIF_TEXT,
  pairClassText,
  pairSides,
  presentMetric,
  type AnswerSummary,
  type Focus,
  type ScenarioKey,
  type Workspace,
} from './data';

export const STEPS = [
  { n: 1, title: 'Result', hint: 'Fills, fees and P&L in both scenarios' },
  { n: 2, title: 'Where they first differ', hint: 'The first order that played out differently' },
  { n: 3, title: 'The order, step by step', hint: 'Its lifecycle in both scenarios' },
  { n: 4, title: 'Assumptions & limits', hint: 'What the model assumes and cannot show' },
] as const;

export interface StepGuideProps {
  ws: Workspace;
  step: number;
  summary: AnswerSummary;
  names: Record<ScenarioKey, string>;
  focus: Focus;
  orderId: number | null;
  order: AsyncResult<Record<ScenarioKey, OrderDetail | null> | null>;
  /** `t` null: start the replay at the order's decision once it is loaded. */
  onSelectOrder: (orderId: number, t: number | null) => void;
  onPick: (t: number) => void;
  onStep: (step: number) => void;
}

export function StepGuide(props: StepGuideProps) {
  const { step, onStep } = props;
  const meta = STEPS[step - 1];
  return (
    <div className="pb-guide">
      <h2 className="pb-guide-title">
        <span className="pb-guide-n">Step {step} of 4</span> {meta.title}
      </h2>
      {step === 1 ? <ResultStep {...props} /> : null}
      {step === 2 ? <DivergenceStep {...props} /> : null}
      {step === 3 ? <OrderStep {...props} /> : null}
      {step === 4 ? <AssumptionsStep {...props} /> : null}
      <div className="pb-guide-nav">
        {step > 1 ? (
          <button type="button" onClick={() => onStep(step - 1)}>
            &larr; {STEPS[step - 2].title}
          </button>
        ) : null}
        {step < 4 ? (
          <button type="button" className="btn-primary" onClick={() => onStep(step + 1)}>
            Next: {STEPS[step].title} &rarr;
          </button>
        ) : null}
      </div>
    </div>
  );
}

// ----- 1 Result ---------------------------------------------------------------------------

function ResultStep({ ws, summary, names, focus }: StepGuideProps) {
  const [all, setAll] = useState(false);
  const rows = scenarioMetrics(ws.story).map((m) => ({ m, p: presentMetric(m) }));
  const visible = all ? rows : rows.filter((r) => r.p.primary);
  const { fast: f, slow: s } = summary;
  if (rows.length === 0) {
    return (
      <>
        <p className="pb-lead">{summary.headline}</p>
        <Notice tag="NO COMPARISON">
          No stored comparison of these two runs is published, so per-metric differences and divergences cannot be shown.
        </Notice>
      </>
    );
  }
  return (
    <>
      <p className="pb-lead">{summary.headline}</p>
      {summary.split ? <p>{summary.split}</p> : null}
      <p className="pb-note">
        A smaller loss is still a loss. This is one strategy on one synthetic dataset, replayed without market impact; it does not
        show that slower execution is better in general.
      </p>

      <h3 className="pb-h3">Where the net result comes from</h3>
      <table className="pb-table">
        <caption className="pb-table-cap">
          Net result = realized trading P&amp;L + unrealized P&amp;L − fees (quote currency).{' '}
          {summary.identityHolds
            ? 'The stored values satisfy this identity in both scenarios.'
            : `The stored values do not satisfy this identity within ${PNL_IDENTITY_TOLERANCE}; the split is shown but not used as an explanation.`}
        </caption>
        <thead>
          <tr>
            <th scope="col">Part</th>
            <th scope="col" className="num" data-focus={focus === 'fast' ? '' : undefined}>{names.fast}</th>
            <th scope="col" className="num" data-focus={focus === 'slow' ? '' : undefined}>{names.slow}</th>
          </tr>
        </thead>
        <tbody>
          <tr><th scope="row">Realized trading P&amp;L</th><td className="num">{fmtSigned(f.realized, 2)}</td><td className="num">{fmtSigned(s.realized, 2)}</td></tr>
          <tr><th scope="row">Unrealized P&amp;L at the end</th><td className="num">{fmtSigned(f.unrealized, 2)}</td><td className="num">{fmtSigned(s.unrealized, 2)}</td></tr>
          <tr><th scope="row">Fees (subtracted)</th><td className="num">{fmtSigned(-f.fees, 2)}</td><td className="num">{fmtSigned(-s.fees, 2)}</td></tr>
          <tr className="pb-total"><th scope="row">Net result</th><td className="num">{fmtSigned(f.net, 2)}</td><td className="num">{fmtSigned(s.net, 2)}</td></tr>
        </tbody>
      </table>

      <h3 className="pb-h3">Measured differences</h3>
      <div className="pb-scroll-x">
        <table className="pb-table pb-metrics">
          <caption className="pb-table-cap">
            Stored comparison of the two runs. Change = {names.slow} minus {names.fast}; it is not marked good or bad.
          </caption>
          <thead>
            <tr>
              <th scope="col">Measure</th>
              <th scope="col">Unit</th>
              <th scope="col" className="num" data-focus={focus === 'fast' ? '' : undefined}>{names.fast}</th>
              <th scope="col" className="num" data-focus={focus === 'slow' ? '' : undefined}>{names.slow}</th>
              <th scope="col" className="num">Change</th>
            </tr>
          </thead>
          {METRIC_GROUPS.map((g) => {
            const inGroup = visible.filter((r) => r.p.group === g);
            if (inGroup.length === 0) return null;
            return (
              <tbody key={g}>
                <tr className="pb-group-row">
                  <th scope="rowgroup" colSpan={5}>{g}</th>
                </tr>
                {inGroup.map(({ m, p }) => (
                  <tr key={m.key}>
                    <th scope="row">{p.label}</th>
                    <td className="dim">{p.unit}</td>
                    <td className="num">{p.value(m.fast)}</td>
                    <td className="num">{p.value(m.slow)}</td>
                    <td className="num">{p.delta(m.delta)}</td>
                  </tr>
                ))}
              </tbody>
            );
          })}
        </table>
      </div>
      <button type="button" className="btn-link pb-more" aria-expanded={all} onClick={() => setAll((v) => !v)}>
        {all ? 'Show the key measures only' : `Show all ${rows.length} measures`}
      </button>
    </>
  );
}

// ----- 2 Divergence -----------------------------------------------------------------------

const CLASS_ORDER: PairClass[] = [
  'identical',
  'same_fills_different_history',
  'same_fills_different_fees',
  'same_outcome_different_timing',
  'same_decision_different_outcome',
  'different_decision',
  'only_a',
  'only_b',
];

function sideLine(d: DecisionOutcome | null, inst: InstrumentMeta): string {
  if (!d) return 'Not decided in this scenario.';
  const parts = [d.exch_arrival_ts === null ? 'never reached the exchange' : `reached the exchange at ${fmtDuration(d.exch_arrival_ts)}`];
  if (d.first_fill_ts !== null) parts.push(`filled ${fmtQty(d.filled_qty, inst)} at ${fmtDuration(d.first_fill_ts)}`);
  else parts.push(words(d.outcome));
  return `${parts.join(', ')}.`;
}

/** "49 ms later" / "3 ms earlier" / "at the same time". */
function laterText(ns: number): string {
  if (ns === 0) return 'at the same time';
  return `${fmtDuration(Math.abs(ns))} ${ns > 0 ? 'later' : 'earlier'}`;
}

function PairCard({ ws, pair, title, names, inst, selected, onSelect }: { ws: Workspace; pair: OrderPair; title: string; names: Record<ScenarioKey, string>; inst: InstrumentMeta; selected: boolean; onSelect: () => void }) {
  const sides = pairSides(ws.story, pair);
  const any = sides.fast ?? sides.slow;
  const timing = pair.timing;
  const sign = ws.story.oriented ? 1 : -1;
  return (
    <section className="pb-pair" data-selected={selected ? '' : undefined} aria-label={title}>
      <h3 className="pb-h3">{title}</h3>
      <p>
        <strong>Order {pair.order_id}</strong>, decided at <ClockText ns={pair.decision_ts} />
        {any ? `: ${any.side} ${fmtQty(any.qty, inst)} at ${fmtPrice(any.price_tick, inst)} (${TIF_TEXT[any.tif] ?? any.tif})` : ''}.
      </p>
      <p className="pb-pair-class">{pairClassText(ws.story, pair.class)}</p>
      <dl className="pb-dl">
        <dt>{names.fast}</dt>
        <dd>{sideLine(sides.fast, inst)}</dd>
        <dt>{names.slow}</dt>
        <dd>{sideLine(sides.slow, inst)}</dd>
        {timing && timing.exch_arrival_ns !== null ? (
          <>
            <dt>{names.slow} vs {names.fast}</dt>
            <dd>
              Reached the exchange {laterText(sign * timing.exch_arrival_ns)}
              {timing.first_fill_ns !== null ? `, filled ${laterText(sign * timing.first_fill_ns)}` : ''}.
            </dd>
          </>
        ) : null}
      </dl>
      <button type="button" className="btn-primary pb-cta" aria-pressed={selected} onClick={onSelect}>
        {selected ? `Order ${pair.order_id} is in the replay` : `Jump the replay to order ${pair.order_id}`}
      </button>
    </section>
  );
}

function DivergenceStep({ ws, names, orderId, onSelectOrder }: StepGuideProps) {
  const { first, firstOutcome } = divergences(ws.story);
  const al = ws.story.comparison?.alignment ?? null;
  const inst = ws.dataset.instrument;
  if (!al) {
    return (
      <Notice tag="NO ALIGNMENT">
        The stored comparison does not align the two runs order by order, so there is no first divergence to show.
      </Notice>
    );
  }
  const knockOn = (ws.story.comparison?.interpretation ?? []).filter((n) => /knock-on/i.test(n.statement));
  return (
    <>
      <p className="pb-lead">
        Both scenarios run the same strategy on the same recorded market. Each order is matched with the order from the same decision
        slot in the other scenario; the first pair that is not identical is where the scenarios start to differ.
      </p>
      <div className="pb-pairs">
        {first ? (
          <PairCard ws={ws} pair={first} title="First difference of any kind" names={names} inst={inst} selected={orderId === first.order_id} onSelect={() => onSelectOrder(first.order_id, first.decision_ts)} />
        ) : (
          <p>The two runs never differ.</p>
        )}
        {firstOutcome && firstOutcome.order_id !== first?.order_id ? (
          <PairCard
            ws={ws}
            pair={firstOutcome}
            title="First difference in a decision or fill"
            names={names}
            inst={inst}
            selected={orderId === firstOutcome.order_id}
            onSelect={() => onSelectOrder(firstOutcome.order_id, firstOutcome.decision_ts)}
          />
        ) : null}
      </div>
      {knockOn.map((n) => (
        <p key={n.statement} className="pb-note">
          <BasisBadge basis={n.basis} /> {humanizeNsText(n.statement)}
        </p>
      ))}
      <h3 className="pb-h3">How all matched orders compare</h3>
      <table className="pb-table">
        <thead>
          <tr>
            <th scope="col">Kind of difference</th>
            <th scope="col" className="num">Orders</th>
          </tr>
        </thead>
        <tbody>
          {CLASS_ORDER.map((c) => (
            <tr key={c}>
              <th scope="row">{pairClassText(ws.story, c)}</th>
              <td className="num">{fmtInt(al[c])}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

// ----- 3 Order ----------------------------------------------------------------------------

function OrderStep({ ws, names, focus, orderId, order, onSelectOrder, onPick }: StepGuideProps) {
  const { first, firstOutcome } = divergences(ws.story);
  const [tab, setTab] = useState<ScenarioKey>(focus === 'slow' ? 'slow' : 'fast');
  useEffect(() => {
    if (focus !== 'both') setTab(focus);
  }, [focus]);
  const [draft, setDraft] = useState('');
  const candidates = [first, firstOutcome].filter((p, i, a): p is OrderPair => p !== null && a.findIndex((q) => q?.order_id === p.order_id) === i);
  const detail = order?.data?.[tab] ?? null;
  const tsOfSeq = new Map(detail?.events.map((e) => [e.seq, e.ts]) ?? []);
  const submit = () => {
    const id = Number(draft);
    if (Number.isInteger(id) && id >= 0) onSelectOrder(id, null);
  };

  return (
    <>
      <div className="pb-row" role="group" aria-label="Choose an order">
        {candidates.map((p) => (
          <button key={p.order_id} type="button" aria-pressed={orderId === p.order_id} onClick={() => onSelectOrder(p.order_id, p.decision_ts)}>
            Order {p.order_id} ({p === first ? 'first difference' : 'first decision or fill difference'})
          </button>
        ))}
        <form
          className="pb-row"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <label htmlFor="pb-order-id">Other order</label>
          <input id="pb-order-id" inputMode="numeric" size={6} value={draft} onChange={(e) => setDraft(e.target.value.replace(/\D/g, ''))} />
          <button type="submit" disabled={draft === ''}>
            Show
          </button>
        </form>
      </div>
      {orderId === null ? (
        <p className="pb-empty">Choose an order above to see what happened to it in each scenario.</p>
      ) : order.error ? (
        <ErrorBox error={order.error} onRetry={order.reload} />
      ) : !order.data ? (
        <Spinner label={`Loading order ${orderId}`} />
      ) : (
        <>
          <div className="pb-tabs" role="tablist" aria-label="Scenario">
            {(['fast', 'slow'] as const).map((k) => (
              <button key={k} type="button" role="tab" id={`pb-tab-${k}`} aria-selected={tab === k} aria-controls="pb-tabpanel" onClick={() => setTab(k)}>
                {names[k]}
              </button>
            ))}
          </div>
          <div id="pb-tabpanel" role="tabpanel" aria-labelledby={`pb-tab-${tab}`}>
            {detail ? (
              <>
                <p className="pb-lead">{humanizeNsText(detail.explanation.headline)}</p>
                <ol className="pb-steps">
                  {detail.explanation.steps.map((s, i) => {
                    const ts = s.event_seqs.map((q) => tsOfSeq.get(q)).find((v): v is number => v !== undefined);
                    return (
                      <li key={i}>
                        <BasisBadge basis={s.basis} /> <span>{humanizeNsText(s.statement)}</span>{' '}
                        {ts !== undefined ? (
                          <button type="button" className="btn-link" onClick={() => onPick(ts)}>
                            Show at <ClockText ns={ts} /> in the replay
                          </button>
                        ) : null}
                      </li>
                    );
                  })}
                </ol>
                <h3 className="pb-h3">Caveats for this order</h3>
                <ul className="pb-list">
                  {detail.explanation.caveats.map((c) => (
                    <li key={c}>{humanizeNsText(c)}</li>
                  ))}
                </ul>
                <p>
                  <Link to={paths.order(tab === 'fast' ? ws.story.fast.runId : ws.story.slow.runId, orderId)}>
                    Open the full forensics page for order {orderId} ({names[tab]}) &rarr;
                  </Link>
                </p>
              </>
            ) : (
              <p className="pb-empty">
                The {names[tab]} scenario has no order {orderId}: the two runs&rsquo; decision sequences differ by this point, so this decision exists
                only in the other scenario. Differences after the first divergence may be knock-on effects.
              </p>
            )}
          </div>
        </>
      )}
    </>
  );
}

// ----- 4 Assumptions ----------------------------------------------------------------------

const VARIED_LABEL: Record<string, string> = {
  'execution.order_latency.entry_ns': 'Order latency, strategy to exchange',
  'execution.order_latency.response_ns': 'Response latency, exchange to strategy',
};

function AssumptionsStep({ ws, names }: StepGuideProps) {
  const { story, dataset } = ws;
  const cfg = story.fast.run.config;
  const e = cfg.execution;
  const cmp = story.comparison;
  const kind = cfg.strategy.kind;
  return (
    <>
      <p className="pb-lead">
        {dataset.kind === 'synthetic'
          ? `The market is “${dataset.title}”: synthetic data generated by BlackBox, not a recording of a real market.`
          : `The market is the recorded dataset “${dataset.title}”.`}{' '}
        Both results are simulated historical replays: the simulated orders never move prices, take displayed liquidity away or change
        other participants’ behaviour (no market impact).
      </p>

      <h3 className="pb-h3">What differs between the scenarios</h3>
      <table className="pb-table">
        <thead>
          <tr>
            <th scope="col">Setting</th>
            <th scope="col" className="num">{names.fast}</th>
            <th scope="col" className="num">{names.slow}</th>
          </tr>
        </thead>
        <tbody>
          {(cmp?.config_diffs ?? []).map((d) => (
            <tr key={d.path}>
              <th scope="row">{VARIED_LABEL[d.path] ?? words(d.path.split('.').at(-1) ?? d.path)}</th>
              {[d.a, d.b].map((v, i) => (
                <td key={i} className="num">
                  {typeof v === 'number' && d.path.endsWith('_ns') ? fmtDurationExact(v) : JSON.stringify(v)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {story.demo.how_to_read ? <p>{humanizeNsText(story.demo.how_to_read)}</p> : null}

      <h3 className="pb-h3">What the model assumes (identical in both)</h3>
      <dl className="pb-dl pb-dl-wide">
        <dt>Strategy</dt>
        <dd>
          {STRATEGY_INFO[kind.type].label}: {STRATEGY_INFO[kind.type].help} Decides every {fmtDuration(cfg.strategy.interval_ns)}.
        </dd>
        <dt>Latency model</dt>
        <dd>
          {words(e.order_latency.type)} ({latencyLabel(e.order_latency)} in the {names.fast} scenario): every order and cancel takes exactly this
          long each way.
        </dd>
        <dt>Fills</dt>
        <dd>
          {EXCHANGE_MODEL_INFO[e.exchange_model].label}: {EXCHANGE_MODEL_INFO[e.exchange_model].help}
        </dd>
        <dt>Queue position</dt>
        <dd>
          {QUEUE_MODEL_INFO[e.queue_model.type].label}
          {'n' in e.queue_model ? ` (n = ${e.queue_model.n})` : ''}: {QUEUE_MODEL_INFO[e.queue_model.type].help} The data shows total size per
          price, not individual orders, so queue position is always an estimate.
        </dd>
        <dt>Fees</dt>
        <dd>
          {fractionToBps(e.fees.maker)} bp of traded value when providing liquidity, {fractionToBps(e.fees.taker)} bp when taking it.
        </dd>
        <dt>Risk limits</dt>
        <dd>
          Position at most {fmtNum(cfg.risk.max_position, 3)}, at most {cfg.risk.max_open_orders} open orders.
        </dd>
      </dl>

      {cmp && cmp.interpretation.length > 0 ? (
        <>
          <h3 className="pb-h3">How to read the comparison</h3>
          <ul className="pb-list">
            {cmp.interpretation.map((n) => (
              <li key={n.statement}>
                <BasisBadge basis={n.basis} /> {humanizeNsText(n.statement)}
              </li>
            ))}
          </ul>
        </>
      ) : null}

      <details className="pb-details">
        <summary>Technical details and identifiers</summary>
        <dl className="pb-dl pb-dl-wide">
          <dt>Runs</dt>
          <dd>
            {names.fast}: <HashText hash={story.fast.runId} chars={16} />; {names.slow}: <HashText hash={story.slow.runId} chars={16} />
          </dd>
          <dt>Comparison</dt>
          <dd>{story.comparisonId ? <span className="mono">{story.comparisonId}</span> : DASH}</dd>
          <dt>Dataset</dt>
          <dd>
            <span className="mono">{dataset.id}</span> version {story.fast.run.dataset.version}, events SHA-256 <HashText hash={dataset.events_sha256} />
          </dd>
          <dt>Timestamps</dt>
          <dd>{dataset.timestamps.note}</dd>
          <dt>Held identical</dt>
          <dd>
            <ul className="pb-list">
              {story.demo.controlled.map((c) => (
                <li key={c}>{c}</li>
              ))}
            </ul>
          </dd>
          <dt>Reproduce</dt>
          <dd>
            {story.demo.reproduce.map((c) => (
              <pre key={c} className="pb-code">
                {c}
              </pre>
            ))}
          </dd>
        </dl>
      </details>
    </>
  );
}
