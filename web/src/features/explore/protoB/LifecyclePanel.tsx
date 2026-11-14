// One order's lifecycle in both scenarios, side by side: a two-lane timing diagram and a
// milestone table. Every time is a button that moves the shared replay to that moment.

import { Link } from 'react-router-dom';

import type { OrderDetail } from '../../../api/model';
import type { InstrumentMeta, OrderRecord } from '../../../api/types';
import { DASH, fmtDuration, fmtNum, fmtPrice, fmtQty, humanizeNsText, words } from '../../../components/fmt';
import { ClockText } from '../../../components/ui';
import { paths } from '../../../shell/paths';
import { fmtAxisNs, niceStep, ticksBetween } from '../../forensics/axis';
import { fmtSince } from '../../forensics/labels';
import { useElementSize } from '../../forensics/useElementSize';
import type { LatencyStory } from '../story';
import { TIF_TEXT, type Focus, type ScenarioKey } from './data';

const KEYS: ScenarioKey[] = ['fast', 'slow'];

export interface LifecyclePanelProps {
  story: LatencyStory;
  orderId: number;
  details: Record<ScenarioKey, OrderDetail | null>;
  inst: InstrumentMeta;
  names: Record<ScenarioKey, string>;
  focus: Focus;
  /** Moves the shared replay time. */
  onPick: (t: number) => void;
}

/** First local response that reported an execution: when the strategy learned of the fill. */
function learnedFillTs(d: OrderDetail): number | null {
  const ev = d.events.find((e) => e.body.type === 'local_response' && e.body.exec_qty > 0);
  return ev ? ev.ts : null;
}

interface Milestones {
  decided: number;
  arrived: number | null;
  confirmed: number | null;
  firstFill: number | null;
  learnedFill: number | null;
  cancelSent: number | null;
  finished: number | null;
}

function milestones(d: OrderDetail): Milestones {
  const o = d.order;
  return {
    decided: o.decision_ts,
    arrived: o.exch_arrival_ts,
    confirmed: o.first_response_ts,
    firstFill: o.first_fill_ts,
    learnedFill: learnedFillTs(d),
    cancelSent: o.cancel_sent_ts,
    finished: o.terminal_ts,
  };
}

const TIME_ROWS: { key: keyof Milestones; label: string }[] = [
  { key: 'decided', label: 'Strategy decided to place it' },
  { key: 'arrived', label: 'Reached the exchange' },
  { key: 'confirmed', label: 'Exchange confirmation reached the strategy' },
  { key: 'firstFill', label: 'Filled at the exchange' },
  { key: 'learnedFill', label: 'Strategy learned of the fill' },
  { key: 'cancelSent', label: 'Strategy sent a cancel' },
  { key: 'finished', label: 'Order finished' },
];

function orderLine(o: OrderRecord, inst: InstrumentMeta): string {
  return `${o.side === 'buy' ? 'Buy' : 'Sell'} ${fmtQty(o.qty, inst)} at ${fmtPrice(o.price_tick, inst)}, ${TIF_TEXT[o.tif] ?? o.tif} ${o.ord_type}`;
}

export function LifecyclePanel({ story, orderId, details, inst, names, focus, onPick }: LifecyclePanelProps) {
  const ms = { fast: details.fast ? milestones(details.fast) : null, slow: details.slow ? milestones(details.slow) : null };
  const any = details.fast ?? details.slow;
  if (!any) return <p className="pb-empty">Order {orderId} was not decided in either scenario.</p>;
  const runOf = (k: ScenarioKey) => (k === 'fast' ? story.fast : story.slow);

  return (
    <div className="pb-life">
      <p className="pb-life-head">
        <strong>Order {orderId}</strong> &middot; {orderLine(any.order, inst)}
      </p>
      <TimingDiagram ms={ms} names={names} focus={focus} onPick={onPick} />
      <div className="pb-scroll-x">
      <table className="pb-table pb-life-table">
        <caption className="pb-table-cap">
          Times after each scenario&rsquo;s decision (the decision row shows time since the dataset start). Select a time to move the replay there.
        </caption>
        <thead>
          <tr>
            <th scope="col">Step</th>
            {KEYS.map((k) => (
              <th key={k} scope="col" data-focus={focus === k ? '' : undefined}>
                {names[k]}
              </th>
            ))}
            <th scope="col" className="num">
              {names.slow} vs {names.fast}
            </th>
          </tr>
        </thead>
        <tbody>
          {TIME_ROWS.filter((r) => ms.fast?.[r.key] != null || ms.slow?.[r.key] != null).map((r) => {
            const a = ms.fast?.[r.key] ?? null;
            const b = ms.slow?.[r.key] ?? null;
            return (
              <tr key={r.key}>
                <th scope="row">{r.label}</th>
                {[a, b].map((v, i) => {
                  const k = KEYS[i];
                  const origin = ms[k]?.decided ?? 0;
                  return (
                    <td key={k} data-focus={focus === k ? '' : undefined}>
                      {v === null ? (
                        <span className="dim">{details[k] ? 'did not happen' : 'not decided'}</span>
                      ) : (
                        <button type="button" className="pb-timebtn" onClick={() => onPick(v)} aria-label={`${r.label} in ${names[k]}: ${fmtDuration(v)} after the dataset start. Show in replay`}>
                          {r.key === 'decided' ? <ClockText ns={v} digits={3} /> : <span className="mono">{fmtSince(v, origin)}</span>}
                        </button>
                      )}
                    </td>
                  );
                })}
                <td className="num">{a !== null && b !== null ? (b === a ? 'same time' : `${fmtDuration(Math.abs(b - a))} ${b > a ? 'later' : 'earlier'}`) : DASH}</td>
              </tr>
            );
          })}
          <tr>
            <th scope="row">Queue ahead on arrival (modeled)</th>
            {KEYS.map((k) => (
              <td key={k} className="num-left" data-focus={focus === k ? '' : undefined}>
                {details[k] ? fmtQty(details[k]!.order.queue_ahead_at_entry, inst) : DASH}
              </td>
            ))}
            <td />
          </tr>
          <tr>
            <th scope="row">Outcome</th>
            {KEYS.map((k) => (
              <td key={k} data-focus={focus === k ? '' : undefined}>
                {details[k] ? `${words(details[k]!.order.outcome)}${details[k]!.order.filled_qty > 0 ? `, ${fmtQty(details[k]!.order.filled_qty, inst)} at ${fmtPrice(details[k]!.order.avg_fill_tick, inst)}` : ''}` : 'not decided in this scenario'}
              </td>
            ))}
            <td />
          </tr>
          <tr>
            <th scope="row">Fee paid</th>
            {KEYS.map((k) => (
              <td key={k} className="num-left" data-focus={focus === k ? '' : undefined}>
                {details[k] ? fmtNum(details[k]!.order.fees, 4) : DASH}
              </td>
            ))}
            <td />
          </tr>
        </tbody>
      </table>
      </div>
      <ul className="pb-life-why">
        {KEYS.map((k) =>
          details[k] ? (
            <li key={k}>
              <span className="pb-tag">{names[k]}</span> {humanizeNsText(details[k]!.explanation.headline)}{' '}
              <Link to={paths.order(runOf(k).runId, orderId)}>Full forensics for this order ({names[k]}) &rarr;</Link>
            </li>
          ) : null,
        )}
      </ul>
    </div>
  );
}

// ----- timing diagram ---------------------------------------------------------------------

const LANE = 34;
const PAD = { l: 64, r: 14, t: 8, b: 24 };

function TimingDiagram({
  ms,
  names,
  focus,
  onPick,
}: {
  ms: Record<ScenarioKey, Milestones | null>;
  names: Record<ScenarioKey, string>;
  focus: Focus;
  onPick: (t: number) => void;
}) {
  const [attach, size] = useElementSize<HTMLDivElement>();
  const times = KEYS.flatMap((k) => (ms[k] ? Object.values(ms[k]!).filter((v): v is number => v !== null) : []));
  const t0 = Math.min(...times);
  const t1 = Math.max(...times, t0 + 1);
  const span = t1 - t0;
  const a = t0 - span * 0.04;
  const b = t1 + span * 0.04;
  const width = Math.max(260, Math.floor(size.width));
  const height = PAD.t + LANE * 2 + PAD.b;
  const plotW = width - PAD.l - PAD.r;
  const x = (ts: number) => PAD.l + ((ts - a) / (b - a)) * plotW;
  const xStep = niceStep((b - a) / Math.max(2, Math.floor(plotW / 100)));
  const label = KEYS.map((k) => {
    const m = ms[k];
    if (!m) return `${names[k]}: not decided`;
    return `${names[k]}: reached the exchange ${m.arrived === null ? 'never' : `${fmtDuration(m.arrived - m.decided)} after the decision`}, ${m.firstFill === null ? 'no fill' : `filled ${fmtDuration(m.firstFill - m.decided)} after the decision`}`;
  }).join('; ');

  return (
    <div ref={attach} className="pb-timing">
      {size.width > 0 ? (
        <svg width={width} height={height} className="pb-svg" role="img" aria-label={`Order timing. ${label}.`}>
          {ticksBetween(a, b, xStep).map((ts) => (
            <g key={ts}>
              <line x1={x(ts)} x2={x(ts)} y1={PAD.t} y2={PAD.t + LANE * 2} className="pb-grid" />
              <text x={x(ts)} y={height - 7} textAnchor="middle" className="pb-axis">
                {fmtAxisNs(ts, xStep, b)}
              </text>
            </g>
          ))}
          {KEYS.map((k, i) => {
            const m = ms[k];
            const y = PAD.t + LANE * i + LANE / 2;
            return (
              <g key={k} className={`pb-lane pb-lane-${k}`} data-dim={focus !== 'both' && focus !== k ? '' : undefined}>
                <text x={PAD.l - 8} y={y + 4} textAnchor="end" className="pb-lane-label">
                  {names[k]}
                </text>
                {m ? (
                  <>
                    {m.arrived !== null ? <line x1={x(m.decided)} x2={x(m.arrived)} y1={y} y2={y} className="pb-seg-travel" /> : null}
                    {m.arrived !== null ? <line x1={x(m.arrived)} x2={x(m.finished ?? t1)} y1={y} y2={y} className="pb-seg-rest" /> : null}
                    {m.firstFill !== null && m.learnedFill !== null ? (
                      <line x1={x(m.firstFill)} x2={x(m.learnedFill)} y1={y + 7} y2={y + 7} className="pb-seg-back" />
                    ) : null}
                    <Mark kind="decided" cx={x(m.decided)} cy={y} onPick={() => onPick(m.decided)} />
                    {m.arrived !== null ? <Mark kind="arrived" cx={x(m.arrived)} cy={y} onPick={() => onPick(m.arrived!)} /> : null}
                    {m.firstFill !== null ? <Mark kind="fill" cx={x(m.firstFill)} cy={y} onPick={() => onPick(m.firstFill!)} /> : null}
                    {m.learnedFill !== null ? <Mark kind="learned" cx={x(m.learnedFill)} cy={y + 7} onPick={() => onPick(m.learnedFill!)} /> : null}
                  </>
                ) : (
                  <text x={PAD.l + 4} y={y + 4} className="pb-axis">
                    not decided in this scenario
                  </text>
                )}
              </g>
            );
          })}
        </svg>
      ) : null}
      <div className="pb-legend pb-legend-small" aria-hidden="true">
        <span className="pb-key"><span className="pb-mk pb-mk-decided" /> decided</span>
        <span className="pb-key"><span className="pb-seg pb-seg-travel-k" /> travelling to exchange</span>
        <span className="pb-key"><span className="pb-mk pb-mk-arrived" /> reached exchange</span>
        <span className="pb-key"><span className="pb-seg pb-seg-rest-k" /> resting</span>
        <span className="pb-key"><span className="pb-mk pb-mk-fill" /> filled</span>
        <span className="pb-key"><span className="pb-mk pb-mk-learned" /> strategy learned of fill</span>
      </div>
    </div>
  );
}

function Mark({ kind, cx, cy, onPick }: { kind: 'decided' | 'arrived' | 'fill' | 'learned'; cx: number; cy: number; onPick: () => void }) {
  const cls = `pb-mark pb-mark-${kind}`;
  if (kind === 'decided') return <path d={`M${cx},${cy - 6}L${cx + 6},${cy}L${cx},${cy + 6}L${cx - 6},${cy}Z`} className={cls} onClick={onPick} />;
  if (kind === 'fill') return <rect x={cx - 5} y={cy - 5} width={10} height={10} className={cls} onClick={onPick} />;
  return <circle cx={cx} cy={cy} r={kind === 'learned' ? 3.5 : 5} className={cls} onClick={onPick} />;
}
