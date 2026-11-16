import { describe, expect, it } from 'vitest';

import type { RunConfig } from '../../api/types';
import { buildSections, startingConfig } from './fields';
import { assumptionChips, assumptionRows, changeDelta, changeValue, presetChips, presetShortTitle, strategyRole } from './review';

const INSTRUMENT = { tick_size: 0.1, lot_size: 0.001, contract_size: 1 };
const base = startingConfig('ds', INSTRUMENT);

function latency(c: RunConfig, ms: number): RunConfig {
  return { ...c, execution: { ...c.execution, order_latency: { type: 'constant', entry_ns: ms * 1e6, response_ns: ms * 1e6 } } };
}

describe('preset cards', () => {
  it('drops the strategy prefix the tag already shows', () => {
    expect(presetShortTitle('Touch maker: slower line', 'touch_maker')).toBe('Slower line');
    expect(presetShortTitle('Avellaneda-Stoikov market maker', 'avellaneda_stoikov')).toBe('Avellaneda-Stoikov market maker');
    expect(presetShortTitle('Forensics probe: five scripted orders', 'scripted')).toBe('Forensics probe: five scripted orders');
  });

  it('tags strategy families', () => {
    expect(strategyRole('touch_maker')).toBe('maker');
    expect(strategyRole('imbalance_taker')).toBe('taker');
    expect(strategyRole('scripted')).toBe('scripted');
    expect(strategyRole('future_kind')).toBeNull();
  });

  it('words three key assumptions compactly', () => {
    expect(assumptionChips(base).map((c) => c.text)).toEqual(['1 ms latency', 'Risk-averse queue', 'Fees 0 / 0 bp']);
    const rebate = { ...base, execution: { ...base.execution, fees: { maker: -0.0001, taker: 0.0004 } } };
    expect(assumptionChips(rebate)[2].text).toBe('Maker rebate 1 bp, taker 4 bp');
  });

  it('marks the assumption a variant changes against the value most presets share', () => {
    const chips = presetChips([base, base, latency(base, 50)]);
    expect(chips[0].some((c) => c.differs)).toBe(false);
    expect(chips[2].find((c) => c.key === 'latency')).toEqual({ key: 'latency', text: '50 ms latency', differs: true });
    expect(chips[2].find((c) => c.key === 'queue')?.differs).toBe(false);
    // Too few presets to call anything the usual value.
    expect(presetChips([base, latency(base, 50)]).flat().some((c) => c.differs)).toBe(false);
  });
});

describe('review', () => {
  const fields = new Map(buildSections(base, { instrument: INSTRUMENT, presets: [] }).flatMap((s) => s.fields.map((f) => [f.id, f] as const)));
  const entry = fields.get('execution.order_latency.entry_ns');

  it('shows a change in form units with its signed difference', () => {
    expect(changeValue(entry, '1000000')).toBe('1 ms');
    expect(changeValue(entry, '7000000')).toBe('7 ms');
    expect(changeDelta(entry, '1000000', '7000000')).toBe('+6 ms');
    expect(changeDelta(entry, '7000000', '1000000')).toBe('\u22126 ms');
    expect(changeDelta(fields.get('execution.order_latency.type'), '"constant"', '"jitter"')).toBeNull();
  });

  it('lists the modeled assumptions and flags the changed ones', () => {
    const rows = assumptionRows(latency(base, 7), base);
    expect(rows.map((r) => r.key)).toEqual(['queue', 'latency', 'fill', 'fees']);
    expect(rows.find((r) => r.key === 'latency')).toMatchObject({ value: '7.00 ms / 7.00 ms', changed: true });
    expect(rows.filter((r) => r.changed).map((r) => r.key)).toEqual(['latency']);
    expect(assumptionRows(base, null).some((r) => r.changed)).toBe(false);
  });
});
