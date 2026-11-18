// The synchronized replay: the synthetic dataset's best bid/ask of the window around the shared time with
// both scenarios' simulated arrivals and fills (PriceChart), and the book at that same time with
// each scenario's resting orders. One time drives both: the chart's time line and the book.
// Wide pages (`split`) show the book beside the chart; narrow ones show one of them under a
// "Price chart / Order book" tab whose choice lives in the URL.

import { lazy, Suspense, useMemo, type KeyboardEvent, type ReactNode, type RefObject } from 'react';

import type { InstrumentMeta } from '../../api/types';
import { Spinner } from '../../components/ui';
import { lastAtOrBefore } from '../forensics/axis';
import { fmtSec, restingAt, shows, type Focus, type ScenarioKey } from './data';
import { Ladder } from './Ladder';
import { PlainError } from './PlainError';
import type { Moment } from './story';

// The chart library (Lightweight Charts, ~170 kB) is its own chunk so the page's text paints
// without waiting for it (PHASE6_PERFORMANCE). The download starts when this module is evaluated,
// i.e. with the page's first render, so it overlaps the page's API requests; the chart box keeps
// its fixed height while it arrives.
const priceChartModule = import('./PriceChart');
const PriceChart = lazy(() => priceChartModule.then((m) => ({ default: m.PriceChart })));

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
  /** Book beside the chart; otherwise one of them under a tab. */
  split: boolean;
  pane: 'chart' | 'book';
  onPane: (pane: 'chart' | 'book') => void;
  /** The replay-time slider, shown in the tab row (narrow pages only). */
  seek?: ReactNode;
  chartRef?: RefObject<HTMLDivElement | null>;
}

const PANES = [
  { key: 'chart', label: 'Price chart' },
  { key: 'book', label: 'Order book' },
] as const;

export function ReplayPanel({ inst, names, moment, loading, error, onRetry, t, focus, selected, onSelectOrder, onPick, split, pane, onPane, seek, chartRef }: ReplayPanelProps) {
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

  const onTabsKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) return;
    e.preventDefault();
    const next = e.key === 'Home' ? 'chart' : e.key === 'End' ? 'book' : pane === 'chart' ? 'book' : 'chart';
    onPane(next);
    document.getElementById(`ex-tab-${next}`)?.focus();
  };

  return (
    <div ref={chartRef} className="ex-replay" data-split={split ? '' : undefined}>
      {split ? null : (
        <div className="ex-pane-bar">
          <div className="ex-pane-tabs" role="tablist" aria-label="Replay view" onKeyDown={onTabsKey}>
            {PANES.map((p) => (
              <button
                key={p.key}
                id={`ex-tab-${p.key}`}
                type="button"
                role="tab"
                aria-selected={pane === p.key}
                aria-controls={`ex-pane-${p.key}`}
                tabIndex={pane === p.key ? 0 : -1}
                onClick={() => onPane(p.key)}
              >
                {p.label}
              </button>
            ))}
          </div>
          {seek}
        </div>
      )}
      <section
        id="ex-pane-chart"
        className="ex-replay-chart"
        aria-labelledby="ex-replay-title"
        role={split ? undefined : 'tabpanel'}
        hidden={!split && pane !== 'chart'}
      >
        <div className="ex-replay-head">
          <h2 id="ex-replay-title">
            Market replay{moment ? ` ${fmtSec(moment.window.t0, 3)} – ${fmtSec(moment.window.t1, 3)}` : ''}
          </h2>
          <span className="ex-time">
            Replay time <span className="ex-num" title={`${Math.round(t)} ns`}>{fmtSec(t, 3)}</span>
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
            <Suspense
              fallback={
                <div className="ex-chart-empty">
                  <Spinner label="Loading the chart" />
                </div>
              }
            >
              <PriceChart moment={moment} focus={focus} inst={inst} selected={selected} names={names} t={t} onSelectOrder={onSelectOrder} onPick={onPick} />
            </Suspense>
          ) : !error ? (
            <div className="ex-chart-empty">
              <Spinner label="Loading the market replay" />
            </div>
          ) : null}
        </div>
        <ul className="ex-legend" aria-label="Chart legend">
          <li>
            <span className="ex-key-line ex-key-bid" aria-hidden="true" /> Best bid <span className="ex-tagsm">synthetic</span>
          </li>
          <li>
            <span className="ex-key-line ex-key-ask" aria-hidden="true" /> Best ask <span className="ex-tagsm">synthetic</span>
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
            Hover for prices, quantities and times; drag or scroll to zoom; Reset view fits the whole window again. Click a marker to open its order; click anywhere else to move the replay time there. The book and trades are the
            recorded synthetic market; simulated orders never change it, so it is identical in both scenarios. Chart drawn with{' '}
            <a href="https://www.tradingview.com/" target="_blank" rel="noreferrer">TradingView Lightweight Charts</a>.
          </p>
        </details>
      </section>
      <section id="ex-pane-book" className="ex-book" aria-labelledby="ex-book-title" role={split ? undefined : 'tabpanel'} hidden={!split && pane !== 'book'}>
        <h2 id="ex-book-title">Order book at the replay time</h2>
        {moment && current ? (
          <Ladder frame={frame} t={t} inst={inst} resting={resting} focus={focus} names={names} selected={selected} onSelectOrder={onSelectOrder} />
        ) : (
          <p className="ex-muted">{error ? `No book for ${fmtSec(t, 3)} yet.` : 'Loading the book at the replay time…'}</p>
        )}
      </section>
    </div>
  );
}
