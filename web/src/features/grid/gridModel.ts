// Sweep form model: drafts, explicit unit conversion, validation mirroring `GridSpec::validate`
// in bb-core, and the result matrix's metric and colour definitions.

import type { SweepCell } from "../../api/model";
import type { GridAxis, QueueModelConfig } from "../../api/types";
import { bpsToFraction, fractionToBps } from "../../components/describe";
import { fmtDurationExact, fmtInt, fmtMoney, fmtPnl } from "../../components/fmt";
import type { GlossaryKey } from "../../components/glossary";
import { NsError, nsFromDecimal, nsToSafeNumber } from "../../components/ns";
import { queueModelLabel } from "../forensics/labels";

export type AxisParam = GridAxis["type"];
export type QueueVariant = QueueModelConfig["type"];
export type PowerVariant = "power_prob" | "power_prob2" | "power_prob3";

export interface AxisParamInfo {
  param: AxisParam;
  label: string;
  /** Unit the user types in; converted explicitly before submission. */
  input: "ms" | "bp" | "queue";
  help: string;
}

export const AXIS_PARAMS: readonly AxisParamInfo[] = [
  {
    param: "order_latency_ns",
    label: "Order latency (entry and response)",
    input: "ms",
    help: "Sets both the order-entry and the order-response latency to each value.",
  },
  {
    param: "entry_latency_ns",
    label: "Order entry latency only",
    input: "ms",
    help: "Sets only the order-entry latency; the response latency stays as in the base run.",
  },
  {
    param: "feed_latency_offset_ns",
    label: "Feed latency offset",
    input: "ms",
    help: "Added to every feed event's local receipt time. Zero keeps the dataset's own feed latency.",
  },
  {
    param: "queue_model",
    label: "Queue model",
    input: "queue",
    help: "The model that estimates the order's position in the queue on Market-By-Price data.",
  },
  {
    param: "maker_fee",
    label: "Maker fee",
    input: "bp",
    help: "Fee on passive fills as a fraction of traded value. Negative is a rebate.",
  },
  {
    param: "taker_fee",
    label: "Taker fee",
    input: "bp",
    help: "Fee on aggressive fills as a fraction of traded value.",
  },
];

export function axisInfo(param: AxisParam): AxisParamInfo {
  const info = AXIS_PARAMS.find((p) => p.param === param);
  if (!info) throw new Error(`unknown grid axis parameter: ${param}`);
  return info;
}

export const QUEUE_VARIANTS: readonly QueueVariant[] = [
  "risk_adverse",
  "log_prob",
  "log_prob2",
  "power_prob",
  "power_prob2",
  "power_prob3",
];

export function isPowerVariant(variant: QueueVariant): variant is PowerVariant {
  return variant === "power_prob" || variant === "power_prob2" || variant === "power_prob3";
}

export interface AxisDraft {
  param: AxisParam;
  /** Milliseconds for latency parameters, basis points for fees; comma or space separated. */
  text: string;
  queue: Record<QueueVariant, boolean>;
  /** Exponents `n` per power-law variant; comma or space separated, one model per exponent. */
  exponents: Record<PowerVariant, string>;
}

export function newDraft(param: AxisParam): AxisDraft {
  return {
    param,
    text: "",
    queue: {
      risk_adverse: false,
      log_prob: false,
      log_prob2: false,
      power_prob: false,
      power_prob2: false,
      power_prob3: false,
    },
    exponents: { power_prob: "", power_prob2: "", power_prob3: "" },
  };
}

export interface ParsedAxis {
  /** The axis built from every value that parsed; submitted only when there are no problems. */
  axis: GridAxis;
  /** One line per value showing the conversion that will be sent, e.g. `5 ms = 5000000 ns`. */
  conversions: string[];
  problems: string[];
}

const NUMBER_RE = /^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/i;

function tokens(text: string): string[] {
  return text.split(/[\s,;]+/).filter((t) => t !== "");
}

function parseNumbers(text: string, where: string, problems: string[]): Array<[string, number]> {
  const out: Array<[string, number]> = [];
  for (const token of tokens(text)) {
    const value = NUMBER_RE.test(token) ? Number(token) : NaN;
    if (Number.isFinite(value)) out.push([token, value]);
    else problems.push(`${where}: "${token}" is not a number`);
  }
  return out;
}

export function parseAxis(draft: AxisDraft, name: "X" | "Y"): ParsedAxis {
  const where = `${name} axis`;
  const problems: string[] = [];
  const conversions: string[] = [];

  if (draft.param === "queue_model") {
    const values: QueueModelConfig[] = [];
    for (const variant of QUEUE_VARIANTS) {
      if (!draft.queue[variant]) continue;
      if (!isPowerVariant(variant)) {
        values.push({ type: variant });
        continue;
      }
      const exponents = parseNumbers(draft.exponents[variant], `${where}, ${variant}`, problems);
      if (exponents.length === 0) {
        problems.push(`${where}: ${variant} needs at least one exponent n`);
      }
      for (const [, n] of exponents) {
        if (n > 0) values.push({ type: variant, n });
        else problems.push(`${where}: queue model exponent n must be finite and positive`);
      }
    }
    for (const value of values) conversions.push(queueModelLabel(value));
    return { axis: { type: "queue_model", values }, conversions, problems };
  }

  if (draft.param === "maker_fee" || draft.param === "taker_fee") {
    const values: number[] = [];
    for (const [token, bp] of parseNumbers(draft.text, where, problems)) {
      // Basis points to a fraction of traded value: 1 bp = 0.0001 (same conversion as the Lab).
      const fraction = bpsToFraction(bp);
      values.push(fraction);
      conversions.push(`${token} bp = ${fraction} of traded value`);
    }
    return { axis: { type: draft.param, values }, conversions, problems };
  }

  const values: number[] = [];
  for (const token of tokens(draft.text)) {
    // Milliseconds to nanoseconds with integer arithmetic on the typed decimal (1 ms =
    // 1,000,000 ns). The contract carries whole nanoseconds; finer digits are refused.
    let ns: number;
    try {
      ns = nsToSafeNumber(nsFromDecimal(token, "ms"));
    } catch (e) {
      if (!(e instanceof NsError)) throw e;
      problems.push(`${where}: ${e.message}`);
      continue;
    }
    if (ns < 0) {
      problems.push(
        draft.param === "feed_latency_offset_ns"
          ? `${where}: feed_latency_offset_ns must be non-negative`
          : `${where}: order latency must be non-negative`,
      );
    } else {
      values.push(ns);
      conversions.push(`${token} ms = ${fmtInt(ns)} ns`);
    }
  }
  return { axis: { type: draft.param, values }, conversions, problems };
}

/**
 * Problems that stop the sweep from being submitted; empty means it may be. `maxCells` is the
 * server's `limits.max_sweep_cells` (`MAX_GRID_CELLS` only while the status is unknown).
 */
export function validateGrid(x: ParsedAxis, y: ParsedAxis, maxCells: number): string[] {
  const problems = [...x.problems, ...y.problems];
  if (x.axis.values.length === 0 || y.axis.values.length === 0) {
    problems.push("both grid axes need at least one value");
  }
  if (x.axis.type === y.axis.type) {
    problems.push("grid axes must sweep different parameters");
  }
  const latencyPair = [x.axis.type, y.axis.type];
  if (latencyPair.includes("order_latency_ns") && latencyPair.includes("entry_latency_ns")) {
    problems.push("order_latency_ns and entry_latency_ns both set the entry latency");
  }
  const cells = x.axis.values.length * y.axis.values.length;
  if (cells > maxCells) {
    problems.push(`grid has ${cells} cells; the limit is ${maxCells}`);
  }
  return problems;
}

/** Readable label of each value of a submitted axis: exact durations and basis points, not raw ns. */
export function axisValueLabels(axis: GridAxis): string[] {
  switch (axis.type) {
    case "order_latency_ns":
    case "entry_latency_ns":
    case "feed_latency_offset_ns":
      return axis.values.map((ns) => fmtDurationExact(ns));
    case "queue_model":
      return axis.values.map((q) => queueModelLabel(q));
    case "maker_fee":
    case "taker_fee":
      return axis.values.map((f) => `${fractionToBps(f)} bp`);
    default:
      return [];
  }
}

// ---------------------------------------------------------------------------------------------
// Result matrix metrics and colour scale
// ---------------------------------------------------------------------------------------------

/** The per-cell metrics a sweep serves (API_CONTRACT §8.7); nothing else exists per cell. */
export type CellMetrics = NonNullable<SweepCell["metrics"]>;

export interface MetricDef {
  key: keyof CellMetrics;
  label: string;
  /** Unit of the printed values, for headers. */
  unit: string;
  /** Glossary entry explaining the metric. */
  term?: GlossaryKey;
  /** Signed metrics use a diverging scale centred on zero; the others a sequential one. */
  signed: boolean;
  format: (v: number) => string;
}

export const METRICS: readonly MetricDef[] = [
  { key: "net_pnl", label: "Net P&L", unit: "quote", signed: true, format: (v) => fmtPnl(v, 4) },
  { key: "fees", label: "Fees (negative is a rebate)", unit: "quote", term: "fees", signed: true, format: (v) => fmtPnl(v, 4) },
  { key: "fill_count", label: "Simulated fills", unit: "count", signed: false, format: (v) => fmtInt(v) },
  { key: "max_drawdown", label: "Max drawdown", unit: "quote", term: "drawdown", signed: false, format: (v) => fmtMoney(v, 4) },
];

export type Scale =
  | { kind: "diverging"; maxAbs: number }
  | { kind: "sequential"; lo: number; hi: number };

export function buildScale(def: MetricDef, values: readonly number[]): Scale | null {
  if (values.length === 0) return null;
  if (def.signed) {
    return { kind: "diverging", maxAbs: Math.max(...values.map((v) => Math.abs(v))) };
  }
  return { kind: "sequential", lo: Math.min(...values), hi: Math.max(...values) };
}

/** Strongest tint: kept below full saturation so cell text stays readable on a dark surface. */
const MAX_MIX = 62;

// Data ramp hues (a data encoding, not chrome; UI_DESIGN_SPEC.md §13). They encode sign or
// magnitude only, are deliberately not the side colours and do not mean good or bad.
const RAMP_POSITIVE = "#3f8fd0";
const RAMP_NEGATIVE = "#d08a3f";
const RAMP_SEQUENTIAL = "#3f8fd0";

/** Background for a value: a CSS colour mixing the scale hue into the neutral cell surface. */
export function scaleColor(scale: Scale, value: number): string {
  if (scale.kind === "diverging") {
    const t = scale.maxAbs > 0 ? Math.min(1, Math.abs(value) / scale.maxAbs) : 0;
    if (t === 0 || value === 0) return "var(--bg-2)";
    const hue = value > 0 ? RAMP_POSITIVE : RAMP_NEGATIVE;
    return `color-mix(in srgb, ${hue} ${Math.round(6 + t * (MAX_MIX - 6))}%, var(--bg-2))`;
  }
  const span = scale.hi - scale.lo;
  const t = span > 0 ? Math.min(1, Math.max(0, (value - scale.lo) / span)) : 0.5;
  return `color-mix(in srgb, ${RAMP_SEQUENTIAL} ${Math.round(6 + t * (MAX_MIX - 6))}%, var(--bg-2))`;
}

/** Evenly spaced values across the scale for the numeric legend. */
export function legendValues(scale: Scale): number[] {
  if (scale.kind === "diverging") {
    return [-1, -2 / 3, -1 / 3, 0, 1 / 3, 2 / 3, 1].map((t) => t * scale.maxAbs);
  }
  if (scale.hi <= scale.lo) return [scale.lo];
  return [0, 0.25, 0.5, 0.75, 1].map((t) => scale.lo + t * (scale.hi - scale.lo));
}
