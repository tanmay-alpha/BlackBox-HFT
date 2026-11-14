import { describe, expect, it } from 'vitest';

import { humanizeCaveat } from './gridModel';

describe('humanizeCaveat', () => {
  it('replaces known metric keys and keeps numbers and unknown words', () => {
    expect(humanizeCaveat('Selected by `net_pnl` over 12 runs; max_drawdown 0.5 and some_field kept.')).toBe(
      'Selected by Net P&L over 12 runs; Max drawdown 0.5 and some_field kept.',
    );
  });
});
