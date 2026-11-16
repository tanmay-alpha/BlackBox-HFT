// Validation problems as the Lab lists them: one entry per invalid field (its own message, in
// display units), plus the server rules that no field already covers. A rule about a field that
// shows its own problem is the same mistake, so it is attached to that field instead of counted
// twice (PHASE3_REVIEW.md D3).

import type { RunConfig } from '../../api/types';
import { fieldProblem } from './fields';
import type { Section } from './fields';
import { ruleFieldPaths, validateRunConfig } from './validate';

export interface FieldProblem {
  sectionId: string;
  fieldId: string;
  /** `Section › Field: message`, in the form's units. */
  text: string;
}

export interface Problems {
  fields: FieldProblem[];
  /** Server rules about no flagged field, verbatim. */
  rules: string[];
  /** Server rules about a flagged field, verbatim, keyed by that field. */
  rawByField: Map<string, string[]>;
  /** What blocks the run: one per field, plus the uncovered rules. */
  count: number;
}

/** Every field's own problem, in section order. */
export function fieldProblems(sections: Section[], config: RunConfig): FieldProblem[] {
  const out: FieldProblem[] = [];
  for (const s of sections) {
    for (const f of s.fields) {
      const p = fieldProblem(f, config);
      if (p) out.push({ sectionId: s.id, fieldId: f.id, text: `${s.title} \u203a ${f.label}: ${p}` });
    }
  }
  return out;
}

/** Field problems merged with `validateRunConfig`'s rules, one problem per field. */
export function collectProblems(sections: Section[], config: RunConfig): Problems {
  const fields = fieldProblems(sections, config);
  const flagged = new Set(fields.map((p) => p.fieldId));
  const rules: string[] = [];
  const rawByField = new Map<string, string[]>();
  for (const text of validateRunConfig(config)) {
    const hit = ruleFieldPaths(text).filter((id) => flagged.has(id));
    if (hit.length === 0) rules.push(text);
    for (const id of hit) rawByField.set(id, [...(rawByField.get(id) ?? []), text]);
  }
  return { fields, rules, rawByField, count: fields.length + rules.length };
}
