// Public loader API of the study feature; other features import only from this module.
//
//   loadStudyResult(id, signal): Promise<StudyLoadResult>
//     Downloads `/study-results/<id>/study-result.json` and its `.sha256` file, checks the SHA-256
//     first, then the contract (`blackbox.study-result` v1) and every field the viewer reads.
//     Resolves `{ ok: true, study, sha256 }` or `{ ok: false, kind, message }`; never throws for a
//     bad or missing study. It rejects only when `signal` aborts (the fetch's AbortError).
//
//   StudyFailureKind: 'unavailable' | 'hash_mismatch' | 'unsupported_contract' | 'invalid'
//   PUBLISHED_STUDIES: the studies shipped with the web build; `queue-shadow` has its own page
//   (the Queue Model Lab, `features/queuelab`, `/queue-lab`), to which `/studies/queue-shadow` redirects.
//   download(url, signal): the fetch every published-study loader uses (`unavailable` on failure).
//   studyHref(id, { cell?, metric? }): the viewer route, e.g. `/studies/exec-validation?cell=run.<run_id>`.
//   runCellKey(runId), groupCellKey(aggregate): the `?cell=` values (format in ./cellKey.ts).
//
// The shape of `StudyResult` is docs/research/EXECUTION_STUDY_VIEWER.md; it is synthetic and modelled.

import { isAbort } from '../../api/client';
import { STUDY_FILE, StudyLoadError, verifyStudy, type StudyErrorKind, type StudyResult } from './contract';

export type { StudyResult } from './contract';
export { groupCellKey, runCellKey } from './cellKey';

export type StudyFailureKind = StudyErrorKind;

export type StudyLoadResult =
  | { ok: true; study: StudyResult; /** Lowercase hex SHA-256 the bytes were verified against. */ sha256: string }
  | { ok: false; kind: StudyFailureKind; /** Meant to be displayed as is. */ message: string };

/** Studies published with the web build (`web/public/study-results/<id>/`). */
export const PUBLISHED_STUDIES = [
  {
    id: 'exec-validation',
    title: 'Execution model validation',
    summary:
      'How simulated touch-maker fills, inventory and P&L respond to the queue-position model, the order-latency distribution, cancel delay, feed latency and synthetic market regime.',
  },
  {
    id: 'queue-shadow',
    title: 'Queue model and fill uncertainty lab',
    summary:
      'Hypothetical shadow orders followed through a reconstructed L3 FIFO queue and through nine L2 queue-position models on the same synthetic messages: fill fractions with Wilson and Newcombe intervals, queue-ahead error and the pre-registered hypotheses.',
  },
] as const;

/** Downloads a published file; a network failure or HTTP error is `unavailable`, an abort rethrows. */
export async function download(url: string, signal: AbortSignal): Promise<Response> {
  let res: Response;
  try {
    res = await fetch(url, { signal, cache: 'no-cache' });
  } catch (e) {
    if (isAbort(e)) throw e;
    throw new StudyLoadError('unavailable', `Could not download ${url}: ${e instanceof Error ? e.message : String(e)}.`);
  }
  if (!res.ok) throw new StudyLoadError('unavailable', `${url} answered HTTP ${res.status}.`);
  return res;
}

export async function loadStudyResult(id: string, signal: AbortSignal): Promise<StudyLoadResult> {
  const url = `${import.meta.env.BASE_URL}study-results/${encodeURIComponent(id)}/${STUDY_FILE}`;
  try {
    const [body, sums] = await Promise.all([
      download(url, signal).then((r) => r.arrayBuffer()),
      download(`${url}.sha256`, signal).then((r) => r.text()),
    ]);
    const { study, sha256 } = await verifyStudy(body, sums);
    return { ok: true, study, sha256 };
  } catch (e) {
    if (isAbort(e) || signal.aborted) throw e;
    if (e instanceof StudyLoadError) return { ok: false, kind: e.kind, message: e.message };
    return { ok: false, kind: 'unavailable', message: `Could not read ${url}: ${e instanceof Error ? e.message : String(e)}.` };
  }
}

/** Route of a study in the viewer, optionally with a selected cell and metric. */
export function studyHref(id: string, opts: { cell?: string; metric?: string } = {}): string {
  const q = new URLSearchParams();
  if (opts.metric) q.set('metric', opts.metric);
  if (opts.cell) q.set('cell', opts.cell);
  const s = q.toString();
  return `/studies/${encodeURIComponent(id)}${s ? `?${s}` : ''}`;
}
