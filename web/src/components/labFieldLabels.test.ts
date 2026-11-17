import { describe, expect, it } from 'vitest';

import type { RunConfig } from '../api/types';
import { LATENCY_MODELS, buildSections, startingConfig, strategyKindDefaults, type Field, type Section } from '../features/lab/fields';
import { STRATEGY_INFO } from './describe';
import { LAB_FIELD_LABELS, LATENCY_MODEL_LABELS } from './labFieldLabels';

function selectField(sections: Section[], id: string): Extract<Field<RunConfig>, { kind: 'select' }> | null {
  for (const s of sections) for (const f of s.fields) if (f.id === id && f.kind === 'select') return f;
  return null;
}

/** Every field label the Lab can show: each strategy kind, each latency model (cancel leg on), a queue model with n. */
function labFieldLabels(): Record<string, string> {
  const map = new Map<string, string>();
  const ctx = { instrument: null, presets: [] };
  const base = startingConfig('', null);
  for (const type of Object.keys(STRATEGY_INFO) as (keyof typeof STRATEGY_INFO)[]) {
    let c: RunConfig = { ...base, strategy: { ...base.strategy, kind: strategyKindDefaults(type, null, []) } };
    c = selectField(buildSections(c, ctx), 'execution.queue_model.type')?.set(c, 'power_prob') ?? c;
    for (const model of LATENCY_MODELS) {
      let cfg = selectField(buildSections(c, ctx), 'execution.order_latency.type')?.set(c, model.value) ?? c;
      cfg = selectField(buildSections(cfg, ctx), 'execution.order_latency.cancel')?.set(cfg, 'own') ?? cfg;
      for (const s of buildSections(cfg, ctx)) for (const f of s.fields) if (!map.has(f.id)) map.set(f.id, f.label);
    }
  }
  return Object.fromEntries(map);
}

describe('static Lab label maps', () => {
  it('match the Lab form definitions (regenerate labFieldLabels.ts when this fails)', () => {
    expect(LAB_FIELD_LABELS).toEqual(labFieldLabels());
    expect(LATENCY_MODEL_LABELS).toEqual(Object.fromEntries(LATENCY_MODELS.map((m) => [m.value, m.label])));
  });
});
