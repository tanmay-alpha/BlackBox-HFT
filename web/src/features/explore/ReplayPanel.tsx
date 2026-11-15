// The synchronized replay: the observed best bid/ask of the window around the shared time with
// both scenarios' simulated arrivals and fills (PriceChart), and the book at that same time with
// each scenario's resting orders. One time drives both: the chart's time line and the book.
// `between` is rendered between the chart and the book (the page puts the order lifecycle there,
// so on narrow screens it follows the chart directly; wide layouts place the parts in a grid).

import { useMemo, type ReactNode, type RefObject } from 'react';

import type { InstrumentMeta } from '../../api/types';
import { Spinner } from '../../components/ui';
import { lastAtOrBefore } from '../forensics/axis';
import { fmtSec, restingAt, shows, type Focus, type ScenarioKey } from './data';
import { Ladder } from './Ladder';
import { PlainError } from './PlainError';
import { PriceChart } from './PriceChart';
import type { Moment } from './story';

export interface ReplayPanelProps {
  inst: InstrumentMeta;
  names: Record<ScenarioKey, string>;
  /** The loaded window, or the last good one while a new window loads or failed. */
  moment: (Moment & { complete: boolean }) | null;
  loading: boolean;
  error: Error | null;
  onRetry: () => void;
  t: number;
  focus: Focus;
  selected: number | null;
  onSelectOrder: (orderId: number) => void;
  onPick: (t: number) => void;
  between?: ReactNode;
  chartRef?: RefObject<HTMLElement | null>;
}

export function ReplayPanel({ inst, names, moment, loading, error, onRetry, t, focus, selected, onSelectOrder, onPick, between, chartRef }: ReplayPanelProps) {
  // The window on screen holds `t` unless a new window is still loading or failed to load.
  const current = moment !== null && t >= moment.window.t0 && t <= moment.window.t1;
  const frame = useMemo(() => {
    if (!moment || !current) return null;
    const i = lastAtOrBefore(
      moment.market.depth.map((d) => d.ts),
      t,
    );
    return i < 0 ? null : moment.market.depth[i];
  }, [moment, current, t]);
  const resting = useMemo(
    () =>
      moment && current
        ? { fast: shows(focus, 'fast') ? restingAt(moment.fast, t) : [], slow: shows(focus, 'slow') ? restingAt(moment.slow, t) : [] }
        : { fast: [], slow: [] },
    [moment, current, t, focus],
  );

  return (
    <>
      <section ref={chartRef} className="ex-replay-chart" aria-labelledby="ex-replay-title">
        <div className="ex-replay-head">
          <h2 id="ex-replay-title">
            Market replay{moment ? ` ${fmtSec(moment.window.t0, 3)} – ${fmtSec(moment.window.t1, 3)}` : ''}
          </h2>
          <span className="ex-time">
            Replay time <span className="ex-num">{fmtSec(t, 6)}</span>
          </span>
          {loading ? <Spinner label="Loading the market around this time" /> : null}
        </div>
        {error ? (
          <PlainError
            error={error}
            onRetry={onRetry}
            what={moment ? `The market around ${fmtSec(t, 3)} could not be loaded; the chart still shows the previous window` : 'Market replay'}
          />
        ) : null}
        {moment && !moment.complete ? (
          <p className="ex-warn" role="note">
            This window holds more market trades than one response carries; some trades are not drawn.
          </p>
        ) : null}
        <div className={`ex-chart-wrap${loading || !current ? ' is-stale' : ''}`}>
          {moment ? (
            <PriceChart moment={moment} focus={focus} inst={inst} selected={selected} names={names} t={t} onSelectOrder={onSelectOrder} onPick={onPick} />
          ) : !error ? (
            <div className="ex-chart-empty">
              <Spinner label="Loading the market replay" />
            </div>
          ) : null}
        </div>
        <ul className="ex-legend" aria-label="Chart legend">
          <li>
            <span className="ex-key-line ex-key-bid" aria-hidden="true" /> Best bid <span className="ex-tagsm">observed</span>
          </li>
          <li>
            <span className="ex-key-line ex-key-ask" aria-hidden="true" /> Best ask <span className="ex-tagsm">observed</span>
          </li>
          {(['fast', 'slow'] as const)
            .filter((k) => shows(focus, k))
            .map((k) => (
              <li key={k}>
                <span className={`ex-key-mark ex-key-${k}`} aria-hidden="true" /> {names[k]}: order reaches the exchange ·{' '}
                <span className={`ex-${k}-text`} aria-hidden="true">
                  ▲▼
                </span>{' '}
                fill <span className="ex-tagsm ex-tagsm-sim">simulated</span>
              </li>
            ))}
          <li>
            <span className="ex-key-line ex-key-cursor" aria-hidden="true" /> replay time
          </li>
        </ul>
        <details className="ex-chart-note">
          <summary>How to read this replay</summary>
          <p className="ex-muted">
            Drag or scroll to zoom. Click a marker to open its order; click anywhere else to move the replay time there. The book and trades are the
            recorded synthetic market; simulated orders never change it, so it is identical in both scenarios. Chart drawn with{' '}
            <a href="https://www.tradingview.com/" target="_blank" rel="noreferrer">TradingView Lightweight Charts</a>.
          </p>
        </details>
      </section>
      {between}
      <section className="ex-book" aria-labelledby="ex-book-title">
        <h2 id="ex-book-title">Order book at the replay time</h2>
        {moment && current ? (
          <Ladder frame={frame} t={t} inst={inst} resting={resting} focus={focus} names={names} selected={selected} onSelectOrder={onSelectOrder} />
        ) : (
          <p className="ex-muted">{error ? `No book for ${fmtSec(t, 3)} yet.` : 'Loading the book at the replay time…'}</p>
        )}
      </section>
    </>
  );
}
