// Equity of both scenarios over the whole run. Server-downsampled buckets are drawn as what the
// server returned: the bucket's first-to-last line plus its min..max range as a light band.
// Clicking the plot (or arrow keys while it is focused) moves the shared replay time. The curves load only once the panel is on
// screen, so a visit that never scrolls to it does not spend the demo's data allowance on it.

import { useEffect, useMemo, useState, type KeyboardEvent } from 'react';

import { isBucketed, type EquityPage } from '../../api/model';
import { fmtDuration, fmtInt, fmtSigned } from '../../components/fmt';
import { Spinner, useAsync } from '../../components/ui';
import { decimalsForStep, fmtAxisNs, niceStep, ticksBetween } from '../forensics/axis';
import { useElementSize } from '../forensics/useElementSize';
import { fmtSec, loadEquity, type Focus, type ScenarioKey } from './data';
import { PlainError } from './PlainError';
import type { LatencyStory } from './story';

const SEC_NS = 1_000_000_000;

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

const H = 240;
const PAD = { l: 72, r: 16, t: 12, b: 28 };

export interface EquityPanelProps {
  story: LatencyStory;
  names: Record<ScenarioKey, string>;
  focus: Focus;
  /** Shared replay time. */
  t: number;
  divergenceTs: number | null;
  onPick: (t: number) => void;
}

/** Loads the curves once the panel scrolls into view, then draws them. */
export function EquityPanel(props: EquityPanelProps) {
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  const [seen, setSeen] = useState(false);
  useEffect(() => {
    if (!host || seen) return;
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) setSeen(true);
    });
    io.observe(host);
    return () => io.disconnect();
  }, [host, seen]);
  const { story } = props;
  const equity = useAsync((signal) => (seen ? loadEquity(story, signal) : Promise.resolve(null)), [story, seen]);
  return (
    <div ref={setHost} className="ex-equity">
      {equity.error ? (
        <PlainError error={equity.error} onRetry={equity.reload} what="Equity curves" />
      ) : equity.data ? (
        <EquityChart pages={equity.data} {...props} />
      ) : (
        <div className="ex-equity-wait">
          <Spinner label="Loading the equity curves" />
        </div>
      )}
    </div>
  );
}

function EquityChart({ pages, names, focus, t, divergenceTs, onPick }: EquityPanelProps & { pages: Record<ScenarioKey, EquityPage> }) {
  const [attach, size] = useElementSize<HTMLDivElement>();
  const series = useMemo(() => ({ fast: toSeries(pages.fast), slow: toSeries(pages.slow) }), [pages]);
  const width = Math.max(280, Math.floor(size.width));
  const height = H;
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
    const xStep = niceStep((t1 - t0) / Math.max(2, Math.floor(plotW / 120)));
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
  const page = downsampled.length > 0 ? pages[downsampled[0]] : null;
  const label = `Equity over the whole run for both scenarios. ${names.fast} ends at ${fmtSigned(series.fast.last, 2)}, ${names.slow} ends at ${fmtSigned(series.slow.last, 2)} (quote currency). Click a point in time, or use the arrow keys (1 second steps, Shift for 10 seconds, Home and End for the ends), to move the replay there.`;
  const onKey = (e: KeyboardEvent<SVGSVGElement>) => {
    if (!geom) return;
    const step = e.shiftKey ? 10 * SEC_NS : SEC_NS;
    const cur = Math.min(geom.t1, Math.max(geom.t0, t));
    const next =
      e.key === 'ArrowRight' || e.key === 'ArrowUp' ? cur + step
      : e.key === 'ArrowLeft' || e.key === 'ArrowDown' ? cur - step
      : e.key === 'Home' ? geom.t0
      : e.key === 'End' ? geom.t1
      : null;
    if (next === null) return;
    e.preventDefault();
    onPick(Math.round(Math.min(geom.t1, Math.max(geom.t0, next))));
  };

  return (
    <>
      <div className="ex-legend-row" aria-hidden="true">
        {(['fast', 'slow'] as const).map((k) => (
          <span key={k} className={`ex-key ex-key-series-${k}`} data-dim={focus !== 'both' && focus !== k ? '' : undefined}>
            <span className="ex-key-line" /> {names[k]} <span className="ex-num">{fmtSigned(series[k].last, 2)}</span>
          </span>
        ))}
        {divergenceTs !== null ? (
          <span className="ex-key ex-key-div">
            <span className="ex-key-line" /> first divergence
          </span>
        ) : null}
        <span className="ex-key ex-key-now">
          <span className="ex-key-line" /> replay time
        </span>
      </div>
      <div ref={attach} className="ex-equity-stage">
        {geom && size.width > 0 ? (
          <svg
            width={width}
            height={height}
            role="slider"
            tabIndex={0}
            aria-label={label}
            aria-valuemin={geom.t0}
            aria-valuemax={geom.t1}
            aria-valuenow={Math.min(geom.t1, Math.max(geom.t0, t))}
            aria-valuetext={`Replay time ${fmtSec(t, 3)}`}
            onKeyDown={onKey}
            className="ex-svg ex-clickable"
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              const frac = (e.clientX - r.left - PAD.l) / plotW;
              if (frac >= 0 && frac <= 1) onPick(Math.round(geom.t0 + frac * (geom.t1 - geom.t0)));
            }}
          >
            {geom.yTicks.map((tk) => (
              <g key={tk.label}>
                <line x1={PAD.l} x2={width - PAD.r} y1={tk.y} y2={tk.y} className="ex-grid" />
                <text x={PAD.l - 6} y={tk.y + 5} textAnchor="end" className="ex-axis">
                  {tk.label}
                </text>
              </g>
            ))}
            {geom.xTicks.map((tk) => (
              <text key={tk.label} x={tk.x} y={height - 8} textAnchor={tk.x + 30 > width ? 'end' : 'middle'} className="ex-axis">
                {tk.label}
              </text>
            ))}
            <line x1={PAD.l} x2={width - PAD.r} y1={geom.zero} y2={geom.zero} className="ex-zero" />
            {(['slow', 'fast'] as const).map((k) => (
              <g key={k} className={`ex-series ex-series-${k}`} data-dim={focus !== 'both' && focus !== k ? '' : undefined}>
                <path d={bandPath(series[k])} className="ex-band" />
                <path d={path(series[k])} className="ex-line" />
              </g>
            ))}
            {divergenceTs !== null ? <line x1={geom.x(divergenceTs)} x2={geom.x(divergenceTs)} y1={PAD.t} y2={PAD.t + plotH} className="ex-div-line" /> : null}
            {t >= geom.t0 && t <= geom.t1 ? <line x1={geom.x(t)} x2={geom.x(t)} y1={PAD.t} y2={PAD.t + plotH} className="ex-now-line" /> : null}
          </svg>
        ) : null}
      </div>
      <p className="ex-muted">
        {page
          ? `Downsampled by the server to ${fmtInt(page.items.length)} buckets${page.downsampled && page.bucket_ns !== null ? ` of ${fmtDuration(page.bucket_ns)}` : ''}; the light band is each bucket’s low-to-high range.`
          : 'Every recorded sample is drawn.'}{' '}
        Click the chart to move the replay time there; the slider above does the same from the keyboard.
      </p>
    </>
  );
}
