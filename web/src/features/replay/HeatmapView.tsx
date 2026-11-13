// Thin React wrapper around the canvas renderer in `heatmap.ts`: lifecycle, resize, theme,
// tooltip and legend. All drawing and pointer interaction live in the renderer.

import { useEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent } from 'react';

import type { InstrumentMeta, ReplayWindow } from '../../api/types';
import { DASH, fmtDuration, fmtPrice, fmtQty } from '../../components/fmt';
import { cssVar } from '../../components/theme';
import { ClockText, ErrorBox } from '../../components/ui';
import { HeatmapRenderer, heatCss } from './heatmap';
import type {
  HeatmapHover,
  HeatmapStats,
  HeatmapTheme,
  IntensityScale,
  LevelQty,
} from './heatmap';

export interface HeatmapViewProps {
  win: ReplayWindow;
  instrument: InstrumentMeta;
  scale: IntensityScale;
  /** Playback cursor (exchange clock, relative ns). */
  cursor: number;
  /** Page the view to keep the cursor visible (used while playing). */
  follow: boolean;
  onCursor: (t: number) => void;
}

/** The renderer draws with the same custom properties as the rest of the console. */
function readTheme(): HeatmapTheme {
  return {
    bg: cssVar('--bg', '#0f1113'),
    surface: cssVar('--surface', '#15181b'),
    border: cssVar('--border', '#2c3238'),
    fg: cssVar('--fg', '#d7dbe0'),
    muted: cssVar('--muted', '#929ba5'),
    accent: cssVar('--accent', '#9db8dc'),
    buy: cssVar('--buy', '#4fbf8d'),
    sell: cssVar('--sell', '#ee7468'),
    mono: cssVar('--mono', 'monospace'),
  };
}

const LEGEND_FRACTIONS = [0.02, 0.1, 0.25, 0.5, 1];
const TOOLTIP_FLIP_MARGIN = 280;

function levelText(level: LevelQty | null, inst: InstrumentMeta): string {
  if (level === null) return `${DASH} (before the first frame)`;
  if (!level.known) return `${DASH} (beyond the fetched levels)`;
  return level.qty > 0 ? fmtQty(level.qty, inst) : '0 (none displayed)';
}

function Tooltip({
  hover,
  inst,
  flip,
}: {
  hover: HeatmapHover;
  inst: InstrumentMeta;
  flip: boolean;
}) {
  const b = hover.bbo;
  return (
    <div
      className="heatmap-tooltip"
      style={{
        left: flip ? hover.x - 14 : hover.x + 14,
        top: hover.y + 14,
        transform: flip ? 'translateX(-100%)' : undefined,
      }}
      aria-hidden="true"
    >
      <dl className="kv">
        <dt>Time</dt>
        <dd>
          <ClockText ns={Math.round(hover.t)} />
        </dd>
        <dt>Price</dt>
        <dd>
          {fmtPrice(hover.tick, inst)} <span className="dim">tick {hover.tick}</span>
        </dd>
        <dt>Bid qty here</dt>
        <dd className="buy">{levelText(hover.bid, inst)}</dd>
        <dt>Ask qty here</dt>
        <dd className="sell">{levelText(hover.ask, inst)}</dd>
        <dt>Frame</dt>
        <dd>{hover.frameTs === null ? DASH : <ClockText ns={hover.frameTs} />}</dd>
        <dt>Best bid</dt>
        <dd>
          {b && b.bidTick !== null
            ? `${fmtPrice(b.bidTick, inst)} \u00d7 ${fmtQty(b.bidQty, inst)}`
            : `${DASH} (no bid)`}
        </dd>
        <dt>Best ask</dt>
        <dd>
          {b && b.askTick !== null
            ? `${fmtPrice(b.askTick, inst)} \u00d7 ${fmtQty(b.askQty, inst)}`
            : `${DASH} (no ask)`}
        </dd>
      </dl>
    </div>
  );
}

function Legend({
  stats,
  scale,
  theme,
  inst,
  levels,
}: {
  stats: HeatmapStats;
  scale: IntensityScale;
  theme: HeatmapTheme;
  inst: InstrumentMeta;
  levels: number;
}) {
  const stops = LEGEND_FRACTIONS.map((f) => f * stats.qRef);
  return (
    <div className="heatmap-legend" aria-label="Heatmap legend">
      <div className="heatmap-legend-group">
        <span className="dim">Displayed quantity ({scale} scale)</span>
        <table className="heatmap-ramp">
          <tbody>
            {(['ask', 'bid'] as const).map((side) => (
              <tr key={side}>
                <th scope="row" className={side === 'bid' ? 'buy' : 'sell'}>
                  {side}
                </th>
                <td>
                  <span className="heatmap-swatch" style={{ background: theme.bg }} />
                </td>
                {stops.map((q, i) => (
                  <td key={i}>
                    <span
                      className="heatmap-swatch"
                      style={{ background: heatCss(theme, side, q, stats.qRef, scale) }}
                    />
                  </td>
                ))}
              </tr>
            ))}
            <tr className="mono dim">
              <th scope="row">qty</th>
              <td>0</td>
              {stops.map((q, i) => (
                <td key={i}>
                  {i === stops.length - 1 ? '\u2265' : ''}
                  {fmtQty(q, inst)}
                </td>
              ))}
            </tr>
          </tbody>
        </table>
        <span className="dim">
          Saturates at the 99th percentile of level sizes in this window. Only the top {levels}{' '}
          levels per side were fetched; anything beyond is not drawn (dark is not proof of an
          empty level there).
        </span>
      </div>
      <div className="heatmap-legend-group">
        <span className="dim">Lines and marks</span>
        <span>
          <span className="legend-line legend-line-bid" aria-hidden="true" /> best bid (step line)
        </span>
        <span>
          <span className="legend-line legend-line-ask" aria-hidden="true" /> best ask (step line)
        </span>
        <span>
          <span className="legend-dot legend-dot-buy" aria-hidden="true" /> trade, buyer aggressor
        </span>
        <span>
          <span className="legend-dot legend-dot-sell" aria-hidden="true" /> trade, seller
          aggressor
        </span>
        <span>
          <span className="legend-dot legend-dot-unknown" aria-hidden="true" /> trade, aggressor
          not stated
        </span>
        <span className="dim">
          Mark area is proportional to trade quantity (largest in window:{' '}
          <span className="mono">{fmtQty(stats.maxTradeQty, inst)}</span>).
        </span>
        <span>
          <span className="legend-line legend-line-cursor" aria-hidden="true" /> playback cursor
        </span>
      </div>
      {stats.ticksPerRow > 1 ? (
        <p className="notice">
          The price range of this window is tall: each drawn row merges {stats.ticksPerRow} price
          ticks and shows their summed quantity. The tooltip always reads exact per-tick values.
        </p>
      ) : null}
    </div>
  );
}

export function HeatmapView({ win, instrument, scale, cursor, follow, onCursor }: HeatmapViewProps) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const rendererRef = useRef<HeatmapRenderer | null>(null);

  const [hover, setHover] = useState<HeatmapHover | null>(null);
  const [view, setView] = useState<[number, number]>([win.t0, win.t1]);
  const [stats, setStats] = useState<HeatmapStats | null>(null);
  const [fatal, setFatal] = useState<Error | null>(null);
  const theme = useMemo(readTheme, []);

  // The renderer is created once; these refs let its callbacks and initial options see current
  // props without recreating it.
  const onCursorRef = useRef(onCursor);
  onCursorRef.current = onCursor;
  const instrumentRef = useRef(instrument);
  instrumentRef.current = instrument;
  const scaleRef = useRef(scale);
  scaleRef.current = scale;

  useEffect(() => {
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas || !wrap) return;
    let renderer: HeatmapRenderer;
    try {
      renderer = new HeatmapRenderer(canvas, {
        theme,
        scale: scaleRef.current,
        formatPrice: (tick) => fmtPrice(tick, instrumentRef.current),
        onHover: setHover,
        onView: (v0, v1) => setView([v0, v1]),
        onCursor: (t) => onCursorRef.current(t),
      });
    } catch (e) {
      setFatal(e instanceof Error ? e : new Error(String(e)));
      return;
    }
    rendererRef.current = renderer;
    const observer = new ResizeObserver(() => renderer.resize());
    observer.observe(wrap);
    return () => {
      observer.disconnect();
      renderer.destroy();
      rendererRef.current = null;
    };
  }, [theme]);

  useEffect(() => {
    const renderer = rendererRef.current;
    if (!renderer) return;
    renderer.setData(win);
    setStats(renderer.getStats());
  }, [win]);

  useEffect(() => {
    rendererRef.current?.setScale(scale);
  }, [scale]);

  useEffect(() => {
    // Axis labels depend on tick size; repaint them when the instrument changes.
    rendererRef.current?.setFormatPrice((tick) => fmtPrice(tick, instrumentRef.current));
  }, [instrument]);

  useEffect(() => {
    rendererRef.current?.setCursor(cursor, follow);
  }, [cursor, follow, win]);

  const onKeyDown = (e: KeyboardEvent<HTMLCanvasElement>) => {
    const r = rendererRef.current;
    if (!r) return;
    switch (e.key) {
      case 'ArrowLeft':
        r.panBy(-0.1);
        break;
      case 'ArrowRight':
        r.panBy(0.1);
        break;
      case '+':
      case '=':
        r.zoomBy(0.8);
        break;
      case '-':
      case '_':
        r.zoomBy(1.25);
        break;
      case '0':
        r.resetView();
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  if (fatal) return <ErrorBox error={fatal} />;

  const total = win.t1 - win.t0;
  const zoomed = view[0] > win.t0 || view[1] < win.t1;
  const wrapWidth = wrapRef.current?.clientWidth ?? 0;

  return (
    <div className="col heatmap">
      <div className="row heatmap-toolbar">
        <button type="button" onClick={() => rendererRef.current?.zoomBy(0.5)}>
          Zoom in
        </button>
        <button type="button" onClick={() => rendererRef.current?.zoomBy(2)} disabled={!zoomed}>
          Zoom out
        </button>
        <button type="button" onClick={() => rendererRef.current?.resetView()} disabled={!zoomed}>
          Reset view
        </button>
        <span className="mono dim">
          view <ClockText ns={Math.round(view[0])} /> {'\u2192'}{' '}
          <ClockText ns={Math.round(view[1])} /> ({fmtDuration(Math.round(view[1] - view[0]))} of{' '}
          {fmtDuration(total)} fetched)
        </span>
        <span className="spacer" />
        <span className="dim small" id="heatmap-help">
          Wheel zooms time, drag pans, click places the cursor, double-click resets. With focus:{' '}
          <kbd>{'\u2190'}</kbd> <kbd>{'\u2192'}</kbd> pan, <kbd>+</kbd> <kbd>-</kbd> zoom,{' '}
          <kbd>0</kbd> reset.
        </span>
      </div>
      <div className="heatmap-wrap" ref={wrapRef}>
        <canvas
          ref={canvasRef}
          className="heatmap-canvas"
          tabIndex={0}
          role="application"
          aria-roledescription="order book heatmap"
          aria-label="Order book heatmap reconstructed from Level-2 updates: time on the horizontal axis, price on the vertical axis, brightness is displayed quantity."
          aria-describedby="heatmap-help"
          onKeyDown={onKeyDown}
        />
        {hover ? (
          <Tooltip
            hover={hover}
            inst={instrument}
            flip={wrapWidth > 0 && hover.x > wrapWidth - TOOLTIP_FLIP_MARGIN}
          />
        ) : null}
      </div>
      {stats ? (
        <Legend stats={stats} scale={scale} theme={theme} inst={instrument} levels={win.levels} />
      ) : null}
    </div>
  );
}
