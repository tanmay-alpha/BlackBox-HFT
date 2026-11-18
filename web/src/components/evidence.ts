// Source-data wording conditioned on the dataset kind the API declares
// (docs/engineering/EVIDENCE_TERMINOLOGY.md). Synthetic events are never "historical" or
// "observed market data"; when the page does not know the kind, the neutral wording is used.

import type { DatasetKind } from '../api/types';

/** What a simulation replays (`a replay of recorded market data`, `a replay of a synthetic dataset`). */
export function replayPhrase(kind: DatasetKind | null | undefined): string {
  if (kind === 'recorded') return 'a replay of recorded market data';
  if (kind === 'synthetic') return 'a replay of a synthetic dataset';
  return "a replay of the dataset's events";
}
