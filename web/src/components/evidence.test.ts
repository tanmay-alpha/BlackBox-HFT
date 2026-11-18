import { describe, expect, it } from 'vitest';

import { replayPhrase } from './evidence';

describe('replayPhrase', () => {
  it('names recorded data only for recorded datasets and never calls synthetic data historical', () => {
    expect(replayPhrase('recorded')).toBe('a replay of recorded market data');
    expect(replayPhrase('synthetic')).toBe('a replay of a synthetic dataset');
    expect(replayPhrase(null)).toBe("a replay of the dataset's events");
    for (const k of ['recorded', 'synthetic', null] as const) expect(replayPhrase(k)).not.toMatch(/historical|observed/i);
  });
});
