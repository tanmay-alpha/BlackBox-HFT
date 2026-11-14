// Two-clock timeline renderer (Canvas 2D).
//
// Two lanes share one relative-time axis that starts at the strategy's decision: LOCAL is the
// strategy host, EXCHANGE is the simulated matching engine. Every tape event is a marker on the
// lane of its clock. A modeled latency is a slanted connector between the lanes, because that is
// exactly what it is: the same message at two different times on two different clocks.
//
// Intervals in one order's life span microseconds to minutes, so the axis has two modes. Linear
// is true to scale and is zoomed/panned; ordinal places each distinct instant one step apart,
// which keeps every event legible but does not show durations.

import type { FlightEvent } from "../../api/types";
import { fmtDuration } from "../../components/fmt";
import { cssVar } from "../../components/theme";
import { fmtAxisNs, niceStep, ticksBetween } from "./axis";

export type AxisMode = "linear" | "ordinal";
export type Lane = "local" | "exchange";

export type Glyph =
  | "intent"
  | "cancel_intent"
  | "risk"
  | "request"
  | "cancel_request"
  | "refused"
  | "queued"
  | "queue_trade"
  | "queue_depth"
  | "fill"
  | "response"
  | "cancel_response"
  | "local_response"
  | "cancel_ack"
  | "truncated"
  | "other";

export interface TimelineMarker {
  seq: number;
  lane: Lane;
  /** Nanoseconds since the decision. */
  t: number;
  /** Rank of `t` among the distinct instants of this order. */
  ord: number;
  glyph: Glyph;
  event: FlightEvent;
}

/** A modeled latency: one message leaving one clock domain and arriving in the other. */
export interface TimelineConnector {
  seq: number;
  from: Lane;
  t0: number;
  ord0: number;
  t1: number;
  ord1: number;
  cancel: boolean;
}

export interface TimelineModel {
  markers: TimelineMarker[];
  connectors: TimelineConnector[];
  /** Distinct instants (ns since the decision), ascending. Index = ordinal position. */
  instants: number[];
}

function glyphOf(ev: FlightEvent): Glyph {
  const b = ev.body;
  switch (b.type) {
    case "order_intent":
      return "intent";
    case "cancel_intent":
      return "cancel_intent";
    case "risk_blocked":
    case "risk_halt":
      return "risk";
    case "request_sent":
      return b.req === "cancel" ? "cancel_request" : "request";
    case "request_refused":
      return "refused";
    case "exch_queued":
      return "queued";
    case "queue_update":
      return b.cause.type === "trade" ? "queue_trade" : "queue_depth";
    case "exch_fill":
      return "fill";
    case "exch_response":
      // `request_rejected` is only ever set on the answer to a cancel.
      return b.status === "canceled" || b.request_rejected ? "cancel_response" : "response";
    case "local_response":
      return b.status === "canceled" || b.request_rejected ? "cancel_ack" : "local_response";
    case "queue_truncated":
      return "truncated";
    default:
      return "other";
  }
}

export function buildTimeline(events: readonly FlightEvent[], originNs: number): TimelineModel {
  const pending: Array<Omit<TimelineMarker, "ord">> = [];
  const links: Array<Omit<TimelineConnector, "ord0" | "ord1">> = [];
  const seen = new Set<number>();

  for (const ev of events) {
    const t = ev.ts - originNs;
    const glyph = glyphOf(ev);
    seen.add(t);
    pending.push({ seq: ev.seq, lane: ev.clock, t, glyph, event: ev });

    const b = ev.body;
    if (b.type === "request_sent" && b.exch_arrival_ts !== null) {
      const t1 = b.exch_arrival_ts - originNs;
      seen.add(t1);
      links.push({ seq: ev.seq, from: "local", t0: t, t1, cancel: b.req === "cancel" });
    } else if (b.type === "exch_response") {
      const t1 = b.local_arrival_ts - originNs;
      seen.add(t1);
      links.push({ seq: ev.seq, from: "exchange", t0: t, t1, cancel: glyph === "cancel_response" });
    }
  }

  const instants = Array.from(seen).sort((a, b) => a - b);
  const rank = new Map<number, number>();
  instants.forEach((t, i) => rank.set(t, i));
  const ordOf = (t: number) => rank.get(t) ?? 0;

  return {
    markers: pending.map((m) => ({ ...m, ord: ordOf(m.t) })),
    connectors: links.map((c) => ({ ...c, ord0: ordOf(c.t0), ord1: ordOf(c.t1) })),
    instants,
  };
}

interface Palette {
  fg: string;
  muted: string;
  border: string;
  accent: string;
  buy: string;
  sell: string;
  observed: string;
  modeled: string;
  warn: string;
  error: string;
  mono: string;
  sans: string;
}

/** Token values at draw time (styles/tokens.css); the side palette setting applies on redraw. */
function readPalette(): Palette {
  return {
    fg: cssVar("--fg-0"),
    muted: cssVar("--fg-2"),
    border: cssVar("--border"),
    accent: cssVar("--accent"),
    buy: cssVar("--bid"),
    sell: cssVar("--ask"),
    observed: cssVar("--observed"),
    modeled: cssVar("--modeled"),
    warn: cssVar("--warn"),
    error: cssVar("--error"),
    mono: cssVar("--font-mono"),
    sans: cssVar("--font-sans"),
  };
}

const PAD_RIGHT = 18;
const AXIS_HEIGHT = 30;
const HIT_RADIUS = 10;
/** Linear zoom floor: below this a nanosecond-resolution tape has nothing further to resolve. */
const MIN_SPAN_NS = 40;
const MIN_SPAN_ORDINAL = 2;

export class TimelineRenderer {
  private readonly canvas: HTMLCanvasElement;
  private readonly ctx: CanvasRenderingContext2D;
  private palette: Palette;
  private width = 0;
  private height = 0;
  private dpr = 1;
  private model: TimelineModel = { markers: [], connectors: [], instants: [] };
  private mode: AxisMode = "linear";
  /** Visible window in axis units: ns since the decision (linear) or instant rank (ordinal). */
  private v0 = 0;
  private v1 = 1;
  private selected: number | null = null;
  private cited: ReadonlySet<number> = new Set<number>();
  private hovered: number | null = null;
  private side: "buy" | "sell" = "buy";
  private frame = 0;

  constructor(canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("Canvas 2D is not available in this browser");
    this.canvas = canvas;
    this.ctx = ctx;
    this.palette = readPalette();
  }

  destroy(): void {
    if (this.frame !== 0) cancelAnimationFrame(this.frame);
    this.frame = 0;
  }

  resize(cssWidth: number, cssHeight: number, dpr: number): void {
    this.width = Math.max(0, Math.floor(cssWidth));
    this.height = Math.max(0, Math.floor(cssHeight));
    this.dpr = dpr > 0 ? dpr : 1;
    this.canvas.width = Math.round(this.width * this.dpr);
    this.canvas.height = Math.round(this.height * this.dpr);
    this.palette = readPalette();
    // Resizing clears the bitmap, so paint in the same task to avoid a blank frame.
    this.draw();
  }

  setModel(model: TimelineModel, side: "buy" | "sell"): void {
    this.model = model;
    this.side = side;
    this.resetView();
  }

  setMode(mode: AxisMode): void {
    if (mode === this.mode) return;
    this.mode = mode;
    this.resetView();
  }

  setMarks(selected: number | null, cited: ReadonlySet<number>): void {
    this.selected = selected;
    this.cited = cited;
    this.invalidate();
  }

  setHover(seq: number | null): void {
    if (seq === this.hovered) return;
    this.hovered = seq;
    this.invalidate();
  }

  resetView(): void {
    const [lo, hi] = this.extent();
    this.v0 = lo;
    this.v1 = hi;
    this.invalidate();
  }

  /** Zooms about the pixel column `x`; `factor < 1` zooms in. */
  zoomAt(x: number, factor: number): void {
    const { left, plotWidth } = this.layout();
    if (plotWidth <= 0) return;
    const frac = Math.min(1, Math.max(0, (x - left) / plotWidth));
    const span = this.v1 - this.v0;
    const [lo, hi] = this.extent();
    const minSpan = this.mode === "linear" ? MIN_SPAN_NS : MIN_SPAN_ORDINAL;
    const next = Math.min(hi - lo, Math.max(Math.min(minSpan, hi - lo), span * factor));
    const anchor = this.v0 + frac * span;
    this.v0 = anchor - frac * next;
    this.v1 = this.v0 + next;
    this.clampView();
    this.invalidate();
  }

  panBy(dxPx: number): void {
    const { plotWidth } = this.layout();
    if (plotWidth <= 0) return;
    const shift = (-dxPx / plotWidth) * (this.v1 - this.v0);
    this.v0 += shift;
    this.v1 += shift;
    this.clampView();
    this.invalidate();
  }

  /** Scrolls the window, keeping its span, so the marker of `seq` is visible. */
  reveal(seq: number): void {
    const m = this.model.markers.find((x) => x.seq === seq);
    if (!m) return;
    const v = this.axisValue(m.t, m.ord);
    if (v >= this.v0 && v <= this.v1) return;
    const span = this.v1 - this.v0;
    this.v0 = v - span / 2;
    this.v1 = v + span / 2;
    this.clampView();
    this.invalidate();
  }

  /** The marker nearest to a canvas-relative point, if one is within reach. */
  hitTest(x: number, y: number): TimelineMarker | null {
    let best: TimelineMarker | null = null;
    let bestScore = Infinity;
    for (const m of this.model.markers) {
      const dx = this.xOf(this.axisValue(m.t, m.ord)) - x;
      const dy = this.laneY(m.lane) - y;
      const dist = Math.hypot(dx, dy);
      if (dist > HIT_RADIUS) continue;
      // Queue updates are dense; lifecycle markers win when both are in reach.
      const score = dist + (m.glyph === "queue_trade" || m.glyph === "queue_depth" ? 4 : 0);
      if (score < bestScore) {
        bestScore = score;
        best = m;
      }
    }
    return best;
  }

  private invalidate(): void {
    if (this.frame !== 0) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.draw();
    });
  }

  private extent(): [number, number] {
    const { instants } = this.model;
    if (this.mode === "ordinal") {
      return instants.length <= 1 ? [-1, 1] : [-0.6, instants.length - 0.4];
    }
    const last = instants.length > 0 ? (instants[instants.length - 1] as number) : 0;
    const first = instants.length > 0 ? Math.min(0, instants[0] as number) : 0;
    if (last - first <= 0) return [first - 500, first + 500];
    const pad = (last - first) * 0.04;
    return [first - pad, last + pad];
  }

  private clampView(): void {
    const [lo, hi] = this.extent();
    const span = Math.min(this.v1 - this.v0, hi - lo);
    if (this.v0 < lo) this.v0 = lo;
    if (this.v0 + span > hi) this.v0 = hi - span;
    this.v1 = this.v0 + span;
  }

  private layout(): { left: number; plotWidth: number; narrow: boolean } {
    const narrow = this.width < 560;
    const left = narrow ? 84 : 168;
    return { left, plotWidth: this.width - left - PAD_RIGHT, narrow };
  }

  private laneY(lane: Lane): number {
    const top = 44;
    const bottom = this.height - AXIS_HEIGHT - 30;
    return lane === "local" ? top : Math.max(top + 40, bottom);
  }

  private axisValue(t: number, ord: number): number {
    return this.mode === "linear" ? t : ord;
  }

  private xOf(value: number): number {
    const { left, plotWidth } = this.layout();
    return left + ((value - this.v0) / (this.v1 - this.v0)) * plotWidth;
  }

  private draw(): void {
    const { ctx, palette: p } = this;
    if (this.width <= 0 || this.height <= 0) return;
    const { left, plotWidth, narrow } = this.layout();
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, this.width, this.height);
    if (plotWidth <= 0) return;

    const yLocal = this.laneY("local");
    const yExch = this.laneY("exchange");
    const axisY = this.height - AXIS_HEIGHT;

    this.drawAxis(left, plotWidth, axisY);

    // Lanes and their labels.
    ctx.lineWidth = 1;
    ctx.strokeStyle = p.border;
    ctx.beginPath();
    ctx.moveTo(left, yLocal + 0.5);
    ctx.lineTo(left + plotWidth, yLocal + 0.5);
    ctx.moveTo(left, yExch + 0.5);
    ctx.lineTo(left + plotWidth, yExch + 0.5);
    ctx.stroke();

    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    ctx.fillStyle = p.fg;
    ctx.font = `600 11px ${p.mono}`;
    ctx.fillText("LOCAL", 10, yLocal - (narrow ? 0 : 6));
    ctx.fillText("EXCHANGE", 10, yExch - (narrow ? 0 : 6));
    if (!narrow) {
      ctx.fillStyle = p.muted;
      ctx.font = `11px ${p.sans}`;
      ctx.fillText("strategy host", 10, yLocal + 8);
      ctx.fillText("simulated matching engine", 10, yExch + 8);
    }

    ctx.save();
    ctx.beginPath();
    ctx.rect(left - 14, 0, plotWidth + 14 + PAD_RIGHT, axisY);
    ctx.clip();

    for (const c of this.model.connectors) {
      const x0 = this.xOf(this.axisValue(c.t0, c.ord0));
      const x1 = this.xOf(this.axisValue(c.t1, c.ord1));
      const y0 = c.from === "local" ? yLocal : yExch;
      const y1 = c.from === "local" ? yExch : yLocal;
      this.drawConnector(x0, y0, x1, y1, c.cancel);
    }

    // Dense queue ticks first, lifecycle markers above them, fills on top.
    const order = (m: TimelineMarker) =>
      m.glyph === "queue_depth" ? 0 : m.glyph === "queue_trade" ? 1 : m.glyph === "fill" ? 3 : 2;
    for (let layer = 0; layer <= 3; layer++) {
      for (const m of this.model.markers) {
        if (order(m) !== layer) continue;
        const x = this.xOf(this.axisValue(m.t, m.ord));
        if (x < left - 20 || x > left + plotWidth + 20) continue;
        this.drawGlyph(m.glyph, x, this.laneY(m.lane));
      }
    }

    for (const m of this.model.markers) {
      const isSelected = m.seq === this.selected;
      const isCited = this.cited.has(m.seq);
      const isHovered = m.seq === this.hovered;
      if (!isSelected && !isCited && !isHovered) continue;
      const x = this.xOf(this.axisValue(m.t, m.ord));
      const y = this.laneY(m.lane);
      ctx.setLineDash(isCited && !isSelected ? [3, 3] : []);
      ctx.lineWidth = isSelected ? 2 : 1.25;
      ctx.strokeStyle = isSelected || isCited ? p.accent : p.fg;
      ctx.globalAlpha = isSelected || isCited ? 1 : 0.7;
      ctx.beginPath();
      ctx.arc(x, y, isSelected ? 14 : 12, 0, Math.PI * 2);
      ctx.stroke();
      ctx.globalAlpha = 1;
      ctx.setLineDash([]);
    }

    ctx.restore();
  }

  private drawAxis(left: number, plotWidth: number, axisY: number): void {
    const { ctx, palette: p } = this;
    ctx.lineWidth = 1;
    ctx.font = `11px ${p.mono}`;
    ctx.textBaseline = "top";
    ctx.textAlign = "center";

    const gridLine = (x: number) => {
      ctx.strokeStyle = p.border;
      ctx.globalAlpha = 0.55;
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, 18);
      ctx.lineTo(Math.round(x) + 0.5, axisY + 4);
      ctx.stroke();
      ctx.globalAlpha = 1;
    };

    ctx.strokeStyle = p.border;
    ctx.beginPath();
    ctx.moveTo(left, axisY + 0.5);
    ctx.lineTo(left + plotWidth, axisY + 0.5);
    ctx.stroke();

    ctx.fillStyle = p.muted;
    if (this.mode === "linear") {
      const step = niceStep((this.v1 - this.v0) / Math.max(2, Math.floor(plotWidth / 120)));
      const scale = Math.max(Math.abs(this.v0), Math.abs(this.v1));
      for (const tick of ticksBetween(this.v0, this.v1, step)) {
        const x = this.xOf(tick);
        gridLine(x);
        const label =
          tick === 0 ? "decision" : `${tick > 0 ? "+" : ""}${fmtAxisNs(tick, step, scale)}`;
        ctx.fillText(label, x, axisY + 8);
      }
      return;
    }

    const { instants } = this.model;
    const from = Math.max(0, Math.ceil(this.v0));
    const to = Math.min(instants.length - 1, Math.floor(this.v1));
    let lastLabelX = -Infinity;
    for (let i = from; i <= to; i++) {
      const x = this.xOf(i);
      if (x - lastLabelX < 92) continue;
      lastLabelX = x;
      gridLine(x);
      const t = instants[i] as number;
      ctx.fillText(t === 0 ? "decision" : `+${fmtDuration(t)}`, x, axisY + 8);
    }
  }

  private drawConnector(x0: number, y0: number, x1: number, y1: number, cancel: boolean): void {
    const { ctx, palette: p } = this;
    const color = cancel ? p.warn : p.modeled;
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineWidth = 1.25;
    ctx.globalAlpha = 0.85;
    ctx.setLineDash(cancel ? [5, 3] : []);
    ctx.beginPath();
    ctx.moveTo(x0, y0);
    ctx.lineTo(x1, y1);
    ctx.stroke();
    ctx.setLineDash([]);

    // Arrow head just short of the scheduled-arrival point, so both stay readable.
    const angle = Math.atan2(y1 - y0, x1 - x0);
    const tipX = x1 - Math.cos(angle) * 5;
    const tipY = y1 - Math.sin(angle) * 5;
    ctx.beginPath();
    ctx.moveTo(tipX, tipY);
    ctx.lineTo(tipX - Math.cos(angle - 0.42) * 8, tipY - Math.sin(angle - 0.42) * 8);
    ctx.lineTo(tipX - Math.cos(angle + 0.42) * 8, tipY - Math.sin(angle + 0.42) * 8);
    ctx.closePath();
    ctx.fill();

    // The scheduled arrival is a modeled instant, not a tape event: an open dot, never a marker.
    ctx.beginPath();
    ctx.arc(x1, y1, 2.5, 0, Math.PI * 2);
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  private drawGlyph(glyph: Glyph, x: number, y: number): void {
    const { ctx, palette: p } = this;
    ctx.setLineDash([]);
    ctx.lineWidth = 1.5;

    const diamond = (r: number) => {
      ctx.beginPath();
      ctx.moveTo(x, y - r);
      ctx.lineTo(x + r, y);
      ctx.lineTo(x, y + r);
      ctx.lineTo(x - r, y);
      ctx.closePath();
    };
    // `dir` = +1 points down (toward the exchange lane), -1 points up (toward the local lane).
    const triangle = (r: number, dir: 1 | -1) => {
      ctx.beginPath();
      ctx.moveTo(x, y + dir * r);
      ctx.lineTo(x + r, y - dir * r * 0.8);
      ctx.lineTo(x - r, y - dir * r * 0.8);
      ctx.closePath();
    };
    const circle = (r: number) => {
      ctx.beginPath();
      ctx.arc(x, y, r, 0, Math.PI * 2);
    };
    const cross = (r: number) => {
      ctx.beginPath();
      ctx.moveTo(x - r, y - r);
      ctx.lineTo(x + r, y + r);
      ctx.moveTo(x + r, y - r);
      ctx.lineTo(x - r, y + r);
    };
    const vertical = (r: number) => {
      ctx.beginPath();
      ctx.moveTo(Math.round(x) + 0.5, y - r);
      ctx.lineTo(Math.round(x) + 0.5, y + r);
    };

    switch (glyph) {
      case "intent":
        ctx.fillStyle = p.fg;
        diamond(6.5);
        ctx.fill();
        break;
      case "cancel_intent":
        ctx.strokeStyle = p.warn;
        diamond(6.5);
        ctx.stroke();
        break;
      case "risk":
        ctx.fillStyle = p.error;
        diamond(6.5);
        ctx.fill();
        break;
      case "request":
        ctx.fillStyle = p.modeled;
        triangle(6, 1);
        ctx.fill();
        break;
      case "cancel_request":
        ctx.strokeStyle = p.warn;
        triangle(6, 1);
        ctx.stroke();
        break;
      case "refused":
        ctx.strokeStyle = p.error;
        ctx.lineWidth = 2;
        cross(5);
        ctx.stroke();
        break;
      case "queued":
        ctx.strokeStyle = p.modeled;
        circle(5);
        ctx.stroke();
        break;
      case "queue_trade":
        ctx.strokeStyle = p.observed;
        vertical(8);
        ctx.stroke();
        break;
      case "queue_depth":
        ctx.strokeStyle = p.muted;
        ctx.lineWidth = 1;
        vertical(4);
        ctx.stroke();
        break;
      case "fill": {
        const color = this.side === "buy" ? p.buy : p.sell;
        ctx.fillStyle = color;
        ctx.strokeStyle = color;
        circle(6);
        ctx.fill();
        circle(9.5);
        ctx.stroke();
        break;
      }
      case "response":
        ctx.fillStyle = p.modeled;
        triangle(6, -1);
        ctx.fill();
        break;
      case "cancel_response":
        ctx.strokeStyle = p.warn;
        triangle(6, -1);
        ctx.stroke();
        break;
      case "local_response":
        ctx.fillStyle = p.fg;
        circle(4);
        ctx.fill();
        break;
      case "cancel_ack":
        ctx.strokeStyle = p.warn;
        circle(6);
        ctx.stroke();
        cross(2.5);
        ctx.stroke();
        break;
      case "truncated":
        ctx.strokeStyle = p.muted;
        ctx.strokeRect(x - 4.5, y - 4.5, 9, 9);
        break;
      case "other":
        ctx.fillStyle = p.muted;
        circle(3);
        ctx.fill();
        break;
    }
  }
}
