import { useMemo, useState } from "react";
import type { EquitySample } from "../../api/types";
import { fmtNum, fmtSigned } from "../../components/fmt";
import { Empty } from "../../components/ui";
import {
  decimalsForStep,
  fmtAxisNs,
  lastAtOrBefore,
  niceStep,
  ticksBetween,
} from "../forensics/axis";
import { ClockText } from "../forensics/evidence";
import { useElementSize } from "../forensics/useElementSize";

const HEIGHT = 300;
const LEFT = 78;
const RIGHT = 16;
const TOP = 22;
const AXIS_BAND = 26;
const PLOT_HEIGHT = HEIGHT - TOP - AXIS_BAND;

interface Trace {
  ts: number[];
  equity: number[];
}

function toTrace(samples: EquitySample[]): Trace {
  return { ts: samples.map((s) => s.ts), equity: samples.map((s) => s.equity) };
}

/** First and last sample index needed to draw `[t0, t1]`, including one sample beyond each edge. */
function visibleRange(trace: Trace, t0: number, t1: number): [number, number] {
  const from = Math.max(0, lastAtOrBefore(trace.ts, t0));
  const to = Math.min(trace.ts.length - 1, lastAtOrBefore(trace.ts, t1) + 1);
  return [from, to];
}

/**
 * Polyline through the samples. Samples that land on the same pixel column are reduced to that
 * column's first, lowest, highest and last value, which draws the same pixels as the full series
 * without emitting one path segment per sample.
 */
function tracePath(
  trace: Trace,
  from: number,
  to: number,
  xOf: (ts: number) => number,
  yOf: (v: number) => number,
): string {
  const parts: string[] = [];
  const emit = (x: number, v: number) => {
    parts.push(`${parts.length === 0 ? "M" : "L"}${x.toFixed(1)},${yOf(v).toFixed(1)}`);
  };
  let column = NaN;
  let lo = 0;
  let hi = 0;
  let last = 0;
  const flush = () => {
    if (Number.isNaN(column)) return;
    if (lo !== hi) {
      emit(column, lo);
      emit(column, hi);
    }
    emit(column, last);
  };
  for (let i = from; i <= to; i++) {
    const v = trace.equity[i] as number;
    const x = Math.round(xOf(trace.ts[i] as number));
    if (x !== column) {
      flush();
      column = x;
      lo = v;
      hi = v;
      emit(x, v);
    } else {
      lo = Math.min(lo, v);
      hi = Math.max(hi, v);
    }
    last = v;
  }
  flush();
  return parts.join("");
}

function valueAt(trace: Trace, ts: number): number | null {
  const i = lastAtOrBefore(trace.ts, ts);
  return i < 0 ? null : (trace.equity[i] as number);
}

export interface EquityOverlayProps {
  a: EquitySample[];
  b: EquitySample[];
  /** Local-clock decision time of the first divergence, if there is one. */
  divergenceTs: number | null;
  sameDataset: boolean;
}

export function EquityOverlay({ a, b, divergenceTs, sameDataset }: EquityOverlayProps) {
  const [attach, size] = useElementSize<HTMLDivElement>();
  const [aroundDivergence, setAroundDivergence] = useState(false);
  const [cursorTs, setCursorTs] = useState<number | null>(null);
  const width = Math.max(360, Math.floor(size.width));
  const plotWidth = width - LEFT - RIGHT;

  const traceA = useMemo(() => toTrace(a), [a]);
  const traceB = useMemo(() => toTrace(b), [b]);

  const chart = useMemo(() => {
    const starts = [traceA.ts[0], traceB.ts[0]].filter((t): t is number => t !== undefined);
    const ends = [traceA.ts[traceA.ts.length - 1], traceB.ts[traceB.ts.length - 1]].filter(
      (t): t is number => t !== undefined,
    );
    if (starts.length === 0) return null;
    const fullStart = Math.min(...starts);
    const fullEnd = Math.max(Math.max(...ends), fullStart + 1);

    let t0 = fullStart;
    let t1 = fullEnd;
    if (aroundDivergence && divergenceTs !== null) {
      const half = Math.max((fullEnd - fullStart) * 0.02, 1);
      t0 = Math.max(fullStart, divergenceTs - half);
      t1 = Math.min(fullEnd, divergenceTs + half);
      if (t1 <= t0) {
        t0 = fullStart;
        t1 = fullEnd;
      }
    }

    const rangeA = visibleRange(traceA, t0, t1);
    const rangeB = visibleRange(traceB, t0, t1);
    let lo = Infinity;
    let hi = -Infinity;
    for (const [trace, [from, to]] of [
      [traceA, rangeA],
      [traceB, rangeB],
    ] as const) {
      for (let i = from; i <= to; i++) {
        const v = trace.equity[i] as number;
        lo = Math.min(lo, v);
        hi = Math.max(hi, v);
      }
    }
    if (!Number.isFinite(lo) || !Number.isFinite(hi)) return null;
    if (hi - lo <= 0) {
      const bump = Math.max(Math.abs(hi) * 0.01, 1e-9);
      lo -= bump;
      hi += bump;
    }
    const pad = (hi - lo) * 0.06;
    const yMin = lo - pad;
    const yMax = hi + pad;

    const xOf = (ts: number) => LEFT + ((ts - t0) / (t1 - t0)) * plotWidth;
    const yOf = (v: number) => TOP + ((yMax - v) / (yMax - yMin)) * PLOT_HEIGHT;

    const yStep = niceStep((yMax - yMin) / 5);
    const yDecimals = decimalsForStep(yStep);
    const xStep = niceStep((t1 - t0) / Math.max(2, Math.floor(plotWidth / 130)));
    const xScale = Math.max(Math.abs(t0), Math.abs(t1));

    return {
      t0,
      t1,
      xOf,
      pathA: tracePath(traceA, rangeA[0], rangeA[1], xOf, yOf),
      pathB: tracePath(traceB, rangeB[0], rangeB[1], xOf, yOf),
      zeroY: yMin < 0 && yMax > 0 ? yOf(0) : null,
      yTicks: ticksBetween(yMin, yMax, yStep).map((v) => ({
        y: yOf(v),
        label: v.toFixed(yDecimals),
      })),
      xTicks: ticksBetween(t0, t1, xStep).map((t) => ({
        x: xOf(t),
        label: fmtAxisNs(t, xStep, xScale),
      })),
    };
  }, [traceA, traceB, aroundDivergence, divergenceTs, plotWidth]);

  if (a.length === 0 && b.length === 0) {
    return <Empty>Neither run recorded an equity sample.</Empty>;
  }

  const bottom = TOP + PLOT_HEIGHT;
  const right = width - RIGHT;
  const divergenceX =
    chart && divergenceTs !== null && divergenceTs >= chart.t0 && divergenceTs <= chart.t1
      ? chart.xOf(divergenceTs)
      : null;
  const cursorX =
    chart && cursorTs !== null && cursorTs >= chart.t0 && cursorTs <= chart.t1
      ? chart.xOf(cursorTs)
      : null;
  const atA = cursorTs === null ? null : valueAt(traceA, cursorTs);
  const atB = cursorTs === null ? null : valueAt(traceB, cursorTs);

  return (
    <div className="cmp-equity">
      <div className="cmp-equity-bar">
        <ul className="cmp-equity-key">
          <li>
            <span className="cmp-line cmp-line-a" aria-hidden="true" /> A
          </li>
          <li>
            <span className="cmp-line cmp-line-b" aria-hidden="true" /> B
          </li>
          <li>
            <span className="cmp-line cmp-line-div" aria-hidden="true" /> first divergence
          </li>
        </ul>
        <span className="cmp-equity-read mono" aria-hidden="true">
          {cursorTs === null ? (
            <span className="dim">hover the chart to read both curves</span>
          ) : (
            <>
              <ClockText ns={cursorTs} /> A {fmtNum(atA, 4)} B {fmtNum(atB, 4)} B-A{" "}
              {fmtSigned(atA !== null && atB !== null ? atB - atA : null, 4)}
            </>
          )}
        </span>
        {divergenceTs !== null ? (
          <button
            type="button"
            className="cmp-btn"
            aria-pressed={aroundDivergence}
            onClick={() => setAroundDivergence((v) => !v)}
          >
            Zoom to first divergence
          </button>
        ) : null}
      </div>
      <div ref={attach} className="cmp-equity-stage">
        {size.width > 0 && chart ? (
          <svg
            className="cmp-equity-svg"
            width={width}
            height={HEIGHT}
            viewBox={`0 0 ${width} ${HEIGHT}`}
            role="img"
            aria-label="Sampled equity of scenario A and scenario B over dataset-relative time, with a vertical marker at the first divergence. The metrics table lists the final values."
            onPointerMove={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              const frac = (e.clientX - r.left - LEFT) / plotWidth;
              setCursorTs(
                frac < 0 || frac > 1 ? null : Math.round(chart.t0 + frac * (chart.t1 - chart.t0)),
              );
            }}
            onPointerLeave={() => setCursorTs(null)}
          >
            {chart.yTicks.map((t) => (
              <g key={`y${t.y}`}>
                <line className="cmp-grid" x1={LEFT} x2={right} y1={t.y} y2={t.y} />
                <text className="cmp-tick" x={LEFT - 8} y={t.y} textAnchor="end" dominantBaseline="middle">
                  {t.label}
                </text>
              </g>
            ))}
            {chart.xTicks.map((t) => (
              <g key={`x${t.x}`}>
                <line className="cmp-grid" x1={t.x} x2={t.x} y1={TOP} y2={bottom + 4} />
                <text className="cmp-tick" x={t.x} y={bottom + 16} textAnchor="middle">
                  {t.label}
                </text>
              </g>
            ))}
            <line className="cmp-axis" x1={LEFT} x2={right} y1={bottom} y2={bottom} />
            {chart.zeroY !== null ? (
              <line className="cmp-zero" x1={LEFT} x2={right} y1={chart.zeroY} y2={chart.zeroY} />
            ) : null}
            <clipPath id="cmp-equity-clip">
              <rect x={LEFT} y={TOP - 4} width={plotWidth} height={PLOT_HEIGHT + 8} />
            </clipPath>
            <g clipPath="url(#cmp-equity-clip)">
              <path className="cmp-trace cmp-trace-a" d={chart.pathA} />
              <path className="cmp-trace cmp-trace-b" d={chart.pathB} />
            </g>
            {divergenceX !== null ? (
              <g>
                <line className="cmp-div" x1={divergenceX} x2={divergenceX} y1={TOP - 6} y2={bottom} />
                <text
                  className="cmp-div-label"
                  x={divergenceX > width - 150 ? divergenceX - 6 : divergenceX + 6}
                  y={TOP - 10}
                  textAnchor={divergenceX > width - 150 ? "end" : "start"}
                >
                  first divergence
                </text>
              </g>
            ) : null}
            {cursorX !== null ? (
              <line className="cmp-cursor" x1={cursorX} x2={cursorX} y1={TOP} y2={bottom} />
            ) : null}
          </svg>
        ) : null}
      </div>
      <p className="dim cmp-note">
        Equity is realized P&amp;L plus unrealized P&amp;L minus fees, in quote currency, as
        sampled on each run's local clock; lines connect consecutive samples. The horizontal axis
        is time relative to the dataset origin.
        {sameDataset
          ? ""
          : " The two runs use different datasets, so each curve's time is relative to its own dataset's origin."}
      </p>
    </div>
  );
}
