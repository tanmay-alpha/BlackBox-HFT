// Order-book heatmap renderer (Canvas 2D). Framework-free: it owns one <canvas>, its pointer and
// wheel interaction, and reports hover/view/cursor changes through callbacks.
//
// What is drawn is exactly the fetched `ReplayWindow`:
//   * heat layer  - one column per depth frame, one row per price tick, brightness = displayed
//                   quantity. Built once into an offscreen bitmap (ImageData), then blitted.
//   * step lines  - best bid / best ask from the BBO series.
//   * marks       - market trades, area proportional to quantity, coloured by aggressor side.
// Nothing is interpolated: a frame's column holds until the next frame, and levels beyond the
// fetched depth are left dark.

import type { DepthFrame, ReplayWindow, Side } from '../../api/types';

export type IntensityScale = 'sqrt' | 'log';

export interface HeatmapTheme {
  bg: string;
  surface: string;
  border: string;
  fg: string;
  muted: string;
  accent: string;
  /** Tint of bid-side depth and colour of buy-aggressor trades. */
  buy: string;
  /** Tint of ask-side depth and colour of sell-aggressor trades. */
  sell: string;
  /** CSS font-family for axis labels. */
  mono: string;
}

/** Displayed quantity at one price level of one frame. */
export interface LevelQty {
  /** `false` when the level lies beyond the fetched depth: its quantity was not requested. */
  known: boolean;
  qty: number;
}

export interface HeatmapHover {
  /** Pointer position in CSS pixels relative to the canvas. */
  x: number;
  y: number;
  /** Time under the pointer. */
  t: number;
  /** Price tick under the pointer. */
  tick: number;
  /** Timestamp of the depth frame in force at `t`, or `null` before the first frame. */
  frameTs: number | null;
  bid: LevelQty | null;
  ask: LevelQty | null;
  /** Top of book in force at `t`, or `null` before the first BBO entry. */
  bbo: {
    ts: number;
    bidTick: number | null;
    askTick: number | null;
    bidQty: number;
    askQty: number;
  } | null;
}

export interface HeatmapStats {
  frames: number;
  /** Quantity at which the colour scale saturates (99th percentile of displayed level sizes). */
  qRef: number;
  maxTradeQty: number;
  tickMin: number | null;
  tickMax: number | null;
  /** Price ticks merged into one bitmap row (1 unless the price range is very tall). */
  ticksPerRow: number;
}

export interface HeatmapOptions {
  theme: HeatmapTheme;
  scale: IntensityScale;
  formatPrice: (tick: number) => string;
  onHover?: (hover: HeatmapHover | null) => void;
  onView?: (v0: number, v1: number) => void;
  /** The user clicked the plot to place the playback cursor at `t`. */
  onCursor?: (t: number) => void;
}

type Rgb = [number, number, number];

const AXIS_W = 84;
const AXIS_H = 22;
const PAD_TOP = 6;
/** Upper bound on bitmap rows; taller price ranges merge ticks into rows. */
const MAX_ROWS = 4096;
/** Any displayed quantity is at least this bright, so "small" never reads as "empty". */
const MIN_INTENSITY = 0.1;
const LOG_K = 99;
const DRAG_THRESHOLD_PX = 3;

// ----- pure helpers (also used by the depth ladder and the legend) --------------------------

/** Index of the last element `<= t` in ascending `ts`, or -1. */
export function seriesIndexAt(ts: number[], t: number): number {
  let lo = 0;
  let hi = ts.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (ts[mid] <= t) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}

/** Index of the depth frame in force at `t` (last frame with `ts <= t`), or -1. */
export function frameIndexAt(depth: DepthFrame[], t: number): number {
  let lo = 0;
  let hi = depth.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (depth[mid].ts <= t) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}

/**
 * Displayed quantity at `tick` on one side of `frame`. A tick past the last fetched level of a
 * full side is reported as unknown rather than zero.
 */
export function lookupLevel(
  frame: DepthFrame,
  side: 'bid' | 'ask',
  tick: number,
  levelsCap: number,
): LevelQty {
  const levels = side === 'bid' ? frame.bids : frame.asks;
  for (let i = 0; i < levels.length; i += 1) {
    if (levels[i][0] === tick) return { known: true, qty: levels[i][1] };
  }
  if (levels.length >= levelsCap && levels.length > 0) {
    const worst = levels[levels.length - 1][0];
    const beyond = side === 'bid' ? tick < worst : tick > worst;
    if (beyond) return { known: false, qty: 0 };
  }
  return { known: true, qty: 0 };
}

/** Brightness in `[0, 1]` for a displayed quantity. Zero quantity is exactly 0. */
export function intensity(qty: number, qRef: number, scale: IntensityScale): number {
  if (!(qty > 0) || !(qRef > 0)) return 0;
  const r = Math.min(1, qty / qRef);
  const base = scale === 'sqrt' ? Math.sqrt(r) : Math.log1p(LOG_K * r) / Math.log1p(LOG_K);
  return MIN_INTENSITY + (1 - MIN_INTENSITY) * base;
}

const rgbCache = new Map<string, Rgb>();

/** Resolves any opaque CSS colour to RGB by letting the canvas parse it. */
export function cssToRgb(color: string): Rgb {
  const cached = rgbCache.get(color);
  if (cached) return cached;
  const c = document.createElement('canvas');
  c.width = 1;
  c.height = 1;
  const ctx = c.getContext('2d', { willReadFrequently: true });
  if (!ctx) return [0, 0, 0];
  ctx.fillStyle = color;
  ctx.fillRect(0, 0, 1, 1);
  const d = ctx.getImageData(0, 0, 1, 1).data;
  const rgb: Rgb = [d[0], d[1], d[2]];
  rgbCache.set(color, rgb);
  return rgb;
}

function mix(a: Rgb, b: Rgb, k: number): Rgb {
  return [
    Math.round(a[0] + (b[0] - a[0]) * k),
    Math.round(a[1] + (b[1] - a[1]) * k),
    Math.round(a[2] + (b[2] - a[2]) * k),
  ];
}

function rgbCss(c: Rgb): string {
  return `rgb(${c[0]}, ${c[1]}, ${c[2]})`;
}

/** CSS colour of a heat cell, for legends: the same mapping the bitmap uses. */
export function heatCss(
  theme: HeatmapTheme,
  side: 'bid' | 'ask',
  qty: number,
  qRef: number,
  scale: IntensityScale,
): string {
  const tint = cssToRgb(side === 'bid' ? theme.buy : theme.sell);
  return rgbCss(mix(cssToRgb(theme.bg), tint, intensity(qty, qRef, scale)));
}

const TIME_STEPS_NS: number[] = (() => {
  const steps: number[] = [];
  for (let decade = 1; decade < 1e9; decade *= 10) {
    steps.push(decade, 2 * decade, 5 * decade);
  }
  for (const s of [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600, 86400]) {
    steps.push(s * 1e9);
  }
  return steps;
})();

function niceTimeStep(raw: number): number {
  for (const s of TIME_STEPS_NS) {
    if (s >= raw) return s;
  }
  return TIME_STEPS_NS[TIME_STEPS_NS.length - 1];
}

function niceTickStep(raw: number): number {
  if (raw <= 1) return 1;
  const decade = 10 ** Math.floor(Math.log10(raw));
  for (const m of [1, 2, 5, 10]) {
    if (m * decade >= raw) return m * decade;
  }
  return 10 * decade;
}

/** `mm:ss`, `mm:ss.mmm` or `mm:ss.mmmuuu`, whichever resolves `stepNs`. */
function clockLabel(t: number, stepNs: number): string {
  const neg = t < 0;
  const us = Math.floor(Math.abs(t) / 1000);
  const totalSec = Math.floor(us / 1_000_000);
  const mm = String(Math.floor(totalSec / 60)).padStart(2, '0');
  const ss = String(totalSec % 60).padStart(2, '0');
  let text = `${neg ? '-' : ''}${mm}:${ss}`;
  if (stepNs < 1e9) {
    const micros = String(us % 1_000_000).padStart(6, '0');
    text += `.${stepNs < 1e6 ? micros : micros.slice(0, 3)}`;
  }
  return text;
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

// ----- renderer ---------------------------------------------------------------------------

interface HeatBitmap {
  canvas: HTMLCanvasElement;
  frames: number;
  rows: number;
  /** Highest tick covered; row 0 spans `(tickTop - ticksPerRow + 0.5, tickTop + 0.5]`. */
  tickTop: number;
  ticksPerRow: number;
  bidQty: Float32Array;
  askQty: Float32Array;
}

interface Drag {
  pointerId: number;
  startX: number;
  v0: number;
  v1: number;
  moved: boolean;
}

export class HeatmapRenderer {
  private readonly canvas: HTMLCanvasElement;
  private readonly ctx: CanvasRenderingContext2D;
  private readonly scene: HTMLCanvasElement;
  private readonly sceneCtx: CanvasRenderingContext2D;
  private opts: HeatmapOptions;

  private win: ReplayWindow | null = null;
  private heat: HeatBitmap | null = null;
  private frameLo: Float64Array = new Float64Array(0);
  private frameHi: Float64Array = new Float64Array(0);
  private stats: HeatmapStats = {
    frames: 0,
    qRef: 1,
    maxTradeQty: 0,
    tickMin: null,
    tickMax: null,
    ticksPerRow: 1,
  };

  // View (time) and derived price range.
  private v0 = 0;
  private v1 = 1;
  private yLo = 0;
  private yHi = 1;
  private hasPrices = false;

  private cursor: number | null = null;
  private pointer: { x: number; y: number } | null = null;
  private drag: Drag | null = null;

  // Layout in device pixels.
  private dpr = 1;
  private cssW = 0;
  private cssH = 0;
  private plotX = 0;
  private plotY = 0;
  private plotW = 1;
  private plotH = 1;

  private sceneDirty = true;
  private raf = 0;
  private destroyed = false;
  private dprQuery: MediaQueryList | null = null;

  constructor(canvas: HTMLCanvasElement, opts: HeatmapOptions) {
    const ctx = canvas.getContext('2d');
    const scene = document.createElement('canvas');
    const sceneCtx = scene.getContext('2d');
    if (!ctx || !sceneCtx) {
      throw new Error('Canvas 2D is not available in this browser.');
    }
    this.canvas = canvas;
    this.ctx = ctx;
    this.scene = scene;
    this.sceneCtx = sceneCtx;
    this.opts = opts;

    canvas.addEventListener('wheel', this.onWheel, { passive: false });
    canvas.addEventListener('pointerdown', this.onPointerDown);
    canvas.addEventListener('pointermove', this.onPointerMove);
    canvas.addEventListener('pointerup', this.onPointerUp);
    canvas.addEventListener('pointercancel', this.onPointerCancel);
    canvas.addEventListener('pointerleave', this.onPointerLeave);
    canvas.addEventListener('dblclick', this.onDoubleClick);
    this.watchDpr();
    this.resize();
  }

  destroy(): void {
    this.destroyed = true;
    if (this.raf) cancelAnimationFrame(this.raf);
    this.raf = 0;
    const c = this.canvas;
    c.removeEventListener('wheel', this.onWheel);
    c.removeEventListener('pointerdown', this.onPointerDown);
    c.removeEventListener('pointermove', this.onPointerMove);
    c.removeEventListener('pointerup', this.onPointerUp);
    c.removeEventListener('pointercancel', this.onPointerCancel);
    c.removeEventListener('pointerleave', this.onPointerLeave);
    c.removeEventListener('dblclick', this.onDoubleClick);
    this.dprQuery?.removeEventListener('change', this.onDprChange);
    this.dprQuery = null;
  }

  // ----- public state setters -------------------------------------------------------------

  /** Replaces the data and resets the view to the whole window. */
  setData(win: ReplayWindow | null): void {
    this.win = win;
    this.buildTables();
    this.colorize();
    if (win) {
      this.v0 = win.t0;
      this.v1 = Math.max(win.t1, win.t0 + 1);
    } else {
      this.v0 = 0;
      this.v1 = 1;
    }
    this.fitPrices();
    this.invalidate(true);
    this.opts.onView?.(this.v0, this.v1);
    this.emitHover();
  }

  setScale(scale: IntensityScale): void {
    if (scale === this.opts.scale) return;
    this.opts = { ...this.opts, scale };
    this.colorize();
    this.invalidate(true);
  }

  setTheme(theme: HeatmapTheme): void {
    this.opts = { ...this.opts, theme };
    this.colorize();
    this.invalidate(true);
  }

  setFormatPrice(formatPrice: (tick: number) => string): void {
    this.opts = { ...this.opts, formatPrice };
    this.invalidate(true);
  }

  /**
   * Moves the playback cursor. With `follow`, a cursor that leaves the visible time range pages
   * the view so it stays on screen.
   */
  setCursor(t: number | null, follow = false): void {
    this.cursor = t;
    if (follow && t !== null && this.win && (t < this.v0 || t > this.v1)) {
      const span = this.v1 - this.v0;
      this.setView(t - span * 0.1, t + span * 0.9);
    }
    this.invalidate(false);
  }

  getStats(): HeatmapStats {
    return this.stats;
  }

  resetView(): void {
    if (!this.win) return;
    this.setView(this.win.t0, this.win.t1);
  }

  /** `factor < 1` zooms in. The anchor defaults to the centre of the view. */
  zoomBy(factor: number, anchor?: number): void {
    const a = anchor ?? (this.v0 + this.v1) / 2;
    this.setView(a - (a - this.v0) * factor, a + (this.v1 - a) * factor);
  }

  /** Pans by a fraction of the visible span (positive moves later in time). */
  panBy(fraction: number): void {
    const d = (this.v1 - this.v0) * fraction;
    this.setView(this.v0 + d, this.v1 + d);
  }

  /** Re-reads the canvas CSS size and device pixel ratio. Call from a ResizeObserver. */
  resize(): void {
    if (this.destroyed) return;
    const rect = this.canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(rect.width * dpr));
    const h = Math.max(1, Math.round(rect.height * dpr));
    this.dpr = dpr;
    this.cssW = rect.width;
    this.cssH = rect.height;
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    if (this.scene.width !== w || this.scene.height !== h) {
      this.scene.width = w;
      this.scene.height = h;
    }
    this.plotX = 0;
    this.plotY = Math.round(PAD_TOP * dpr);
    this.plotW = Math.max(1, w - Math.round(AXIS_W * dpr));
    this.plotH = Math.max(1, h - Math.round(AXIS_H * dpr) - this.plotY);
    this.sceneDirty = true;
    // Resizing clears the backing store; paint synchronously so the canvas never flashes blank.
    this.paint();
  }

  // ----- data preparation -----------------------------------------------------------------

  private buildTables(): void {
    const win = this.win;
    this.heat = null;
    this.frameLo = new Float64Array(0);
    this.frameHi = new Float64Array(0);
    this.stats = {
      frames: 0,
      qRef: 1,
      maxTradeQty: 0,
      tickMin: null,
      tickMax: null,
      ticksPerRow: 1,
    };
    if (!win) return;

    const depth = win.depth;
    const n = depth.length;
    const lo = new Float64Array(n).fill(Number.NaN);
    const hi = new Float64Array(n).fill(Number.NaN);
    let tickMin = Number.POSITIVE_INFINITY;
    let tickMax = Number.NEGATIVE_INFINITY;
    let levelCount = 0;
    for (let i = 0; i < n; i += 1) {
      const f = depth[i];
      let fLo = Number.POSITIVE_INFINITY;
      let fHi = Number.NEGATIVE_INFINITY;
      for (const side of [f.bids, f.asks]) {
        for (let k = 0; k < side.length; k += 1) {
          const tick = side[k][0];
          if (tick < fLo) fLo = tick;
          if (tick > fHi) fHi = tick;
          if (side[k][1] > 0) levelCount += 1;
        }
      }
      if (fLo <= fHi) {
        lo[i] = fLo;
        hi[i] = fHi;
        if (fLo < tickMin) tickMin = fLo;
        if (fHi > tickMax) tickMax = fHi;
      }
    }
    this.frameLo = lo;
    this.frameHi = hi;

    let maxTradeQty = 0;
    for (const q of win.trades.qty) {
      if (q > maxTradeQty) maxTradeQty = q;
    }
    this.stats = { ...this.stats, frames: n, maxTradeQty };
    if (!(tickMin <= tickMax)) return;

    // Saturate the colour scale at the 99th percentile so one outsized level does not flatten
    // everything else. Values above it render at full brightness.
    const sizes = new Float64Array(levelCount);
    let s = 0;
    for (let i = 0; i < n; i += 1) {
      const f = depth[i];
      for (const side of [f.bids, f.asks]) {
        for (let k = 0; k < side.length; k += 1) {
          if (side[k][1] > 0) {
            sizes[s] = side[k][1];
            s += 1;
          }
        }
      }
    }
    sizes.sort();
    const qRef = levelCount > 0 ? sizes[Math.floor(0.99 * (levelCount - 1))] : 1;

    const span = tickMax - tickMin + 1;
    const ticksPerRow = Math.max(1, Math.ceil(span / MAX_ROWS));
    const rows = Math.ceil(span / ticksPerRow);
    const bidQty = new Float32Array(n * rows);
    const askQty = new Float32Array(n * rows);
    for (let i = 0; i < n; i += 1) {
      const f = depth[i];
      for (let k = 0; k < f.bids.length; k += 1) {
        const row = Math.floor((tickMax - f.bids[k][0]) / ticksPerRow);
        bidQty[row * n + i] += f.bids[k][1];
      }
      for (let k = 0; k < f.asks.length; k += 1) {
        const row = Math.floor((tickMax - f.asks[k][0]) / ticksPerRow);
        askQty[row * n + i] += f.asks[k][1];
      }
    }

    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, n);
    canvas.height = Math.max(1, rows);
    this.heat = { canvas, frames: n, rows, tickTop: tickMax, ticksPerRow, bidQty, askQty };
    this.stats = {
      frames: n,
      qRef: qRef > 0 ? qRef : 1,
      maxTradeQty,
      tickMin,
      tickMax,
      ticksPerRow,
    };
  }

  /** Paints the heat bitmap from the quantity tables with the current scale and theme. */
  private colorize(): void {
    const heat = this.heat;
    if (!heat || heat.frames === 0 || heat.rows === 0) return;
    const ctx = heat.canvas.getContext('2d');
    if (!ctx) return;
    const { theme, scale } = this.opts;
    const bg = cssToRgb(theme.bg);
    const bid = cssToRgb(theme.buy);
    const ask = cssToRgb(theme.sell);
    const qRef = this.stats.qRef;
    const img = ctx.createImageData(heat.frames, heat.rows);
    const px = img.data;
    const cells = heat.frames * heat.rows;
    for (let c = 0; c < cells; c += 1) {
      const b = heat.bidQty[c];
      const a = heat.askQty[c];
      let rgb = bg;
      if (a > 0 || b > 0) {
        rgb = a > b ? mix(bg, ask, intensity(a, qRef, scale)) : mix(bg, bid, intensity(b, qRef, scale));
      }
      const o = c * 4;
      px[o] = rgb[0];
      px[o + 1] = rgb[1];
      px[o + 2] = rgb[2];
      px[o + 3] = 255;
    }
    ctx.putImageData(img, 0, 0);
  }

  /** Fits the price axis to the depth frames inside the visible time range. */
  private fitPrices(): void {
    const win = this.win;
    this.hasPrices = false;
    this.yLo = 0;
    this.yHi = 1;
    if (!win) return;
    let lo = Number.POSITIVE_INFINITY;
    let hi = Number.NEGATIVE_INFINITY;
    const depth = win.depth;
    const i0 = Math.max(0, frameIndexAt(depth, this.v0));
    for (let i = i0; i < depth.length; i += 1) {
      if (depth[i].ts > this.v1) break;
      const fLo = this.frameLo[i];
      const fHi = this.frameHi[i];
      if (fLo <= fHi) {
        if (fLo < lo) lo = fLo;
        if (fHi > hi) hi = fHi;
      }
    }
    if (!(lo <= hi)) {
      // No depth in view: fall back to whatever prices the window does contain.
      const consider = (tick: number | null) => {
        if (tick === null || tick === undefined) return;
        if (tick < lo) lo = tick;
        if (tick > hi) hi = tick;
      };
      win.bbo.bid_tick.forEach(consider);
      win.bbo.ask_tick.forEach(consider);
      win.trades.price_tick.forEach(consider);
    }
    if (!(lo <= hi)) return;
    this.hasPrices = true;
    this.yLo = lo - 1.5;
    this.yHi = hi + 1.5;
  }

  // ----- coordinate transforms (device pixels) --------------------------------------------

  private xOf(t: number): number {
    return this.plotX + ((t - this.v0) / (this.v1 - this.v0)) * this.plotW;
  }

  private tOf(xDev: number): number {
    return this.v0 + ((xDev - this.plotX) / this.plotW) * (this.v1 - this.v0);
  }

  private yOf(tick: number): number {
    return this.plotY + ((this.yHi - tick) / (this.yHi - this.yLo)) * this.plotH;
  }

  private tickOf(yDev: number): number {
    return this.yHi - ((yDev - this.plotY) / this.plotH) * (this.yHi - this.yLo);
  }

  private setView(a: number, b: number): void {
    const win = this.win;
    if (!win) return;
    const total = Math.max(1, win.t1 - win.t0);
    const minSpan = Math.min(total, Math.max(win.step_ns * 4, 1000));
    const span = clamp(b - a, minSpan, total);
    // A zoom that hit the span limit keeps its centre rather than drifting.
    let start = b - a === span ? a : (a + b) / 2 - span / 2;
    start = clamp(start, win.t0, win.t0 + total - span);
    if (start === this.v0 && start + span === this.v1) return;
    this.v0 = start;
    this.v1 = start + span;
    this.fitPrices();
    this.invalidate(true);
    this.opts.onView?.(this.v0, this.v1);
    this.emitHover();
  }

  // ----- painting -------------------------------------------------------------------------

  private invalidate(scene: boolean): void {
    if (scene) this.sceneDirty = true;
    if (this.raf || this.destroyed) return;
    this.raf = requestAnimationFrame(() => {
      this.raf = 0;
      this.paint();
    });
  }

  private paint(): void {
    if (this.destroyed) return;
    if (this.sceneDirty) {
      this.paintScene();
      this.sceneDirty = false;
    }
    const ctx = this.ctx;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.drawImage(this.scene, 0, 0);
    this.paintOverlay(ctx);
  }

  private font(sizeCss: number): string {
    return `${Math.round(sizeCss * this.dpr)}px ${this.opts.theme.mono}`;
  }

  private paintScene(): void {
    const ctx = this.sceneCtx;
    const { theme } = this.opts;
    const W = this.scene.width;
    const H = this.scene.height;
    const { plotX, plotY, plotW, plotH } = this;
    const win = this.win;

    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = theme.surface;
    ctx.fillRect(0, 0, W, H);
    ctx.fillStyle = theme.bg;
    ctx.fillRect(plotX, plotY, plotW, plotH);

    if (!win || !this.hasPrices) {
      ctx.fillStyle = theme.muted;
      ctx.font = this.font(12);
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText(
        win ? 'No book levels, quotes or trades in this window' : 'No window loaded',
        plotX + plotW / 2,
        plotY + plotH / 2,
      );
      this.paintFrame(ctx);
      return;
    }

    ctx.save();
    ctx.beginPath();
    ctx.rect(plotX, plotY, plotW, plotH);
    ctx.clip();

    this.paintHeat(ctx, win);
    this.paintBbo(ctx, win);
    this.paintTrades(ctx, win);

    ctx.restore();
    this.paintAxes(ctx);
    this.paintFrame(ctx);
  }

  private paintFrame(ctx: CanvasRenderingContext2D): void {
    const { theme } = this.opts;
    ctx.strokeStyle = theme.border;
    ctx.lineWidth = Math.max(1, Math.round(this.dpr));
    const half = ctx.lineWidth / 2;
    ctx.strokeRect(
      this.plotX + half,
      this.plotY + half,
      this.plotW - ctx.lineWidth,
      this.plotH - ctx.lineWidth,
    );
  }

  private paintHeat(ctx: CanvasRenderingContext2D, win: ReplayWindow): void {
    const heat = this.heat;
    if (!heat) return;
    const depth = win.depth;
    const topEdge = heat.tickTop + 0.5;
    const r0 = clamp(Math.floor((topEdge - this.yHi) / heat.ticksPerRow), 0, heat.rows);
    const r1 = clamp(Math.ceil((topEdge - this.yLo) / heat.ticksPerRow), 0, heat.rows);
    if (r1 <= r0) return;
    const dy0 = this.yOf(topEdge - r0 * heat.ticksPerRow);
    const dy1 = this.yOf(topEdge - r1 * heat.ticksPerRow);
    // Nearest-neighbour keeps cells crisp; smoothing is only used when several ticks share one
    // device pixel, where it averages instead of dropping rows.
    ctx.imageSmoothingEnabled = r1 - r0 > dy1 - dy0;
    const i0 = Math.max(0, frameIndexAt(depth, this.v0));
    for (let i = i0; i < depth.length; i += 1) {
      const ts = depth[i].ts;
      if (ts >= this.v1) break;
      const te = i + 1 < depth.length ? depth[i + 1].ts : Math.max(win.t1, ts);
      const x0 = Math.round(this.xOf(ts));
      const x1 = Math.round(this.xOf(te));
      if (x1 <= x0) continue;
      ctx.drawImage(heat.canvas, i, r0, 1, r1 - r0, x0, dy0, x1 - x0, dy1 - dy0);
    }
    ctx.imageSmoothingEnabled = true;
  }

  private paintBbo(ctx: CanvasRenderingContext2D, win: ReplayWindow): void {
    const { theme } = this.opts;
    const fg = cssToRgb(theme.fg);
    const bidLine = rgbCss(mix(cssToRgb(theme.buy), fg, 0.55));
    const askLine = rgbCss(mix(cssToRgb(theme.sell), fg, 0.55));
    ctx.lineWidth = Math.max(1, 1.25 * this.dpr);
    ctx.lineJoin = 'miter';
    this.strokeSteps(ctx, win, win.bbo.bid_tick, bidLine);
    this.strokeSteps(ctx, win, win.bbo.ask_tick, askLine);
  }

  private strokeSteps(
    ctx: CanvasRenderingContext2D,
    win: ReplayWindow,
    ticks: (number | null)[],
    color: string,
  ): void {
    const ts = win.bbo.ts;
    const n = Math.min(ts.length, ticks.length);
    if (n === 0) return;
    ctx.strokeStyle = color;
    ctx.beginPath();
    let pen = false;
    for (let i = Math.max(0, seriesIndexAt(ts, this.v0)); i < n; i += 1) {
      const t = ts[i];
      if (t > this.v1) break;
      const tick = ticks[i];
      if (tick === null || tick === undefined) {
        pen = false;
        continue;
      }
      const tEnd = i + 1 < n ? ts[i + 1] : win.t1;
      const x = this.xOf(Math.max(t, this.v0));
      const xEnd = this.xOf(Math.min(Math.max(tEnd, t), this.v1));
      const y = this.yOf(tick);
      if (pen) ctx.lineTo(x, y);
      else ctx.moveTo(x, y);
      ctx.lineTo(xEnd, y);
      pen = true;
    }
    ctx.stroke();
  }

  private paintTrades(ctx: CanvasRenderingContext2D, win: ReplayWindow): void {
    const { theme } = this.opts;
    const tr = win.trades;
    const n = Math.min(tr.ts.length, tr.price_tick.length, tr.qty.length, tr.side.length);
    if (n === 0) return;
    const maxQ = this.stats.maxTradeQty > 0 ? this.stats.maxTradeQty : 1;
    const colorOf = (side: Side | null): string =>
      side === 'buy' ? theme.buy : side === 'sell' ? theme.sell : theme.fg;
    ctx.lineWidth = Math.max(1, this.dpr);
    // seriesIndexAt gives the last trade at or before v0; start there so a mark straddling the
    // left edge is still drawn.
    for (let i = Math.max(0, seriesIndexAt(tr.ts, this.v0)); i < n; i += 1) {
      const t = tr.ts[i];
      if (t > this.v1) break;
      // Radius grows with sqrt(qty): the mark's area is proportional to traded quantity.
      const r = (1.5 + 5.5 * Math.sqrt(Math.max(0, tr.qty[i]) / maxQ)) * this.dpr;
      const x = this.xOf(t);
      const y = this.yOf(tr.price_tick[i]);
      const side = tr.side[i] ?? null;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
      if (side === null) {
        // Aggressor not stated by the data: hollow mark, no colour claim.
        ctx.strokeStyle = theme.bg;
        ctx.lineWidth = Math.max(1, 3 * this.dpr);
        ctx.stroke();
        ctx.strokeStyle = colorOf(null);
        ctx.lineWidth = Math.max(1, this.dpr);
        ctx.stroke();
      } else {
        ctx.fillStyle = colorOf(side);
        ctx.fill();
        ctx.strokeStyle = theme.bg;
        ctx.lineWidth = Math.max(1, this.dpr);
        ctx.stroke();
      }
    }
  }

  private paintAxes(ctx: CanvasRenderingContext2D): void {
    const { theme, formatPrice } = this.opts;
    const dpr = this.dpr;
    const { plotX, plotY, plotW, plotH } = this;
    ctx.font = this.font(11);
    ctx.fillStyle = theme.muted;
    ctx.strokeStyle = theme.border;
    ctx.lineWidth = Math.max(1, Math.round(dpr));

    // Price axis (right).
    const rowsWanted = Math.max(2, plotH / (26 * dpr));
    const tickStep = niceTickStep((this.yHi - this.yLo) / rowsWanted);
    ctx.textAlign = 'left';
    ctx.textBaseline = 'middle';
    const firstTick = Math.ceil(this.yLo / tickStep) * tickStep;
    for (let tick = firstTick; tick <= this.yHi; tick += tickStep) {
      const y = Math.round(this.yOf(tick));
      if (y < plotY + 6 * dpr || y > plotY + plotH - 6 * dpr) continue;
      ctx.beginPath();
      ctx.moveTo(plotX + plotW, y + 0.5);
      ctx.lineTo(plotX + plotW + 4 * dpr, y + 0.5);
      ctx.stroke();
      ctx.fillText(formatPrice(tick), plotX + plotW + 7 * dpr, y);
    }

    // Time axis (bottom).
    const colsWanted = Math.max(2, plotW / (96 * dpr));
    const timeStep = niceTimeStep((this.v1 - this.v0) / colsWanted);
    ctx.textAlign = 'center';
    ctx.textBaseline = 'top';
    const firstT = Math.ceil(this.v0 / timeStep) * timeStep;
    for (let t = firstT; t <= this.v1; t += timeStep) {
      const x = Math.round(this.xOf(t));
      ctx.beginPath();
      ctx.moveTo(x + 0.5, plotY + plotH);
      ctx.lineTo(x + 0.5, plotY + plotH + 4 * dpr);
      ctx.stroke();
      const label = clockLabel(t, timeStep);
      const half = ctx.measureText(label).width / 2;
      if (x - half < plotX || x + half > plotX + plotW) continue;
      ctx.fillText(label, x, plotY + plotH + 6 * dpr);
    }
  }

  private paintOverlay(ctx: CanvasRenderingContext2D): void {
    const win = this.win;
    if (!win || !this.hasPrices) return;
    const { theme, formatPrice } = this.opts;
    const dpr = this.dpr;
    const { plotX, plotY, plotW, plotH } = this;

    // Playback cursor.
    if (this.cursor !== null && this.cursor >= this.v0 && this.cursor <= this.v1) {
      const x = Math.round(this.xOf(this.cursor)) + 0.5;
      ctx.strokeStyle = theme.accent;
      ctx.lineWidth = Math.max(1, Math.round(1.5 * dpr));
      ctx.beginPath();
      ctx.moveTo(x, plotY);
      ctx.lineTo(x, plotY + plotH);
      ctx.stroke();
      ctx.fillStyle = theme.accent;
      ctx.beginPath();
      ctx.moveTo(x - 5 * dpr, plotY - PAD_TOP * dpr);
      ctx.lineTo(x + 5 * dpr, plotY - PAD_TOP * dpr);
      ctx.lineTo(x, plotY + 1);
      ctx.closePath();
      ctx.fill();
    }

    // Crosshair with axis read-outs.
    const p = this.pointer;
    if (!p || this.drag?.moved) return;
    const xDev = p.x * dpr;
    const yDev = p.y * dpr;
    if (xDev < plotX || xDev > plotX + plotW || yDev < plotY || yDev > plotY + plotH) return;
    const tick = Math.round(this.tickOf(yDev));
    const yRow = Math.round(this.yOf(tick)) + 0.5;
    const xLine = Math.round(xDev) + 0.5;

    ctx.strokeStyle = theme.fg;
    ctx.globalAlpha = 0.55;
    ctx.lineWidth = Math.max(1, Math.round(dpr));
    ctx.setLineDash([3 * dpr, 3 * dpr]);
    ctx.beginPath();
    ctx.moveTo(xLine, plotY);
    ctx.lineTo(xLine, plotY + plotH);
    ctx.moveTo(plotX, yRow);
    ctx.lineTo(plotX + plotW, yRow);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;

    ctx.font = this.font(11);
    const boxH = 16 * dpr;

    // Price read-out on the right axis.
    ctx.fillStyle = theme.fg;
    ctx.fillRect(plotX + plotW + 1, yRow - boxH / 2, this.scene.width - (plotX + plotW) - 1, boxH);
    ctx.fillStyle = theme.bg;
    ctx.textAlign = 'left';
    ctx.textBaseline = 'middle';
    ctx.fillText(formatPrice(tick), plotX + plotW + 7 * dpr, yRow);

    // Time read-out on the bottom axis.
    const span = this.v1 - this.v0;
    const label = clockLabel(this.tOf(xDev), span > 60e9 ? 1e6 : 1e3);
    const w = ctx.measureText(label).width + 10 * dpr;
    const bx = clamp(xLine - w / 2, plotX, plotX + plotW - w);
    ctx.fillStyle = theme.fg;
    ctx.fillRect(bx, plotY + plotH + 1, w, boxH + 2 * dpr);
    ctx.fillStyle = theme.bg;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'top';
    ctx.fillText(label, bx + w / 2, plotY + plotH + 5 * dpr);
  }

  // ----- hover ----------------------------------------------------------------------------

  private hoverAt(xCss: number, yCss: number): HeatmapHover | null {
    const win = this.win;
    if (!win || !this.hasPrices) return null;
    const xDev = xCss * this.dpr;
    const yDev = yCss * this.dpr;
    if (
      xDev < this.plotX ||
      xDev > this.plotX + this.plotW ||
      yDev < this.plotY ||
      yDev > this.plotY + this.plotH
    ) {
      return null;
    }
    const t = this.tOf(xDev);
    const tick = Math.round(this.tickOf(yDev));
    const fi = frameIndexAt(win.depth, t);
    const frame = fi >= 0 ? win.depth[fi] : null;
    const bi = seriesIndexAt(win.bbo.ts, t);
    return {
      x: xCss,
      y: yCss,
      t,
      tick,
      frameTs: frame ? frame.ts : null,
      bid: frame ? lookupLevel(frame, 'bid', tick, win.levels) : null,
      ask: frame ? lookupLevel(frame, 'ask', tick, win.levels) : null,
      bbo:
        bi >= 0
          ? {
              ts: win.bbo.ts[bi],
              bidTick: win.bbo.bid_tick[bi] ?? null,
              askTick: win.bbo.ask_tick[bi] ?? null,
              bidQty: win.bbo.bid_qty[bi],
              askQty: win.bbo.ask_qty[bi],
            }
          : null,
    };
  }

  private emitHover(): void {
    const p = this.pointer;
    this.opts.onHover?.(p && !this.drag?.moved ? this.hoverAt(p.x, p.y) : null);
  }

  // ----- events ---------------------------------------------------------------------------

  private localPoint(e: MouseEvent): { x: number; y: number } {
    const rect = this.canvas.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  }

  private readonly onWheel = (e: WheelEvent): void => {
    if (!this.win) return;
    e.preventDefault();
    const p = this.localPoint(e);
    const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? this.cssH : 1;
    const dx = e.deltaX * unit;
    const dy = e.deltaY * unit;
    const plotWCss = this.plotW / this.dpr;
    if (e.shiftKey || Math.abs(dx) > Math.abs(dy)) {
      const d = Math.abs(dx) > Math.abs(dy) ? dx : dy;
      this.panBy(d / plotWCss);
      return;
    }
    const anchor = clamp(this.tOf(p.x * this.dpr), this.v0, this.v1);
    this.zoomBy(Math.exp(clamp(dy, -400, 400) * 0.0015), anchor);
  };

  private readonly onPointerDown = (e: PointerEvent): void => {
    if (e.button !== 0 || !this.win) return;
    const p = this.localPoint(e);
    this.pointer = p;
    this.drag = { pointerId: e.pointerId, startX: p.x, v0: this.v0, v1: this.v1, moved: false };
    this.canvas.setPointerCapture(e.pointerId);
  };

  private readonly onPointerMove = (e: PointerEvent): void => {
    const p = this.localPoint(e);
    this.pointer = p;
    const drag = this.drag;
    if (drag && drag.pointerId === e.pointerId) {
      const dx = p.x - drag.startX;
      if (!drag.moved && Math.abs(dx) > DRAG_THRESHOLD_PX) {
        drag.moved = true;
        this.canvas.style.cursor = 'grabbing';
        this.opts.onHover?.(null);
      }
      if (drag.moved) {
        const dt = (dx / (this.plotW / this.dpr)) * (drag.v1 - drag.v0);
        this.setView(drag.v0 - dt, drag.v1 - dt);
        return;
      }
    }
    this.emitHover();
    this.invalidate(false);
  };

  private readonly onPointerUp = (e: PointerEvent): void => {
    const drag = this.drag;
    if (!drag || drag.pointerId !== e.pointerId) return;
    this.drag = null;
    this.canvas.style.cursor = '';
    if (this.canvas.hasPointerCapture(e.pointerId)) {
      this.canvas.releasePointerCapture(e.pointerId);
    }
    const win = this.win;
    if (!drag.moved && win) {
      const p = this.localPoint(e);
      const xDev = p.x * this.dpr;
      if (xDev >= this.plotX && xDev <= this.plotX + this.plotW) {
        this.opts.onCursor?.(clamp(this.tOf(xDev), win.t0, win.t1));
      }
    }
    this.emitHover();
    this.invalidate(false);
  };

  private readonly onPointerCancel = (e: PointerEvent): void => {
    if (this.drag && this.drag.pointerId === e.pointerId) {
      this.drag = null;
      this.canvas.style.cursor = '';
    }
  };

  private readonly onPointerLeave = (): void => {
    if (this.drag) return;
    this.pointer = null;
    this.opts.onHover?.(null);
    this.invalidate(false);
  };

  private readonly onDoubleClick = (): void => {
    this.resetView();
  };

  private readonly onDprChange = (): void => {
    this.watchDpr();
    this.resize();
  };

  /** A resolution media query fires once when the ratio changes, so it is re-armed each time. */
  private watchDpr(): void {
    this.dprQuery?.removeEventListener('change', this.onDprChange);
    this.dprQuery = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
    this.dprQuery.addEventListener('change', this.onDprChange);
  }
}
