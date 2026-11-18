// Cell selection over a verified study: what a `?cell=` key names, how it differs from the
// baseline (controlled differences), and what kind of comparison its values are (B3). Every label
// is derived from artefact fields (run ids, dataset hashes, comparison flags, config diffs), never
// from a row's position. Pure functions, no React.

import { groupCellKey, parseCellKey, runCellKey } from './cellKey';
import type { Aggregate, AnchorRun, ConfigDiff, PublicRun, StudyCell, StudyResult } from './contract';
import { latencyLabel, queueLabel, regimeLabel } from './model';

export type Selection =
  /** One study run; `group` is the aggregate group it belongs to. */
  | { kind: 'run'; key: string; run: StudyCell; group: Aggregate | null }
  /** One demo anchor run, recomputed on the demo's own datasets. */
  | { kind: 'anchor'; key: string; run: AnchorRun; reference: AnchorRun | null }
  /** One aggregate group: a deterministic run or the seeds of a stochastic latency model. */
  | { kind: 'group'; key: string; group: Aggregate; members: StudyCell[] };

const sameGroup = (a: Aggregate, regime: string, queue: string, latency: string, feed: number) =>
  a.regime === regime && a.queue_model === queue && a.latency === latency && a.feed_offset_ns === feed;

export function findGroup(study: StudyResult, regime: string, queue: string, latency: string, feed: number): Aggregate | null {
  return (
    study.aggregates.find((a) => sameGroup(a, regime, queue, latency, feed)) ??
    study.sensitivity_surfaces.flatMap((s) => s.rows).find((a) => sameGroup(a, regime, queue, latency, feed)) ??
    null
  );
}

/** The group a study run belongs to (its run id is a member). */
export function groupOfRun(study: StudyResult, runId: string): Aggregate | null {
  return study.aggregates.find((a) => a.members.includes(runId)) ?? null;
}

/** What `raw` (a `?cell=` value) selects in `study`; `null` when it names nothing. */
export function resolveSelection(study: StudyResult, raw: string | null): Selection | null {
  const key = parseCellKey(raw);
  if (!key) return null;
  if (key.kind === 'group') {
    const group = findGroup(study, key.regime, key.queueModel, key.latency, key.feedOffsetNs);
    if (!group) return null;
    const byId = new Map(study.cells.map((c) => [c.run_id, c]));
    const members = group.members.map((id) => byId.get(id)).filter((c): c is StudyCell => c !== undefined);
    return { kind: 'group', key: groupCellKey(group), group, members };
  }
  const cell = study.cells.find((c) => c.run_id === key.runId);
  if (cell) return { kind: 'run', key: runCellKey(cell.run_id), run: cell, group: groupOfRun(study, cell.run_id) };
  const anchor = study.demo_anchors.runs.find((r) => r.run_id === key.runId);
  if (!anchor) return null;
  const reference = anchor.reference_index === null ? null : (study.demo_anchors.runs.find((r) => r.index === anchor.reference_index) ?? null);
  return { kind: 'anchor', key: runCellKey(anchor.run_id), run: anchor, reference };
}

// ---- controlled differences -----------------------------------------------------------------

const PATH_LABEL: Record<string, string> = {
  dataset_id: 'Dataset',
  seed: 'Run seed',
  'execution.queue_model.type': 'Queue-position model',
  'execution.queue_model.n': 'Queue model exponent n',
  'execution.order_latency.type': 'Order-latency model',
  'execution.order_latency.entry_ns': 'Order entry latency (fixed)',
  'execution.order_latency.response_ns': 'Exchange response latency (fixed)',
  'execution.order_latency.jitter_ns': 'Added uniform latency jitter',
  'execution.order_latency.entry': 'Order entry latency distribution',
  'execution.order_latency.response': 'Exchange response latency distribution',
  'execution.order_latency.cancel': 'Cancel entry latency distribution',
  'execution.feed_latency_offset_ns': 'Feed latency offset',
};

/** Readable name of a configuration path; unknown paths fall back to their last segment. */
export function pathLabel(path: string): string {
  const known = PATH_LABEL[path];
  if (known) return known;
  const last = path.split('.').pop() ?? path;
  const words = last.replace(/_ns$/, '').replace(/_/g, ' ');
  return words.charAt(0).toUpperCase() + words.slice(1);
}

const ms = (ns: number) => `${ns / 1e6} ms`;

/** One configuration value in words: nanoseconds as milliseconds, latency legs described. */
export function configValue(path: string, v: unknown): string {
  if (v === null || v === undefined) return 'not set';
  if (typeof v === 'number') return path.endsWith('_ns') ? ms(v) : String(v);
  if (typeof v === 'string') return v;
  if (typeof v === 'object' && !Array.isArray(v)) {
    const leg = v as Record<string, unknown>;
    if (typeof leg.median_ns === 'number') return `log-normal, median ${ms(leg.median_ns)}, sigma ${String(leg.sigma)}`;
    if (typeof leg.min_ns === 'number') return `clipped cubic, min ${ms(leg.min_ns)}, jitter ${String(leg.jitter)}, clip ${String(leg.clip)}, unit ${String(leg.unit)}`;
  }
  return JSON.stringify(v);
}

export interface ControlledDiff {
  /** Raw configuration path (`execution.queue_model.type`); level paths for anchors without a comparison. */
  path: string;
  label: string;
  baseline: string;
  value: string;
}

export interface DiffSet {
  /** What the differences are taken against, in words. */
  against: string;
  diffs: ControlledDiff[];
  /** Why differences could not be derived (then `diffs` is empty). */
  problem: string | null;
}

const toDiff = (path: string, a: unknown, b: unknown): ControlledDiff => ({ path, label: pathLabel(path), baseline: configValue(path, a), value: configValue(path, b) });

/** Config diffs that change a value: a field absent on one side and null on the other does not. */
const realDiffs = (diffs: ConfigDiff[]) => diffs.filter((d) => !(d.a == null && d.b == null));

/** Whether `cell` is the regime-baseline run of its regime: the study baseline levels on that regime's dataset. */
function isRegimeBaseline(study: StudyResult, cell: StudyCell): boolean {
  const b = study.baseline;
  return (
    cell.design === 'regime_baseline' &&
    cell.queue_model === b.queue_model &&
    cell.latency === b.latency &&
    cell.feed_offset_ns === b.feed_offset_ns &&
    cell.seed === b.config.seed
  );
}

/** Differences of a study run against the study baseline, from its artefact comparison. */
function runDiffs(study: StudyResult, run: StudyCell): DiffSet {
  const b = study.baseline;
  const c = run.comparison;
  if (run.run_id === b.run_id) return { against: 'the study baseline (this run)', diffs: [], problem: null };
  if (!c) return { against: 'the study baseline', diffs: [], problem: 'The artefact carries no configuration comparison for this run.' };
  const fromComparison = realDiffs(c.config_diffs)
    .filter((d) => d.path !== 'dataset_id')
    .map((d) => toDiff(d.path, d.a, d.b));
  if (c.reference_run_id === b.run_id) {
    const dataset = run.dataset.id !== b.dataset.id ? [toDiff('dataset_id', b.dataset.id, run.dataset.id)] : [];
    return { against: `the study baseline run ${b.run_id}`, diffs: [...dataset, ...fromComparison], problem: null };
  }
  const ref = study.cells.find((x) => x.run_id === c.reference_run_id);
  if (ref && isRegimeBaseline(study, ref) && ref.dataset.sha256 === run.dataset.sha256) {
    const dataset = ref.dataset.id !== b.dataset.id ? [toDiff('dataset_id', b.dataset.id, ref.dataset.id)] : [];
    return {
      against: `the study baseline; configuration fields from the comparison with the ${regimeLabel(ref.regime)} baseline run ${ref.run_id} (study baseline levels on that dataset)`,
      diffs: [...dataset, ...fromComparison],
      problem: null,
    };
  }
  if (ref && ref.run_id === run.run_id && isRegimeBaseline(study, run)) {
    return { against: 'the study baseline', diffs: run.dataset.id !== b.dataset.id ? [toDiff('dataset_id', b.dataset.id, run.dataset.id)] : [], problem: null };
  }
  return {
    against: 'the study baseline',
    diffs: [],
    problem: `The comparison reference ${c.reference_run_id} is neither the study baseline nor a regime baseline run with the same dataset; differences are not derived.`,
  };
}

/** Level-by-level differences of a run without a comparison against the study baseline. */
function levelDiffs(study: StudyResult, run: PublicRun): ControlledDiff[] {
  const b = study.baseline;
  const out: ControlledDiff[] = [];
  if (run.dataset.id !== b.dataset.id) out.push(toDiff('dataset_id', b.dataset.id, run.dataset.id));
  if (run.queue_model !== b.queue_model) out.push({ path: 'queue_model (study level)', label: 'Queue-position model', baseline: queueLabel(b.queue_model), value: queueLabel(run.queue_model) });
  if (run.latency !== b.latency) out.push({ path: 'latency (study level)', label: 'Order-latency model', baseline: latencyLabel(b.latency), value: latencyLabel(run.latency) });
  if (run.feed_offset_ns !== b.feed_offset_ns) out.push(toDiff('execution.feed_latency_offset_ns', b.feed_offset_ns, run.feed_offset_ns));
  const seed = b.config.seed;
  if (typeof seed === 'number' && run.seed !== seed) out.push(toDiff('seed', seed, run.seed));
  return out;
}

const diffSig = (d: ControlledDiff) => `${d.path}=${d.baseline}>${d.value}`;

/**
 * Controlled differences of a selection against the study baseline: only the fields that differ.
 * A group lists its members' shared differences and the seeds it averages over; members that
 * differ in anything but the seed make the group's differences underivable.
 */
export function controlledDifferences(study: StudyResult, sel: Selection): DiffSet {
  if (sel.kind === 'run') return runDiffs(study, sel.run);
  if (sel.kind === 'anchor') {
    const ref = sel.reference;
    const c = sel.run.comparison;
    if (ref && c && c.reference_run_id === ref.run_id) {
      return { against: `demo run #${ref.index} (${ref.label})`, diffs: realDiffs(c.config_diffs).map((d) => toDiff(d.path, d.a, d.b)), problem: null };
    }
    return { against: 'the study baseline (study levels; this demo run has no comparison)', diffs: levelDiffs(study, sel.run), problem: null };
  }
  const { members, group } = sel;
  if (members.length !== group.members.length) {
    return { against: 'the study baseline', diffs: [], problem: 'Some member runs of this group are missing from the artefact.' };
  }
  const sets = members.map((m) => runDiffs(study, m));
  const bad = sets.find((s) => s.problem);
  if (bad) return { against: 'the study baseline', diffs: [], problem: bad.problem };
  const shared = sets.map((s) => s.diffs.filter((d) => d.path !== 'seed'));
  const sig = shared[0]?.map(diffSig).join('|') ?? '';
  if (shared.some((s) => s.map(diffSig).join('|') !== sig)) {
    return { against: 'the study baseline', diffs: [], problem: 'Member runs of this group differ in more than the seed; no shared differences.' };
  }
  const diffs = shared[0] ?? [];
  const baseSeed = study.baseline.config.seed;
  const seeds = members.map((m) => m.seed);
  if (members.length > 1) {
    diffs.push({ path: 'seed', label: 'Run seed', baseline: String(baseSeed ?? '?'), value: `${seeds.length} seeds: ${seeds.join(', ')} (mean over seeds)` });
  } else if (members.length === 1 && typeof baseSeed === 'number' && members[0].seed !== baseSeed) {
    diffs.push(toDiff('seed', baseSeed, members[0].seed));
  }
  return { against: sets[0]?.against ?? 'the study baseline', diffs, problem: null };
}

// ---- comparison labels (B3) -------------------------------------------------------------------

export type ComparisonKind = 'paired' | 'metrics_only' | 'cross_dataset' | 'multi_seed' | 'not_attributable';

export const COMPARISON_LABEL: Record<ComparisonKind, string> = {
  paired: 'Like-for-like paired',
  metrics_only: 'Metrics only',
  cross_dataset: 'Cross-dataset sensitivity',
  multi_seed: 'Multi-seed',
  not_attributable: 'Difference not causally attributable',
};

export const COMPARISON_MEANING: Record<ComparisonKind, string> = {
  paired:
    'Same dataset (events hash) and the same decision schedule: orders align one by one with the reference run, so the difference is the effect of the changed assumption inside the model.',
  metrics_only: 'Orders could not be aligned with the reference run; only the summary measures are compared.',
  cross_dataset:
    'Different synthetic datasets (events hashes differ): the difference mixes the market regime with knock-on strategy effects and is a sensitivity to the dataset as a whole.',
  multi_seed: 'The value is the mean over the seeds of a stochastic latency model; its interval is seed variation only.',
  not_attributable:
    'More than one controlled factor differs (or what differs is not in the artefact): the difference cannot be attributed to any single one of them.',
};

/** A controlled factor of the design; configuration paths map onto these. */
export type Factor = 'dataset' | 'queue_model' | 'order_latency' | 'feed_latency' | 'seed' | `other:${string}`;

export function factorOf(path: string): Factor {
  if (path === 'dataset_id') return 'dataset';
  if (path === 'seed') return 'seed';
  if (path.startsWith('execution.queue_model') || path.startsWith('queue_model')) return 'queue_model';
  if (path.startsWith('execution.order_latency') || path.startsWith('latency')) return 'order_latency';
  if (path === 'execution.feed_latency_offset_ns') return 'feed_latency';
  return `other:${path}`;
}

export interface PairFacts {
  /** Both sides replay the same events (equal dataset SHA-256). */
  sameDataset: boolean;
  /** Orders align one by one (`comparison.aligned && like_for_like`); false when unknown. */
  aligned: boolean;
  /** Controlled factors that differ besides the dataset; a multi-seed mean excludes the seed. */
  factors: Factor[];
  /** Runs behind the compared value (seeds of a stochastic group). */
  runs: number;
  /** The differences are known from the artefact. */
  derivable: boolean;
}

/** The comparison kinds of a pair, from its facts alone. */
export function classifyComparison(f: PairFacts): ComparisonKind[] {
  const kinds: ComparisonKind[] = [];
  if (f.runs > 1) kinds.push('multi_seed');
  if (!f.sameDataset) kinds.push('cross_dataset');
  if (f.sameDataset && f.aligned) kinds.push('paired');
  else kinds.push('metrics_only');
  const confounded = f.sameDataset ? f.factors.length > 1 : f.factors.length > 0;
  if (!f.derivable || confounded) kinds.push('not_attributable');
  return kinds;
}

export interface ComparisonLabel {
  kinds: ComparisonKind[];
  /** The reference side, in words. */
  against: string;
  /** Cell key of the reference, when it is selectable. */
  referenceKey: string | null;
}

const uniqueFactors = (paths: string[], dropSeed: boolean): Factor[] =>
  [...new Set(paths.map(factorOf))].filter((f) => f !== 'dataset' && !(dropSeed && f === 'seed'));

/** The group a group's values are compared with: its regime baseline, or the study baseline for a regime baseline. */
export function referenceGroup(study: StudyResult, g: Aggregate): Aggregate | null {
  const b = study.baseline;
  const isRegimeBase = g.queue_model === b.queue_model && g.latency === b.latency && g.feed_offset_ns === b.feed_offset_ns;
  if (isRegimeBase) return g.regime === b.regime ? null : findGroup(study, b.regime, b.queue_model, b.latency, b.feed_offset_ns);
  return findGroup(study, g.regime, b.queue_model, b.latency, b.feed_offset_ns);
}

const groupDataset = (study: StudyResult, g: Aggregate): string | null => {
  const ids = new Set(g.members.map((id) => study.cells.find((c) => c.run_id === id)?.dataset.sha256 ?? '?'));
  return ids.size === 1 && !ids.has('?') ? [...ids][0] : null;
};

/** How a group's values compare with its reference group; `null` for the study baseline itself. */
export function labelGroup(study: StudyResult, g: Aggregate): ComparisonLabel | null {
  const ref = referenceGroup(study, g);
  const b = study.baseline;
  if (!ref) {
    if (g.regime === b.regime && g.queue_model === b.queue_model && g.latency === b.latency && g.feed_offset_ns === b.feed_offset_ns) return null;
    return {
      kinds: classifyComparison({ sameDataset: false, aligned: false, factors: [], runs: g.members.length, derivable: false }),
      against: 'no reference group in the artefact',
      referenceKey: null,
    };
  }
  const against = `${regimeLabel(ref.regime)} baseline (${queueLabel(ref.queue_model)}, ${latencyLabel(ref.latency)})`;
  const refRun = ref.members.length === 1 ? ref.members[0] : null;
  const ds = groupDataset(study, g);
  const refDs = groupDataset(study, ref);
  const sameDataset = ds !== null && ds === refDs;
  const members = g.members.map((id) => study.cells.find((c) => c.run_id === id));
  const comparisons = members.map((m) => m?.comparison ?? null);
  const pairedToRef = refRun !== null && comparisons.every((c) => c !== null && c.reference_run_id === refRun);
  const aligned = pairedToRef && comparisons.every((c) => c!.aligned && c!.like_for_like);
  const derivable = sameDataset ? pairedToRef : ds !== null && refDs !== null;
  const paths = sameDataset && pairedToRef ? comparisons.flatMap((c) => realDiffs(c!.config_diffs).map((d) => d.path)) : [];
  return {
    kinds: classifyComparison({ sameDataset, aligned, factors: uniqueFactors(paths, g.members.length > 1), runs: g.members.length, derivable }),
    against,
    referenceKey: groupCellKey(ref),
  };
}

/**
 * How one run compares with its artefact reference run; `null` for the study baseline and the
 * demo's reference run. A regime baseline (its own comparison reference) compares with the study
 * baseline run: cross-regime orders are not aligned, so that is never paired.
 */
export function labelRun(study: StudyResult, run: PublicRun): ComparisonLabel | null {
  const c = run.comparison;
  const anchors = study.demo_anchors.runs;
  const all: PublicRun[] = [...study.cells, ...anchors];
  if (!c || c.reference_run_id === run.run_id) {
    if (run.run_id === study.baseline.run_id || anchors.some((a) => a.run_id === run.run_id && a.reference_index === null)) return null;
    const base = study.cells.find((r) => r.run_id === study.baseline.run_id) ?? null;
    return {
      kinds: classifyComparison({
        sameDataset: base !== null && base.dataset.sha256 === run.dataset.sha256,
        aligned: false,
        factors: uniqueFactors(levelDiffs(study, run).map((d) => d.path), false),
        runs: 1,
        derivable: base !== null,
      }),
      against: `the study baseline run ${study.baseline.run_id}`,
      referenceKey: base ? runCellKey(base.run_id) : null,
    };
  }
  const ref = all.find((r) => r.run_id === c.reference_run_id) ?? null;
  const sameDataset = ref !== null && ref.dataset.sha256 === run.dataset.sha256;
  const anchorRef = anchors.find((a) => a.run_id === c.reference_run_id);
  const against = anchorRef ? `demo run #${anchorRef.index} (${anchorRef.label})` : ref ? `run ${ref.run_id}` : `run ${c.reference_run_id} (not in the artefact)`;
  return {
    kinds: classifyComparison({
      sameDataset,
      aligned: c.aligned && c.like_for_like,
      factors: uniqueFactors(realDiffs(c.config_diffs).map((d) => d.path), false),
      runs: 1,
      derivable: ref !== null,
    }),
    against,
    referenceKey: ref ? runCellKey(ref.run_id) : null,
  };
}

/** The comparison label of any selection. */
export function labelSelection(study: StudyResult, sel: Selection): ComparisonLabel | null {
  return sel.kind === 'group' ? labelGroup(study, sel.group) : labelRun(study, sel.run);
}
