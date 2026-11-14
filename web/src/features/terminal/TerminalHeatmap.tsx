// The dataset heatmap of `replay/heatmap.ts` with the run's own orders and fills painted on top
// through the renderer's `paintExtra` hook.

import { useEffect, useMemo, useRef, useState } from 'react';

import type { FillRecord, InstrumentMeta, OrderRecord, ReplayWindow } from '../../api/types';
import { DASH, fmtPrice, fmtQty } from '../../components/fmt';
import { cssVar } from '../../components/theme';
import { ClockText, ErrorBox } from '../../components/ui';
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
}: TerminalHeatmapProps) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const rendererRef = useRef<HeatmapRenderer | null>(null);
  const [hover, setHover] = useState<HeatmapHover | null>(null);
  const [fatal, setFatal] = useState<Error | null>(null);
  const themes = useMemo(readThemes, []);

  const onCursorRef = useRef(onCursor);
  onCursorRef.current = onCursor;
  const instrumentRef = useRef(instrument);
  instrumentRef.current = instrument;

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

  const appliedFocus = useRef<number | null>(null);
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
      <div className="heatmap-wrap" ref={wrapRef}>
        <canvas
          ref={canvasRef}
          className="heatmap-canvas"
          role="img"
          aria-label="Order book heatmap reconstructed from Level-2 updates, with this run's simulated orders as dashed price lines and simulated fills as diamonds. Click to move the cursor; wheel zooms; drag pans; double-click resets."
        />
      </div>
      <div className="row term-hover mono small" aria-live="off">
        {hover ? (
          <>
            <span>
              <ClockText ns={Math.round(hover.t)} />
            </span>
            <span>{fmtPrice(hover.tick, instrument)}</span>
            <span className="bid">bid qty {levelText(hover.bid, instrument)}</span>
            <span className="ask">ask qty {levelText(hover.ask, instrument)}</span>
          </>
        ) : (
          <span className="dim">
            Hover for time, price and displayed quantity. Click places the cursor, wheel zooms,
            drag pans, double-click resets the view.
          </span>
        )}
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
          <span className="term-swatch-order term-swatch-ask" aria-hidden="true" /> own order resting
          at the simulated exchange (modeled)
        </span>
        <span>
          <span className="term-swatch-fill" aria-hidden="true" /> simulated fill (modeled)
        </span>
        <span>
          <span className="legend-line legend-line-cursor" aria-hidden="true" /> cursor (exchange
          clock)
        </span>
      </div>
    </div>
  );
}
