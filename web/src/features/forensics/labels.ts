// Wording and small formatters shared by the forensics, comparison and grid views. Definitions
// are taken from the bb-core type documentation so the UI never says more than the contract does.

import type { Basis, OrderOutcome, QueueModelConfig, Source } from "../../api/types";
import { fmtDuration } from "../../components/fmt";

export type BadgeKind =
  | "observed"
  | "modeled"
  | "derived"
  | "unobservable"
  | "engine"
  | "strategy"
  | "recorder"
  | "buy"
  | "sell"
  | "ok"
  | "warn"
  | "error"
  | "neutral";

export const EM_DASH = "\u2014";

export const BASES: readonly Basis[] = ["observed", "modeled", "derived", "unobservable"];

export const BASIS_MEANING: Record<Basis, string> = {
  observed: "Present in the historical market data.",
  modeled:
    "Output of the configured simulation model (queue position, latency, fills). An estimate.",
  derived: "Arithmetic over observed and modeled values.",
  unobservable: "Not knowable from the available data.",
};

export const SOURCES: readonly Source[] = ["engine", "strategy", "recorder", "derived"];

export const SOURCE_MEANING: Record<Source, string> = {
  engine:
    "Captured inside a callback of the pinned simulation engine. The values are the engine's own.",
  strategy: "Emitted by the strategy harness: decisions, intents and risk rulings.",
  recorder: "Recorder bookkeeping that is not an engine event, such as a truncation marker.",
  derived: "Computed after the run from recorded events and market data.",
};

/** Same text as `QueueModelConfig::label` in bb-core. */
export function queueModelLabel(q: QueueModelConfig): string {
  return "n" in q ? `${q.type}(n=${q.n})` : q.type;
}

/** What moves the estimate under each queue model, from the bb-core documentation. */
export function queueModelMeaning(q: QueueModelConfig): string {
  switch (q.type) {
    case "risk_adverse":
      return "The modeled queue ahead shrinks only when trades print at the order's price. Cancellations ahead are never credited.";
    case "log_prob":
      return "A decrease in displayed quantity is split between ahead and behind with p(behind) = f(back) / (f(back) + f(front)), f(x) = ln(1 + x).";
    case "log_prob2":
      return "A decrease in displayed quantity is split between ahead and behind with p(behind) = f(back) / f(back + front), f(x) = ln(1 + x).";
    case "power_prob":
      return "A decrease in displayed quantity is split between ahead and behind with p(behind) = f(back) / (f(back) + f(front)), f(x) = x^n.";
    case "power_prob2":
      return "A decrease in displayed quantity is split between ahead and behind with p(behind) = f(back) / f(back + front), f(x) = x^n.";
    case "power_prob3":
      return "A decrease in displayed quantity is split between ahead and behind with p(behind) = 1 - (front / (front + back))^n.";
    default:
      return "";
  }
}

export function words(snake: string): string {
  return snake.replace(/_/g, " ");
}

export function outcomeKind(outcome: OrderOutcome): BadgeKind {
  switch (outcome) {
    case "filled":
      return "ok";
    case "partially_filled":
      return "warn";
    case "risk_blocked":
    case "refused":
      return "error";
    default:
      return "neutral";
  }
}

export const OUTCOME_MEANING: Record<OrderOutcome, string> = {
  risk_blocked: "Stopped by a pre-trade risk rule; never sent.",
  refused: "The engine refused the request synchronously; never sent.",
  in_flight_at_end: "Sent, but the data ended before the simulated exchange processed it.",
  filled: "Filled completely in the simulation.",
  partially_filled:
    "Partly filled in the simulation, then cancelled, expired or still working at the end.",
  canceled: "Cancelled with no simulated fill.",
  expired:
    "Expired by the exchange model with no simulated fill (post-only would cross, or IOC/FOK found nothing).",
  working_at_end: "Still resting, unfilled, when the data ended.",
};

/** A duration with an explicit sign; `fmtDuration` is only ever given a magnitude. */
export function fmtSignedDuration(ns: number | null): string {
  if (ns === null) return EM_DASH;
  if (ns === 0) return fmtDuration(0);
  return `${ns > 0 ? "+" : "\u2212"}${fmtDuration(Math.abs(ns))}`;
}

/** Offset of `ts` from `origin` on the same relative-time scale, e.g. `+1.25 ms`. */
export function fmtSince(ts: number | null, origin: number): string {
  if (ts === null) return EM_DASH;
  const d = ts - origin;
  return d === 0 ? "+0" : fmtSignedDuration(d);
}
