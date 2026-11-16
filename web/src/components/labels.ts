// Plain-language names for identifiers the server sends as code: dotted `RunConfig` paths
// (`execution.order_latency.entry_ns`), unit names (`quote_currency`, `bps_of_turnover`), the
// demo-id prefix of canonical run labels (`cost_of_latency: 1 ms …`) and the experiment
// descriptions' "Controlled" lines. Every page that shows one of these reads it from here, so the
// same field never has two names; the raw identifier stays available as a `title`.

import { useEffect, useState } from 'react';

import { api } from '../api/client';
import type { RunConfig } from '../api/types';
import { LATENCY_MODELS, buildSections, startingConfig, strategyKindDefaults, type Field, type Section } from '../features/lab/fields';
import { EXCHANGE_MODEL_INFO, QUEUE_MODEL_INFO, SIGMA_SOURCE_INFO, STRATEGY_INFO, fractionToBps } from './describe';
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

const STRATEGY_TYPES = Object.keys(STRATEGY_INFO) as (keyof typeof STRATEGY_INFO)[];

function selectField(sections: Section[], id: string): Extract<Field<RunConfig>, { kind: 'select' }> | null {
  for (const s of sections) {
    for (const f of s.fields) if (f.id === id && f.kind === 'select') return f;
  }
  return null;
}

/**
 * Labels of every field the Strategy Lab can show, collected by building its form for each
 * strategy kind, each order-latency model (cancel leg on) and a queue model with an exponent.
 */
function labFieldLabels(): Map<string, string> {
  const map = new Map<string, string>();
  const ctx = { instrument: null, presets: [] };
  const base = startingConfig('', null);
  for (const type of STRATEGY_TYPES) {
    let c: RunConfig = { ...base, strategy: { ...base.strategy, kind: strategyKindDefaults(type, null, []) } };
    c = selectField(buildSections(c, ctx), 'execution.queue_model.type')?.set(c, 'power_prob') ?? c;
    for (const model of LATENCY_MODELS) {
      let cfg = selectField(buildSections(c, ctx), 'execution.order_latency.type')?.set(c, model.value) ?? c;
      cfg = selectField(buildSections(cfg, ctx), 'execution.order_latency.cancel')?.set(cfg, 'own') ?? cfg;
      for (const s of buildSections(cfg, ctx)) {
        for (const f of s.fields) if (!map.has(f.id)) map.set(f.id, f.label);
      }
    }
  }
  return map;
}

let pathLabels: Map<string, string> | null = null;

function knownPathLabel(path: string): string | undefined {
  pathLabels ??= labFieldLabels();
  return PATH_OVERRIDES[path] ?? pathLabels.get(path);
}

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
  return knownPathLabel(path) ?? segmentWords(path);
}

// ----- values -------------------------------------------------------------------------------------

const VALUE_LABEL: Record<string, string> = Object.fromEntries([
  ...Object.entries(STRATEGY_INFO).map(([k, v]) => [k, v.label]),
  ...Object.entries(EXCHANGE_MODEL_INFO).map(([k, v]) => [k, v.label]),
  ...Object.entries(QUEUE_MODEL_INFO).map(([k, v]) => [k, v.label]),
  ...Object.entries(SIGMA_SOURCE_INFO).map(([k, v]) => [k, v.label]),
  ...LATENCY_MODELS.map((m) => [m.value, m.label]),
]);

/** A configuration enum value (`no_partial_fill`, `power_prob3`) in words. */
export function valueLabel(value: string): string {
  return VALUE_LABEL[value] ?? value.replace(/_/g, ' ');
}

/** One configuration value as read: durations for `_ns`, basis points beside fee fractions. */
export function configValueText(path: string, v: unknown): string {
  if (v === null || v === undefined) return 'none';
  if (typeof v === 'boolean') return v ? 'yes' : 'no';
  if (typeof v === 'string') return valueLabel(v);
  if (typeof v === 'number') {
    if (path.endsWith('_ns') && Number.isFinite(v)) return fmtDurationExact(v);
    if (path.startsWith('execution.fees.') && Number.isFinite(v)) return `${v} (${fractionToBps(v)} bp)`;
    return String(v);
  }
  return JSON.stringify(v);
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
    const hits = list.filter(([k]) => knownPathLabel(`${r}.${k}`) !== undefined).length;
    if (hits > best) {
      best = hits;
      root = `${r}.`;
    }
  }
  return list.map(([k, v]) => `${pathLabels?.get(`${root}${k}`) ?? pathLabel(`${root}${k}`)} = ${configValueText(`${root}${k}`, v)}`).join('; ');
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
