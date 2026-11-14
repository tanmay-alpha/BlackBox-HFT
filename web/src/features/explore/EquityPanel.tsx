// Equity of both scenarios over the whole run. Server-downsampled buckets are drawn as what the
// server returned: the bucket's first-to-last line plus its min..max range as a light band.
// Clicking the plot moves the shared replay time.

import { useMemo } from 'react';

import { isBucketed, type EquityPage } from '../../../api/model';
import { fmtDuration, fmtInt, fmtSigned } from '../../../components/fmt';
import { decimalsForStep, fmtAxisNs, niceStep, ticksBetween } from '../../forensics/axis';
import { useElementSize } from '../../forensics/useElementSize';
import type { Focus, ScenarioKey } from './data';

interface Series {
  /** Polyline points [ts, equity]. */
  line: [number, number][];
  /** Band per bucket [t0, t1, min, max]; empty for raw samples. */
  band: [number, number, number, number][];
  last: number | null;
}

function toSeries(page: EquityPage): Series {
  if (isBucketed(page)) {
    const line: [number, number][] = [];
    const band: [number, number, number, number][] = [];
    for (const b of page.items) {
      line.push([b.t0_ns, b.first], [b.t1_ns, b.last]);
      band.push([b.t0_ns, b.t1_ns, b.min_equity, b.max_equity]);
    }
    return { line, band, last: page.items.at(-1)?.last ?? null };
  }
  return { line: page.items.map((s) => [s.ts, s.equity]), band: [], last: page.items.at(-1)?.equity ?? null };
}

const H = 220;
const PAD = { l: 64, r: 16, t: 12, b: 26 };

export interface EquityPanelProps {
  pages: Record<ScenarioKey, EquityPage>;
  names: Record<ScenarioKey, string>;
  focus: Focus;
  /** Shared replay time. */
  t: number;
  divergenceTs: number | null;
  onPick: (t: number) => void;
}

export function EquityPanel({ pages, names, focus, t, divergenceTs, onPick }: EquityPanelProps) {
  const [attach, size] = useElementSize<HTMLDivElement>();
  const series = useMemo(() => ({ fast: toSeries(pages.fast), slow: toSeries(pages.slow) }), [pages]);
  const width = Math.max(280, Math.floor(size.width));
  const height = Math.max(160, Math.floor(size.height) || H);
  const plotW = width - PAD.l - PAD.r;
  const plotH = height - PAD.t - PAD.b;

  const geom = useMemo(() => {
    const all = [...series.fast.line, ...series.slow.line];
    if (all.length === 0) return null;
    let t0 = Infinity;
    let t1 = -Infinity;
    let lo = 0;
    let hi = 0;
    for (const [ts, v] of all) {
      t0 = Math.min(t0, ts);
      t1 = Math.max(t1, ts);
      lo = Math.min(lo, v);
      hi = Math.max(hi, v);
    }
    for (const [, , mn, mx] of [...series.fast.band, ...series.slow.band]) {
      lo = Math.min(lo, mn);
      hi = Math.max(hi, mx);
    }
    if (t1 <= t0) t1 = t0 + 1;
    if (hi - lo <= 0) hi = lo + 1;
    const pad = (hi - lo) * 0.06;
    lo -= pad;
    hi += pad;
    const x = (ts: number) => PAD.l + ((ts - t0) / (t1 - t0)) * plotW;
    const y = (v: number) => PAD.t + ((hi - v) / (hi - lo)) * plotH;
    const yStep = niceStep((hi - lo) / 4);
    const xStep = niceStep((t1 - t0) / Math.max(2, Math.floor(plotW / 110)));
    return {
      t0,
      t1,
      x,
      y,
      yTicks: ticksBetween(lo, hi, yStep).map((v) => ({ y: y(v), label: v.toFixed(decimalsForStep(yStep)).replace('-', '\u2212') })),
      xTicks: ticksBetween(t0, t1, xStep).map((ts) => ({ x: x(ts), label: fmtAxisNs(ts, xStep, t1) })),
      zero: y(0),
    };
  }, [series, plotW, plotH]);

  const path = (s: Series) => (geom ? s.line.map(([ts, v], i) => `${i ? 'L' : 'M'}${geom.x(ts).toFixed(1)},${geom.y(v).toFixed(1)}`).join('') : '');
  const bandPath = (s: Series) =>
    geom ? s.band.map(([a, b, mn, mx]) => `M${geom.x(a).toFixed(1)},${geom.y(mx).toFixed(1)}H${geom.x(b).toFixed(1)}V${geom.y(mn).toFixed(1)}H${geom.x(a).toFixed(1)}Z`).join('') : '';

  const downsampled = (['fast', 'slow'] as const).filter((k) => pages[k].downsampled);
  const label = `Equity over the whole run for both scenarios. ${names.fast} ends at ${fmtSigned(series.fast.last, 2)}, ${names.slow} ends at ${fmtSigned(series.slow.last, 2)} (quote currency). Click a point in time to move the replay there.`;

  return (
    <div className="pb-equity">
      <div className="pb-legend" aria-hidden="true">
        {(['fast', 'slow'] as const).map((k) => (
          <span key={k} className={`pb-key pb-key-${k}`} data-dim={focus !== 'both' && focus !== k ? '' : undefined}>
            <span className="pb-swatch" /> {names[k]} <span className="num">{fmtSigned(series[k].last, 2)}</span>
          </span>
        ))}
        {divergenceTs !== null ? (
          <span className="pb-key pb-key-div">
            <span className="pb-swatch" /> first divergence
          </span>
        ) : null}
        <span className="pb-key pb-key-cursor">
          <span className="pb-swatch" /> replay time
        </span>
      </div>
      <div ref={attach} className="pb-equity-stage">
        {geom && size.width > 0 ? (
          <svg
            width={width}
            height={height}
            role="img"
            aria-label={label}
            className="pb-svg pb-clickable"
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              const frac = (e.clientX - r.left - PAD.l) / plotW;
              if (frac >= 0 && frac <= 1) onPick(Math.round(geom.t0 + frac * (geom.t1 - geom.t0)));
            }}
          >
            {geom.yTicks.map((tk) => (
              <g key={tk.label}>
                <line x1={PAD.l} x2={width - PAD.r} y1={tk.y} y2={tk.y} className="pb-grid" />
                <text x={PAD.l - 6} y={tk.y + 4} textAnchor="end" className="pb-axis">
                  {tk.label}
                </text>
              </g>
            ))}
            {geom.xTicks.map((tk) => (
              <text key={tk.label} x={tk.x} y={height - 8} textAnchor="middle" className="pb-axis">
                {tk.label}
              </text>
            ))}
            <line x1={PAD.l} x2={width - PAD.r} y1={geom.zero} y2={geom.zero} className="pb-zero" />
            {(['slow', 'fast'] as const).map((k) => (
              <g key={k} className={`pb-series pb-series-${k}`} data-dim={focus !== 'both' && focus !== k ? '' : undefined}>
                <path d={bandPath(series[k])} className="pb-band" />
                <path d={path(series[k])} className="pb-line" />
              </g>
            ))}
            {divergenceTs !== null ? (
              <line x1={geom.x(divergenceTs)} x2={geom.x(divergenceTs)} y1={PAD.t} y2={PAD.t + plotH} className="pb-div-line" />
            ) : null}
            {t >= geom.t0 && t <= geom.t1 ? (
              <line x1={geom.x(t)} x2={geom.x(t)} y1={PAD.t} y2={PAD.t + plotH} className="pb-cursor-line" />
            ) : null}
          </svg>
        ) : null}
      </div>
      {downsampled.length > 0 ? (
        <p className="pb-caption">
          Downsampled by the server to {fmtInt(pages[downsampled[0]].items.length)} buckets
          {(() => {
            const page = pages[downsampled[0]];
            return page.downsampled && page.bucket_ns !== null ? ` of ${fmtDuration(page.bucket_ns)}` : '';
          })()}
          ; the light band is each bucket&rsquo;s low-to-high range. Click the chart to move the replay.
        </p>
      ) : (
        <p className="pb-caption">Every recorded sample is drawn. Click the chart to move the replay.</p>
      )}
    </div>
  );
}
