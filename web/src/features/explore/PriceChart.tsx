// The replay chart: the synthetic dataset's best bid and ask of a ≤ 1 s window as step lines, with each
// scenario's simulated exchange arrivals and fills as markers (scenario colour + shape), and a
// labelled vertical line at the shared replay time. Hovering shows a tooltip: the marker's
// scenario, order, side, quantity, price and exchange-clock time, or the observed bid/ask there.
//
// Lightweight Charts spaces data points evenly and has a one-second time axis. To keep the
// horizontal axis proportional to time, chart time counts 100 µs units after the dataset origin
// and both lines carry a point for every unit of the window (the book value held at that time).
// The axis formatters print seconds; exact times are in the order lifecycle.

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

import type { InstrumentMeta } from '../../api/types';
import { fmtPrice, fmtQty, stepDecimals } from '../../components/fmt';
import { usePhone } from '../../components/PaneTabs';
import { PlainError } from './PlainError';
import type { Moment } from './story';
import { shows, type Focus, type ScenarioKey } from './data';

/** Nanoseconds per chart time unit. */
const UNIT_NS = 100_000;
const toUnit = (ns: number) => Math.round(ns / UNIT_NS);

/** Pointer distance (CSS px) within which a click selects, and hover describes, a marker. */
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

/** What a marker stands for, for its tooltip. */
interface MarkerInfo {
  k: ScenarioKey;
  orderId: number;
  kind: 'arrival' | 'fill';
  side: 'buy' | 'sell';
  qty: number;
  priceTick: number;
  ts: number;
}

interface Built {
  bid: (LineData<Time> | WhitespaceData<Time>)[];
  ask: (LineData<Time> | WhitespaceData<Time>)[];
  markers: SeriesMarker<Time>[];
  info: Map<string, MarkerInfo>;
  arrivals: Record<ScenarioKey, number>;
  fills: Record<ScenarioKey, number>;
}

function build(m: Moment, focus: Focus, inst: InstrumentMeta, selected: number | null, colors: Record<ScenarioKey, string>): Built {
  const { t0, t1 } = m.window;
  const markers: SeriesMarker<Time>[] = [];
  const info = new Map<string, MarkerInfo>();
  const arrivals: Record<ScenarioKey, number> = { fast: 0, slow: 0 };
  const fills: Record<ScenarioKey, number> = { fast: 0, slow: 0 };
  const price = (tick: number) => tick * inst.tick_size;
  // With an order selected, every other marker is drawn at 35 % opacity (tokens are #rrggbb).
  const tone = (k: ScenarioKey, orderId: number) =>
    selected === null || orderId === selected || !/^#[0-9a-f]{6}$/i.test(colors[k]) ? colors[k] : `${colors[k]}59`;
  for (const k of ['fast', 'slow'] as const) {
    if (!shows(focus, k)) continue;
    const tl = m[k];
    for (const o of tl.orders) {
      const at = o.exch_arrival_ts;
      if (at === null || at < t0 || at > t1) continue;
      arrivals[k] += 1;
      const id = `o:${o.order_id}:${k}:arr`;
      info.set(id, { k, orderId: o.order_id, kind: 'arrival', side: o.side, qty: o.qty, priceTick: o.price_tick, ts: at });
      markers.push({
        id,
        time: toUnit(at) as UTCTimestamp,
        position: 'atPriceMiddle',
        price: price(o.price_tick),
        shape: k === 'fast' ? 'circle' : 'square',
        color: tone(k, o.order_id),
        size: o.order_id === selected ? 2.5 : 1.5,
      });
    }
    for (const f of tl.fills) {
      if (f.ts < t0 || f.ts > t1) continue;
      fills[k] += 1;
      const id = `o:${f.order_id}:${k}:fill:${f.seq}`;
      info.set(id, { k, orderId: f.order_id, kind: 'fill', side: f.side, qty: f.qty, priceTick: f.price_tick, ts: f.ts });
      markers.push({
        id,
        time: toUnit(f.ts) as UTCTimestamp,
        position: f.side === 'buy' ? 'atPriceBottom' : 'atPriceTop',
        price: price(f.price_tick),
        shape: f.side === 'buy' ? 'arrowUp' : 'arrowDown',
        color: tone(k, f.order_id),
        size: f.order_id === selected ? 2.5 : 1.5,
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
  return { bid, ask, markers, info, arrivals, fills };
}

export interface PriceChartProps {
  moment: Moment;
  focus: Focus;
  inst: InstrumentMeta;
  selected: number | null;
  names: Record<ScenarioKey, string>;
  /** Shared replay time (ns), drawn as a vertical line. */
  t: number;
  onSelectOrder: (orderId: number) => void;
  /** A click on the chart away from a marker moves the shared replay time there. */
  onPick: (t: number) => void;
}

export function PriceChart({ moment, focus, inst, selected, names, t, onSelectOrder, onPick }: PriceChartProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const cursorRef = useRef<HTMLDivElement | null>(null);
  const cursorLabelRef = useRef<HTMLSpanElement | null>(null);
  const tipRef = useRef<HTMLDivElement | null>(null);
  const chartRef = useRef<{ chart: IChartApi; bid: ISeriesApi<'Line'>; ask: ISeriesApi<'Line'>; markers: ISeriesMarkersPluginApi<Time> } | null>(null);
  const [failure, setFailure] = useState<Error | null>(null);
  const [colors, setColors] = useState<Record<ScenarioKey, string> | null>(null);
  const handlers = useRef({ onSelectOrder, onPick, names });
  handlers.current = { onSelectOrder, onPick, names };
  const markersRef = useRef<{ list: SeriesMarker<Time>[]; info: Map<string, MarkerInfo> }>({ list: [], info: new Map() });
  // The first unit of the window: its tick label would be cut in half by the left edge.
  const firstUnit = useRef(toUnit(moment.window.t0));
  firstUnit.current = toUnit(moment.window.t0);
  const tRef = useRef(t);
  const phone = usePhone();
  tRef.current = t;

  /** Places the time line over the chart; the chart redraws on zoom and resize without React. */
  const placeCursor = useRef(() => {});
  placeCursor.current = () => {
    const c = chartRef.current;
    const line = cursorRef.current;
    if (!c || !line) return;
    const x = c.chart.timeScale().timeToCoordinate(toUnit(tRef.current) as UTCTimestamp);
    line.style.display = x === null ? 'none' : 'block';
    if (x !== null) line.style.transform = `translateX(${Math.round(x)}px)`;
    if (cursorLabelRef.current) cursorLabelRef.current.textContent = `${(tRef.current / 1e9).toFixed(3)} s`;
  };

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
          fontSize: 13,
          // The logo link has no accessible name; the credit is a named text link under the chart.
          attributionLogo: false,
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
          // Room for the last time-axis label, which would otherwise clip at the right edge.
          rightOffsetPixels: 36,
          tickMarkFormatter: (time: Time) => (time === firstUnit.current ? '' : unitLabel(time, 3)),
        },
        handleScroll: { vertTouchDrag: false },
        localization: {
          timeFormatter: (time: Time) => unitLabel(time, 5),
          priceFormatter: (v: number) => v.toFixed(decimals),
        },
      });
      const priceFormat = { type: 'price' as const, precision: decimals, minMove: inst.tick_size };
      const line = (color: string, title: string) =>
        chart.addSeries(LineSeries, { color, lineWidth: 2, lineType: LineType.WithSteps, title, priceLineVisible: false, priceFormat });
      const bid = line(token(host, '--bid'), 'Best bid');
      const ask = line(token(host, '--ask'), 'Best ask');
      const markers = createSeriesMarkers(bid, [], { autoScale: true });
      // Markers are small: the nearest one within HIT_PX of the pointer is the one meant.
      const nearest = (point: { x: number; y: number }): string | null => {
        let best: { id: string; d: number } | null = null;
        for (const mk of markersRef.current.list) {
          const x = chart.timeScale().timeToCoordinate(mk.time);
          const y = mk.price === undefined ? null : bid.priceToCoordinate(mk.price);
          if (x === null || y === null || mk.id === undefined) continue;
          const d = Math.hypot(x - point.x, y - point.y);
          if (d <= HIT_PX && (!best || d < best.d)) best = { id: mk.id, d };
        }
        return best?.id ?? null;
      };
      // A click selects the marker's order, else moves the replay time there.
      const click = (p: MouseEventParams<Time>) => {
        const id = p.point ? nearest(p.point) : null;
        if (id) handlers.current.onSelectOrder(Number(id.split(':')[1]));
        else if (typeof p.time === 'number') handlers.current.onPick(p.time * UNIT_NS);
      };
      chart.subscribeClick(click);
      // Hover: describe the marker under the pointer, else the observed bid/ask at that time.
      const hover = (p: MouseEventParams<Time>) => {
        const tip = tipRef.current;
        if (!tip) return;
        if (!p.point || typeof p.time !== 'number') {
          tip.hidden = true;
          return;
        }
        const id = nearest(p.point);
        const m = id ? markersRef.current.info.get(id) : undefined;
        const val = (s: ISeriesApi<'Line'>) => {
          const d = p.seriesData.get(s);
          return d && 'value' in d ? d.value.toFixed(decimals) : '—';
        };
        tip.textContent = m
          ? `${handlers.current.names[m.k]} · order ${m.orderId}\n${m.kind === 'arrival' ? 'Reached the exchange' : 'Simulated fill'}: ${m.side === 'buy' ? 'buy' : 'sell'} ${fmtQty(m.qty, inst)} @ ${fmtPrice(m.priceTick, inst)}\n${(m.ts / 1e9).toFixed(6)} s, exchange clock`
          : `${unitLabel(p.time, 4)}\nBid ${val(bid)} · Ask ${val(ask)} (synthetic source data)`;
        tip.hidden = false;
        const w = host.clientWidth;
        const left = p.point.x + 14 + tip.offsetWidth > w ? p.point.x - 14 - tip.offsetWidth : p.point.x + 14;
        tip.style.transform = `translate(${Math.max(0, Math.round(left))}px, ${Math.max(0, Math.round(p.point.y - tip.offsetHeight - 10))}px)`;
      };
      chart.subscribeCrosshairMove(hover);
      // The window always fits the width: refit when the layout resizes the chart.
      const refit = () => chart.timeScale().fitContent();
      const moved = () => placeCursor.current();
      chart.timeScale().subscribeSizeChange(refit);
      chart.timeScale().subscribeVisibleLogicalRangeChange(moved);
      chartRef.current = { chart, bid, ask, markers };
      setColors({ fast: token(host, '--ex-fast'), slow: token(host, '--ex-slow') });
      setFailure(null);
      return () => {
        chart.unsubscribeClick(click);
        chart.unsubscribeCrosshairMove(hover);
        chart.timeScale().unsubscribeSizeChange(refit);
        chart.timeScale().unsubscribeVisibleLogicalRangeChange(moved);
        chart.remove();
        chartRef.current = null;
      };
    } catch (e) {
      setFailure(e instanceof Error ? e : new Error(String(e)));
      return undefined;
    }
  }, [inst]);

  const built = useMemo(() => (colors ? build(moment, focus, inst, selected, colors) : null), [moment, focus, inst, selected, colors]);

  useEffect(() => {
    const c = chartRef.current;
    if (!c || !built) return;
    c.bid.setData(built.bid);
    c.ask.setData(built.ask);
    c.markers.setMarkers(built.markers);
    markersRef.current = { list: built.markers, info: built.info };
    placeCursor.current();
  }, [built]);

  // Re-fit only when a new window arrives, so a zoom survives scenario and selection changes.
  useEffect(() => {
    chartRef.current?.chart.timeScale().fitContent();
    placeCursor.current();
  }, [moment, colors]);

  useEffect(() => placeCursor.current(), [t]);

  // Below 600 px the series titles covered a third of the plot and the 13 px axis labels left
  // room for one or two time ticks; the legend under the chart names both lines.
  useEffect(() => {
    const c = chartRef.current;
    if (!c) return;
    c.bid.applyOptions({ title: phone ? '' : 'Best bid' });
    c.ask.applyOptions({ title: phone ? '' : 'Best ask' });
    c.chart.applyOptions({ layout: { fontSize: phone ? 11 : 13 } });
  }, [phone, colors]);

  const desc = built
    ? `Price chart from ${(moment.window.t0 / 1e9).toFixed(3)} s to ${(moment.window.t1 / 1e9).toFixed(3)} s, replay time line at ${(t / 1e9).toFixed(3)} s. Synthetic best bid and ask from ${fmtPrice(
        moment.market.bbo.bid_tick[0] ?? null,
        inst,
      )} / ${fmtPrice(moment.market.bbo.ask_tick[0] ?? null, inst)}. ${(['fast', 'slow'] as const)
        .filter((k) => shows(focus, k))
        .map((k) => `${names[k]} scenario: ${built.arrivals[k]} simulated order arrivals and ${built.fills[k]} simulated fills`)
        .join('; ')}. The table "Orders in this window" has the same orders as buttons.`
    : 'Price chart loading';

  return (
    <>
      {failure ? <PlainError error={failure} what="Chart" /> : null}
      <div className="ex-chart-host" ref={hostRef} role="img" aria-label={desc}>
        <div ref={cursorRef} className="ex-chart-cursor" aria-hidden="true">
          <span ref={cursorLabelRef} className="ex-chart-cursor-label ex-num" />
        </div>
        <div ref={tipRef} className="ex-chart-tip" aria-hidden="true" hidden />
      </div>
      <button
        type="button"
        className="ex-chart-reset"
        title="Fit the whole replay window again after zooming or panning"
        onClick={() => {
          chartRef.current?.chart.timeScale().fitContent();
          placeCursor.current();
        }}
      >
        Reset view
      </button>
    </>
  );
}
