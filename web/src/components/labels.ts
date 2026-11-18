// Plain-language names for identifiers the server sends as code: dotted `RunConfig` paths
// (`execution.order_latency.entry_ns`), unit names (`quote_currency`, `bps_of_turnover`), the
// demo-id prefix of canonical run labels (`cost_of_latency: 1 ms …`) and the experiment
// descriptions' "Controlled" lines. Every page that shows one of these reads it from here, so the
// same field never has two names; the raw identifier stays available as a `title`.

import { useEffect, useState } from 'react';

import { api } from '../api/client';
import { LAB_FIELD_LABELS, LATENCY_MODEL_LABELS } from './labFieldLabels';
import { EXCHANGE_MODEL_INFO, QUEUE_MODEL_INFO, SIGMA_SOURCE_INFO, STRATEGY_INFO, fractionToBps, strategyName } from './describe';
import { fmtDurationExact } from './fmt';

// ----- configuration paths ----------------------------------------------------------------------

/**
 * Names that must stand on their own (a comparison row, an experiment's "Varied" list), where the
 * Lab's in-section label ("Entry latency" under "Order latency") would be ambiguous, and paths the
 * Lab form does not edit (eligibility fields of a comparison).
 */
const PATH_OVERRIDES: Record<string, string> = {
  'execution.order_latency.entry_ns': 'Order latency, strategy to exchange',
  'execution.order_latency.response_ns': 'Response latency, exchange to strategy',
  'execution.order_latency.jitter_ns': 'Order latency jitter range',
  'execution.order_latency.type': 'Order latency model',
  'execution.queue_model.n': 'Queue model exponent n',
  'risk.max_position': 'Risk limit: max position',
  'risk.max_order_qty': 'Risk limit: max order quantity',
  'risk.max_open_orders': 'Risk limit: max open orders',
  'risk.max_drawdown': 'Risk limit: max drawdown',
  'strategy.kind': 'Strategy kind and parameters',
  dataset: 'Dataset',
  dataset_id: 'Dataset',
  'dataset.sha256': 'Dataset events SHA-256',
  instrument: 'Instrument',
  'instrument.tick_size': 'Tick size',
  'instrument.lot_size': 'Lot size',
  'instrument.contract_size': 'Contract size',
  simulation_revision: 'Simulator revision',
  engine: 'Matching engine version',
};

function sentence(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** Last path segment as words, without a unit suffix (`feed_latency_offset_ns` -> `Feed latency offset`). */
function segmentWords(path: string): string {
  const last = path.split('.').at(-1) ?? path;
  return sentence(last.replace(/_ns$/, '').replace(/_/g, ' '));
}

/** Readable name of a dotted `RunConfig` (or comparison eligibility) path. Show `path` as its title. */
export function pathLabel(path: string): string {
  return PATH_OVERRIDES[path] ?? LAB_FIELD_LABELS[path] ?? segmentWords(path);
}

// ----- values -------------------------------------------------------------------------------------

const VALUE_LABEL: Record<string, string> = Object.fromEntries([
  ...Object.entries(STRATEGY_INFO).map(([k, v]) => [k, v.label]),
  ...Object.entries(EXCHANGE_MODEL_INFO).map(([k, v]) => [k, v.label]),
  ...Object.entries(QUEUE_MODEL_INFO).map(([k, v]) => [k, v.label]),
  ...Object.entries(SIGMA_SOURCE_INFO).map(([k, v]) => [k, v.label]),
  ...Object.entries(LATENCY_MODEL_LABELS),
]);

/** A configuration enum value (`no_partial_fill`, `power_prob3`) in words. */
export function valueLabel(value: string): string {
  return VALUE_LABEL[value] ?? value.replace(/_/g, ' ');
}

/** A strategy's reason with its kind id prefix (`touch_maker: quote bid …`) named in words. Show the raw reason as the title. */
export function reasonText(reason: string): string {
  const m = /^([a-z][a-z0-9_]*): ([\s\S]*)$/.exec(reason);
  return m ? `${strategyName(m[1])}: ${m[2]}` : reason;
}

/** Keys of a configuration object listed first, in this order (a latency leg reads `min …, jitter …`). */
const KEY_ORDER = ['min_ns', 'median_ns', 'mean_ns', 'jitter', 'sigma', 'clip', 'unit'];

function keyRank(k: string): number {
  const i = KEY_ORDER.indexOf(k);
  return i === -1 ? KEY_ORDER.length : i;
}

/**
 * The one formatter for a stored configuration value: `none` for null/empty, durations for `_ns`
 * (numbers or integer strings), basis points beside fee fractions, enum words for strings and
 * `key value` lists for objects (`min 768.92 µs, jitter 0.5, clip 0.1, unit 10`).
 */
export function configValueText(path: string, v: unknown): string {
  if (v === null || v === undefined) return 'none';
  if (typeof v === 'boolean') return v ? 'yes' : 'no';
  if (path.endsWith('_ns') && (typeof v === 'number' || (typeof v === 'string' && /^-?\d+$/.test(v)))) {
    return typeof v === 'number' && !Number.isFinite(v) ? String(v) : fmtDurationExact(v);
  }
  if (typeof v === 'number') {
    return path.startsWith('execution.fees.') && Number.isFinite(v) ? `${v} (${fractionToBps(v)} bp)` : String(v);
  }
  if (typeof v === 'string') return valueLabel(v);
  if (Array.isArray(v)) return v.length === 0 ? 'none' : v.map((x) => configValueText(path, x)).join(', ');
  if (typeof v !== 'object') return String(v);
  const entries = Object.entries(v as Record<string, unknown>).sort(([x], [y]) => keyRank(x) - keyRank(y));
  if (entries.length === 0) return 'none';
  return entries
    .map(([k, x]) => {
      const name = k.replace(/_ns$/, '').replace(/_/g, ' ');
      const text = configValueText(`${path}.${k}`, x);
      return `${name} ${x !== null && typeof x === 'object' && !Array.isArray(x) ? `(${text})` : text}`;
    })
    .join(', ');
}

function leaves(obj: Record<string, unknown>, prefix = ''): [string, unknown][] {
  return Object.entries(obj).flatMap(([k, v]): [string, unknown][] =>
    v !== null && typeof v === 'object' && !Array.isArray(v) ? leaves(v as Record<string, unknown>, `${prefix}${k}.`) : [[`${prefix}${k}`, v]],
  );
}

const ROOTS = ['strategy', 'execution', 'risk', 'record'] as const;

/**
 * A JSON object of configuration values, listed `Name = value`; its section is the root that
 * names most of its keys. Inside a section the Lab's short field labels read best ("Max
 * position" under "Risk limits").
 */
function objectText(obj: Record<string, unknown>): string {
  const list = leaves(obj);
  let root = '';
  let best = 0;
  for (const r of ROOTS) {
    const hits = list.filter(([k]) => (PATH_OVERRIDES[`${r}.${k}`] ?? LAB_FIELD_LABELS[`${r}.${k}`]) !== undefined).length;
    if (hits > best) {
      best = hits;
      root = `${r}.`;
    }
  }
  return list.map(([k, v]) => `${LAB_FIELD_LABELS[`${root}${k}`] ?? pathLabel(`${root}${k}`)} = ${configValueText(`${root}${k}`, v)}`).join('; ');
}

export interface ControlledItem {
  label: string;
  value: string;
  /** The server's line, verbatim. */
  raw: string;
}

/**
 * One line of a demo's "Controlled" list (`Risk limits: {"max_drawdown":null,…}`), with JSON
 * values listed by name and nanosecond values as durations. Unparseable lines pass through.
 */
export function controlledItem(raw: string): ControlledItem {
  const m = /^([^:]+): ([\s\S]*)$/.exec(raw);
  if (!m) return { label: raw, value: '', raw };
  let label = m[1];
  const rest = m[2];
  let parsed: unknown;
  try {
    parsed = JSON.parse(rest);
  } catch {
    return { label, value: rest, raw };
  }
  let value: string;
  if (parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed)) {
    value = objectText(parsed as Record<string, unknown>);
  } else if (typeof parsed === 'number' && / in nanoseconds$/.test(label)) {
    label = label.replace(/ in nanoseconds$/, '');
    value = fmtDurationExact(parsed);
  } else if (typeof parsed === 'number' && /fraction of traded value$/.test(label)) {
    value = `${parsed} (${fractionToBps(parsed)} bp)`;
  } else {
    value = configValueText('', parsed);
  }
  return { label, value, raw };
}

// ----- units --------------------------------------------------------------------------------------

const UNIT_LABEL: Record<string, string> = {
  quote: 'quote currency',
  quote_currency: 'quote currency',
  base: 'base units',
  base_units: 'base units',
  qty: 'base units',
  'quote/base': 'quote per base unit',
  bps: 'bp',
  bps_of_turnover: 'bp of turnover',
  probability: 'probability',
  dimensionless: 'no unit',
  ratio: 'percent',
  ns: 'time',
  ticks: 'ticks',
  count: 'count',
  lots: 'lots',
};

/** A unit name as served (`quote_currency`, `bps_of_turnover`, `base_units`) in words. */
export function unitLabel(unit: string): string {
  return UNIT_LABEL[unit] ?? unit.replace(/_/g, ' ');
}

// ----- run labels ---------------------------------------------------------------------------------

let demoTitles: Promise<ReadonlyMap<string, string>> | null = null;

function loadDemoTitles(): Promise<ReadonlyMap<string, string>> {
  demoTitles ??= api.demos().then(
    (list) => new Map(list.map((d) => [d.id, d.title])),
    () => {
      demoTitles = null;
      return new Map<string, string>();
    },
  );
  return demoTitles;
}

/** Experiment titles by demo id from `GET /demos`, fetched once per page load; empty until then. */
export function useDemoTitles(): ReadonlyMap<string, string> {
  const [titles, setTitles] = useState<ReadonlyMap<string, string>>(new Map());
  useEffect(() => {
    let live = true;
    void loadDemoTitles().then((m) => {
      if (live) setTitles(m);
    });
    return () => {
      live = false;
    };
  }, []);
  return titles;
}

/**
 * A run label with its demo-id prefix (`cost_of_latency: 1 ms …`) replaced by the experiment's
 * title (`Cost of latency: 1 ms …`). Without the demo list, a snake_case prefix is put in words.
 * Show the raw label as the title.
 */
export function runLabelText(label: string, titles: ReadonlyMap<string, string>): string {
  // Stored demo labels say "Risk-adverse" (hftbacktest's type name); the English is "risk-averse".
  const text = label.replace(/\b([Rr])isk-adverse\b/g, '$1isk-averse');
  const m = /^([a-z][a-z0-9]*(?:_[a-z0-9]+)+): (.+)$/.exec(text);
  if (!m) return text;
  return `${titles.get(m[1]) ?? sentence(m[1].replace(/_/g, ' '))}: ${m[2]}`;
}
