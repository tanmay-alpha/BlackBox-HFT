// Every nanosecond field of every v1 response passes through `components/ns.ts` here, before any
// view sees it (UI_DESIGN_SPEC.md §7.3, API_CONTRACT.md §1). The wire carries every ns value as a
// decimal i64 string. Relative values (times since a dataset origin, durations, latencies) are
// parsed into a `bigint` and converted to an exact JS number: one beyond 2^53 - 1 is rejected
// with an `ApiError` (`unsafe_integer`) naming the field instead of being shown rounded.
// Absolute epoch ns (`origin_ns`, `*_at_ns`, `server_time_ns`) stay strings and are only
// validated. `seed` (u64) is converted the same way as a relative value.
//
// Requests go the other way through `toWire`: the views' exact numbers become decimal strings.
//
// Not nanosecond fields, and deliberately not touched: `Quantiles` of ns quantities (`f64`
// statistics, e.g. a mean), `TimingSummary` delta statistics, counts, ticks, quantities, money.
// `DatasetManifest.generator` is untyped JSON echoed for display only.

import { NsError, nsToSafeNumber, parseNs, type NsInput } from '../components/ns';
import { ApiError } from './http';
import type * as M from './model';
import type {
  CubicLeg,
  DatasetManifest,
  DecisionOutcome,
  EquitySample,
  Explanation,
  FillRecord,
  FlightEvent,
  GridAxis,
  GridSpec,
  LogNormalLeg,
  MarketView,
  MarkoutStat,
  OrderLatencyConfig,
  OrderPair,
  OrderRecord,
  Preset,
  ReplayWindow,
  RunConfig,
} from './types';
import type * as W from './v1';

type Rec = Record<string, unknown>;

function fail(path: string, e: unknown): never {
  if (e instanceof NsError) throw new ApiError('unsafe_integer', `${path}: ${e.message}`, 0);
  throw e;
}

/** Converts `obj[key]` (a ns decimal string) in place to an exact JS number. */
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

function absNsOpt(obj: object, key: string, path: string): void {
  const v = (obj as Rec)[key];
  if (v === null || v === undefined) return;
  absNs(obj, key, path);
}

function config(raw: object, path: string): RunConfig {
  const c = raw as RunConfig;
  ns(c, 'seed', path);
  ns(c.strategy, 'interval_ns', `${path}.strategy`);
  ns(c.execution, 'feed_latency_offset_ns', `${path}.execution`);
  nsOpt(c.execution, 'think_time_ns', `${path}.execution`);
  const lat = c.execution.order_latency;
  const lp = `${path}.execution.order_latency`;
  switch (lat.type) {
    case 'constant':
    case 'jitter':
      ns(lat, 'entry_ns', lp);
      ns(lat, 'response_ns', lp);
      if (lat.type === 'jitter') ns(lat, 'jitter_ns', lp);
      break;
    case 'log_normal':
    case 'heavy_tail': {
      const key = lat.type === 'log_normal' ? 'median_ns' : 'min_ns';
      ns(lat.entry, key, `${lp}.entry`);
      ns(lat.response, key, `${lp}.response`);
      if (lat.cancel) ns(lat.cancel, key, `${lp}.cancel`);
      break;
    }
  }
  return c;
}

/** The order-latency model with every ns value as a decimal string. */
function latencyOut(lat: OrderLatencyConfig): unknown {
  const p = 'config.execution.order_latency';
  switch (lat.type) {
    case 'constant':
      return { ...lat, entry_ns: nsOut(lat.entry_ns, `${p}.entry_ns`), response_ns: nsOut(lat.response_ns, `${p}.response_ns`) };
    case 'jitter':
      return {
        ...lat,
        entry_ns: nsOut(lat.entry_ns, `${p}.entry_ns`),
        response_ns: nsOut(lat.response_ns, `${p}.response_ns`),
        jitter_ns: nsOut(lat.jitter_ns, `${p}.jitter_ns`),
      };
    case 'log_normal': {
      const leg = (l: LogNormalLeg, name: string) => ({ ...l, median_ns: nsOut(l.median_ns, `${p}.${name}.median_ns`) });
      return { ...lat, entry: leg(lat.entry, 'entry'), response: leg(lat.response, 'response'), cancel: lat.cancel && leg(lat.cancel, 'cancel') };
    }
    case 'heavy_tail': {
      const leg = (l: CubicLeg, name: string) => ({ ...l, min_ns: nsOut(l.min_ns, `${p}.${name}.min_ns`) });
      return { ...lat, entry: leg(lat.entry, 'entry'), response: leg(lat.response, 'response'), cancel: lat.cancel && leg(lat.cancel, 'cancel') };
    }
  }
}

function markouts(raw: readonly object[], path: string): void {
  const list = raw as MarkoutStat[];
  list.forEach((m, i) => ns(m, 'horizon_ns', `${path}[${i}]`));
}

function run(r: W.RunSummary, path: string): M.Run {
  absNs(r.dataset, 'origin_ns', `${path}.dataset`);
  absNs(r, 'created_at_ns', path);
  nsOpt(r, 'interval_ns', path);
  markouts(r.metrics.markouts, `${path}.metrics.markouts`);
  if (r.config) config(r.config, `${path}.config`);
  return r as unknown as M.Run;
}

const JOB_AT = ['created_at_ns', 'queued_at_ns', 'started_at_ns', 'finished_at_ns', 'last_heartbeat_at_ns', 'cancel_requested_at_ns'] as const;

function job(j: W.Job, path: string): W.Job {
  absNs(j, 'created_at_ns', path);
  for (const k of JOB_AT) absNsOpt(j, k, path);
  return j;
}

function experiment(e: W.Experiment, path: string): M.Experiment {
  absNs(e, 'created_at_ns', path);
  nsOpt(e, 'interval_ns', path);
  if (e.job) job(e.job, `${path}.job`);
  if (e.run) run(e.run, `${path}.run`);
  if (e.config) config(e.config, `${path}.config`);
  return e as unknown as M.Experiment;
}

function manifest(raw: object, path: string): DatasetManifest {
  const d = raw as DatasetManifest;
  absNs(d, 'origin_ns', path);
  ns(d, 'start', path);
  ns(d, 'end', path);
  ns(d.timestamps, 'local_ts_shift_ns', `${path}.timestamps`);
  return d;
}

function datasetVersion(v: W.DatasetVersion, path: string): M.DatasetVersion {
  absNs(v, 'origin_ns', path);
  ns(v, 'start_ns', path);
  ns(v, 'end_ns', path);
  absNsOpt(v, 'retired_at_ns', path);
  if (v.manifest) manifest(v.manifest, `${path}.manifest`);
  return v as unknown as M.DatasetVersion;
}

function datasetSummary(d: W.DatasetSummary, path: string): M.DatasetSummary {
  absNs(d, 'created_at_ns', path);
  if (d.current_version) datasetVersion(d.current_version, `${path}.current_version`);
  return d as unknown as M.DatasetSummary;
}

function market(m: MarketView, path: string): void {
  nsOpt(m, 'feed_exch_ts', path);
  nsOpt(m, 'feed_local_ts', path);
}

function event(raw: object, path: string): void {
  const e = raw as FlightEvent;
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

function events(raw: readonly object[], path: string): FlightEvent[] {
  const list = raw as FlightEvent[];
  list.forEach((e, i) => event(e, `${path}[${i}]`));
  return list;
}

const ORDER_TS = ['exch_arrival_ts', 'first_response_ts', 'first_fill_ts', 'last_fill_ts', 'cancel_sent_ts', 'terminal_ts'] as const;

function order(raw: object, path: string): void {
  const o = raw as OrderRecord;
  ns(o, 'decision_ts', path);
  for (const k of ORDER_TS) nsOpt(o, k, path);
}

function orders(raw: readonly object[], path: string): OrderRecord[] {
  const list = raw as OrderRecord[];
  list.forEach((o, i) => order(o, `${path}[${i}]`));
  return list;
}

function fills(raw: readonly object[], path: string): FillRecord[] {
  const list = raw as FillRecord[];
  list.forEach((f, i) => ns(f, 'ts', `${path}[${i}]`));
  return list;
}

function equity(raw: readonly object[], path: string): EquitySample[] {
  const list = raw as EquitySample[];
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

function explanation(raw: object, path: string): void {
  const x = raw as Explanation;
  for (const k of LATENCY_KEYS) nsOpt(x.latency, k, `${path}.latency`);
  x.queue_path.forEach((q, i) => ns(q, 'ts', `${path}.queue_path[${i}]`));
}

const PAIR_TIMING = ['decision_ns', 'exch_arrival_ns', 'first_response_ns', 'first_fill_ns', 'completion_ns'] as const;

function decision(raw: object | null | undefined, path: string): void {
  const d = (raw ?? null) as DecisionOutcome | null;
  if (!d) return;
  ns(d, 'decision_ts', path);
  for (const k of ORDER_TS) nsOpt(d, k, path);
}

function pair(raw: object, path: string): OrderPair {
  const p = raw as OrderPair;
  ns(p, 'decision_ts', path);
  decision(p.a, `${path}.a`);
  decision(p.b, `${path}.b`);
  if (p.timing) for (const k of PAIR_TIMING) nsOpt(p.timing, k, `${path}.timing`);
  return p;
}

function axis(raw: object, path: string): void {
  const a = raw as GridAxis;
  if (a.type === 'order_latency_ns' || a.type === 'entry_latency_ns' || a.type === 'feed_latency_offset_ns') {
    nsArray(a.values, `${path}.values`);
  }
}

function comparison(c: W.Comparison, path: string): M.Comparison {
  absNs(c, 'created_at_ns', path);
  const al = c.alignment;
  if (al) {
    (al.pairs ?? []).forEach((p, i) => pair(p, `${path}.alignment.pairs[${i}]`));
    if (al.first_divergence) pair(al.first_divergence, `${path}.alignment.first_divergence`);
    if (al.first_outcome_divergence) pair(al.first_outcome_divergence, `${path}.alignment.first_outcome_divergence`);
  }
  return c as unknown as M.Comparison;
}

function sweep(s: W.Sweep, path: string): M.Sweep {
  absNs(s, 'created_at_ns', path);
  if (s.spec) {
    config(s.spec.base, `${path}.spec.base`);
    axis(s.spec.x, `${path}.spec.x`);
    axis(s.spec.y, `${path}.spec.y`);
  }
  return s as unknown as M.Sweep;
}

function cursorPage<T, U>(p: W.CursorPage<T>, path: string, each: (item: T, path: string) => U): W.CursorPage<U> {
  const items = p.items.map((item, i) => each(item, `${path}.items[${i}]`));
  return { ...p, items };
}

/** One normaliser per response type; each converts in place and returns its argument. */
export const normalize = {
  status(s: W.Status): W.Status {
    absNs(s, 'server_time_ns', 'status');
    absNsOpt(s.queue, 'as_of_ns', 'status.queue');
    return s;
  },
  whoami(w: W.Whoami): W.Whoami {
    if (w.key) absNsOpt(w.key, 'expires_at_ns', 'whoami.key');
    return w;
  },
  datasets: (p: W.CursorPage<W.DatasetSummary>) => cursorPage(p, 'datasets', datasetSummary),
  dataset(d: W.Dataset): M.Dataset {
    datasetSummary(d, 'dataset');
    return d as unknown as M.Dataset;
  },
  datasetVersions: (p: W.CursorPage<W.DatasetVersion>) => cursorPage(p, 'versions', datasetVersion),
  datasetVersion: (v: W.DatasetVersion) => datasetVersion(v, 'version'),
  manifest: (d: DatasetManifest) => manifest(d, 'manifest'),
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
  demos(list: W.Demo[]): M.Demo[] {
    list.forEach((d, i) => d.runs.forEach((r, j) => config(r.config, `demos[${i}].runs[${j}].config`)));
    return list as unknown as M.Demo[];
  },
  run: (r: W.RunSummary) => run(r, 'run'),
  runs: (p: W.CursorPage<W.RunSummary>) => cursorPage(p, 'runs', run),
  job: (j: W.Job) => job(j, 'job'),
  jobs: (p: W.CursorPage<W.Job>) => cursorPage(p, 'jobs', job),
  experiment: (e: W.Experiment) => experiment(e, 'experiment'),
  experiments: (p: W.CursorPage<W.Experiment>) => cursorPage(p, 'experiments', experiment),
  submitted(s: W.SubmitExperimentResponse): M.SubmitExperimentResult {
    return {
      created: s.created,
      experiment: experiment(s.experiment, 'submit.experiment'),
      job: s.job ? job(s.job, 'submit.job') : null,
      run: s.run ? run(s.run, 'submit.run') : null,
    };
  },
  orders(page: W.OffsetPage<OrderRecord>): W.OffsetPage<OrderRecord> {
    orders(page.items, 'orders.items');
    return page;
  },
  orderDetail(d: W.OrderDetail): M.OrderDetail {
    order(d.order, 'order');
    events(d.events, 'events');
    fills(d.fills, 'fills');
    explanation(d.explanation, 'explanation');
    return d as unknown as M.OrderDetail;
  },
  fills(p: W.CursorPage<FillRecord>): W.CursorPage<FillRecord> {
    fills(p.items, 'fills.items');
    return p;
  },
  equity(p: W.EquityPage): M.EquityPage {
    nsOpt(p, 'bucket_ns', 'equity');
    if (p.downsampled) {
      (p.items as W.EquityBucket[]).forEach((b, i) => {
        ns(b, 't0_ns', `equity.items[${i}]`);
        ns(b, 't1_ns', `equity.items[${i}]`);
      });
    } else {
      equity(p.items, 'equity.items');
    }
    return p as unknown as M.EquityPage;
  },
  tape(p: W.CursorPage<FlightEvent>): W.CursorPage<FlightEvent> {
    events(p.items, 'tape.items');
    return p;
  },
  /** The wire's `t0_ns`/`t1_ns`/`run_start_ns`/`run_end_ns` become the bb-core view names. */
  timeline(t: W.RunTimeline): M.RunTimeline {
    for (const k of ['t0_ns', 't1_ns', 'run_start_ns', 'run_end_ns'] as const) ns(t, k, 'timeline');
    const view = {
      run_id: t.run_id,
      dataset_id: t.dataset_id,
      t0: t.t0_ns as unknown as number,
      t1: t.t1_ns as unknown as number,
      run_start: t.run_start_ns as unknown as number,
      run_end: t.run_end_ns as unknown as number,
      events: events(t.events, 'timeline.events'),
      events_total: t.events_total,
      events_truncated: t.events.length < t.events_total,
      fills: fills(t.fills, 'timeline.fills'),
      equity: equity(t.equity, 'timeline.equity'),
      orders: orders(t.orders, 'timeline.orders'),
      truncated_by: t.truncated_by,
    };
    return view;
  },
  comparison: (c: W.Comparison) => comparison(c, 'comparison'),
  comparisons: (p: W.CursorPage<W.Comparison>) => cursorPage(p, 'comparisons', comparison),
  pairs: (p: W.CursorPage<OrderPair>) => cursorPage(p, 'pairs', pair),
  sweep: (s: W.Sweep) => sweep(s, 'sweep'),
  sweeps: (p: W.CursorPage<W.Sweep>) => cursorPage(p, 'sweeps', sweep),
  report(r: W.Report): W.Report {
    absNs(r, 'created_at_ns', 'report');
    return r;
  },
  reports: (p: W.CursorPage<W.Report>) =>
    cursorPage(p, 'reports', (r: W.Report, path) => {
      absNs(r, 'created_at_ns', path);
      return r;
    }),
  events(list: W.SystemEvent[]): W.SystemEvent[] {
    list.forEach((e, i) => absNs(e, 'at_ns', `events[${i}]`));
    return list;
  },
  /** Integer ns fields of the analytics stay strings; each is checked to be an exact i64. */
  analytics(a: W.RunAnalytics): W.RunAnalytics {
    const walk = (v: unknown, path: string): void => {
      if (Array.isArray(v)) {
        v.forEach((x, i) => walk(x, `${path}[${i}]`));
      } else if (typeof v === 'object' && v !== null) {
        for (const [k, x] of Object.entries(v)) {
          if (k.endsWith('_ns') && typeof x === 'string') absNs(v, k, path);
          else walk(x, `${path}.${k}`);
        }
      }
    };
    walk(a, 'analytics');
    a.execution?.opportunities.forEach((o, i) => absNs(o, '0', `analytics.execution.opportunities[${i}]`));
    return a;
  },
  apiKeys(list: W.ApiKeyInfo[]): W.ApiKeyInfo[] {
    list.forEach((k, i) => {
      const path = `api_keys[${i}]`;
      absNs(k, 'created_at_ns', path);
      for (const f of ['expires_at_ns', 'revoked_at_ns', 'last_used_at_ns'] as const) absNsOpt(k, f, path);
    });
    return list;
  },
};

// ----- requests --------------------------------------------------------------------------------

/** An exact ns number (or a decimal typed by the user) as the wire's decimal string. */
function nsOut(v: NsInput, path: string): string {
  try {
    return (typeof v === 'string' ? parseNs(v) : BigInt(nsToSafeNumber(v))).toString();
  } catch (e) {
    return fail(path, e);
  }
}

/** Request bodies: every ns value and `seed` as a decimal string (API_CONTRACT §1). */
export const toWire = {
  config(c: RunConfig): unknown {
    // `think_time_ns` is omitted when zero, as the server serializes it.
    const { think_time_ns: think, ...execution } = c.execution;
    const withThink = think === undefined || think === 0 ? {} : { think_time_ns: nsOut(think, 'config.execution.think_time_ns') };
    return {
      ...c,
      seed: nsOut(c.seed, 'config.seed'),
      strategy: { ...c.strategy, interval_ns: nsOut(c.strategy.interval_ns, 'config.strategy.interval_ns') },
      execution: {
        ...execution,
        ...withThink,
        feed_latency_offset_ns: nsOut(c.execution.feed_latency_offset_ns, 'config.execution.feed_latency_offset_ns'),
        order_latency: latencyOut(c.execution.order_latency),
      },
    };
  },
  axis(a: GridAxis): unknown {
    if (a.type === 'order_latency_ns' || a.type === 'entry_latency_ns' || a.type === 'feed_latency_offset_ns') {
      return { ...a, values: a.values.map((v, i) => nsOut(v, `${a.type}.values[${i}]`)) };
    }
    return a;
  },
  gridSpec(s: GridSpec): unknown {
    return { base: toWire.config(s.base), x: toWire.axis(s.x), y: toWire.axis(s.y) };
  },
  /** A query-parameter ns value. */
  ns: (v: NsInput) => nsOut(v, 'query'),
};
