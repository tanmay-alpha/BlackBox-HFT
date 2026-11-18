// Links from a study run to a stored public demo run (E3). A study run links only when the API
// lists a demo run with the same run id (the first 16 hex characters of the config SHA-256, which
// covers the run contract, engine, dataset hash and the complete configuration) and that stored
// run's dataset events SHA-256 equals the study run's. Labels, positions or dataset ids are never
// used to guess a match; everything else gets the local reproduction command.

import { paths } from '../../shell/paths';
import { STUDY_FILE, type PublicRun, type StudyResult } from './contract';

/** A demo run as `GET /demos` lists it. */
export interface ListedDemoRun {
  demoId: string;
  label: string;
  runId: string;
}

/** The facts of a stored run that a match checks (`GET /runs/{id}`). */
export interface StoredRunFacts {
  run_id: string;
  result_sha256: string;
  dataset: { id: string; events_sha256: string };
}

export type NotStoredReason =
  /** No demo lists a run with this config hash. */
  | 'no_stored_run'
  /** A demo lists the run id, but its stored dataset hash differs. */
  | 'dataset_mismatch'
  /** The stored public runs could not be read (offline API, error). */
  | 'unchecked';

export type RunLink =
  | {
      kind: 'stored';
      runId: string;
      demoId: string;
      demoLabel: string;
      /** The stored result hash equals the study's (same platform class); else only the inputs match. */
      resultMatches: boolean;
    }
  | { kind: 'not_stored'; reason: NotStoredReason };

/** Every run id of `demos` (each `GET /demos` entry's simulated runs). */
export function listedDemoRuns(demos: readonly { id: string; runs: readonly { label: string; run_id?: string | null }[] }[]): ListedDemoRun[] {
  const out: ListedDemoRun[] = [];
  for (const d of demos) {
    for (const r of d.runs) {
      if (typeof r.run_id === 'string' && r.run_id !== '') out.push({ demoId: d.id, label: r.label, runId: r.run_id });
    }
  }
  return out;
}

/** The study runs whose config hash a listed demo run carries: the only runs worth fetching. */
export function candidateRunIds(runs: readonly PublicRun[], listed: readonly ListedDemoRun[]): string[] {
  const ids = new Set(listed.map((l) => l.runId));
  return [...new Set(runs.filter((r) => ids.has(r.run_id) && r.config_sha256.startsWith(r.run_id)).map((r) => r.run_id))];
}

/**
 * Whether `run` is a stored public run. `listed` is `null` when the demo list could not be read;
 * `stored` holds the fetched facts by run id (a missing entry means the fetch failed).
 */
export function matchRun(run: PublicRun, listed: readonly ListedDemoRun[] | null, stored: ReadonlyMap<string, StoredRunFacts>): RunLink {
  if (listed === null) return { kind: 'not_stored', reason: 'unchecked' };
  const entry = listed.find((l) => l.runId === run.run_id && run.config_sha256.startsWith(l.runId));
  if (!entry) return { kind: 'not_stored', reason: 'no_stored_run' };
  const facts = stored.get(entry.runId);
  if (!facts || facts.run_id !== run.run_id) return { kind: 'not_stored', reason: 'unchecked' };
  if (facts.dataset.events_sha256 !== run.dataset.sha256) return { kind: 'not_stored', reason: 'dataset_mismatch' };
  return { kind: 'stored', runId: entry.runId, demoId: entry.demoId, demoLabel: entry.label, resultMatches: facts.result_sha256 === run.result_sha256 };
}

/** The evidence chain of a stored run, in reading order; a step is `null` when it does not apply. */
export interface RunChain {
  run: string;
  comparison: string | null;
  firstDivergence: string | null;
  forensics: string | null;
  report: string;
}

/**
 * Route links for a stored run. The comparison needs the reference run stored too; the first
 * divergence and its order forensics need an aligned comparison with a divergence.
 */
export function runChain(run: PublicRun, link: RunLink, linkOf: (runId: string) => RunLink): RunChain | null {
  if (link.kind !== 'stored') return null;
  const c = run.comparison;
  const reference = c ? linkOf(c.reference_run_id) : null;
  const comparison = c && reference?.kind === 'stored' ? paths.comparePair(c.reference_run_id, link.runId) : null;
  const fd = comparison !== null && c?.aligned ? c.first_divergence : null;
  return {
    run: paths.run(link.runId),
    comparison,
    firstDivergence: fd ? paths.bookRun(link.runId, { t: String(fd.decision_ts_ns), order: fd.order_id }) : null,
    forensics: fd ? paths.order(link.runId, fd.order_id) : null,
    report: paths.report(link.runId),
  };
}

/** Local data directory the reproduction command writes to. */
export function studyOutDir(study: StudyResult): string {
  return `data/studies/${study.study_id}`;
}

/**
 * Shell lines that reproduce a run locally: the study command (it regenerates every synthetic
 * dataset, so no download is needed), where the run's summary lands, and the hashes to compare.
 */
export function reproduceCommand(study: StudyResult, run: PublicRun, where: { detail: string } | { anchorIndex: number }): string {
  const out = studyOutDir(study);
  const read =
    'detail' in where
      ? `# full cell (manifest, queue estimates, fill-time curves): ${out}/${where.detail}`
      : `# summary: demo_anchors.runs[${where.anchorIndex}] in ${out}/public/${STUDY_FILE}`;
  return [
    study.generator.command.replace('<dir>', out),
    read,
    `# expect run ${run.run_id}, config_sha256 ${run.config_sha256}`,
    `# and result_sha256 ${run.result_sha256} on platform class ${study.generator.platform}`,
  ].join('\n');
}
