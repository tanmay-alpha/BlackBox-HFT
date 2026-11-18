// Step-line chart over event index for the workbench's metric series. The x axis is the source
// event index (the series is downsampled by event index), so values hold until the next sample
// and are drawn as steps. A guide marks the selected event; a click selects the sampled event at
// or before the pointer (never a later one). SVG in real pixels, as the forensics charts do.

import { useMemo } from 'react';

import { groupThousands } from '../../components/ns';
import { niceStep, ticksBetween } from '../forensics/axis';
import { useElementSize } from '../forensics/useElementSize';
import { axisLeftMargin, clampAxisLabel, sampleAtOrBefore } from './model';

export interface ChartLine {
  id: string;
  label: string;
  /** CSS colour, normally a token (`var(--bid)`). */
  color: string;
  /** One value per x; `null` breaks the line. */
  values: readonly (number | null)[];
  dashed?: boolean;
}

export interface MetricChartProps {
  /** Event index of each sample, ascending. */
  xs: readonly number[];
  lines: ChartLine[];
  selected: number | null;
  onSelect: (eventIndex: number) => void;
  fmt: (v: number) => string;
  /** Text alternative stating what is drawn and its units. */
  ariaLabel: string;
  /** Fixed value range (for bounded measures such as imbalance). */
  domain?: [number, number];
  /** Draw a rule at zero. */
  zero?: boolean;
}

const HEIGHT = 196;
const RIGHT = 12;
const TOP = 10;
const AXIS = 24;

export function MetricChart({ xs, lines, selected, onSelect, fmt, ariaLabel, domain, zero }: MetricChartProps) {
  const [attach, size] = useElementSize<HTMLDivElement>();
  const width = Math.max(200, Math.floor(size.width));

  const geo = useMemo(() => {
    const x0 = xs.length > 0 ? xs[0] : 0;
    const x1 = xs.length > 0 ? xs[xs.length - 1] : 1;
    let lo = Infinity;
    let hi = -Infinity;
    for (const l of lines) {
      for (const v of l.values) {
        if (v === null || !Number.isFinite(v)) continue;
        if (v < lo) lo = v;
        if (v > hi) hi = v;
      }
    }
    if (domain) [lo, hi] = domain;
    if (zero && Number.isFinite(lo)) {
      lo = Math.min(lo, 0);
      hi = Math.max(hi, 0);
    }
    if (!Number.isFinite(lo)) {
      lo = 0;
      hi = 1;
    }
    if (hi === lo) {
      hi += 1;
      lo -= lo === 0 ? 0 : 1;
    }
    const yStep = niceStep((hi - lo) / 4);
    const yValues = ticksBetween(lo, hi, yStep);
    const yLabels = yValues.map((v) => fmt(v));
    // The plot starts right of the widest value label, so no label is cut at the left edge.
    const left = axisLeftMargin(yLabels);
    const plotW = width - left - RIGHT;
    const plotH = HEIGHT - TOP - AXIS;
    const spanX = x1 > x0 ? x1 - x0 : 1;
    const sx = (x: number) => left + ((x - x0) / spanX) * plotW;
    const sy = (v: number) => TOP + (1 - (v - lo) / (hi - lo)) * plotH;
    const paths = lines.map((l) => {
      let d = '';
      let prevY: number | null = null;
      for (let i = 0; i < xs.length; i += 1) {
        const v = l.values[i];
        if (v === null || v === undefined || !Number.isFinite(v)) {
          prevY = null;
          continue;
        }
        const x = sx(xs[i]);
        const y = sy(v);
        d += prevY === null ? `M${x.toFixed(1)},${y.toFixed(1)}` : `H${x.toFixed(1)}V${y.toFixed(1)}`;
        prevY = y;
      }
      return d;
    });
    const yTicks = yValues.map((v, i) => ({ y: sy(v), label: yLabels[i] }));
    const xStep = niceStep(spanX / Math.max(2, Math.floor(plotW / 96)));
    const xTicks = ticksBetween(x0, x1, xStep).map((x) => {
      const label = `#${groupThousands(String(Math.round(x)))}`;
      return { key: x, x: clampAxisLabel(sx(x), label, width), label };
    });
    return { x0, x1, sx, sy, paths, yTicks, xTicks, plotW, left, zeroY: lo <= 0 && hi >= 0 ? sy(0) : null };
  }, [xs, lines, width, domain, zero, fmt]);

  const guide = selected !== null && selected >= geo.x0 && selected <= geo.x1 ? geo.sx(selected) : null;

  const pick = (clientX: number, rect: DOMRect) => {
    if (xs.length === 0) return;
    const rel = (clientX - rect.left - geo.left) / geo.plotW;
    const target = geo.x0 + Math.max(0, Math.min(1, rel)) * (geo.x1 - geo.x0);
    const i = sampleAtOrBefore(xs.map((event_index) => ({ event_index })), target);
    onSelect(xs[Math.max(0, i)]);
  };

  return (
    <div className="mk-chart" ref={attach}>
      {size.width > 0 ? (
        <svg
          width={width}
          height={HEIGHT}
          viewBox={`0 0 ${width} ${HEIGHT}`}
          role="img"
          aria-label={ariaLabel}
          onClick={(e) => pick(e.clientX, e.currentTarget.getBoundingClientRect())}
        >
          {geo.yTicks.map((t) => (
            <g key={`y${t.y}`}>
              <line className="mk-grid" x1={geo.left} x2={width - RIGHT} y1={t.y} y2={t.y} />
              <text className="mk-axis-label" x={geo.left - 6} y={t.y} textAnchor="end" dominantBaseline="middle">
                {t.label}
              </text>
            </g>
          ))}
          {geo.xTicks.map((t) => (
            <text key={`x${t.key}`} className="mk-axis-label" x={t.x} y={HEIGHT - 6} textAnchor="middle">
              {t.label}
            </text>
          ))}
          {geo.zeroY !== null && zero ? <line className="mk-zero" x1={geo.left} x2={width - RIGHT} y1={geo.zeroY} y2={geo.zeroY} /> : null}
          {lines.map((l, i) => (
            <path
              key={l.id}
              d={geo.paths[i]}
              fill="none"
              stroke={l.color}
              strokeWidth={1.5}
              strokeDasharray={l.dashed ? '4 3' : undefined}
            />
          ))}
          {guide !== null ? <line className="mk-guide" x1={guide} x2={guide} y1={TOP} y2={HEIGHT - AXIS} /> : null}
        </svg>
      ) : null}
      <ul className="mk-chart-key" aria-label="Series">
        {lines.map((l) => (
          <li key={l.id}>
            <span
              className="mk-key-line"
              style={{ borderTopColor: l.color, borderTopStyle: l.dashed ? 'dashed' : 'solid' }}
              aria-hidden="true"
            />
            {l.label}
          </li>
        ))}
      </ul>
    </div>
  );
}
