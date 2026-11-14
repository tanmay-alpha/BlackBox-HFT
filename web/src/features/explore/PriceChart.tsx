// The central chart: the observed best bid and ask of a ≤ 1 s replay window as step lines, with
// each scenario's simulated exchange arrivals and fills as markers (scenario colour + shape).
//
// Lightweight Charts spaces data points evenly and has a one-second time axis. To keep the
// horizontal axis proportional to time, chart time counts 100 µs units after the dataset origin
// and both lines carry a point for every unit of the window (the book value held at that time).
// The axis formatters print seconds; exact nanosecond times are in the inspector.

import { useEffect, useMemo, useRef, useState } from 'react';
import { ColorType, CrosshairMode, LineSeries, LineType, createChart, createSeriesMarkers } from 'lightweight-charts';
import type {
  IChartApi,
  ISeriesApi,
  ISeriesMarkersPluginApi,
  LineData,
  MouseEventParams,
  SeriesMarker,
  Time,
  UTCTimestamp,
  WhitespaceData,
} from 'lightweight-charts';

import type { InstrumentMeta } from '../../../api/types';
import { fmtPrice, stepDecimals } from '../../../components/fmt';
import { ErrorBox } from '../../../components/ui';
import type { Moment } from '../story';
import { shows, type View, type Which } from './data';

/** Nanoseconds per chart time unit. */
export const UNIT_NS = 100_000;
const toUnit = (ns: number) => Math.round(ns / UNIT_NS);

/** Click distance (CSS px) within which a click selects a marker. */
const HIT_PX = 10;

function unitLabel(time: Time, digits: number): string {
  if (typeof time !== 'number') return String(time);
  return `${((time * UNIT_NS) / 1e9).toFixed(digits)} s`;
}

/** A resolved custom property on `el` (page-scoped tokens live on the page root). */
function token(el: Element, name: string): string {
  const v = getComputedStyle(el).getPropertyValue(name).trim();
  if (!v) throw new Error(`token ${name} is not defined`);
  return v;
}

interface Built {
  bid: (LineData<Time> | WhitespaceData<Time>)[];
  ask: (LineData<Time> | WhitespaceData<Time>)[];
  markers: SeriesMarker<Time>[];
  arrivals: Record<Which, number>;
  fills: Record<Which, number>;
}

function build(m: Moment, view: View, inst: InstrumentMeta, selected: number | null, colors: Record<Which, string>): Built {
  const { t0, t1 } = m.window;
  const markers: SeriesMarker<Time>[] = [];
  const arrivals: Record<Which, number> = { fast: 0, slow: 0 };
  const fills: Record<Which, number> = { fast: 0, slow: 0 };
  const price = (tick: number) => tick * inst.tick_size;
  // With an order selected, every other marker is drawn at 35 % opacity (tokens are #rrggbb).
  const tone = (which: Which, orderId: number) =>
    selected === null || orderId === selected || !/^#[0-9a-f]{6}$/i.test(colors[which]) ? colors[which] : `${colors[which]}59`;
  for (const which of ['fast', 'slow'] as const) {
    if (!shows(view, which)) continue;
    const tl = m[which];
    for (const o of tl.orders) {
      const at = o.exch_arrival_ts;
      if (at === null || at < t0 || at > t1) continue;
      arrivals[which] += 1;
      const sel = o.order_id === selected;
      markers.push({
        id: `o:${o.order_id}:${which}:arr`,
        time: toUnit(at) as UTCTimestamp,
        position: 'atPriceMiddle',
        price: price(o.price_tick),
        shape: which === 'fast' ? 'circle' : 'square',
        color: tone(which, o.order_id),
        size: sel ? 2 : 0.8,
      });
    }
    for (const f of tl.fills) {
      if (f.ts < t0 || f.ts > t1) continue;
      fills[which] += 1;
      const sel = f.order_id === selected;
      markers.push({
        id: `o:${f.order_id}:${which}:fill`,
        time: toUnit(f.ts) as UTCTimestamp,
        position: f.side === 'buy' ? 'atPriceBottom' : 'atPriceTop',
        price: price(f.price_tick),
        shape: f.side === 'buy' ? 'arrowUp' : 'arrowDown',
        color: tone(which, f.order_id),
        size: sel ? 2 : 1,
      });
    }
  }
  markers.sort((a, b) => (a.time as number) - (b.time as number));

  // One point per unit across the window, holding the book value since the last change.
  const bbo = m.market.bbo;
  const bid: Built['bid'] = [];
  const ask: Built['ask'] = [];
  let j = -1;
  for (let u = toUnit(t0); u <= toUnit(t1); u += 1) {
    while (j + 1 < bbo.ts.length && toUnit(bbo.ts[j + 1]) <= u) j += 1;
    const time = u as UTCTimestamp;
    const b = j < 0 ? null : bbo.bid_tick[j];
    const a = j < 0 ? null : bbo.ask_tick[j];
    bid.push(b === null ? { time } : { time, value: price(b) });
    ask.push(a === null ? { time } : { time, value: price(a) });
  }
  return { bid, ask, markers, arrivals, fills };
}

export interface PriceChartProps {
  moment: Moment;
  view: View;
  inst: InstrumentMeta;
  selected: number | null;
  labels: Record<Which, string>;
  onSelectOrder: (orderId: number) => void;
  /** Time under the pointer (ns), `null` when the pointer leaves. */
  onHover: (t: number | null) => void;
  /** A click on the chart away from a marker pins the book cursor there. */
  onPin: (t: number) => void;
}

export function PriceChart({ moment, view, inst, selected, labels, onSelectOrder, onHover, onPin }: PriceChartProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<{ chart: IChartApi; bid: ISeriesApi<'Line'>; ask: ISeriesApi<'Line'>; markers: ISeriesMarkersPluginApi<Time> } | null>(null);
  const [failure, setFailure] = useState<Error | null>(null);
  const [colors, setColors] = useState<Record<Which, string> | null>(null);
  const handlers = useRef({ onSelectOrder, onHover, onPin });
  handlers.current = { onSelectOrder, onHover, onPin };
  const markersRef = useRef<SeriesMarker<Time>[]>([]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    try {
      const border = token(host, '--border-subtle');
      const decimals = stepDecimals(inst.tick_size);
      const chart = createChart(host, {
        autoSize: true,
        layout: {
          background: { type: ColorType.Solid, color: token(host, '--bg-1') },
          textColor: token(host, '--fg-1'),
          fontFamily: token(host, '--font-mono'),
          fontSize: 12,
          attributionLogo: true,
        },
        grid: { vertLines: { color: border }, horzLines: { color: border } },
        crosshair: { mode: CrosshairMode.Normal },
        rightPriceScale: { borderColor: border },
        timeScale: {
          borderColor: border,
          timeVisible: true,
          secondsVisible: true,
          minBarSpacing: 0.01,
          rightOffset: 0,
          tickMarkFormatter: (time: Time) => unitLabel(time, 3),
        },
        handleScroll: { vertTouchDrag: false },
        localization: {
          timeFormatter: (time: Time) => unitLabel(time, 5),
          priceFormatter: (v: number) => v.toFixed(decimals),
        },
      });
      const priceFormat = { type: 'price' as const, precision: decimals, minMove: inst.tick_size };
      const bid = chart.addSeries(LineSeries, {
        color: token(host, '--bid'),
        lineWidth: 2,
        lineType: LineType.WithSteps,
        title: 'Best bid',
        priceLineVisible: false,
        priceFormat,
      });
      const ask = chart.addSeries(LineSeries, {
        color: token(host, '--ask'),
        lineWidth: 2,
        lineType: LineType.WithSteps,
        title: 'Best ask',
        priceLineVisible: false,
        priceFormat,
      });
      const markers = createSeriesMarkers(bid, [], { autoScale: true });
      const move = (p: MouseEventParams<Time>) => handlers.current.onHover(typeof p.time === 'number' ? p.time * UNIT_NS : null);
      // Markers are small: a click selects the nearest one within HIT_PX, else pins the cursor.
      const click = (p: MouseEventParams<Time>) => {
        let best: { id: string; d: number } | null = null;
        if (p.point) {
          for (const mk of markersRef.current) {
            const x = chart.timeScale().timeToCoordinate(mk.time);
            const y = mk.price === undefined ? null : bid.priceToCoordinate(mk.price);
            if (x === null || y === null || mk.id === undefined) continue;
            const d = Math.hypot(x - p.point.x, y - p.point.y);
            if (d <= HIT_PX && (!best || d < best.d)) best = { id: mk.id, d };
          }
        }
        if (best) {
          handlers.current.onSelectOrder(Number(best.id.split(':')[1]));
          return;
        }
        if (typeof p.time === 'number') handlers.current.onPin(p.time * UNIT_NS);
      };
      chart.subscribeCrosshairMove(move);
      chart.subscribeClick(click);
      // The window always fits the width: refit when the layout resizes the chart.
      const refit = () => chart.timeScale().fitContent();
      chart.timeScale().subscribeSizeChange(refit);
      chartRef.current = { chart, bid, ask, markers };
      setColors({ fast: token(host, '--pa-fast'), slow: token(host, '--pa-slow') });
      setFailure(null);
      return () => {
        chart.unsubscribeCrosshairMove(move);
        chart.unsubscribeClick(click);
        chart.timeScale().unsubscribeSizeChange(refit);
        chart.remove();
        chartRef.current = null;
      };
    } catch (e) {
      setFailure(e instanceof Error ? e : new Error(String(e)));
      return undefined;
    }
  }, [inst]);

  const built = useMemo(() => (colors ? build(moment, view, inst, selected, colors) : null), [moment, view, inst, selected, colors]);

  useEffect(() => {
    const c = chartRef.current;
    if (!c || !built) return;
    c.bid.setData(built.bid);
    c.ask.setData(built.ask);
    c.markers.setMarkers(built.markers);
    markersRef.current = built.markers;
  }, [built]);

  // Re-fit only when a new window arrives, so a zoom survives scenario and selection changes.
  useEffect(() => {
    chartRef.current?.chart.timeScale().fitContent();
  }, [moment, colors]);

  const desc = built
    ? `Price chart from ${(moment.window.t0 / 1e9).toFixed(3)} s to ${(moment.window.t1 / 1e9).toFixed(3)} s. Observed best bid and ask from ${fmtPrice(
        moment.market.bbo.bid_tick[0] ?? null,
        inst,
      )} / ${fmtPrice(moment.market.bbo.ask_tick[0] ?? null, inst)}. ${(['fast', 'slow'] as const)
        .filter((w) => shows(view, w))
        .map((w) => `${labels[w]} scenario: ${built.arrivals[w]} simulated order arrivals and ${built.fills[w]} simulated fills`)
        .join('; ')}. The order list below the chart has the same orders as buttons.`
    : 'Price chart loading';

  return (
    <>
      {failure ? <ErrorBox error={failure} /> : null}
      <div className="pa-chart-host" ref={hostRef} role="img" aria-label={desc} />
    </>
  );
}
