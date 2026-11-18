// Pure derivations behind the Queue Model Lab (`/queue-lab`). Every number shown is an
// artefact value selected here, never recomputed: Wilson, Newcombe and Greenwood bounds, medians,
// shares and outcome flags are read from the verified `blackbox.queue-shadow` v1 result and only
// formatted. URL state (UI_DESIGN_SPEC.md §9) is parsed and validated against the artefact's
// own design lists here too.

import { DASH, fmtNum, fmtSigned } from '../../components/fmt';
import { REFERENCE_MODEL, type CheckpointRow, type HypothesisCell, type Interval, type ModelDef, type QueueShadowResult, type ResultCell, type Value } from './contract';

/** The pre-registered design hash (docs/research/QUEUE_SHADOW_EXPERIMENT.md §2). */
export const FROZEN_DESIGN_SHA256 = '0608a9ba4af5b7f03119277570999af47b8212b9285e18a8f1f73f240846f502';

const DOC = 'https://github.com/tanmay-alpha/BlackBox-HFT/blob/main/docs/research/QUEUE_SHADOW_EXPERIMENT.md';
/** The experiment document on GitHub; `section` is a heading anchor. */
export const docHref = (section?: '1-question-and-hypotheses' | '63-real-data-out-of-sample-opened-after-the-freeze-commit-2e091e3' | '7-hypothesis-outcomes' | '9-limitations') =>
  section ? `${DOC}#${section}` : DOC;

// ----- evidence classes ------------------------------------------------------------------------

export type EvidenceClass = 'reconstructed' | 'estimated' | 'decision' | 'counterfactual' | 'derived';

export const EVIDENCE_LABEL: Record<EvidenceClass, string> = {
  reconstructed: 'Reconstructed state',
  estimated: 'Estimated queue position',
  decision: 'Modelled decision',
  counterfactual: 'Counterfactual shadow fill',
  derived: 'Derived metric',
};

export const EVIDENCE_MEANING: Record<EvidenceClass, string> = {
  reconstructed: 'The L3 queue rebuilt from the synthetic ITCH messages; not an exchange-certified position, and not ground truth for a hypothetical order.',
  estimated: 'An L2 queue model’s front: shares it believes are ahead, from aggregated depth and trades only.',
  decision: 'A rule of the design: when a shadow arrives, at which price, which side, with latency L assumed.',
  counterfactual: 'Whether the hypothetical shadow would have been executed, assuming nobody reacted to it. Never a recorded execution.',
  derived: 'An interval, difference, mean or survival estimate computed over the shadows by the generating binary.',
};

// ----- labels and formatting -------------------------------------------------------------------

/** `0`, `50 µs`, `100 ms`, `1 s`: exact, in the largest unit that divides the value. */
export function fmtSpan(ns: number): string {
  if (ns === 0) return '0';
  if (ns % 1e9 === 0) return `${(ns / 1e9).toLocaleString('en-US')} s`;
  if (ns % 1e6 === 0) return `${ns / 1e6} ms`;
  if (ns % 1e3 === 0) return `${ns / 1e3} \u00b5s`;
  return `${ns} ns`;
}

/** The value of an available scalar, else `null`. */
export function num(v: Value): number | null {
  return v.status === 'available' ? v.value : null;
}

/** `[lower, upper]` when both bounds are available. */
export function bounds(i: Interval): [number, number] | null {
  const lo = num(i.lower);
  const hi = num(i.upper);
  return lo === null || hi === null ? null : [lo, hi];
}

/** Why a scalar is missing, as the artefact states it. */
export function reason(v: Value): string | null {
  return v.status === 'unavailable' ? v.reason : null;
}

/** A probability or share: three decimals, as the document's tables. */
export const fmtProb = (x: number | null) => (x === null ? DASH : fmtNum(x, 3));
/** A difference of probabilities: three decimals with an explicit sign. */
export const fmtDiff = (x: number | null) => (x === null ? DASH : fmtSigned(x, 3));
/** Shares (means over shadows): one decimal. */
export const fmtShares = (x: number | null) => (x === null ? DASH : fmtNum(x, 1));
/** Signed shares. */
export const fmtSignedShares = (x: number | null) => (x === null ? DASH : fmtSigned(x, 1));
/** A time in nanoseconds as milliseconds with one decimal. */
export const fmtMs = (ns: number | null) => (ns === null ? DASH : `${fmtNum(ns / 1e6, 1)} ms`);
/** A count with thousands separators. */
export const fmtCount = (x: number) => fmtNum(x, 0);

/** `[lo, hi]` of an interval formatted with `fmt`, or `null` when a bound is unavailable. */
export function intervalText(i: Interval, fmt: (x: number) => string): string | null {
  const b = bounds(i);
  return b ? `[${fmt(b[0])}, ${fmt(b[1])}]` : null;
}

/** `RiskAdverse`, `LogProb2`, `PowerProb3 n=2` (hftbacktest names, from the family and n). */
export function modelLabel(m: Pick<ModelDef, 'id' | 'family' | 'n'>): string {
  if (m.id === REFERENCE_MODEL) return 'L3 shadow (reconstructed)';
  const base = m.family
    ? m.family
        .split('_')
        .map((s) => (s ? s[0].toUpperCase() + s.slice(1) : s))
        .join('')
    : m.id;
  return m.n === null ? base : `${base} n=${m.n}`;
}

export function modelById(study: QueueShadowResult, id: string): ModelDef {
  const m = study.models.find((x) => x.id === id);
  if (!m) throw new Error(`unknown model ${id}`);
  return m;
}

export const labelOf = (study: QueueShadowResult, id: string) => modelLabel(modelById(study, id));

/** The L2 models, in artefact order (the reference excluded). */
export const l2Models = (study: QueueShadowResult) => study.models.filter((m) => m.id !== REFERENCE_MODEL);

// ----- URL state -------------------------------------------------------------------------------

export type Stage = 'overview' | 'fills' | 'queue' | 'hypotheses';
export const STAGES: { id: Stage; label: string }[] = [
  { id: 'overview', label: 'Overview' },
  { id: 'fills', label: 'Fills' },
  { id: 'queue', label: 'Queue' },
  { id: 'hypotheses', label: 'Hypotheses' },
];

/** Contract v1 pools both sides (even k buys, odd k sells); there is no per-side value. */
export type Side = 'pooled';

export interface LabState {
  symbol: string;
  /** The L2 model in focus. */
  model: string;
  side: Side;
  latency: number;
  tau: number;
  checkpoint: number;
  stage: Stage;
}

export interface ParsedLabState {
  state: LabState;
  /** One sentence per URL parameter that was not used, and why. */
  ignored: string[];
}

export type LabParam = 'symbol' | 'model' | 'side' | 'latency' | 'tau' | 'checkpoint' | 'tab';

const NS_TEXT = /^\d{1,15}$/;

/**
 * The lab's view parameters: `symbol`, `model` (an L2 model id), `side`, `latency`, `tau`,
 * `checkpoint` (nanoseconds as decimal strings, one of the design's values) and `tab` (phone
 * stage). A missing parameter takes the first design value; an unusable one is reported and
 * replaced by it, never guessed.
 */
export function parseLabState(params: URLSearchParams, study: QueueShadowResult): ParsedLabState {
  const ignored: string[] = [];
  const pick = <T,>(name: LabParam, allowed: readonly T[], parse: (raw: string) => T | null, describe: string): T => {
    const raw = params.get(name);
    if (raw === null) return allowed[0];
    const v = parse(raw);
    if (v !== null && allowed.includes(v)) return v;
    ignored.push(`${name}=${raw} is not ${describe}; showing ${String(allowed[0])}.`);
    return allowed[0];
  };
  const ns = (raw: string) => (NS_TEXT.test(raw) ? Number(raw) : null);
  const { design } = study;
  const symbol = pick('symbol', study.results.symbols.map((s) => s.symbol), (r) => r, 'a symbol of this study');
  const model = pick('model', l2Models(study).map((m) => m.id), (r) => r, 'an L2 model of this study');
  const latency = pick('latency', design.latencies_ns, ns, `a design latency (${design.latencies_ns.join(', ')} ns)`);
  const tau = pick('tau', design.horizons_ns, ns, `a design horizon (${design.horizons_ns.join(', ')} ns)`);
  const checkpoint = pick('checkpoint', design.checkpoints_ns, ns, `a design checkpoint (${design.checkpoints_ns.join(', ')} ns)`);
  const stage = pick('tab', STAGES.map((s) => s.id), (r) => r as Stage, 'a view tab');
  const side = params.get('side');
  if (side !== null && side !== 'pooled') {
    ignored.push(
      side === 'buy' || side === 'sell'
        ? `side=${side}: contract v1 pools buy and sell shadows (even k buys, odd k sells) and holds no per-side value; pooled values are shown.`
        : `side=${side} is not a side; pooled values are shown.`,
    );
  }
  return { state: { symbol, model, side: 'pooled', latency, tau, checkpoint, stage }, ignored };
}

/** The query of a lab view (`paths.queueLab`). Every parameter is explicit, so a shared link is exact. */
export function labQuery(s: LabState): Record<LabParam, string> {
  return {
    symbol: s.symbol,
    model: s.model,
    side: s.side,
    latency: String(s.latency),
    tau: String(s.tau),
    checkpoint: String(s.checkpoint),
    tab: s.stage,
  };
}

// ----- selections ------------------------------------------------------------------------------

export function cellAt(study: QueueShadowResult, symbol: string, latency: number, tau: number, model: string): ResultCell {
  const c = study.results.cells.find((x) => x.symbol === symbol && x.latency_ns === latency && x.horizon_ns === tau && x.model === model);
  // The contract check guarantees every combination exists exactly once.
  if (!c) throw new Error(`no cell ${symbol}/${latency}/${tau}/${model}`);
  return c;
}

/** Every path (reference first) at one symbol, latency and horizon. */
export function cellsAt(study: QueueShadowResult, symbol: string, latency: number, tau: number): ResultCell[] {
  return study.models.map((m) => cellAt(study, symbol, latency, tau, m.id));
}

export interface GridRow {
  latency: number;
  tau: number;
  reference: ResultCell;
  model: ResultCell;
}

/** The reference and one model at every (L, τ) of the design. */
export function gridRows(study: QueueShadowResult, symbol: string, model: string): GridRow[] {
  return study.design.latencies_ns.flatMap((latency) =>
    study.design.horizons_ns.map((tau) => ({
      latency,
      tau,
      reference: cellAt(study, symbol, latency, tau, REFERENCE_MODEL),
      model: cellAt(study, symbol, latency, tau, model),
    })),
  );
}

export function checkpointAt(study: QueueShadowResult, symbol: string, latency: number, checkpoint: number, model: string): CheckpointRow {
  const c = study.results.checkpoints.find((x) => x.symbol === symbol && x.latency_ns === latency && x.checkpoint_ns === checkpoint && x.model === model);
  if (!c) throw new Error(`no checkpoint ${symbol}/${latency}/${checkpoint}/${model}`);
  return c;
}

/** Every L2 model at one checkpoint. */
export function checkpointModels(study: QueueShadowResult, symbol: string, latency: number, checkpoint: number): CheckpointRow[] {
  return l2Models(study).map((m) => checkpointAt(study, symbol, latency, checkpoint, m.id));
}

/** One model at every checkpoint. */
export function checkpointSeries(study: QueueShadowResult, symbol: string, latency: number, model: string): CheckpointRow[] {
  return study.design.checkpoints_ns.map((c) => checkpointAt(study, symbol, latency, c, model));
}

export function volumeAt(study: QueueShadowResult, symbol: string, latency: number, tau: number) {
  const v = study.results.cancelled_volume.find((x) => x.symbol === symbol && x.latency_ns === latency && x.horizon_ns === tau);
  if (!v) throw new Error(`no cancelled volume ${symbol}/${latency}/${tau}`);
  return v;
}

// ----- hypotheses ------------------------------------------------------------------------------

export type Outcome = 'supported' | 'not_supported' | 'not_testable';

export const OUTCOME_TEXT: Record<Outcome, string> = {
  supported: 'supported',
  not_supported: 'not supported',
  not_testable: 'not testable',
};

export interface HypothesisRow {
  cell: HypothesisCell;
  /** Eligible shadows of the L3 reference in this cell. */
  eligible: number;
  testable: boolean;
  ha: Outcome;
  /** H-B part 1: the probability models closer to L3 than RiskAdverse (point estimates). */
  closer: string[];
  /** Probability models compared for H-B part 1 (every L2 model but RiskAdverse). */
  probabilityModels: number;
  /** H0 rejected: L2 models whose Newcombe interval excludes 0 (not in `h0_not_rejected`). */
  h0Rejected: string[];
}

const RISK_ADVERSE = 'risk_adverse';

export function hypothesisRows(study: QueueShadowResult): HypothesisRow[] {
  const l2 = l2Models(study).map((m) => m.id);
  return study.hypotheses.cells.map((cell) => {
    const eligible = cellAt(study, cell.symbol, cell.latency_ns, cell.horizon_ns, REFERENCE_MODEL).eligible;
    const testable = eligible > 0 && cell.h_a_supported !== null;
    return {
      cell,
      eligible,
      testable,
      ha: !testable ? 'not_testable' : cell.h_a_supported ? 'supported' : 'not_supported',
      closer: cell.closer_than_risk_adverse,
      probabilityModels: l2.filter((m) => m !== RISK_ADVERSE).length,
      h0Rejected: testable ? l2.filter((m) => !cell.h0_not_rejected.includes(m)) : [],
    };
  });
}

export interface HbPart2Row {
  latency: number;
  tau: number;
  models: string[];
  /** Some symbol has eligible shadows at this (L, τ). */
  testable: boolean;
  /** H-B part 2 holds when no model is within the interval on every symbol. */
  outcome: Outcome;
}

export function hbPart2Rows(study: QueueShadowResult, rows: HypothesisRow[] = hypothesisRows(study)): HbPart2Row[] {
  return study.hypotheses.models_within_interval_on_every_symbol.map((w) => {
    const testable = rows.some((r) => r.testable && r.cell.latency_ns === w.latency_ns && r.cell.horizon_ns === w.horizon_ns);
    return {
      latency: w.latency_ns,
      tau: w.horizon_ns,
      models: w.models,
      testable,
      outcome: !testable ? 'not_testable' : w.models.length === 0 ? 'supported' : 'not_supported',
    };
  });
}

export interface OutcomeSummary {
  cells: number;
  testable: number;
  haSupported: number;
  /** Model x cell comparisons with L3 in testable cells, and how many rejected H0. */
  h0Tested: number;
  h0Rejected: number;
  hb2Testable: number;
  hb2Holds: number;
  /** Testable cells where every probability model is closer than RiskAdverse / none is. */
  closerAll: number;
  closerNone: number;
}

export function outcomeSummary(study: QueueShadowResult): OutcomeSummary {
  const rows = hypothesisRows(study);
  const hb2 = hbPart2Rows(study, rows);
  const tested = rows.filter((r) => r.testable);
  const l2 = l2Models(study).length;
  return {
    cells: rows.length,
    testable: tested.length,
    haSupported: tested.filter((r) => r.ha === 'supported').length,
    h0Tested: tested.length * l2,
    h0Rejected: tested.reduce((n, r) => n + r.h0Rejected.length, 0),
    hb2Testable: hb2.filter((r) => r.testable).length,
    hb2Holds: hb2.filter((r) => r.outcome === 'supported').length,
    closerAll: tested.filter((r) => r.closer.length === r.probabilityModels).length,
    closerNone: tested.filter((r) => r.closer.length === 0).length,
  };
}

/** One paragraph stating the synthetic outcome, every count read from the artefact's flags. */
export function outcomeSentence(s: OutcomeSummary): string {
  const untestable = s.cells - s.testable;
  return [
    `${s.testable} of ${s.cells} symbol \u00d7 L \u00d7 \u03c4 cells have eligible shadows${untestable > 0 ? ` (${untestable} have none, so nothing is tested there)` : ''}.`,
    `H-A is nominally supported in ${s.haSupported} of ${s.testable}.`,
    `H0 is nominally rejected in ${s.h0Rejected} of ${s.h0Tested} model-versus-L3 comparisons (exploratory; unadjusted for multiplicity).`,
    `H-B part 1 (exploratory): every probability model is closer than RiskAdverse in ${s.closerAll} cells and none is in ${s.closerNone}.`,
    `H-B part 2 nominally holds in ${s.hb2Holds} of ${s.hb2Testable} testable (L, \u03c4) pairs.`,
  ].join(' ');
}
