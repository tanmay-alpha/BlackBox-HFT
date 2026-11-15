// The dataset heatmap of `replay/heatmap.ts` with the run's own orders and fills painted on top
// through the renderer's `paintExtra` hook.

import { useEffect, useMemo, useRef, useState } from 'react';

import type { FillRecord, InstrumentMeta, OrderRecord, ReplayWindow } from '../../api/types';
import { DASH, fmtPrice, fmtQty } from '../../components/fmt';
import { cssVar } from '../../components/theme';
import { Toolbar } from '../../components/Toolbar';
import { ClockText, ErrorBox, Spinner } from '../../components/ui';
import { HeatmapRenderer } from '../replay/heatmap';
import type { HeatmapHover, HeatmapTheme, LevelQty } from '../replay/heatmap';
import { readHeatmapTheme } from '../replay/HeatmapView';
import { ownOrdersPainter } from './overlay';
import type { OverlayTheme } from './overlay';

export interface TerminalHeatmapProps {
  win: ReplayWindow;
  instrument: InstrumentMeta;
  orders: OrderRecord[];
  fills: FillRecord[];
  selectedOrder: number | null;
  /** Playback cursor (exchange clock, relative ns). */
  cursor: number;
  follow: boolean;
  onCursor: (t: number) => void;
  /** Zooms the view once to a few seconds around `t` when `key` changes (jump to an order). */
  focus: { t: number; key: number } | null;
  /** Set while the next window's pages load: shown over the chart, which keeps the old window. */
  loadingLabel?: string | null;
}

/** Visible span after jumping to an order. */
const FOCUS_SPAN_NS = 4_000_000_000;

function readThemes(): { heat: HeatmapTheme; overlay: OverlayTheme } {
  const heat = readHeatmapTheme();
  return {
    heat,
    overlay: {
      buy: heat.buy,
      sell: heat.sell,
      modeled: cssVar('--modeled'),
      accent: heat.accent,
      bg: heat.bg,
    },
  };
}

function levelText(level: LevelQty | null, inst: InstrumentMeta): string {
  if (level === null) return DASH;
  if (!level.known) return `${DASH} (beyond fetched levels)`;
  return fmtQty(level.qty, inst);
}

export function TerminalHeatmap({
  win,
  instrument,
  orders,
  fills,
  selectedOrder,
  cursor,
  follow,
  onCursor,
  focus,
  loadingLabel = null,
}: TerminalHeatmapProps) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const rendererRef = useRef<HeatmapRenderer | null>(null);
  const [hover, setHover] = useState<HeatmapHover | null>(null);
  const [fatal, setFatal] = useState<Error | null>(null);
  const [zoomed, setZoomed] = useState(false);
  const themes = useMemo(readThemes, []);

  const onCursorRef = useRef(onCursor);
  onCursorRef.current = onCursor;
  const instrumentRef = useRef(instrument);
  instrumentRef.current = instrument;
  const winRef = useRef<ReplayWindow | null>(win);
  winRef.current = win;
  const appliedFocus = useRef<number | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas || !wrap) return;
    let renderer: HeatmapRenderer;
    try {
      renderer = new HeatmapRenderer(canvas, {
        theme: themes.heat,
        scale: 'sqrt',
        formatPrice: (tick) => fmtPrice(tick, instrumentRef.current),
        onHover: setHover,
        onView: (v0, v1) => {
          const w = winRef.current;
          setZoomed(w !== null && (v0 > w.t0 + 1 || v1 < w.t1 - 1));
        },
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
      // A new renderer starts on the whole window; let it apply the pending focus again.
      appliedFocus.current = null;
    };
  }, [themes]);

  useEffect(() => {
    rendererRef.current?.setData(win);
  }, [win]);

  useEffect(() => {
    rendererRef.current?.setPaintExtra(ownOrdersPainter(orders, fills, selectedOrder, themes.overlay));
  }, [orders, fills, selectedOrder, themes]);

  useEffect(() => {
    rendererRef.current?.setFormatPrice((tick) => fmtPrice(tick, instrumentRef.current));
  }, [instrument]);

  useEffect(() => {
    rendererRef.current?.setCursor(cursor, follow);
  }, [cursor, follow, win]);

  useEffect(() => {
    const r = rendererRef.current;
    if (!r || !focus || appliedFocus.current === focus.key) return;
    if (focus.t < win.t0 || focus.t > win.t1) return;
    appliedFocus.current = focus.key;
    r.resetView();
    r.zoomBy(FOCUS_SPAN_NS / Math.max(1, win.t1 - win.t0), focus.t);
    // The zoom keeps the focus at its relative position; pan it to the middle of the view.
    r.panBy((focus.t - win.t0) / Math.max(1, win.t1 - win.t0) - 0.5);
  }, [focus, win]);

  if (fatal) return <ErrorBox error={fatal} />;

  return (
    <div className="heatmap">
      <div className="heatmap-wrap" ref={wrapRef} aria-busy={loadingLabel !== null}>
        <canvas
          ref={canvasRef}
          className="heatmap-canvas"
          role="img"
          aria-label="Order book heatmap reconstructed from Level-2 updates, with this run's simulated orders as dashed price lines and simulated fills as outlined diamonds. Click to move the cursor; wheel zooms; drag pans; double-click resets."
        />
        {loadingLabel !== null ? (
          <div className="term-heat-loading" role="status">
            <Spinner label={loadingLabel} />
          </div>
        ) : null}
      </div>
      <div className="term-heat-bar">
        <Toolbar label="Heatmap zoom" className="term-zoom">
          <button type="button" onClick={() => rendererRef.current?.zoomBy(0.5)} aria-label="Zoom in" title="Zoom in (wheel)">
            +
          </button>
          <button type="button" onClick={() => rendererRef.current?.zoomBy(2)} disabled={!zoomed} aria-label="Zoom out" title="Zoom out (wheel)">
            {'\u2212'}
          </button>
          <button type="button" onClick={() => rendererRef.current?.resetView()} disabled={!zoomed} title="Show the whole loaded window (double-click)">
            Reset zoom
          </button>
        </Toolbar>
        <div className="term-hover" aria-live="off">
          {hover ? (
            <>
              <span className="term-hover-item">
                <ClockText ns={Math.round(hover.t)} />
              </span>
              <span className="term-hover-item">
                <span className="term-hover-k">price</span> <span className="mono">{fmtPrice(hover.tick, instrument)}</span>
              </span>
              <span className="term-hover-item bid">
                <span className="term-hover-k">bid qty</span> <span className="mono">{levelText(hover.bid, instrument)}</span>
              </span>
              <span className="term-hover-item ask">
                <span className="term-hover-k">ask qty</span> <span className="mono">{levelText(hover.ask, instrument)}</span>
              </span>
            </>
          ) : (
            <span className="dim">Hover for values · click sets the cursor · wheel zooms · drag pans</span>
          )}
        </div>
      </div>
      <div className="row term-legend small">
        <span>
          <span className="legend-line legend-line-bid" aria-hidden="true" /> best bid
        </span>
        <span>
          <span className="legend-line legend-line-ask" aria-hidden="true" /> best ask
        </span>
        <span>
          <span className="legend-dot legend-dot-buy" aria-hidden="true" /> market trade (observed)
        </span>
        <span>
          <span className="term-swatch-order term-swatch-bid" aria-hidden="true" />
          <span className="term-swatch-order term-swatch-ask" aria-hidden="true" /> own resting order (modeled)
        </span>
        <span>
          <span className="term-swatch-fill" aria-hidden="true" /> simulated fill (modeled)
        </span>
        <span>
          <span className="legend-line legend-line-cursor" aria-hidden="true" /> cursor (exchange clock)
        </span>
      </div>
    </div>
  );
}
