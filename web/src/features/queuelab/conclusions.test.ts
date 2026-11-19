import { expect, it } from 'vitest';

import experiment from '../../../../docs/research/QUEUE_SHADOW_EXPERIMENT.md?raw';
import page from './QueueLabPage.tsx?raw';

it('quotes all three clean acquisitions rather than the superseded single-day verdict', () => {
  expect(page).not.toContain('Only 2019-10-30');
  expect(page).toContain('2019-01-30 and 2020-01-30');
  expect(page).toContain('do not replicate uniformly');
  const rows = experiment.split('\n').filter((line) => /^\| OOS-[ACD]:/.test(line));
  expect(rows).toHaveLength(3);
  const columns = rows.map((line) => line.split('|').map((part) => part.trim()));
  expect(columns.map((row) => row[4])).toEqual(['0 / 27', '11 / 27', '3 / 27']);
  expect(columns.map((row) => row[7])).toEqual(['9 / 243', '36 / 243', '3 / 243']);
  expect(columns.map((row) => Number(row[8]).toFixed(4)).join(' / ')).toBe('0.0167 / 0.0243 / 0.0258');
  for (const summary of ['9 / 36 / 3 of 243', '0 / 11 / 3 of 27', '0.0167 / 0.0243 / 0.0258']) {
    expect(page).toContain(summary);
  }
});
