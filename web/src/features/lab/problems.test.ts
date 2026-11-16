import { describe, expect, it } from 'vitest';

import type { RunConfig } from '../../api/types';
import { buildSections, startingConfig } from './fields';
import { collectProblems } from './problems';
import { validateRunConfig } from './validate';

const INSTRUMENT = { tick_size: 0.1, lot_size: 0.001, contract_size: 1 };

function withEntryNs(c: RunConfig, entry_ns: number): RunConfig {
  return { ...c, execution: { ...c.execution, order_latency: { type: 'constant', entry_ns, response_ns: 1_000_000 } } };
}

describe('collectProblems (PHASE3_REVIEW D3: one problem per field)', () => {
  it('finds nothing in the starting values', () => {
    const c = startingConfig('ds', INSTRUMENT);
    expect(collectProblems(buildSections(c, { instrument: INSTRUMENT, presets: [] }), c).count).toBe(0);
  });

  it('counts a negative entry latency once and attaches the server rule to the field', () => {
    const c = withEntryNs(startingConfig('ds', INSTRUMENT), -5_000_000);
    // The server's rules flag it too, in raw nanoseconds …
    expect(validateRunConfig(c).length).toBeGreaterThan(0);
    const p = collectProblems(buildSections(c, { instrument: INSTRUMENT, presets: [] }), c);
    // … but the Lab lists the field's own message only.
    expect(p.count).toBe(1);
    expect(p.fields).toHaveLength(1);
    expect(p.fields[0].fieldId).toBe('execution.order_latency.entry_ns');
    expect(p.fields[0].text).toMatch(/Entry latency: .*0 ms/);
    expect(p.rules).toEqual([]);
    expect(p.rawByField.get('execution.order_latency.entry_ns')?.[0]).toMatch(/ns/);
  });
});
