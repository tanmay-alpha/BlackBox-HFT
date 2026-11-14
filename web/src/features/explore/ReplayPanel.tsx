// Synchronized replay: the observed best bid/ask and trades around the shared time, both
// scenarios' simulated fills on top, and the book ladder at the shared time with each scenario's
// resting orders marked. The window comes from `loadMoment` (≤ 1 s, complete or flagged).

import { useMemo } from 'react';

import type { RunTimeline } from '../../../api/model';
import type { DepthFrame, InstrumentMeta, OrderRecord, ReplayWindow } from '../../../api/types';
import { fmtDuration, fmtPrice, fmtQty } from '../../../components/fmt';
import { ClockText, ErrorBox, Notice, Spinner, useAsync } from '../../../components/ui';
import { fmtAxisNs, lastAtOrBefore, niceStep, ticksBetween } from '../../forensics/axis';
import { useElementSize } from '../../forensics/useElementSize';
import { loadMoment, type LatencyStory, type MomentWindow } from '../story';
import type { Focus, ScenarioKey } from './data';

const KEYS: ScenarioKey[] = ['fast', 'slow'];
const LADDER_HALF = 6;

export interface ReplayPanelProps {
  story: LatencyStory;
  inst: InstrumentMeta;
  names: Record<ScenarioKey, string>;
  win: MomentWindow;
  t: number;
  focus: Focus;
  /** Selected order id, highlighted in the ladder and chart. */
  orderId: number | null;
  onPick: (t: number) => void;
}

/** Orders resting at the simulated exchange at `t`. */
function restingAt(tl: RunTimeline, t: number): OrderRecord[] {
  return tl.orders.filter(
    (o) => o.exch_arrival_ts !== null && o.exch_arrival_ts <= t && (o.terminal_ts === null || t < o.terminal_ts) && o.outcome !== 'refused',
  );
}

export function ReplayPanel({ story, inst, names, win, t, focus, orderId, onPick }: ReplayPanelProps) {
  const moment = useAsync((signal) => loadMoment(story, win, signal), [story, win.t0, win.t1]);
  const shown = KEYS.filter((k) => focus === 'both' || focus === k);

  // Times worth stopping at: decisions, exchange arrivals and fills of the shown scenarios.
  const stops = useMemo(() => {
    if (!moment.data) return [];
    const set = new Set<number>();
    for (const k of shown) {
      const tl = moment.data[k];
      for (const f of tl.fills) set.add(f.ts);
      for (const o of tl.orders) {
        if (o.decision_ts >= win.t0 && o.decision_ts <= win.t1) set.add(o.decision_ts);
        if (o.exch_arrival_ts !== null && o.exch_arrival_ts >= win.t0 && o.exch_arrival_ts <= win.t1) set.add(o.exch_arrival_ts);
      }
    }
    return [...set].sort((a, b) => a - b);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [moment.data, focus, win.t0, win.t1]);
  const prev = [...stops].reverse().find((s) => s < t) ?? null;
  const next = stops.find((s) => s > t) ?? null;

  return (
    <div className="pb-replay">
      <div className="pb-toolbar" role="group" aria-label="Replay time">
        <span className="pb-time">
          <span className="dim">Time</span> <ClockText ns={t} />
        </span>
        <button type="button" disabled={prev === null} onClick={() => prev !== null && onPick(prev)}>
          &larr; Previous event
        </button>
        <button type="button" disabled={next === null} onClick={() => next !== null && onPick(next)}>
          Next event &rarr;
        </button>
        <label className="pb-slider">
          <span className="visually-hidden">Replay time within this window</span>
          <input
            type="range"
            min={win.t0}
            max={win.t1}
            step={1_000_000}
            value={Math.min(win.t1, Math.max(win.t0, t))}
            aria-valuetext={`${fmtDuration(t)} after the dataset start`}
            onChange={(e) => onPick(Number(e.target.value))}
          />
        </label>
      </div>
      {moment.error ? (
        <ErrorBox error={moment.error} onRetry={moment.reload} />
      ) : !moment.data ? (
        <div className="pb-loading">
          <Spinner label="Loading the market around this time" />
        </div>
      ) : (
        <>
          {!moment.data.complete ? (
            <Notice tag="PARTIAL">This window holds more events than one response carries; some events are not drawn.</Notice>
          ) : null}
          <div className="pb-replay-body">
            <BboChart
              market={moment.data.market}
              timelines={{ fast: moment.data.fast, slow: moment.data.slow }}
              shown={shown}
              names={names}
              inst={inst}
              t={t}
              orderId={orderId}
              onPick={onPick}
            />
            <Ladder
              frame={frameAt(moment.data.market.depth, t)}
              resting={Object.fromEntries(KEYS.map((k) => [k, shown.includes(k) ? restingAt(moment.data![k], t) : []])) as Record<ScenarioKey, OrderRecord[]>}
              names={names}
              inst={inst}
              orderId={orderId}
            />
          </div>
          <p className="pb-caption">
            Book and trades are the recorded (synthetic) market; simulated orders never change it, so the market is
            identical in both scenarios. Window {fmtDuration(win.t1 - win.t0)}.
          </p>
        </>
      )}
    </div>
  );
}

function frameAt(depth: DepthFrame[], t: number): DepthFrame | null {
  const i = lastAtOrBefore(
    depth.map((d) => d.ts),
    t,
  );
  return i < 0 ? (depth[0] ?? null) : depth[i];
}

// ----- BBO chart --------------------------------------------------------------------------

const PAD = { l: 64, r: 12, t: 10, b: 24 };

interface BboChartProps {
  market: ReplayWindow;
  timelines: Record<ScenarioKey, RunTimeline>;
  shown: ScenarioKey[];
  names: Record<ScenarioKey, string>;
  inst: InstrumentMeta;
  t: number;
  orderId: number | null;
  onPick: (t: number) => void;
}

function BboChart({ market, timelines, shown, names, inst, t, orderId, onPick }: BboChartProps) {
  const [attach, size] = useElementSize<HTMLDivElement>();
  const width = Math.max(260, Math.floor(size.width));
  const height = Math.max(180, Math.floor(size.height));
  const plotW = width - PAD.l - PAD.r;
  const plotH = height - PAD.t - PAD.b;
  const { bbo, trades } = market;

  const g = useMemo(() => {
    let lo = Infinity;
    let hi = -Infinity;
    for (let i = 0; i < bbo.ts.length; i++) {
      const b = bbo.bid_tick[i];
      const a = bbo.ask_tick[i];
      if (b !== null) lo = Math.min(lo, b);
      if (a !== null) hi = Math.max(hi, a);
    }
    if (!Number.isFinite(lo) || !Number.isFinite(hi)) return null;
    lo -= 1;
    hi += 1;
    const t0 = market.t0;
    const t1 = market.t1;
    const x = (ts: number) => PAD.l + ((ts - t0) / (t1 - t0)) * plotW;
    const y = (tick: number) => PAD.t + ((hi - tick) / (hi - lo)) * plotH;
    const step = (ticks: (number | null)[]) => {
      let d = '';
      for (let i = 0; i < bbo.ts.length; i++) {
        const v = ticks[i];
        const xa = x(Math.max(t0, bbo.ts[i]));
        const xb = x(i + 1 < bbo.ts.length ? bbo.ts[i + 1] : t1);
        if (v === null) continue;
        d += `M${xa.toFixed(1)},${y(v).toFixed(1)}H${xb.toFixed(1)}`;
      }
      return d;
    };
    const xStep = niceStep((t1 - t0) / Math.max(2, Math.floor(plotW / 110)));
    const yStep = Math.max(1, Math.round(niceStep((hi - lo) / 5)));
    return {
      t0,
      t1,
      x,
      y,
      bid: step(bbo.bid_tick),
      ask: step(bbo.ask_tick),
      xTicks: ticksBetween(t0, t1, xStep).map((ts) => ({ x: x(ts), label: fmtAxisNs(ts, xStep, t1) })),
      yTicks: ticksBetween(Math.ceil(lo), Math.floor(hi), yStep).map((tick) => ({ y: y(tick), label: fmtPrice(tick, inst) })),
    };
  }, [bbo, market.t0, market.t1, plotW, plotH, inst]);

  const fillCount = shown.map((k) => `${names[k]}: ${timelines[k].fills.length} fills`).join(', ');
  return (
    <div className="pb-bbo">
      <div ref={attach} className="pb-bbo-stage">
      {g && size.width > 0 ? (
        <svg
          width={width}
          height={height}
          className="pb-svg pb-clickable"
          role="img"
          aria-label={`Best bid and ask from ${fmtDuration(market.t0)} to ${fmtDuration(market.t1)} with ${trades.ts.length} market trades and simulated fills (${fillCount}). Click to move the replay time.`}
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            const frac = (e.clientX - r.left - PAD.l) / plotW;
            if (frac >= 0 && frac <= 1) onPick(Math.round(g.t0 + frac * (g.t1 - g.t0)));
          }}
        >
          {g.yTicks.map((tk) => (
            <g key={tk.label}>
              <line x1={PAD.l} x2={width - PAD.r} y1={tk.y} y2={tk.y} className="pb-grid" />
              <text x={PAD.l - 6} y={tk.y + 4} textAnchor="end" className="pb-axis">
                {tk.label}
              </text>
            </g>
          ))}
          {g.xTicks.map((tk) => (
            <text key={tk.label} x={tk.x} y={height - 7} textAnchor="middle" className="pb-axis">
              {tk.label}
            </text>
          ))}
          <path d={g.bid} className="pb-bid-line" />
          <path d={g.ask} className="pb-ask-line" />
          {trades.ts.map((ts, i) => (
            <circle key={`tr${i}`} cx={g.x(ts)} cy={g.y(trades.price_tick[i])} r={2.5} className="pb-trade" />
          ))}
          {shown.map((k) =>
            timelines[k].orders
              .filter((o) => o.order_id === orderId && o.exch_arrival_ts !== null)
              .map((o) => (
                <line
                  key={`ord-${k}`}
                  x1={g.x(Math.max(g.t0, o.exch_arrival_ts as number))}
                  x2={g.x(Math.min(g.t1, o.terminal_ts ?? g.t1))}
                  y1={g.y(o.price_tick)}
                  y2={g.y(o.price_tick)}
                  className={`pb-order-line pb-order-${k}`}
                />
              )),
          )}
          {shown.map((k) =>
            timelines[k].fills.map((f) =>
              k === 'fast' ? (
                <circle key={`f${k}${f.seq}`} cx={g.x(f.ts)} cy={g.y(f.price_tick)} r={5} className="pb-fill pb-fill-fast" data-sel={f.order_id === orderId ? '' : undefined} />
              ) : (
                <rect key={`f${k}${f.seq}`} x={g.x(f.ts) - 5} y={g.y(f.price_tick) - 5} width={10} height={10} className="pb-fill pb-fill-slow" data-sel={f.order_id === orderId ? '' : undefined} />
              ),
            ),
          )}
          <line x1={g.x(t)} x2={g.x(t)} y1={PAD.t} y2={PAD.t + plotH} className="pb-cursor-line" />
        </svg>
      ) : null}
      </div>
      <div className="pb-legend pb-legend-small" aria-hidden="true">
        <span className="pb-key pb-key-bid"><span className="pb-swatch" /> best bid</span>
        <span className="pb-key pb-key-ask"><span className="pb-swatch" /> best ask</span>
        <span className="pb-key pb-key-trade"><span className="pb-dot" /> market trade</span>
        {shown.includes('fast') ? <span className="pb-key pb-key-fillfast"><span className="pb-dot" /> {names.fast} fill</span> : null}
        {shown.includes('slow') ? <span className="pb-key pb-key-fillslow"><span className="pb-dot" /> {names.slow} fill</span> : null}
      </div>
    </div>
  );
}

// ----- ladder -----------------------------------------------------------------------------

function Ladder({
  frame,
  resting,
  names,
  inst,
  orderId,
}: {
  frame: DepthFrame | null;
  resting: Record<ScenarioKey, OrderRecord[]>;
  names: Record<ScenarioKey, string>;
  inst: InstrumentMeta;
  orderId: number | null;
}) {
  if (!frame || frame.bids.length === 0 || frame.asks.length === 0) {
    return <div className="pb-ladder pb-empty">No two-sided book at this time.</div>;
  }
  const bestBid = frame.bids[0][0];
  const bestAsk = frame.asks[0][0];
  const qty = new Map<number, ['bid' | 'ask', number]>();
  for (const [p, q] of frame.bids) qty.set(p, ['bid', q]);
  for (const [p, q] of frame.asks) qty.set(p, ['ask', q]);
  const top = bestAsk + LADDER_HALF - 1;
  const bottom = bestBid - LADDER_HALF + 1;
  const rows: number[] = [];
  for (let p = top; p >= bottom; p--) rows.push(p);
  const maxQ = Math.max(...[...qty.values()].map(([, q]) => q), 1e-12);

  return (
    <table className="pb-ladder" aria-label={`Order book at ${fmtDuration(frame.ts)}, with resting simulated orders`}>
      <caption className="pb-ladder-cap">
        Book at <ClockText ns={frame.ts} />
      </caption>
      <thead>
        <tr>
          <th scope="col">Price</th>
          <th scope="col" className="num">Size</th>
          <th scope="col">Our orders</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((p) => {
          const level = qty.get(p);
          const side = level?.[0] ?? (p >= bestAsk ? 'ask' : p <= bestBid ? 'bid' : 'gap');
          const ours = (['fast', 'slow'] as const).flatMap((k) => resting[k].filter((o) => o.price_tick === p).map((o) => ({ k, o })));
          return (
            <tr key={p} className={`pb-lvl pb-lvl-${side}`}>
              <td className="num">{fmtPrice(p, inst)}</td>
              <td className="num pb-qty">
                {level ? <span className="pb-bar" style={{ width: `${(level[1] / maxQ) * 100}%` }} aria-hidden="true" /> : null}
                <span>{level ? fmtQty(level[1], inst) : ''}</span>
              </td>
              <td className="pb-ours">
                {ours.map(({ k, o }) => (
                  <span key={`${k}${o.order_id}`} className={`pb-chip pb-chip-${k}`} data-sel={o.order_id === orderId ? '' : undefined}>
                    {names[k]} {o.side}
                  </span>
                ))}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
