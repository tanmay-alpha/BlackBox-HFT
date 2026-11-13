import { useMemo } from "react";
import type { FlightEvent, InstrumentMeta, QueuePoint, Side } from "../../api/types";
import { fmtQty } from "../../components/fmt";
import { Badge, Empty } from "../../components/ui";
import { fmtAxisNs, lastAtOrBefore, niceStep, ticksBetween } from "./axis";
import { fmtSince } from "./labels";
import type { AxisMode } from "./timeline";
import { useElementSize } from "./useElementSize";

const HEIGHT = 286;
const LEFT = 72;
const RIGHT = 18;
const TOP = 16;
const GAP = 16;
const TRADE_BAND = 46;
const AXIS_BAND = 26;
const PLOT_HEIGHT = HEIGHT - TOP - GAP - TRADE_BAND - AXIS_BAND;

interface TradeBar {
  seq: number;
  ts: number;
  qty: number;
  x: number;
}

interface FillMark {
  seq: number;
  ts: number;
  qty: number;
  maker: boolean;
  x: number;
  y: number;
}

interface Geometry {
  stepPath: string;
  points: Array<{ seq: number; ts: number; ahead: number; x: number; y: number }>;
  yTicks: Array<{ y: number; label: string }>;
  xTicks: Array<{ x: number; label: string }>;
  zeroY: number;
  trades: TradeBar[];
  maxTradeQty: number;
  fills: FillMark[];
  /** Pixel column of an exchange-clock instant inside the charted interval, else null. */
  xOfExchangeTs: (ts: number) => number | null;
}

function buildGeometry(
  path: QueuePoint[],
  events: FlightEvent[],
  originNs: number,
  instrument: InstrumentMeta,
  mode: AxisMode,
  width: number,
): Geometry {
  const n = path.length;
  const plotWidth = width - LEFT - RIGHT;
  const times = path.map((p) => p.ts);
  const indexOfSeq = new Map<number, number>();
  path.forEach((p, i) => indexOfSeq.set(p.seq, i));

  const tStart = n > 0 ? (times[0] as number) : 0;
  let tEnd = n > 0 ? (times[n - 1] as number) : 0;
  for (const ev of events) {
    if (ev.clock === "exchange" && ev.ts > tEnd) tEnd = ev.ts;
  }
  if (tEnd <= tStart) tEnd = tStart + 1;

  const xLinear = (ts: number) => LEFT + ((ts - tStart) / (tEnd - tStart)) * plotWidth;
  // Ordinal axis: point i sits at column i of n + 1, leaving room for the last step to extend.
  const xIndex = (i: number) => LEFT + (i / Math.max(1, n)) * plotWidth;
  const ordinalOf = (ts: number) => {
    const i = lastAtOrBefore(times, ts);
    if (i < 0) return 0;
    return times[i] === ts ? i : i + 0.5;
  };
  const xOfTs = (ts: number) => (mode === "linear" ? xLinear(ts) : xIndex(ordinalOf(ts)));
  const xOfPoint = (i: number) => (mode === "linear" ? xLinear(times[i] as number) : xIndex(i));
  const xEnd = mode === "linear" ? xLinear(tEnd) : xIndex(n);

  let lo = 0;
  let hi = 0;
  for (const p of path) {
    lo = Math.min(lo, p.ahead);
    hi = Math.max(hi, p.ahead);
  }
  if (hi - lo <= 0) hi = lo + (instrument.lot_size > 0 ? instrument.lot_size : 1);
  const pad = (hi - lo) * 0.08;
  const yMin = lo - pad;
  const yMax = hi + pad;
  const yOf = (v: number) => TOP + ((yMax - v) / (yMax - yMin)) * PLOT_HEIGHT;

  const points = path.map((p, i) => ({
    seq: p.seq,
    ts: p.ts,
    ahead: p.ahead,
    x: xOfPoint(i),
    y: yOf(p.ahead),
  }));

  let stepPath = "";
  points.forEach((p, i) => {
    stepPath +=
      i === 0
        ? `M${p.x.toFixed(1)},${p.y.toFixed(1)}`
        : `H${p.x.toFixed(1)}V${p.y.toFixed(1)}`;
  });
  if (n > 0) stepPath += `H${xEnd.toFixed(1)}`;

  const yStep = niceStep((yMax - yMin) / 4);
  const yTicks = ticksBetween(yMin, yMax, yStep).map((v) => ({
    y: yOf(v),
    label: fmtQty(v, instrument),
  }));

  const xTicks: Array<{ x: number; label: string }> = [];
  if (mode === "linear") {
    const rel0 = tStart - originNs;
    const rel1 = tEnd - originNs;
    const step = niceStep((rel1 - rel0) / Math.max(2, Math.floor(plotWidth / 120)));
    const scale = Math.max(Math.abs(rel0), Math.abs(rel1));
    for (const tick of ticksBetween(rel0, rel1, step)) {
      xTicks.push({
        x: xLinear(originNs + tick),
        label: `${tick > 0 ? "+" : ""}${fmtAxisNs(tick, step, scale)}`,
      });
    }
  } else if (n > 0) {
    const stride = Math.max(1, Math.ceil(96 / (plotWidth / n)));
    for (let i = 0; i < n; i += stride) {
      xTicks.push({ x: xIndex(i), label: fmtSince(times[i] as number, originNs) });
    }
  }

  const trades: TradeBar[] = [];
  const fills: FillMark[] = [];
  let maxTradeQty = 0;
  for (const ev of events) {
    const b = ev.body;
    const at = indexOfSeq.get(ev.seq);
    const x = at !== undefined ? xOfPoint(at) : xOfTs(ev.ts);
    if (b.type === "queue_update" && b.cause.type === "trade") {
      trades.push({ seq: ev.seq, ts: ev.ts, qty: b.cause.qty, x });
      maxTradeQty = Math.max(maxTradeQty, b.cause.qty);
    } else if (b.type === "exch_fill") {
      const i = at !== undefined ? at : lastAtOrBefore(times, ev.ts);
      const ahead = i >= 0 ? (path[i] as QueuePoint).ahead : 0;
      fills.push({ seq: ev.seq, ts: ev.ts, qty: b.qty, maker: b.maker, x, y: yOf(ahead) });
    }
  }

  return {
    stepPath,
    points,
    yTicks,
    xTicks,
    zeroY: yOf(0),
    trades,
    maxTradeQty,
    fills,
    xOfExchangeTs: (ts) => (ts < tStart || ts > tEnd ? null : xOfTs(ts)),
  };
}

export interface QueueChartProps {
  path: QueuePoint[];
  events: FlightEvent[];
  /** The decision time; axis labels are offsets from it. */
  originNs: number;
  instrument: InstrumentMeta;
  side: Side;
  mode: AxisMode;
  selectedSeq: number | null;
  citedSeqs: ReadonlySet<number>;
  onSelect: (seq: number | null) => void;
}

export function QueueChart({
  path,
  events,
  originNs,
  instrument,
  side,
  mode,
  selectedSeq,
  citedSeqs,
  onSelect,
}: QueueChartProps) {
  const [attach, size] = useElementSize<HTMLDivElement>();
  const width = Math.max(360, Math.floor(size.width));
  const geo = useMemo(
    () => buildGeometry(path, events, originNs, instrument, mode, width),
    [path, events, originNs, instrument, mode, width],
  );

  if (path.length === 0) {
    return (
      <Empty>
        This order never rested at the simulated exchange, so no queue position was modeled for
        it.
      </Empty>
    );
  }

  const first = path[0] as QueuePoint;
  const last = path[path.length - 1] as QueuePoint;
  const tradeBase = TOP + PLOT_HEIGHT + GAP + TRADE_BAND;
  const axisY = tradeBase;
  const right = width - RIGHT;

  // Only exchange-clock events have a position on this chart's exchange-time axis.
  const selectedEvent =
    selectedSeq === null ? undefined : events.find((e) => e.seq === selectedSeq);
  const selectedPoint = geo.points.find((p) => p.seq === selectedSeq);
  const guideX = selectedPoint
    ? selectedPoint.x
    : selectedEvent && selectedEvent.clock === "exchange"
      ? geo.xOfExchangeTs(selectedEvent.ts)
      : null;

  return (
    <div className="fx-queue" ref={attach}>
      {size.width > 0 ? (
        <svg
          className="fx-queue-svg"
          width={width}
          height={HEIGHT}
          viewBox={`0 0 ${width} ${HEIGHT}`}
          role="img"
          aria-label={`Modeled quantity ahead of the order on the exchange clock: ${fmtQty(first.ahead, instrument)} at entry, ${fmtQty(last.ahead, instrument)} at the last recorded update, over ${path.length} recorded points. ${geo.trades.length} market trades printed at the order's price.`}
          onClick={() => onSelect(null)}
        >
          {geo.yTicks.map((t) => (
            <g key={`y${t.y}`}>
              <line className="fx-q-grid" x1={LEFT} x2={right} y1={t.y} y2={t.y} />
              <text className="fx-q-label" x={LEFT - 8} y={t.y} textAnchor="end" dominantBaseline="middle">
                {t.label}
              </text>
            </g>
          ))}
          {geo.xTicks.map((t) => (
            <g key={`x${t.x}`}>
              <line className="fx-q-grid" x1={t.x} x2={t.x} y1={TOP} y2={axisY + 4} />
              <text className="fx-q-label" x={t.x} y={axisY + 16} textAnchor="middle">
                {t.label}
              </text>
            </g>
          ))}

          <line className="fx-q-zero" x1={LEFT} x2={right} y1={geo.zeroY} y2={geo.zeroY} />
          <text className="fx-q-zero-label" x={right} y={geo.zeroY - 5} textAnchor="end">
            0: no modeled quantity ahead
          </text>

          <text
            className="fx-q-label"
            x={LEFT - 8}
            y={tradeBase - TRADE_BAND + 4}
            textAnchor="end"
            dominantBaseline="hanging"
          >
            {geo.maxTradeQty > 0 ? fmtQty(geo.maxTradeQty, instrument) : ""}
          </text>
          <text className="fx-q-label" x={LEFT - 8} y={tradeBase - 4} textAnchor="end">
            trades
          </text>
          <line className="fx-q-axis" x1={LEFT} x2={right} y1={tradeBase} y2={tradeBase} />
          {geo.trades.map((t) => {
            const h = geo.maxTradeQty > 0 ? Math.max(2, (t.qty / geo.maxTradeQty) * (TRADE_BAND - 6)) : 2;
            const marked = t.seq === selectedSeq || citedSeqs.has(t.seq);
            return (
              <rect
                key={t.seq}
                className={marked ? "fx-q-trade is-marked" : "fx-q-trade"}
                x={t.x - 1.5}
                y={tradeBase - h}
                width={3}
                height={h}
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect(t.seq);
                }}
              >
                <title>
                  {`#${t.seq} market trade at the order's price (observed): ${fmtQty(t.qty, instrument)} at ${fmtSince(t.ts, originNs)} from decision`}
                </title>
              </rect>
            );
          })}

          <path className="fx-q-line" d={geo.stepPath} />

          {guideX !== null ? (
            <line className="fx-q-guide" x1={guideX} x2={guideX} y1={TOP} y2={tradeBase} />
          ) : null}

          {geo.points.map((p) => {
            const selected = p.seq === selectedSeq;
            const cited = citedSeqs.has(p.seq);
            return (
              <circle
                key={p.seq}
                className={`fx-q-point${selected ? " is-selected" : ""}${cited ? " is-cited" : ""}`}
                cx={p.x}
                cy={p.y}
                r={selected || cited ? 5 : 2.2}
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect(p.seq);
                }}
              >
                <title>
                  {`#${p.seq} modeled quantity ahead ${fmtQty(p.ahead, instrument)} at ${fmtSince(p.ts, originNs)} from decision`}
                </title>
              </circle>
            );
          })}

          {geo.fills.map((f) => {
            const selected = f.seq === selectedSeq;
            const cited = citedSeqs.has(f.seq);
            return (
              <g
                key={f.seq}
                className={`fx-q-fill fx-tone-${side}${selected ? " is-selected" : ""}${cited ? " is-cited" : ""}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onSelect(f.seq);
                }}
              >
                <circle className="fx-q-fill-ring" cx={f.x} cy={f.y} r={9} />
                <circle className="fx-q-fill-dot" cx={f.x} cy={f.y} r={5} />
                <title>
                  {`#${f.seq} simulated ${f.maker ? "maker" : "taker"} fill of ${fmtQty(f.qty, instrument)} at ${fmtSince(f.ts, originNs)} from decision`}
                </title>
              </g>
            );
          })}
        </svg>
      ) : null}
      <ul className="fx-queue-legend">
        <li>
          <span className="fx-swatch fx-swatch-line" aria-hidden="true" />
          quantity ahead of the order <Badge kind="modeled">modeled</Badge>
        </li>
        <li>
          <span className="fx-swatch fx-swatch-bar" aria-hidden="true" />
          market trades printed at the order's price <Badge kind="observed">observed</Badge>
        </li>
        <li>
          <span className={`fx-swatch fx-swatch-fill fx-tone-${side}`} aria-hidden="true" />
          simulated fill <Badge kind="modeled">modeled</Badge>
        </li>
        <li className="dim">
          {mode === "linear"
            ? "Exchange clock, linear; labels are offsets from the decision."
            : "Exchange clock, one step per recorded point; labels are offsets from the decision."}
        </li>
      </ul>
    </div>
  );
}
