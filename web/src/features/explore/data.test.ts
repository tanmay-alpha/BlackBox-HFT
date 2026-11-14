import { describe, expect, it } from 'vitest';

import { answerSummary, fmtSec } from './data';
import type { LatencyStory } from './story';

/** A story with only what `answerSummary` reads: the two runs' stored P&L metrics and latencies. */
function story(fast: { realized: number; unrealized: number; fees: number; net?: number }, slow: typeof fast): LatencyStory {
  const scenario = (ns: number, m: typeof fast) => ({
    runId: `run-${ns}`,
    label: '',
    entryNs: ns,
    responseNs: ns,
    run: { metrics: { realized_pnl: m.realized, unrealized_pnl: m.unrealized, fees: m.fees, net_pnl: m.net ?? m.realized + m.unrealized - m.fees } },
  });
  // Only the fields above are read; the rest of the story is not built for this test.
  const partial = { fast: scenario(1_000_000, fast), slow: scenario(50_000_000, slow) } as unknown as LatencyStory;
  return partial;
}

describe('answerSummary', () => {
  it('calls a smaller loss a smaller loss and names the dominant part of the gap', () => {
    const s = answerSummary(story({ realized: -0.2485, unrealized: 0, fees: 74.461 }, { realized: -0.1368, unrealized: 0.0013, fees: 44.701 }));
    expect(s.headline).toBe('Both scenarios lose money. At 1 ms the strategy lost 74.71; at 50 ms it lost 44.84 (net after fees, quote currency).');
    expect(s.gap).toMatch(/^50 ms loses 29\.87 less; 99\.\d\d% of that is the difference in fees\.$/);
    expect(s.gap).not.toMatch(/profit|gain/i);
    expect(s.identityHolds).toBe(true);
  });

  it('gives no split when the stored net does not satisfy the identity', () => {
    const s = answerSummary(story({ realized: 1, unrealized: 0, fees: 2 }, { realized: 1, unrealized: 0, fees: 1, net: 5 }));
    expect(s.identityHolds).toBe(false);
    expect(s.gap).toBeNull();
    expect(s.split).toBeNull();
  });
});

describe('fmtSec', () => {
  it('prints plain seconds after the dataset start', () => {
    expect(fmtSec(100_000_000, 6)).toBe('0.100000 s');
    expect(fmtSec(108_869_266)).toBe('0.1089 s');
    expect(fmtSec(null)).toBe('\u2014');
  });
});
