// The `?cell=` key of the study viewer: which run or aggregate group the detail panel shows.
//
//   run.<run_id>                                            one run (a study cell or a demo anchor run)
//   group.<regime>.<queue_model>.<latency>.<feed_offset_ns> one aggregate group (all latency seeds)
//
// Names are the study's own level names (`[A-Za-z0-9_-]`), so the key needs no escaping in a URL.
// A key that does not parse, or names nothing in the study, selects nothing.

export type CellKey =
  | { kind: 'run'; runId: string }
  | { kind: 'group'; regime: string; queueModel: string; latency: string; feedOffsetNs: number };

const TOKEN = /^[A-Za-z0-9_-]+$/;
const RUN_ID = /^[0-9a-f]{16}$/;
const NS = /^(0|[1-9][0-9]{0,18})$/;

export function runCellKey(runId: string): string {
  return `run.${runId}`;
}

export function groupCellKey(g: { regime: string; queue_model: string; latency: string; feed_offset_ns: number }): string {
  return `group.${g.regime}.${g.queue_model}.${g.latency}.${g.feed_offset_ns}`;
}

/** Parses a `?cell=` value; `null` for anything that is not exactly one of the two forms. */
export function parseCellKey(raw: string | null): CellKey | null {
  if (raw === null) return null;
  const parts = raw.split('.');
  if (parts[0] === 'run' && parts.length === 2 && RUN_ID.test(parts[1])) return { kind: 'run', runId: parts[1] };
  if (parts[0] === 'group' && parts.length === 5) {
    const [, regime, queueModel, latency, ns] = parts;
    if (![regime, queueModel, latency].every((t) => TOKEN.test(t)) || !NS.test(ns)) return null;
    const feedOffsetNs = Number(ns);
    if (!Number.isSafeInteger(feedOffsetNs)) return null;
    return { kind: 'group', regime, queueModel, latency, feedOffsetNs };
  }
  return null;
}
