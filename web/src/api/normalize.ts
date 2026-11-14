// Every nanosecond field of every API response passes through `components/ns.ts` here, before any
// view sees it (UI_DESIGN_SPEC.md §7.3). The legacy `/api` sends relative ns, durations and
// latencies as JSON numbers; each one must be a safe integer, or the response is rejected with an
// `ApiError` (`unsafe_integer`) naming the field, instead of showing a rounded value as if exact.
// Decimal strings (the v1 wire format) are accepted too and converted exactly, so the views keep
// their numeric types through the cutover. Absolute epoch ns (`origin_ns`) stay strings and are
// only validated.
//
// Not nanosecond fields, and deliberately not touched: `Quantiles` of ns quantities (`f64`
// statistics, e.g. a mean), counts, ticks, quantities, money. `DatasetManifest.generator` is
// untyped JSON echoed for display only.

import { NsError, nsToSafeNumber, parseNs, type NsInput } from '../components/ns';
import { ApiError } from './client';
import type {
  Comparison,
  DatasetManifest,
  DecisionOutcome,
  Demo,
  EquitySample,
  Explanation,
  FillRecord,
  FlightEvent,
  GridAxis,
  GridResult,
  MarketView,
  MarkoutStat,
  OrderDetail,
  OrderPair,
  OrderRecord,
  OrdersPage,
  Preset,
  ReplayWindow,
  RunConfig,
  RunSummary,
  RunTimeline,
  TapePage,
} from './types';

type Rec = Record<string, unknown>;

function fail(path: string, e: unknown): never {
  if (e instanceof NsError) throw new ApiError('unsafe_integer', `${path}: ${e.message}`, 0);
  throw e;
}

/** Converts `obj[key]` (a ns number or decimal string) in place to an exact JS number. */
function ns(obj: object, key: string, path: string): void {
  const o = obj as Rec;
  try {
    o[key] = nsToSafeNumber(o[key] as NsInput);
  } catch (e) {
    fail(`${path}.${key}`, e);
  }
}

/** As `ns`, allowing `null`. */
function nsOpt(obj: object, key: string, path: string): void {
  const o = obj as Rec;
  if (o[key] === null || o[key] === undefined) return;
  ns(obj, key, path);
}

function nsArray(values: unknown[], path: string): void {
  for (let i = 0; i < values.length; i += 1) {
    try {
      values[i] = nsToSafeNumber(values[i] as NsInput);
    } catch (e) {
      fail(`${path}[${i}]`, e);
    }
  }
}

/** Absolute epoch ns stay decimal strings; they are only checked. */
function absNs(obj: object, key: string, path: string): void {
  const v = (obj as Rec)[key];
  try {
    if (typeof v !== 'string') throw new NsError(`${String(v)} is not a decimal string`);
    parseNs(v);
  } catch (e) {
    fail(`${path}.${key}`, e);
  }
}

function config(c: RunConfig, path: string): RunConfig {
  ns(c.strategy, 'interval_ns', `${path}.strategy`);
  ns(c.execution, 'feed_latency_offset_ns', `${path}.execution`);
  const lat = c.execution.order_latency;
  ns(lat, 'entry_ns', `${path}.execution.order_latency`);
  ns(lat, 'response_ns', `${path}.execution.order_latency`);
  if (lat.type === 'jitter') ns(lat, 'jitter_ns', `${path}.execution.order_latency`);
  return c;
}

function markouts(list: MarkoutStat[], path: string): void {
  list.forEach((m, i) => ns(m, 'horizon_ns', `${path}[${i}]`));
}

function summary(r: RunSummary, path: string): RunSummary {
  absNs(r.manifest.dataset, 'origin_ns', `${path}.manifest.dataset`);
  config(r.manifest.config, `${path}.manifest.config`);
  markouts(r.metrics.markouts, `${path}.metrics.markouts`);
  return r;
}

function market(m: MarketView, path: string): void {
  nsOpt(m, 'feed_exch_ts', path);
  nsOpt(m, 'feed_local_ts', path);
}

function event(e: FlightEvent, path: string): void {
  ns(e, 'ts', path);
  const b = e.body;
  const bp = `${path}.body`;
  switch (b.type) {
    case 'order_intent':
    case 'cancel_intent':
      market(b.market, `${bp}.market`);
      break;
    case 'request_sent':
      ns(b, 'entry_latency_ns', bp);
      nsOpt(b, 'exch_arrival_ts', bp);
      break;
    case 'exch_response':
      ns(b, 'response_latency_ns', bp);
      ns(b, 'local_arrival_ts', bp);
      break;
    case 'local_response':
      ns(b, 'exch_ts', bp);
      break;
    default:
      break;
  }
}

function events(list: FlightEvent[], path: string): void {
  list.forEach((e, i) => event(e, `${path}[${i}]`));
}

const ORDER_TS = ['exch_arrival_ts', 'first_response_ts', 'first_fill_ts', 'last_fill_ts', 'cancel_sent_ts', 'terminal_ts'] as const;

function order(o: OrderRecord, path: string): void {
  ns(o, 'decision_ts', path);
  for (const k of ORDER_TS) nsOpt(o, k, path);
}

function orders(list: OrderRecord[], path: string): void {
  list.forEach((o, i) => order(o, `${path}[${i}]`));
}

function fills(list: FillRecord[], path: string): FillRecord[] {
  list.forEach((f, i) => ns(f, 'ts', `${path}[${i}]`));
  return list;
}

function equity(list: EquitySample[], path: string): EquitySample[] {
  list.forEach((s, i) => ns(s, 'ts', `${path}[${i}]`));
  return list;
}

const LATENCY_KEYS = [
  'feed_staleness_ns',
  'feed_latency_ns',
  'entry_latency_ns',
  'response_latency_ns',
  'time_to_first_fill_ns',
  'resting_time_ns',
  'cancel_entry_latency_ns',
] as const;

function explanation(x: Explanation, path: string): void {
  for (const k of LATENCY_KEYS) nsOpt(x.latency, k, `${path}.latency`);
  x.queue_path.forEach((q, i) => ns(q, 'ts', `${path}.queue_path[${i}]`));
}

function decision(d: DecisionOutcome | null, path: string): void {
  if (!d) return;
  nsOpt(d, 'exch_arrival_ts', path);
  nsOpt(d, 'first_fill_ts', path);
}

function pair(p: OrderPair, path: string): void {
  ns(p, 'decision_ts', path);
  decision(p.a, `${path}.a`);
  decision(p.b, `${path}.b`);
}

function axis(a: GridAxis, path: string): void {
  if (a.type === 'order_latency_ns' || a.type === 'entry_latency_ns' || a.type === 'feed_latency_offset_ns') {
    nsArray(a.values, `${path}.values`);
  }
}

/** One normaliser per response type; each converts in place and returns its argument. */
export const normalize = {
  dataset(d: DatasetManifest, path = 'dataset'): DatasetManifest {
    absNs(d, 'origin_ns', path);
    ns(d, 'start', path);
    ns(d, 'end', path);
    ns(d.timestamps, 'local_ts_shift_ns', `${path}.timestamps`);
    return d;
  },
  datasets(list: DatasetManifest[]): DatasetManifest[] {
    list.forEach((d, i) => normalize.dataset(d, `datasets[${i}]`));
    return list;
  },
  replay(w: ReplayWindow): ReplayWindow {
    ns(w, 't0', 'replay');
    ns(w, 't1', 'replay');
    ns(w, 'step_ns', 'replay');
    nsArray(w.bbo.ts, 'replay.bbo.ts');
    nsArray(w.trades.ts, 'replay.trades.ts');
    w.depth.forEach((f, i) => ns(f, 'ts', `replay.depth[${i}]`));
    return w;
  },
  presets(list: Preset[]): Preset[] {
    list.forEach((p, i) => config(p.config, `presets[${i}].config`));
    return list;
  },
  run(r: RunSummary): RunSummary {
    return summary(r, 'run');
  },
  runs(list: RunSummary[]): RunSummary[] {
    list.forEach((r, i) => summary(r, `runs[${i}]`));
    return list;
  },
  orders(page: OrdersPage): OrdersPage {
    orders(page.orders, 'orders');
    return page;
  },
  orderDetail(d: OrderDetail): OrderDetail {
    order(d.order, 'order');
    events(d.events, 'events');
    fills(d.fills, 'fills');
    explanation(d.explanation, 'explanation');
    return d;
  },
  fills(list: FillRecord[]): FillRecord[] {
    return fills(list, 'fills');
  },
  equity(list: EquitySample[]): EquitySample[] {
    return equity(list, 'equity');
  },
  tape(page: TapePage): TapePage {
    events(page.events, 'tape.events');
    return page;
  },
  compare(c: Comparison): Comparison {
    c.alignment.pairs.forEach((p, i) => pair(p, `compare.alignment.pairs[${i}]`));
    if (c.alignment.first_divergence) pair(c.alignment.first_divergence, 'compare.alignment.first_divergence');
    return c;
  },
  grid(g: GridResult): GridResult {
    config(g.spec.base, 'grid.spec.base');
    axis(g.spec.x, 'grid.spec.x');
    axis(g.spec.y, 'grid.spec.y');
    g.cells.forEach((cell, i) => {
      if (cell.metrics) markouts(cell.metrics.markouts, `grid.cells[${i}].metrics.markouts`);
    });
    return g;
  },
  timeline(t: RunTimeline): RunTimeline {
    ns(t, 't0', 'timeline');
    ns(t, 't1', 'timeline');
    ns(t, 'run_start', 'timeline');
    ns(t, 'run_end', 'timeline');
    events(t.events, 'timeline.events');
    fills(t.fills, 'timeline.fills');
    equity(t.equity, 'timeline.equity');
    orders(t.orders, 'timeline.orders');
    return t;
  },
  demos(list: Demo[]): Demo[] {
    list.forEach((d, i) => d.runs.forEach((r, j) => config(r.config, `demos[${i}].runs[${j}].config`)));
    return list;
  },
};
