// Client-side mirror of `RunConfig::validate` (crates/bb-core/src/config.rs). Same rules, same
// order, same messages, so a problem reads identically whether the browser or the server finds
// it. The server remains the authority: its `invalid_config` response is shown verbatim.

import type { InstrumentMeta, JsonValue, RunConfig } from '../../api/types';

function finitePos(v: number): boolean {
  return Number.isFinite(v) && v > 0;
}

/** `MIN_INTERVAL_NS` in bb-core: shortest strategy evaluation interval (1 ms). */
export const MIN_INTERVAL_NS = 1_000_000;
/** `MAX_INTERVAL_NS` in bb-core: longest strategy evaluation interval (1 h). */
export const MAX_INTERVAL_NS = 3_600_000_000_000;
/** `MAX_LATENCY_NS` in bb-core: bound on any single modeled latency, jitter or feed offset (60 s). */
export const MAX_LATENCY_NS = 60_000_000_000;

/** Human-readable problems; empty means valid. */
export function validateRunConfig(c: RunConfig): string[] {
  const e: string[] = [];
  if (c.dataset_id.length === 0) {
    e.push('dataset_id is empty');
  }
  // Range checks are written as `!(lo <= x && x <= hi)` so NaN from a blank input also fails.
  if (!(c.strategy.interval_ns >= MIN_INTERVAL_NS && c.strategy.interval_ns <= MAX_INTERVAL_NS)) {
    e.push(
      `strategy.interval_ns must be between ${MIN_INTERVAL_NS} (1 ms) and ${MAX_INTERVAL_NS} (1 h)`,
    );
  }
  const latencyOk = (v: number) => v >= 0 && v <= MAX_LATENCY_NS;
  const l = c.execution.order_latency;
  if (l.type === 'constant') {
    if (!latencyOk(l.entry_ns) || !latencyOk(l.response_ns)) {
      e.push(`order latency must be between 0 and ${MAX_LATENCY_NS} ns (60 s)`);
    }
  } else if (!latencyOk(l.entry_ns) || !latencyOk(l.response_ns) || !latencyOk(l.jitter_ns)) {
    e.push(`order latency and jitter must each be between 0 and ${MAX_LATENCY_NS} ns (60 s)`);
  }
  if (!(c.execution.feed_latency_offset_ns >= 0)) {
    e.push(
      'feed_latency_offset_ns must be non-negative: a negative offset would let the strategy see events before the dataset says they were received',
    );
  } else if (c.execution.feed_latency_offset_ns > MAX_LATENCY_NS) {
    e.push(`feed_latency_offset_ns must be at most ${MAX_LATENCY_NS} ns (60 s)`);
  }
  const q = c.execution.queue_model;
  if (q.type === 'power_prob' || q.type === 'power_prob2' || q.type === 'power_prob3') {
    if (!finitePos(q.n)) {
      e.push('queue model exponent n must be finite and positive');
    }
  }
  if (!Number.isFinite(c.execution.fees.maker) || !Number.isFinite(c.execution.fees.taker)) {
    e.push('fees must be finite');
  }
  if (!finitePos(c.risk.max_position)) {
    e.push('risk.max_position must be finite and positive');
  }
  if (!finitePos(c.risk.max_order_qty)) {
    e.push('risk.max_order_qty must be finite and positive');
  }
  if (!(c.risk.max_open_orders >= 1)) {
    e.push('risk.max_open_orders must be at least 1');
  }
  if (c.risk.max_drawdown !== null && !finitePos(c.risk.max_drawdown)) {
    e.push('risk.max_drawdown must be finite and positive when set');
  }
  if (!(c.record.sample_every_ticks >= 1)) {
    e.push('record.sample_every_ticks must be at least 1');
  }
  const k = c.strategy.kind;
  switch (k.type) {
    case 'touch_maker':
      if (!finitePos(k.order_qty) || !finitePos(k.max_position)) {
        e.push('touch_maker: order_qty and max_position must be positive');
      }
      if (!(k.offset_ticks >= 0)) {
        e.push('touch_maker: offset_ticks must be non-negative');
      }
      if (!Number.isFinite(k.skew_ticks_per_unit) || k.skew_ticks_per_unit < 0) {
        e.push('touch_maker: skew_ticks_per_unit must be finite and >= 0');
      }
      break;
    case 'avellaneda_stoikov':
      if (!finitePos(k.order_qty) || !finitePos(k.max_position)) {
        e.push('avellaneda_stoikov: order_qty and max_position must be positive');
      }
      if (!finitePos(k.gamma) || !finitePos(k.kappa) || !finitePos(k.horizon_s)) {
        e.push('avellaneda_stoikov: gamma, kappa, horizon_s must be positive');
      }
      if (!(k.vol_halflife_ticks >= 1)) {
        e.push('avellaneda_stoikov: vol_halflife_ticks must be at least 1');
      }
      break;
    case 'imbalance_taker':
      if (!finitePos(k.order_qty) || !finitePos(k.max_position)) {
        e.push('imbalance_taker: order_qty and max_position must be positive');
      }
      if (!(k.threshold > 0 && k.threshold < 1)) {
        e.push('imbalance_taker: threshold must be in (0, 1)');
      }
      if (!(k.hold_ticks >= 1)) {
        e.push('imbalance_taker: hold_ticks must be at least 1');
      }
      break;
    case 'scripted': {
      const seen = new Set<string>();
      for (const o of k.orders) {
        const key = `${o.at_tick}:${o.slot}`;
        if (seen.has(key)) {
          e.push(`scripted: duplicate (at_tick=${o.at_tick}, slot=${o.slot})`);
        }
        seen.add(key);
        if (!finitePos(o.qty)) {
          e.push(`scripted: order '${o.label}' qty must be positive`);
        }
        if (o.ord_type === 'market') {
          e.push(
            `scripted: order '${o.label}' is a market order; market orders are not supported on hftbacktest 0.9.4 (see docs/EXECUTION_MODEL.md, defect 5); use an immediate-or-cancel limit order`,
          );
        }
      }
      break;
    }
  }
  return e;
}

function isLotMultiple(qty: number, inst: InstrumentMeta): boolean {
  const lots = qty / inst.lot_size;
  return Math.abs(lots - Math.round(lots)) < 1e-6 && Math.round(lots) >= 1;
}

/**
 * Non-blocking observations about a valid configuration: things `RunConfig::validate` accepts
 * but that will show up on the tape as risk blocks. The harness blocks an order that is not a
 * positive multiple of the lot size (`RiskRule::InvalidOrder`) or exceeds `risk.max_order_qty`.
 */
export function configAdvisories(c: RunConfig, inst: InstrumentMeta | null): string[] {
  const out: string[] = [];
  const k = c.strategy.kind;
  const sizes: { what: string; qty: number }[] =
    k.type === 'scripted'
      ? k.orders.map((o, i) => ({ what: `scripted order ${i + 1} qty`, qty: o.qty }))
      : [{ what: `${k.type} order_qty`, qty: k.order_qty }];
  for (const s of sizes) {
    if (!finitePos(s.qty)) continue;
    if (inst && !isLotMultiple(s.qty, inst)) {
      out.push(
        `${s.what} ${s.qty} is not a multiple of the lot size ${inst.lot_size}; the harness blocks such orders (risk rule invalid_order).`,
      );
    }
    if (finitePos(c.risk.max_order_qty) && s.qty > c.risk.max_order_qty) {
      out.push(
        `${s.what} ${s.qty} exceeds risk.max_order_qty ${c.risk.max_order_qty}; every such order will be risk-blocked.`,
      );
    }
  }
  return out;
}

export interface ConfigChange {
  path: string;
  a: string;
  b: string;
}

function flatten(value: JsonValue, path: string, out: Map<string, string>): void {
  if (value !== null && typeof value === 'object') {
    const entries: [string, JsonValue][] = Array.isArray(value)
      ? value.map((v, i): [string, JsonValue] => [String(i), v])
      : Object.entries(value);
    if (entries.length === 0) out.set(path, Array.isArray(value) ? '[]' : '{}');
    for (const [k, v] of entries) {
      flatten(v, path ? `${path}.${k}` : k, out);
    }
    return;
  }
  out.set(path, JSON.stringify(value));
}

/**
 * Leaf-by-leaf differences between two configurations, as dotted paths into the serialized
 * `RunConfig`. Used to show what scenario B changes relative to scenario A before running it.
 */
export function diffConfigs(a: RunConfig, b: RunConfig): ConfigChange[] {
  const fa = new Map<string, string>();
  const fb = new Map<string, string>();
  // A config is plain JSON; the round trip also turns NaN from blank inputs into null.
  flatten(JSON.parse(JSON.stringify(a)) as JsonValue, '', fa);
  flatten(JSON.parse(JSON.stringify(b)) as JsonValue, '', fb);
  const paths = Array.from(new Set([...fa.keys(), ...fb.keys()])).sort();
  const out: ConfigChange[] = [];
  for (const path of paths) {
    const va = fa.get(path);
    const vb = fb.get(path);
    if (va !== vb) out.push({ path, a: va ?? '(absent)', b: vb ?? '(absent)' });
  }
  return out;
}
