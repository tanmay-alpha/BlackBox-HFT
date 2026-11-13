// The run's own simulated orders and fills, drawn over the dataset heatmap through the renderer's
// `paintExtra` hook. Everything here is modeled (simulated on historical replay), so it is drawn
// dashed / outlined in the modeled colour to stay visually distinct from the observed book and
// trade prints underneath.

import type { FillRecord, OrderRecord } from '../../api/types';
import type { HeatmapTransform } from '../replay/heatmap';

export interface OverlayTheme {
  buy: string;
  sell: string;
  modeled: string;
  accent: string;
  bg: string;
}

/**
 * Exchange-clock span during which an order was at the simulated exchange: from arrival to the
 * time it became terminal there. `null` end means it was still working when the data ended.
 * Orders that never reached the exchange have no span.
 */
export function restingSpan(o: OrderRecord): { from: number; to: number | null } | null {
  if (o.exch_arrival_ts === null) return null;
  return { from: o.exch_arrival_ts, to: o.terminal_ts };
}

/** Orders at the simulated exchange at exchange time `t` (arrived, not yet terminal). */
export function ordersRestingAt(orders: OrderRecord[], t: number): OrderRecord[] {
  return orders.filter((o) => {
    const span = restingSpan(o);
    return span !== null && span.from <= t && (span.to === null || span.to > t);
  });
}

export function ownOrdersPainter(
  orders: OrderRecord[],
  fills: FillRecord[],
  selected: number | null,
  theme: OverlayTheme,
): (ctx: CanvasRenderingContext2D, xf: HeatmapTransform) => void {
  return (ctx, xf) => {
    const { dpr, v0, v1 } = xf;
    ctx.save();
    ctx.lineCap = 'butt';

    // Resting price lines, selected order last so it sits on top.
    const ordered = [...orders].sort(
      (a, b) => Number(a.order_id === selected) - Number(b.order_id === selected),
    );
    for (const o of ordered) {
      const span = restingSpan(o);
      if (!span) continue;
      const end = span.to ?? v1;
      if (end < v0 || span.from > v1) continue;
      const x0 = xf.xOf(Math.max(span.from, v0));
      const x1 = Math.max(x0 + 2 * dpr, xf.xOf(Math.min(end, v1)));
      const y = Math.round(xf.yOf(o.price_tick)) + 0.5;
      const isSel = o.order_id === selected;
      if (isSel) {
        ctx.strokeStyle = theme.accent;
        ctx.globalAlpha = 0.45;
        ctx.lineWidth = 7 * dpr;
        ctx.setLineDash([]);
        ctx.beginPath();
        ctx.moveTo(x0, y);
        ctx.lineTo(x1, y);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }
      ctx.strokeStyle = o.side === 'buy' ? theme.buy : theme.sell;
      ctx.lineWidth = (isSel ? 2.5 : 1.75) * dpr;
      ctx.setLineDash([5 * dpr, 3 * dpr]);
      ctx.beginPath();
      ctx.moveTo(x0, y);
      ctx.lineTo(x1, y);
      ctx.stroke();
      ctx.setLineDash([]);
    }

    // Simulated fills: diamonds in the side colour with a modeled-colour outline.
    for (const f of fills) {
      if (f.ts < v0 || f.ts > v1) continue;
      const x = xf.xOf(f.ts);
      const y = xf.yOf(f.price_tick);
      const r = (f.order_id === selected ? 5.5 : 3.5) * dpr;
      ctx.beginPath();
      ctx.moveTo(x, y - r);
      ctx.lineTo(x + r, y);
      ctx.lineTo(x, y + r);
      ctx.lineTo(x - r, y);
      ctx.closePath();
      ctx.fillStyle = f.side === 'buy' ? theme.buy : theme.sell;
      ctx.fill();
      ctx.strokeStyle = theme.modeled;
      ctx.lineWidth = Math.max(1, dpr);
      ctx.stroke();
    }
    ctx.restore();
  };
}
