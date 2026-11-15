// One order's lifecycle in both scenarios side by side: a two-lane timing diagram and the
// lifecycle table (each step with its clock and the time since the decision; rows that differ are
// highlighted). Every time is a button that moves the shared replay there.

import { Link } from 'react-router-dom';

import type { OrderDetail } from '../../api/model';
import type { InstrumentMeta, OrderPair } from '../../api/types';
import { tifLabel } from '../../components/describe';
import { DASH, fmtDuration, fmtNum, fmtPrice, fmtQty, humanizeNsText } from '../../components/fmt';
import { paths } from '../../shell/paths';
import { fmtAxisNs, niceStep, ticksBetween } from '../forensics/axis';
import { eventTitle } from '../forensics/eventFormat';
import { useElementSize } from '../forensics/useElementSize';
import { differenceWords, fmtSec, outcomeWords, pairClassText, SCENARIO_KEYS, type Focus, type ScenarioKey } from './data';
import type { LatencyStory } from './story';

/** First local response that reported an execution: when the strategy learned of the fill. */
function learnedFillTs(d: OrderDetail): number | null {
  const ev = d.events.find((e) => e.body.type === 'local_response' && e.body.exec_qty > 0);
  return ev ? ev.ts : null;
}

interface Stage {
  key: string;
  label: string;
  clock: string;
  ts: (d: OrderDetail) => number | null;
}

const STAGES: Stage[] = [
  { key: 'decided', label: 'Strategy decides', clock: 'strategy clock', ts: (d) => d.order.decision_ts },
  { key: 'arrived', label: 'Order reaches the exchange', clock: 'exchange clock', ts: (d) => d.order.exch_arrival_ts },
  { key: 'confirmed', label: 'Exchange response reaches the strategy', clock: 'strategy clock', ts: (d) => d.order.first_response_ts },
  { key: 'fill', label: 'First fill', clock: 'exchange clock', ts: (d) => d.order.first_fill_ts },
  { key: 'learned', label: 'Strategy learns of the fill', clock: 'strategy clock', ts: learnedFillTs },
  { key: 'cancel', label: 'Cancel sent', clock: 'strategy clock', ts: (d) => d.order.cancel_sent_ts },
  { key: 'finished', label: 'Order finished', clock: 'exchange clock', ts: (d) => d.order.terminal_ts },
];

export interface LifecyclePanelProps {
  story: LatencyStory;
  orderId: number;
  details: Record<ScenarioKey, OrderDetail | null>;
  /** The stored comparison's verdict for this order, when known. */
  pair: OrderPair | 'identical' | null;
  inst: InstrumentMeta;
  names: Record<ScenarioKey, string>;
  focus: Focus;
  /** Moves the shared replay time. */
  onPick: (t: number) => void;
}

export function LifecyclePanel({ story, orderId, details, pair, inst, names, focus, onPick }: LifecyclePanelProps) {
  const any = details.fast ?? details.slow;
  if (!any) return <p className="ex-muted">Order {orderId} was not decided in either scenario.</p>;
  const o = any.order;
  const focusAttr = (k: ScenarioKey) => (focus === k ? '' : undefined);

  return (
    <div className="ex-life">
      <p>
        <span className={o.side === 'buy' ? 'ex-buy' : 'ex-sell'}>{o.side === 'buy' ? 'Buy' : 'Sell'}</span> {fmtQty(o.qty, inst)} at{' '}
        <span className="ex-num">{fmtPrice(o.price_tick, inst)}</span>, {tifLabel(o.tif)} {o.ord_type} order.
      </p>
      {pair === 'identical' ? (
        <p className="ex-verdict">
          <strong>Identical in both scenarios.</strong>
        </p>
      ) : pair ? (
        <p className="ex-verdict">
          <strong>{pairClassText(story, pair.class)}.</strong>
          {pair.differences.length > 0 ? ` Differs in: ${pair.differences.map(differenceWords).join(', ')}.` : null}
        </p>
      ) : !details.fast || !details.slow ? (
        <p className="ex-verdict">
          <strong>Placed only in the {details.fast ? names.fast : names.slow} scenario.</strong>
        </p>
      ) : null}

      <TimingDiagram details={details} names={names} focus={focus} onPick={onPick} />

      <div className="ex-scroll-x">
        <table className="ex-table ex-life-table">
          <caption className="ex-table-cap">
            Seconds after the start of the dataset; “+” is the time since that scenario’s decision. Highlighted rows differ. Select a time to move
            the replay there.
          </caption>
          <thead>
            <tr>
              <th scope="col">Step</th>
              {SCENARIO_KEYS.map((k) => (
                <th key={k} scope="col" data-focus={focusAttr(k)}>
                  <span className={`ex-swatch ex-swatch-${k}`} aria-hidden="true" /> {names[k]}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {STAGES.map((s) => {
              const v = { fast: details.fast ? s.ts(details.fast) : null, slow: details.slow ? s.ts(details.slow) : null };
              if (v.fast === null && v.slow === null) return null;
              const fromDecision = (k: ScenarioKey) => (v[k] ?? 0) - (details[k]?.order.decision_ts ?? 0);
              const differs = v.fast !== v.slow;
              return (
                <tr key={s.key} className={differs ? 'ex-life-differs' : undefined}>
                  <th scope="row">
                    {s.label}
                    <span className="ex-clock">{s.clock}</span>
                  </th>
                  {SCENARIO_KEYS.map((k) => {
                    const ts = v[k];
                    return (
                      <td key={k} data-focus={focusAttr(k)}>
                        {!details[k] ? (
                          <span className="ex-muted">not placed</span>
                        ) : ts === null ? (
                          <span className="ex-muted">did not happen</span>
                        ) : (
                          <button
                            type="button"
                            className="ex-timebtn"
                            onClick={() => onPick(ts)}
                            aria-label={`${s.label} in the ${names[k]} scenario at ${fmtSec(ts, 6)}. Show in the replay`}
                          >
                            <span className="ex-num">{fmtSec(ts, 6)}</span>
                            {s.key !== 'decided' ? <span className="ex-after"> +{fmtDuration(fromDecision(k))}</span> : null}
                          </button>
                        )}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
            <tr>
              <th scope="row">Queue ahead on arrival (modeled)</th>
              {SCENARIO_KEYS.map((k) => (
                <td key={k} className="ex-num-left" data-focus={focusAttr(k)}>
                  {details[k] ? fmtQty(details[k].order.queue_ahead_at_entry, inst) : DASH}
                </td>
              ))}
            </tr>
            <tr>
              <th scope="row">Outcome</th>
              {SCENARIO_KEYS.map((k) => {
                const d = details[k];
                return (
                  <td key={k} data-focus={focusAttr(k)}>
                    {d ? `${outcomeWords(d.order)}${d.order.filled_qty > 0 ? `, ${fmtQty(d.order.filled_qty, inst)} at ${fmtPrice(d.order.avg_fill_tick, inst)}` : ''}` : 'not placed'}
                  </td>
                );
              })}
            </tr>
            <tr>
              <th scope="row">Fee paid</th>
              {SCENARIO_KEYS.map((k) => (
                <td key={k} className="ex-num-left" data-focus={focusAttr(k)}>
                  {details[k] ? fmtNum(details[k].order.fees, 4) : DASH}
                </td>
              ))}
            </tr>
          </tbody>
        </table>
      </div>

      <div className="ex-row">
        <button type="button" onClick={() => onPick(o.decision_ts)}>
          Centre the replay on this order
        </button>
      </div>
      <ul className="ex-life-why">
        {SCENARIO_KEYS.map((k) => {
          const d = details[k];
          return d ? (
            <li key={k}>
              <span className="ex-tag-name">{names[k]}</span> {humanizeNsText(d.explanation.headline)}{' '}
              <Link to={paths.order((k === 'fast' ? story.fast : story.slow).runId, orderId)}>Full forensics for this order ({names[k]}) &rarr;</Link>
            </li>
          ) : null;
        })}
      </ul>
      <details className="ex-details">
        <summary>Every recorded event of this order</summary>
        {SCENARIO_KEYS.map((k) => (
          <div key={k}>
            <h3 className="ex-h3">{names[k]} scenario</h3>
            {details[k] ? (
              <ol className="ex-list">
                {details[k].events.map((ev) => (
                  <li key={ev.seq}>
                    <span className="ex-num">{fmtSec(ev.ts, 6)}</span> {eventTitle(ev)} <span className="ex-muted">({ev.clock} clock)</span>
                  </li>
                ))}
              </ol>
            ) : (
              <p className="ex-muted">Not placed in this scenario.</p>
            )}
          </div>
        ))}
      </details>
    </div>
  );
}

// ----- timing diagram ---------------------------------------------------------------------

const LANE = 36;
const PAD = { l: 72, r: 16, t: 8, b: 28 };

function TimingDiagram({
  details,
  names,
  focus,
  onPick,
}: {
  details: Record<ScenarioKey, OrderDetail | null>;
  names: Record<ScenarioKey, string>;
  focus: Focus;
  onPick: (t: number) => void;
}) {
  const [attach, size] = useElementSize<HTMLDivElement>();
  const ms = {
    fast: details.fast ? Object.fromEntries(STAGES.map((s) => [s.key, s.ts(details.fast!)])) : null,
    slow: details.slow ? Object.fromEntries(STAGES.map((s) => [s.key, s.ts(details.slow!)])) : null,
  } as Record<ScenarioKey, Record<string, number | null> | null>;
  const times = SCENARIO_KEYS.flatMap((k) => (ms[k] ? Object.values(ms[k]!).filter((v): v is number => v !== null) : []));
  const t0 = Math.min(...times);
  const t1 = Math.max(...times, t0 + 1);
  const span = t1 - t0;
  const a = t0 - span * 0.04;
  const b = t1 + span * 0.04;
  const width = Math.max(260, Math.floor(size.width));
  const height = PAD.t + LANE * 2 + PAD.b;
  const plotW = width - PAD.l - PAD.r;
  const x = (ts: number) => PAD.l + ((ts - a) / (b - a)) * plotW;
  const xStep = niceStep((b - a) / Math.max(2, Math.floor(plotW / 110)));
  const label = SCENARIO_KEYS.map((k) => {
    const m = ms[k];
    if (!m) return `${names[k]}: not decided`;
    const since = (v: number | null) => (v === null ? null : fmtDuration(v - (m.decided as number)));
    return `${names[k]}: reached the exchange ${m.arrived === null ? 'never' : `${since(m.arrived)} after the decision`}, ${m.fill === null ? 'no fill' : `filled ${since(m.fill)} after the decision`}`;
  }).join('; ');

  return (
    <div ref={attach} className="ex-timing">
      {size.width > 0 ? (
        <svg width={width} height={height} className="ex-svg" role="img" aria-label={`Order timing. ${label}.`}>
          {ticksBetween(a, b, xStep).map((ts) => (
            <g key={ts}>
              <line x1={x(ts)} x2={x(ts)} y1={PAD.t} y2={PAD.t + LANE * 2} className="ex-grid" />
              <text x={x(ts)} y={height - 8} textAnchor="middle" className="ex-axis">
                {fmtAxisNs(ts, xStep, b)}
              </text>
            </g>
          ))}
          {SCENARIO_KEYS.map((k, i) => {
            const m = ms[k];
            const y = PAD.t + LANE * i + LANE / 2;
            return (
              <g key={k} className={`ex-lane ex-lane-${k}`} data-dim={focus !== 'both' && focus !== k ? '' : undefined}>
                <text x={PAD.l - 8} y={y + 5} textAnchor="end" className="ex-lane-label">
                  {names[k]}
                </text>
                {m ? (
                  <>
                    {m.arrived !== null ? <line x1={x(m.decided!)} x2={x(m.arrived)} y1={y} y2={y} className="ex-seg-travel" /> : null}
                    {m.arrived !== null ? <line x1={x(m.arrived)} x2={x(m.finished ?? t1)} y1={y} y2={y} className="ex-seg-rest" /> : null}
                    {m.fill !== null && m.learned !== null ? <line x1={x(m.fill)} x2={x(m.learned)} y1={y + 8} y2={y + 8} className="ex-seg-back" /> : null}
                    <Mark kind="decided" cx={x(m.decided!)} cy={y} onPick={() => onPick(m.decided!)} />
                    {m.arrived !== null ? <Mark kind="arrived" cx={x(m.arrived)} cy={y} onPick={() => onPick(m.arrived!)} /> : null}
                    {m.fill !== null ? <Mark kind="fill" cx={x(m.fill)} cy={y} onPick={() => onPick(m.fill!)} /> : null}
                    {m.learned !== null ? <Mark kind="learned" cx={x(m.learned)} cy={y + 8} onPick={() => onPick(m.learned!)} /> : null}
                  </>
                ) : (
                  <text x={PAD.l + 4} y={y + 5} className="ex-axis">
                    not decided in this scenario
                  </text>
                )}
              </g>
            );
          })}
        </svg>
      ) : null}
      <div className="ex-legend-row" aria-hidden="true">
        <span className="ex-key"><span className="ex-mk ex-mk-decided" /> decided</span>
        <span className="ex-key"><span className="ex-legseg ex-legseg-travel" /> travelling to the exchange</span>
        <span className="ex-key"><span className="ex-mk ex-mk-arrived" /> reached the exchange</span>
        <span className="ex-key"><span className="ex-legseg ex-legseg-rest" /> resting</span>
        <span className="ex-key"><span className="ex-mk ex-mk-fill" /> filled</span>
        <span className="ex-key"><span className="ex-mk ex-mk-learned" /> strategy learned of the fill</span>
      </div>
    </div>
  );
}

function Mark({ kind, cx, cy, onPick }: { kind: 'decided' | 'arrived' | 'fill' | 'learned'; cx: number; cy: number; onPick: () => void }) {
  const cls = `ex-mark ex-mark-${kind}`;
  if (kind === 'decided') return <path d={`M${cx},${cy - 6}L${cx + 6},${cy}L${cx},${cy + 6}L${cx - 6},${cy}Z`} className={cls} onClick={onPick} />;
  if (kind === 'fill') return <rect x={cx - 5} y={cy - 5} width={10} height={10} className={cls} onClick={onPick} />;
  return <circle cx={cx} cy={cy} r={kind === 'learned' ? 3.5 : 5} className={cls} onClick={onPick} />;
}
