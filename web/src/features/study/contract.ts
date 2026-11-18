// The public, read-only study-result contract `blackbox.study-result` (version 1), written by
// `blackbox study exec-validation` as `public/study-result.json` with its SHA-256 in
// `study-result.json.sha256` (crates/bb-app/src/study.rs, docs/research/EXECUTION_STUDY_VIEWER.md).
// The viewer checks the content hash first, then the contract name and version, then the shape it
// reads; any failure is shown, never papered over.

export const STUDY_SCHEMA = 'blackbox.study-result';
export const STUDY_SCHEMA_VERSION = 1;
export const STUDY_FILE = 'study-result.json';

export type Measures = Record<string, number>;

export interface Bootstrap {
  n: number;
  mean: number;
  lower_95: number;
  upper_95: number;
  replicates: number;
  seed: number;
}

/** One measure of an aggregate group: a single deterministic run, or the mean over latency seeds. */
export type AggregateMetric =
  | { kind: 'deterministic_single'; value: number }
  /** `bootstrap` is `null` when fewer than two defined seed values exist. */
  | { kind: 'stochastic_seed_mean'; bootstrap: Bootstrap | null };

export interface Aggregate {
  regime: string;
  queue_model: string;
  latency: string;
  feed_offset_ns: number;
  design: string;
  members: string[];
  metrics: Record<string, AggregateMetric>;
}

export interface PairSummary {
  order_id: number;
  tick_no: number;
  decision_ts_ns: string | number;
  class: string;
  differences: string[];
}

export interface ConfigDiff {
  path: string;
  a: unknown;
  b: unknown;
}

export interface RunComparison {
  reference_run_id: string;
  aligned: boolean;
  like_for_like: boolean;
  config_diffs: ConfigDiff[];
  first_divergence: PairSummary | null;
  first_outcome_divergence: PairSummary | null;
  lifecycle_counts: Record<string, number> | null;
}

export interface Accounting {
  realized_pnl: number;
  unrealized_pnl: number;
  gross_pnl: number;
  fees: number;
  net_pnl: number;
}

/** A run's public summary: identity hashes, measures, P&L decomposition, comparison. */
export interface PublicRun {
  run_id: string;
  config_sha256: string;
  result_sha256: string;
  dataset: { id: string; sha256: string; rows: number };
  seed: number;
  queue_model: string | null;
  latency: string | null;
  feed_offset_ns: number;
  measures: Measures;
  accounting: Accounting;
  orders: { submitted: number; filled: number; partially_filled: number; canceled: number };
  /** Against the within-regime baseline (study cells) or the curated pair's A run (anchors). */
  comparison: RunComparison | null;
}

export interface StudyCell extends PublicRun {
  regime: string;
  queue_model: string;
  latency: string;
  design: string;
  stochastic: boolean;
  /** Path of the full cell file in the local study output. */
  detail: string;
}

export interface AnchorRun extends PublicRun {
  index: number;
  label: string;
  reference_index: number | null;
}

export interface StudyDataset {
  id: string;
  title: string;
  /** `null`: the default generator without a regime intervention. */
  regime: string | null;
  events_sha256: string;
  events_bytes: number;
  counts: { rows: number; trades: number; depth_updates: number } & Record<string, number>;
  generator: ({ seed?: number; duration_ns?: number } & Record<string, unknown>) | null;
}

/** One study level of a model, in study order (baseline first). */
export interface ModelLevel {
  name: string;
  config: Record<string, unknown>;
}

export interface LatencyModel extends ModelLevel {
  stochastic: boolean;
}

export interface StudyResult {
  schema: typeof STUDY_SCHEMA;
  schema_version: typeof STUDY_SCHEMA_VERSION;
  study_id: string;
  title: string;
  data_class: 'synthetic';
  generator: {
    command: string;
    blackbox_version: string;
    simulation_revision: number;
    run_contract_version: number;
    engine: { name: string; version: string; features?: string[] };
    platform: string;
    source_commit: string | null;
  };
  design: {
    generator_seed: number;
    duration_ns: number;
    latency_seeds: number[];
    bootstrap_seed: number;
    bootstrap_replicates: number;
    bootstrap_method: string;
    matched_median_ns: number;
    cross_regime_alignment: string;
  };
  baseline: {
    preset: string;
    regime: string;
    queue_model: string;
    latency: string;
    feed_offset_ns: number;
    run_id: string;
    config_sha256: string;
    dataset: { id: string; sha256: string };
    config: Record<string, unknown>;
  };
  queue_models: ModelLevel[];
  latency_models: LatencyModel[];
  uncertainty: {
    source: 'seed_variation';
    method: string;
    level: number;
    replicates: number;
    seed: number;
    applies_to_latencies: string[];
    excludes: string;
  };
  datasets: StudyDataset[];
  cells: StudyCell[];
  aggregates: Aggregate[];
  sensitivity_surfaces: { stratum: { regime: string }; axes: string[]; rows: Aggregate[] }[];
  demo_anchors: {
    demo_id: string;
    title: string;
    question: string;
    datasets: StudyDataset[];
    runs: AnchorRun[];
    pairs: [number, number][];
  };
  caveats: string[];
}

export type StudyErrorKind = 'unavailable' | 'hash_mismatch' | 'unsupported_contract' | 'invalid';

/** Why a study could not be shown. The message is meant to be displayed as is. */
export class StudyLoadError extends Error {
  readonly kind: StudyErrorKind;

  constructor(kind: StudyErrorKind, message: string) {
    super(message);
    this.name = 'StudyLoadError';
    this.kind = kind;
  }
}

const HEX64 = /^[0-9a-f]{64}$/;

/** Lowercase hex SHA-256 of `bytes` (WebCrypto). */
export async function sha256Hex(bytes: ArrayBuffer | Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', bytes as BufferSource);
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('');
}

/** The digest of a one-line `sha256sum` file (`<64 hex>  <file>`) that names `file`. */
export function parseShaFile(text: string, file: string = STUDY_FILE): string {
  const lines = text.split('\n').filter((l) => l.trim() !== '');
  const match = lines.length === 1 ? /^([0-9a-f]{64}) [ *](.+)$/.exec(lines[0].trimEnd()) : null;
  if (!match) throw new StudyLoadError('invalid', `The hash file is not one "<sha256>  ${file}" line.`);
  if (match[2] !== file) throw new StudyLoadError('invalid', `The hash file names ${match[2]}, not ${file}.`);
  return match[1];
}

type Json = Record<string, unknown>;

function isObject(v: unknown): v is Json {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function fail(path: string, what: string): never {
  throw new StudyLoadError('invalid', `Study result does not match contract version ${STUDY_SCHEMA_VERSION}: ${path} ${what}.`);
}

function obj(v: unknown, path: string): Json {
  if (!isObject(v)) fail(path, 'is not an object');
  return v;
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

function hash(v: unknown, path: string): string {
  const s = str(v, path);
  if (!HEX64.test(s)) fail(path, 'is not a SHA-256 hex digest');
  return s;
}

function measures(v: unknown, path: string): void {
  for (const [k, x] of Object.entries(obj(v, path))) num(x, `${path}.${k}`);
}

function publicRun(v: unknown, path: string): Json {
  const r = obj(v, path);
  const runId = str(r.run_id, `${path}.run_id`);
  const config = hash(r.config_sha256, `${path}.config_sha256`);
  if (!/^[0-9a-f]{16}$/.test(runId) || !config.startsWith(runId)) fail(`${path}.run_id`, 'is not the prefix of config_sha256');
  hash(r.result_sha256, `${path}.result_sha256`);
  const ds = obj(r.dataset, `${path}.dataset`);
  str(ds.id, `${path}.dataset.id`);
  hash(ds.sha256, `${path}.dataset.sha256`);
  num(r.seed, `${path}.seed`);
  num(r.feed_offset_ns, `${path}.feed_offset_ns`);
  measures(r.measures, `${path}.measures`);
  const acc = obj(r.accounting, `${path}.accounting`);
  for (const k of ['realized_pnl', 'unrealized_pnl', 'gross_pnl', 'fees', 'net_pnl']) num(acc[k], `${path}.accounting.${k}`);
  if (r.comparison !== null) {
    const c = obj(r.comparison, `${path}.comparison`);
    str(c.reference_run_id, `${path}.comparison.reference_run_id`);
    if (typeof c.aligned !== 'boolean') fail(`${path}.comparison.aligned`, 'is not a boolean');
    arr(c.config_diffs, `${path}.comparison.config_diffs`);
    if (c.first_divergence !== null) num(obj(c.first_divergence, `${path}.comparison.first_divergence`).order_id, `${path}.comparison.first_divergence.order_id`);
  }
  return r;
}

function aggregate(v: unknown, path: string): void {
  const a = obj(v, path);
  for (const k of ['regime', 'queue_model', 'latency', 'design']) str(a[k], `${path}.${k}`);
  num(a.feed_offset_ns, `${path}.feed_offset_ns`);
  arr(a.members, `${path}.members`).forEach((m, i) => str(m, `${path}.members[${i}]`));
  for (const [k, m] of Object.entries(obj(a.metrics, `${path}.metrics`))) {
    const metric = obj(m, `${path}.metrics.${k}`);
    if (metric.kind === 'deterministic_single') num(metric.value, `${path}.metrics.${k}.value`);
    else if (metric.kind === 'stochastic_seed_mean') {
      if (metric.bootstrap !== null) {
        const b = obj(metric.bootstrap, `${path}.metrics.${k}.bootstrap`);
        for (const f of ['n', 'mean', 'lower_95', 'upper_95']) num(b[f], `${path}.metrics.${k}.bootstrap.${f}`);
      }
    } else fail(`${path}.metrics.${k}.kind`, 'is neither deterministic_single nor stochastic_seed_mean');
  }
}

function dataset(v: unknown, path: string): void {
  const d = obj(v, path);
  str(d.id, `${path}.id`);
  hash(d.events_sha256, `${path}.events_sha256`);
  if (d.regime !== null) str(d.regime, `${path}.regime`);
  num(obj(d.counts, `${path}.counts`).rows, `${path}.counts.rows`);
}

/**
 * Parses study-result JSON text: the contract name and version first (a mismatch is
 * `unsupported_contract`), then every field the viewer reads (`invalid` names the first bad path).
 */
export function parseStudyResult(text: string): StudyResult {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (e) {
    throw new StudyLoadError('invalid', `Study result is not JSON: ${e instanceof Error ? e.message : String(e)}.`);
  }
  const root = obj(raw, 'the document');
  if (root.schema !== STUDY_SCHEMA || root.schema_version !== STUDY_SCHEMA_VERSION) {
    throw new StudyLoadError(
      'unsupported_contract',
      `This viewer reads ${STUDY_SCHEMA} version ${STUDY_SCHEMA_VERSION}; the file declares ${JSON.stringify(root.schema ?? null)} version ${JSON.stringify(root.schema_version ?? null)}.`,
    );
  }
  str(root.study_id, 'study_id');
  str(root.title, 'title');
  if (root.data_class !== 'synthetic') fail('data_class', 'is not "synthetic"');
  const gen = obj(root.generator, 'generator');
  for (const k of ['command', 'blackbox_version', 'platform']) str(gen[k], `generator.${k}`);
  num(gen.simulation_revision, 'generator.simulation_revision');
  if (gen.source_commit !== null) str(gen.source_commit, 'generator.source_commit');
  const design = obj(root.design, 'design');
  arr(design.latency_seeds, 'design.latency_seeds').forEach((s, i) => num(s, `design.latency_seeds[${i}]`));
  num(design.duration_ns, 'design.duration_ns');
  const base = obj(root.baseline, 'baseline');
  hash(base.config_sha256, 'baseline.config_sha256');
  str(base.run_id, 'baseline.run_id');
  obj(base.config, 'baseline.config');
  arr(root.queue_models, 'queue_models').forEach((m, i) => {
    const level = obj(m, `queue_models[${i}]`);
    str(level.name, `queue_models[${i}].name`);
    obj(level.config, `queue_models[${i}].config`);
  });
  arr(root.latency_models, 'latency_models').forEach((m, i) => {
    const level = obj(m, `latency_models[${i}]`);
    str(level.name, `latency_models[${i}].name`);
    obj(level.config, `latency_models[${i}].config`);
    if (typeof level.stochastic !== 'boolean') fail(`latency_models[${i}].stochastic`, 'is not a boolean');
  });
  const unc = obj(root.uncertainty, 'uncertainty');
  if (unc.source !== 'seed_variation') fail('uncertainty.source', 'is not "seed_variation"');
  str(unc.method, 'uncertainty.method');
  num(unc.level, 'uncertainty.level');
  arr(root.datasets, 'datasets').forEach((d, i) => dataset(d, `datasets[${i}]`));
  arr(root.cells, 'cells').forEach((c, i) => {
    const cell = publicRun(c, `cells[${i}]`);
    for (const k of ['regime', 'queue_model', 'latency', 'design', 'detail']) str(cell[k], `cells[${i}].${k}`);
    if (typeof cell.stochastic !== 'boolean') fail(`cells[${i}].stochastic`, 'is not a boolean');
  });
  arr(root.aggregates, 'aggregates').forEach((a, i) => aggregate(a, `aggregates[${i}]`));
  arr(root.sensitivity_surfaces, 'sensitivity_surfaces').forEach((s, i) => {
    const surface = obj(s, `sensitivity_surfaces[${i}]`);
    str(obj(surface.stratum, `sensitivity_surfaces[${i}].stratum`).regime, `sensitivity_surfaces[${i}].stratum.regime`);
    arr(surface.rows, `sensitivity_surfaces[${i}].rows`).forEach((a, j) => aggregate(a, `sensitivity_surfaces[${i}].rows[${j}]`));
  });
  const anchors = obj(root.demo_anchors, 'demo_anchors');
  str(anchors.demo_id, 'demo_anchors.demo_id');
  arr(anchors.datasets, 'demo_anchors.datasets').forEach((d, i) => dataset(d, `demo_anchors.datasets[${i}]`));
  arr(anchors.runs, 'demo_anchors.runs').forEach((r, i) => {
    const run = publicRun(r, `demo_anchors.runs[${i}]`);
    num(run.index, `demo_anchors.runs[${i}].index`);
    str(run.label, `demo_anchors.runs[${i}].label`);
  });
  arr(root.caveats, 'caveats').forEach((c, i) => str(c, `caveats[${i}]`));
  return raw as StudyResult;
}

/** A verified study: its parsed content and the SHA-256 the bytes were checked against. */
export interface VerifiedStudy {
  study: StudyResult;
  sha256: string;
}

/**
 * Checks `bytes` against the published hash file, then parses them. The hash is checked before
 * the content is read at all, so a truncated or edited file never renders.
 */
export async function verifyStudy(bytes: ArrayBuffer | Uint8Array, shaFile: string): Promise<VerifiedStudy> {
  const expected = parseShaFile(shaFile);
  const actual = await sha256Hex(bytes);
  if (actual !== expected) {
    throw new StudyLoadError('hash_mismatch', `Content hash mismatch: the hash file says ${expected}, the downloaded study hashes to ${actual}.`);
  }
  const text = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  return { study: parseStudyResult(text), sha256: actual };
}
