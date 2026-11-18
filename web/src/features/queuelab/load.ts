// Loader of the Queue Model Lab: downloads `/study-results/queue-shadow/study-result.json` and its
// `.sha256` with the study feature's `download`, verifies the SHA-256 (WebCrypto), then the
// contract `blackbox.queue-shadow` v1 (./contract.ts). Resolves `{ ok: true, study, sha256 }` or
// `{ ok: false, kind, message }`; it rejects only when `signal` aborts.

import { isAbort } from '../../api/client';
import { STUDY_FILE, StudyLoadError, type StudyErrorKind } from '../study/contract';
import { download } from '../study/load';
import { verifyQueueShadow, type QueueShadowResult } from './contract';

export const QUEUE_STUDY_ID = 'queue-shadow';

export type QueueLoadResult =
  | { ok: true; study: QueueShadowResult; /** Lowercase hex SHA-256 the bytes were verified against. */ sha256: string }
  | { ok: false; kind: StudyErrorKind; /** Meant to be displayed as is. */ message: string };

export async function loadQueueShadow(signal: AbortSignal): Promise<QueueLoadResult> {
  const url = `${import.meta.env.BASE_URL}study-results/${QUEUE_STUDY_ID}/${STUDY_FILE}`;
  try {
    const [body, sums] = await Promise.all([
      download(url, signal).then((r) => r.arrayBuffer()),
      download(`${url}.sha256`, signal).then((r) => r.text()),
    ]);
    const { study, sha256 } = await verifyQueueShadow(body, sums);
    return { ok: true, study, sha256 };
  } catch (e) {
    if (isAbort(e) || signal.aborted) throw e;
    if (e instanceof StudyLoadError) return { ok: false, kind: e.kind, message: e.message };
    return { ok: false, kind: 'unavailable', message: `Could not read ${url}: ${e instanceof Error ? e.message : String(e)}.` };
  }
}
