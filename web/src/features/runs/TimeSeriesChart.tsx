// Line chart over run time, drawn with TradingView Lightweight Charts (v5 API).
//
// The library's time axis has one-second resolution, so callers resample with `lastPerSecond`
// and say so beneath the chart. The library's attribution logo stays enabled, as its licence
// asks; the app footer carries the text credit.

import { useEffect, useRef, useState } from 'react';
import { ColorType, LineSeries, LineStyle, LineType, createChart } from 'lightweight-charts';
import type { Time, UTCTimestamp } from 'lightweight-charts';

import type { EquitySample } from '../../api/types';
import { cssVar } from '../../components/theme';
import { ErrorBox } from '../../components/ui';

export interface ChartPoint {
  /** Whole seconds after the dataset origin. */
  time: number;
  value: number;
}

export interface ChartSeries {
  id: string;
  label: string;
  color: string;
  points: ChartPoint[];
  /** Draw as a step line (a value that holds until it changes, such as a position). */
  stepped?: boolean;
  dashed?: boolean;
}

const NS_PER_S = 1_000_000_000;

/**
 * Resamples account snapshots to at most one point per second: the last sample recorded in each
 * second. No averaging and no interpolation; seconds without a sample have no point.
 */
export function lastPerSecond(
  samples: EquitySample[],
  value: (s: EquitySample) => number,
): ChartPoint[] {
  const out: ChartPoint[] = [];
  for (const s of samples) {
    const v = value(s);
    if (!Number.isFinite(v)) continue;
    const sec = Math.floor(s.ts / NS_PER_S);
    const last = out.length > 0 ? out[out.length - 1] : null;
    if (last !== null && sec === last.time) {
      last.value = v;
    } else if (last === null || sec > last.time) {
      out.push({ time: sec, value: v });
    }
  }
  return out;
}

/** `mm:ss` after the dataset origin (minutes are not wrapped at 60). */
function relativeLabel(time: Time): string {
  if (typeof time !== 'number') return String(time);
  const neg = time < 0;
  const s = Math.abs(time);
  const mm = String(Math.floor(s / 60)).padStart(2, '0');
  const ss = String(s % 60).padStart(2, '0');
  return `${neg ? '-' : ''}${mm}:${ss}`;
}

export interface TimeSeriesChartProps {
  series: ChartSeries[];
  formatValue: (v: number) => string;
  /** Text alternative describing what the chart shows. */
  ariaLabel: string;
}

export function TimeSeriesChart({ series, formatValue, ariaLabel }: TimeSeriesChartProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const [failure, setFailure] = useState<Error | null>(null);
  const formatRef = useRef(formatValue);
  formatRef.current = formatValue;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const border = cssVar('--border', '#2c3238');
    try {
      const chart = createChart(host, {
        autoSize: true,
        layout: {
          background: { type: ColorType.Solid, color: cssVar('--surface', '#15181b') },
          textColor: cssVar('--muted', '#929ba5'),
          fontFamily: cssVar('--mono', 'monospace'),
          fontSize: 11,
          attributionLogo: true,
        },
        grid: {
          vertLines: { color: border },
          horzLines: { color: border },
        },
        rightPriceScale: { borderColor: border },
        timeScale: {
          borderColor: border,
          timeVisible: true,
          secondsVisible: true,
          tickMarkFormatter: (time: Time) => relativeLabel(time),
        },
        localization: {
          timeFormatter: (time: Time) => `${relativeLabel(time)} after origin`,
          priceFormatter: (v: number) => formatRef.current(v),
        },
      });
      for (const s of series) {
        const line = chart.addSeries(LineSeries, {
          color: s.color,
          lineWidth: 2,
          lineType: s.stepped ? LineType.WithSteps : LineType.Simple,
          lineStyle: s.dashed ? LineStyle.Dashed : LineStyle.Solid,
          title: s.label,
          priceLineVisible: false,
          lastValueVisible: true,
        });
        line.setData(s.points.map((p) => ({ time: p.time as UTCTimestamp, value: p.value })));
      }
      chart.timeScale().fitContent();
      setFailure(null);
      return () => chart.remove();
    } catch (e) {
      setFailure(e instanceof Error ? e : new Error(String(e)));
      return undefined;
    }
  }, [series]);

  return (
    <div className="col">
      {failure ? <ErrorBox error={failure} /> : null}
      <div className="chart-host" ref={hostRef} role="img" aria-label={ariaLabel} />
      <ul className="chart-key" aria-label="Series">
        {series.map((s) => (
          <li key={s.id}>
            <span
              className={s.dashed ? 'chart-key-line chart-key-line-dashed' : 'chart-key-line'}
              style={{ borderTopColor: s.color }}
              aria-hidden="true"
            />
            {s.label}
          </li>
        ))}
      </ul>
    </div>
  );
}
