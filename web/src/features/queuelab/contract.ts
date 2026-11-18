// The public, read-only queue study contract `blackbox.queue-shadow` (version 1), written by
// `blackbox study queue-shadow` as `study-result.json` with its SHA-256 in
// `study-result.json.sha256` (crates/bb-app/src/queue_study.rs, docs/research/QUEUE_SHADOW_EXPERIMENT.md §3).
// The SHA-256 is checked first with the study feature's helpers, then the contract name and
// version, then every field the Queue Model Lab reads and the completeness of every result grid.
// Only the published synthetic artefact is accepted: real NASDAQ results are never published.

import { STUDY_FILE, StudyLoadError, parseShaFile, sha256Hex } from '../study/contract';

export const QUEUE_SCHEMA = 'blackbox.queue-shadow';
export const QUEUE_SCHEMA_VERSION = 1;
/** The L3 reference path's model id (`keys.reference_model`). */
export const REFERENCE_MODEL = 'l3_shadow';

/** An analytics scalar: available with a value, or unavailable with the reason. */
export type Value =
  | { status: 'available'; value: number; unit: string; n: number; excluded: number }
  | { status: 'unavailable'; reason: string; unit: string; n: number; excluded: number };

export interface Interval {
  lower: Value;
  upper: Value;
  method: string;
}

export interface ModelDef {
  id: string;
  family: string | null;
  n: number | null;
  description: string;
}

export interface FillCauses {
  queue: number;
  trade_through: number;
  book_cross: number;
}

/** A model path against the L3 path on the same eligible shadows. */
export interface VsL3 {
  both: number;
  model_only: number;
  l3_only: number;
  neither: number;
  difference: Value;
  difference_95: Interval;
  /** `null` when the difference is unavailable (no eligible shadow). */
  excludes_zero: boolean | null;
}

/** One symbol x latency x horizon x model. */
export interface ResultCell {
  symbol: string;
  latency_ns: number;
  horizon_ns: number;
  model: string;
  family: string | null;
  n: number | null;
  placed: number;
  eligible: number;
  filled: number;
  fill_fraction: Value;
  fill_fraction_95: Interval;
  fill_causes: FillCauses;
  /** `null` for the L3 reference itself. */
  vs_l3: VsL3 | null;
  km_median_ns: Value;
  km_fill_probability: Value;
  km_fill_probability_95: Interval;
  km_censored: number;
}

export interface CancelledVolume {
  symbol: string;
  latency_ns: number;
  horizon_ns: number;
  placed: number;
  ahead_shares: number;
  behind_shares: number;
  share_ahead: Value;
}

/** `front - ahead_L3` of one L2 model at one checkpoint. */
export interface CheckpointRow {
  symbol: string;
  latency_ns: number;
  checkpoint_ns: number;
  model: string;
  pairs: number;
  mean_abs_error_shares: Value;
  mean_signed_error_shares: Value;
  mean_l3_ahead_shares: Value;
}

export interface SymbolPlacement {
  symbol: string;
  locate: number;
  scheduled: number;
  placed: number;
  skipped_empty_side: number;
  skipped_locked_or_crossed: number;
  not_arrived: number;
}

/** The pre-registered tests of one symbol x latency x horizon; `null` fields: no eligible shadow. */
export interface HypothesisCell {
  symbol: string;
  latency_ns: number;
  horizon_ns: number;
  risk_adverse_difference: number | null;
  risk_adverse_difference_95: [number | null, number | null];
  h_a_supported: boolean | null;
  closer_than_risk_adverse: string[];
  h0_not_rejected: string[];
}

export interface WithinEverySymbol {
  latency_ns: number;
  horizon_ns: number;
  models: string[];
}

export interface QueueShadowResult {
  schema: typeof QUEUE_SCHEMA;
  schema_version: typeof QUEUE_SCHEMA_VERSION;
  study_id: string;
  title: string;
  data_class: 'synthetic';
  labels: string[];
  generator: { command: string; blackbox_version: string; source_commit: string | null };
  /** Optional additive implementation metadata, distinct from the frozen design hash. */
  engine?: { revision: string; deadline_semantics: string; checkpoint_semantics: string };
  source: { kind: string; generator?: string; config?: Record<string, unknown>; bytes: number; sha256: string };
  input: {
    frames_read: number;
    invalid_frames: number;
    messages_applied_before_window_end: number;
    stop: { reason: string; detail: string | null };
  };
  clock: { basis: string; local_clock: string };
  design: {
    latencies_ns: number[];
    horizons_ns: number[];
    checkpoints_ns: number[];
    interval_ns: number;
    shares: number;
    lot_size_shares: number;
    window_start_ns: number;
    window_end_ns: number;
    rules: string[];
  };
  design_sha256: string;
  interval_methods: Record<'fill_fraction_95' | 'difference_95' | 'km' | 'checkpoints' | 'cancelled_volume', string>;
  /** Additive metadata: older v1 artefacts may omit this note. */
  interval_note?: string;
  models: ModelDef[];
  results: {
    tracking_end_ns: number;
    stream_reached_window_end: boolean;
    messages: number;
    symbols: SymbolPlacement[];
    cells: ResultCell[];
    cancelled_volume: CancelledVolume[];
    checkpoints: CheckpointRow[];
  };
  hypotheses: {
    definitions: Record<'h_a' | 'h_b_closer' | 'h_b_no_single_model' | 'h0', string>;
    cells: HypothesisCell[];
    models_within_interval_on_every_symbol: WithinEverySymbol[];
  };
  caveats: string[];
}

type Json = Record<string, unknown>;

const HEX64 = /^[0-9a-f]{64}$/;

function fail(path: string, what: string): never {
  throw new StudyLoadError('invalid', `Queue study result does not match ${QUEUE_SCHEMA} version ${QUEUE_SCHEMA_VERSION}: ${path} ${what}.`);
}

function obj(v: unknown, path: string): Json {
  if (typeof v !== 'object' || v === null || Array.isArray(v)) fail(path, 'is not an object');
  return v as Json;
}

function arr(v: unknown, path: string): unknown[] {
  if (!Array.isArray(v)) fail(path, 'is not an array');
  return v;
}

function str(v: unknown, path: string): string {
  if (typeof v !== 'string') fail(path, 'is not a string');
  return v;
}

function num(v: unknown, path: string): number {
  if (typeof v !== 'number' || !Number.isFinite(v)) fail(path, 'is not a finite number');
  return v;
}

/** A non-negative integer count or nanosecond time (exact in a JS number). */
function int(v: unknown, path: string): number {
  const x = num(v, path);
  if (!Number.isSafeInteger(x) || x < 0) fail(path, 'is not a non-negative integer');
  return x;
}

function bool(v: unknown, path: string): boolean {
  if (typeof v !== 'boolean') fail(path, 'is not a boolean');
  return v;
}

function hash(v: unknown, path: string): string {
  const s = str(v, path);
  if (!HEX64.test(s)) fail(path, 'is not a SHA-256 hex digest');
  return s;
}

function strings(v: unknown, path: string): string[] {
  return arr(v, path).map((s, i) => str(s, `${path}[${i}]`));
}

function oneOf<T>(v: T, allowed: readonly T[], path: string): T {
  if (!allowed.includes(v)) fail(path, `is ${JSON.stringify(v)}, not one of ${allowed.map((a) => JSON.stringify(a)).join(', ')}`);
  return v;
}

function value(v: unknown, path: string): void {
  const o = obj(v, path);
  str(o.unit, `${path}.unit`);
  int(o.n, `${path}.n`);
  int(o.excluded, `${path}.excluded`);
  if (o.status === 'available') num(o.value, `${path}.value`);
  else if (o.status === 'unavailable') str(o.reason, `${path}.reason`);
  else fail(`${path}.status`, 'is not "available" or "unavailable"');
}

function interval(v: unknown, path: string): void {
  const o = obj(v, path);
  value(o.lower, `${path}.lower`);
  value(o.upper, `${path}.upper`);
  str(o.method, `${path}.method`);
}

/** Every key combination of a result grid appears exactly once. */
function complete(keys: string[], expected: number, path: string): void {
  const seen = new Set<string>();
  keys.forEach((k, i) => {
    if (seen.has(k)) fail(`${path}[${i}]`, `repeats the key ${k}`);
    seen.add(k);
  });
  if (seen.size !== expected) fail(path, `has ${seen.size} key combinations, not the ${expected} of the design`);
}

/**
 * Parses queue study JSON text: the contract name and version first (a mismatch is
 * `unsupported_contract`), then every field the lab reads (`invalid` names the first bad path).
 */
export function parseQueueShadow(text: string): QueueShadowResult {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    throw new StudyLoadError('invalid', `Queue study result is not JSON: ${e instanceof Error ? e.message : String(e)}.`);
  }
  const root = obj(raw, 'the document');
  if (root.schema !== QUEUE_SCHEMA || root.schema_version !== QUEUE_SCHEMA_VERSION) {
    throw new StudyLoadError(
      'unsupported_contract',
      `This lab reads ${QUEUE_SCHEMA} version ${QUEUE_SCHEMA_VERSION}; the file declares ${JSON.stringify(root.schema ?? null)} version ${JSON.stringify(root.schema_version ?? null)}.`,
    );
  }
  str(root.study_id, 'study_id');
  str(root.title, 'title');
  if (root.data_class !== 'synthetic') fail('data_class', 'is not "synthetic" (real-data results are never published)');
  strings(root.labels, 'labels');
  const gen = obj(root.generator, 'generator');
  str(gen.command, 'generator.command');
  str(gen.blackbox_version, 'generator.blackbox_version');
  if (gen.source_commit !== null) str(gen.source_commit, 'generator.source_commit');
  if (root.engine !== undefined) {
    const engine = obj(root.engine, 'engine');
    for (const k of ['revision', 'deadline_semantics', 'checkpoint_semantics']) str(engine[k], `engine.${k}`);
  }
  const source = obj(root.source, 'source');
  str(source.kind, 'source.kind');
  int(source.bytes, 'source.bytes');
  hash(source.sha256, 'source.sha256');
  const input = obj(root.input, 'input');
  for (const k of ['frames_read', 'invalid_frames', 'messages_applied_before_window_end']) int(input[k], `input.${k}`);
  str(obj(input.stop, 'input.stop').reason, 'input.stop.reason');
  const clock = obj(root.clock, 'clock');
  str(clock.basis, 'clock.basis');
  str(clock.local_clock, 'clock.local_clock');

  const design = obj(root.design, 'design');
  const ints = (k: string) => arr(design[k], `design.${k}`).map((x, i) => int(x, `design.${k}[${i}]`));
  const latencies = ints('latencies_ns');
  const horizons = ints('horizons_ns');
  const checkpoints = ints('checkpoints_ns');
  if (latencies.length === 0 || horizons.length === 0 || checkpoints.length === 0) fail('design', 'has an empty latency, horizon or checkpoint list');
  for (const k of ['interval_ns', 'shares', 'lot_size_shares', 'window_start_ns', 'window_end_ns']) int(design[k], `design.${k}`);
  strings(design.rules, 'design.rules');
  hash(root.design_sha256, 'design_sha256');
  const methods = obj(root.interval_methods, 'interval_methods');
  for (const k of ['fill_fraction_95', 'difference_95', 'km', 'checkpoints', 'cancelled_volume']) str(methods[k], `interval_methods.${k}`);
  if (root.interval_note !== undefined) str(root.interval_note, 'interval_note');

  const models = arr(root.models, 'models').map((m, i) => {
    const o = obj(m, `models[${i}]`);
    str(o.description, `models[${i}].description`);
    if (o.family !== null) str(o.family, `models[${i}].family`);
    if (o.n !== null) int(o.n, `models[${i}].n`);
    return str(o.id, `models[${i}].id`);
  });
  if (models[0] !== REFERENCE_MODEL) fail('models[0].id', `is not "${REFERENCE_MODEL}"`);
  if (models.length < 2 || new Set(models).size !== models.length) fail('models', 'does not list the reference and distinct L2 models');
  const l2 = models.slice(1);

  const results = obj(root.results, 'results');
  int(results.tracking_end_ns, 'results.tracking_end_ns');
  bool(results.stream_reached_window_end, 'results.stream_reached_window_end');
  int(results.messages, 'results.messages');
  const symbols = arr(results.symbols, 'results.symbols').map((s, i) => {
    const o = obj(s, `results.symbols[${i}]`);
    for (const k of ['locate', 'scheduled', 'placed', 'skipped_empty_side', 'skipped_locked_or_crossed', 'not_arrived']) int(o[k], `results.symbols[${i}].${k}`);
    return str(o.symbol, `results.symbols[${i}].symbol`);
  });
  if (symbols.length === 0 || new Set(symbols).size !== symbols.length) fail('results.symbols', 'is empty or repeats a symbol');

  const cellKeys = arr(results.cells, 'results.cells').map((c, i) => {
    const p = `results.cells[${i}]`;
    const o = obj(c, p);
    const symbol = oneOf(str(o.symbol, `${p}.symbol`), symbols, `${p}.symbol`);
    const latency = oneOf(int(o.latency_ns, `${p}.latency_ns`), latencies, `${p}.latency_ns`);
    const horizon = oneOf(int(o.horizon_ns, `${p}.horizon_ns`), horizons, `${p}.horizon_ns`);
    const model = oneOf(str(o.model, `${p}.model`), models, `${p}.model`);
    for (const k of ['placed', 'eligible', 'filled', 'km_censored']) int(o[k], `${p}.${k}`);
    value(o.fill_fraction, `${p}.fill_fraction`);
    interval(o.fill_fraction_95, `${p}.fill_fraction_95`);
    const causes = obj(o.fill_causes, `${p}.fill_causes`);
    for (const k of ['queue', 'trade_through', 'book_cross']) int(causes[k], `${p}.fill_causes.${k}`);
    value(o.km_median_ns, `${p}.km_median_ns`);
    value(o.km_fill_probability, `${p}.km_fill_probability`);
    interval(o.km_fill_probability_95, `${p}.km_fill_probability_95`);
    if (model === REFERENCE_MODEL) {
      if (o.vs_l3 !== null) fail(`${p}.vs_l3`, 'is not null for the reference');
    } else {
      const vs = obj(o.vs_l3, `${p}.vs_l3`);
      for (const k of ['both', 'model_only', 'l3_only', 'neither']) int(vs[k], `${p}.vs_l3.${k}`);
      value(vs.difference, `${p}.vs_l3.difference`);
      interval(vs.difference_95, `${p}.vs_l3.difference_95`);
      if (vs.excludes_zero !== null) bool(vs.excludes_zero, `${p}.vs_l3.excludes_zero`);
    }
    return `${symbol}/${latency}/${horizon}/${model}`;
  });
  complete(cellKeys, symbols.length * latencies.length * horizons.length * models.length, 'results.cells');

  const volumeKeys = arr(results.cancelled_volume, 'results.cancelled_volume').map((c, i) => {
    const p = `results.cancelled_volume[${i}]`;
    const o = obj(c, p);
    for (const k of ['placed', 'ahead_shares', 'behind_shares']) int(o[k], `${p}.${k}`);
    value(o.share_ahead, `${p}.share_ahead`);
    return `${oneOf(str(o.symbol, `${p}.symbol`), symbols, `${p}.symbol`)}/${oneOf(int(o.latency_ns, `${p}.latency_ns`), latencies, `${p}.latency_ns`)}/${oneOf(int(o.horizon_ns, `${p}.horizon_ns`), horizons, `${p}.horizon_ns`)}`;
  });
  complete(volumeKeys, symbols.length * latencies.length * horizons.length, 'results.cancelled_volume');

  const checkpointKeys = arr(results.checkpoints, 'results.checkpoints').map((c, i) => {
    const p = `results.checkpoints[${i}]`;
    const o = obj(c, p);
    int(o.pairs, `${p}.pairs`);
    for (const k of ['mean_abs_error_shares', 'mean_signed_error_shares', 'mean_l3_ahead_shares']) value(o[k], `${p}.${k}`);
    return [
      oneOf(str(o.symbol, `${p}.symbol`), symbols, `${p}.symbol`),
      oneOf(int(o.latency_ns, `${p}.latency_ns`), latencies, `${p}.latency_ns`),
      oneOf(int(o.checkpoint_ns, `${p}.checkpoint_ns`), checkpoints, `${p}.checkpoint_ns`),
      oneOf(str(o.model, `${p}.model`), l2, `${p}.model`),
    ].join('/');
  });
  complete(checkpointKeys, symbols.length * latencies.length * checkpoints.length * l2.length, 'results.checkpoints');

  const hyp = obj(root.hypotheses, 'hypotheses');
  const defs = obj(hyp.definitions, 'hypotheses.definitions');
  for (const k of ['h_a', 'h_b_closer', 'h_b_no_single_model', 'h0']) str(defs[k], `hypotheses.definitions.${k}`);
  const hypKeys = arr(hyp.cells, 'hypotheses.cells').map((c, i) => {
    const p = `hypotheses.cells[${i}]`;
    const o = obj(c, p);
    if (o.h_a_supported !== null) bool(o.h_a_supported, `${p}.h_a_supported`);
    if (o.risk_adverse_difference !== null) num(o.risk_adverse_difference, `${p}.risk_adverse_difference`);
    const ci = arr(o.risk_adverse_difference_95, `${p}.risk_adverse_difference_95`);
    if (ci.length !== 2) fail(`${p}.risk_adverse_difference_95`, 'is not a [lower, upper] pair');
    ci.forEach((x, j) => (x === null ? null : num(x, `${p}.risk_adverse_difference_95[${j}]`)));
    strings(o.closer_than_risk_adverse, `${p}.closer_than_risk_adverse`).forEach((m, j) => oneOf(m, l2, `${p}.closer_than_risk_adverse[${j}]`));
    strings(o.h0_not_rejected, `${p}.h0_not_rejected`).forEach((m, j) => oneOf(m, l2, `${p}.h0_not_rejected[${j}]`));
    return `${oneOf(str(o.symbol, `${p}.symbol`), symbols, `${p}.symbol`)}/${oneOf(int(o.latency_ns, `${p}.latency_ns`), latencies, `${p}.latency_ns`)}/${oneOf(int(o.horizon_ns, `${p}.horizon_ns`), horizons, `${p}.horizon_ns`)}`;
  });
  complete(hypKeys, symbols.length * latencies.length * horizons.length, 'hypotheses.cells');
  const withinKeys = arr(hyp.models_within_interval_on_every_symbol, 'hypotheses.models_within_interval_on_every_symbol').map((c, i) => {
    const p = `hypotheses.models_within_interval_on_every_symbol[${i}]`;
    const o = obj(c, p);
    strings(o.models, `${p}.models`).forEach((m, j) => oneOf(m, l2, `${p}.models[${j}]`));
    return `${oneOf(int(o.latency_ns, `${p}.latency_ns`), latencies, `${p}.latency_ns`)}/${oneOf(int(o.horizon_ns, `${p}.horizon_ns`), horizons, `${p}.horizon_ns`)}`;
  });
  complete(withinKeys, latencies.length * horizons.length, 'hypotheses.models_within_interval_on_every_symbol');
  strings(root.caveats, 'caveats');
  return raw as QueueShadowResult;
}

/** A verified queue study: its parsed content and the SHA-256 the bytes were checked against. */
export interface VerifiedQueueShadow {
  study: QueueShadowResult;
  sha256: string;
}

/**
 * Checks `bytes` against the published hash file, then parses them. The hash is checked before
 * the content is read at all, so a truncated or edited file never renders.
 */
export async function verifyQueueShadow(bytes: ArrayBuffer | Uint8Array, shaFile: string): Promise<VerifiedQueueShadow> {
  const expected = parseShaFile(shaFile, STUDY_FILE);
  const actual = await sha256Hex(bytes);
  if (actual !== expected) {
    throw new StudyLoadError('hash_mismatch', `Content hash mismatch: the hash file says ${expected}, the downloaded study hashes to ${actual}.`);
  }
  const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  return { study: parseQueueShadow(text), sha256: actual };
}
