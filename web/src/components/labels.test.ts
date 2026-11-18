import { describe, expect, it } from 'vitest';

import { configValueText, controlledItem, pathLabel, runLabelText, unitLabel, valueLabel } from './labels';

describe('pathLabel', () => {
  it('names the latency legs on their own', () => {
    expect(pathLabel('execution.order_latency.entry_ns')).toBe('Order latency, strategy to exchange');
    expect(pathLabel('execution.order_latency.response_ns')).toBe('Response latency, exchange to strategy');
  });

  it('takes the Strategy Lab labels, including fields of other strategies and latency models', () => {
    expect(pathLabel('strategy.interval_ns')).toBe('Evaluation interval');
    expect(pathLabel('execution.fees.maker')).toBe('Maker fee');
    expect(pathLabel('strategy.kind.gamma')).toBe('Risk aversion gamma');
    expect(pathLabel('execution.order_latency.entry.median_ns')).toBe('Entry median');
    expect(pathLabel('execution.queue_model.type')).toBe('Queue-position model');
  });

  it('falls back to the last segment in words', () => {
    expect(pathLabel('foo.some_new_field_ns')).toBe('Some new field');
  });
});

describe('values and units', () => {
  it('reads enum values and units as words', () => {
    expect(valueLabel('no_partial_fill')).toBe('No partial fills');
    expect(valueLabel('constant')).toBe('Constant');
    expect(unitLabel('quote_currency')).toBe('quote currency');
    expect(unitLabel('bps_of_turnover')).toBe('bp of turnover');
    expect(unitLabel('base_units')).toBe('base units');
  });

  it('formats durations and fee fractions without changing the stored value', () => {
    expect(configValueText('execution.order_latency.entry_ns', 1_000_000)).toBe('1 ms');
    expect(configValueText('execution.fees.maker', 0.0002)).toBe('0.0002 (2 bp)');
    expect(configValueText('risk.max_drawdown', null)).toBe('none');
  });
});

describe('configValueText (the single configuration formatter)', () => {
  it('lists a heavy-tail latency leg as key/value text, min first', () => {
    expect(configValueText('execution.order_latency.entry', { clip: 0.1, jitter: 0.5, min_ns: '768920', unit: 10 })).toBe(
      'min 768.92 \u00b5s, jitter 0.5, clip 0.1, unit 10',
    );
  });

  it('reads a log-normal leg with its median as a duration', () => {
    expect(configValueText('execution.order_latency.entry', { median_ns: '20000000', sigma: 0 })).toBe('median 20 ms, sigma 0');
  });

  it('shows null, empty objects, booleans and enums as words', () => {
    expect(configValueText('execution.order_latency.entry', null)).toBe('none');
    expect(configValueText('x', undefined)).toBe('none');
    expect(configValueText('x', {})).toBe('none');
    expect(configValueText('x', [])).toBe('none');
    expect(configValueText('risk.flatten', true)).toBe('yes');
    expect(configValueText('execution.fill_model', 'no_partial_fill')).toBe(valueLabel('no_partial_fill'));
  });

  it('nests objects in parentheses and joins arrays', () => {
    expect(configValueText('a', { inner: { min_ns: 1000 }, k: 2 })).toBe('inner (min 1 \u00b5s), k 2');
    expect(configValueText('a_ns', [1000, 2000])).toBe('1 \u00b5s, 2 \u00b5s');
  });
});

describe('controlledItem', () => {
  it('lists a JSON object by field name under the section that names its keys', () => {
    const c = controlledItem('Risk limits: {"max_drawdown":null,"max_open_orders":8,"max_order_qty":0.05,"max_position":0.1}');
    expect(c.label).toBe('Risk limits');
    expect(c.value).toBe('Max drawdown = none; Max open orders = 8; Max order quantity = 0.05; Max position = 0.1');
  });

  it('reads nanoseconds as durations and enum strings as words', () => {
    expect(controlledItem('Order entry latency in nanoseconds: 1000000')).toMatchObject({ label: 'Order entry latency', value: '1 ms' });
    expect(controlledItem('Exchange fill model: "no_partial_fill"').value).toBe('No partial fills');
  });

  it('passes through lines that are not JSON', () => {
    const raw = 'Dataset id: synthetic-demo; events SHA-256: 0bc2';
    expect(controlledItem(raw)).toEqual({ label: 'Dataset id', value: 'synthetic-demo; events SHA-256: 0bc2', raw });
  });
});

describe('runLabelText', () => {
  it('replaces a demo-id prefix with the experiment title', () => {
    const titles = new Map([['cost_of_latency', 'Cost of latency']]);
    expect(runLabelText('cost_of_latency: 1 ms entry and response latency', titles)).toBe('Cost of latency: 1 ms entry and response latency');
    expect(runLabelText('gross_vs_net: Zero fees', new Map())).toBe('Gross vs net: Zero fees');
    expect(runLabelText('my run: 2', titles)).toBe('my run: 2');
  });
});
