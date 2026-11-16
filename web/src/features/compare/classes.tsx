// Pair classes and eligibility reasons as the server defines them (bb_core::compare). Shared by
// Compare Pair and Compare Set so both pages read the same.

import type { Comparison } from '../../api/model';
import type { EligibilityEffect, PairClass } from '../../api/types';
import { ConfigValue } from '../../components/configValue';
import { pathLabel } from '../../components/labels';
import { Badge, Pill } from '../../components/ui';

const EFFECT_LABEL: Record<EligibilityEffect, string> = {
  no_alignment: 'No per-order alignment',
  not_like_for_like: 'Not like-for-like',
};

/** Display order: identical first, then increasingly significant differences. */
export const PAIR_CLASSES: readonly PairClass[] = [
  'identical',
  'same_fills_different_history',
  'same_fills_different_fees',
  'same_outcome_different_timing',
  'same_decision_different_outcome',
  'different_decision',
  'only_a',
  'only_b',
];

export const CLASS_LABEL: Record<PairClass, string> = {
  identical: 'Identical',
  same_fills_different_history: 'Same fills, different history',
  same_fills_different_fees: 'Same fills, different fees',
  same_outcome_different_timing: 'Same outcome, different timing',
  same_decision_different_outcome: 'Same decision, different outcome',
  different_decision: 'Different decision',
  only_a: 'Only in A',
  only_b: 'Only in B',
};

// Definitions only. What a class implies about the scenarios is left to the interpretation notes
// the backend returns with their evidence basis.
export const CLASS_MEANING: Record<PairClass, string> = {
  identical:
    'Same decision and decision time, exchange arrival, first response, every fill (time, price, quantity, maker/taker, fee), cancel and terminal times, final status and recorded lifecycle.',
  same_fills_different_history:
    'Decision, timing and every fill are identical, but the recorded lifecycle differs (modeled queue, queue updates, a rejected cancel or the market the strategy saw): identical fills under distinct causal histories.',
  same_fills_different_fees:
    'Decision, timing and fills (time, price, quantity, maker/taker) are identical; only the fees charged differ.',
  same_outcome_different_timing:
    'Same decision, outcome and fills (price, quantity, maker/taker), but at least one time differs: decision, exchange arrival, first response, a fill, cancel or completion.',
  same_decision_different_outcome:
    'The strategy made the same decision in both runs; the simulated execution differed (outcome, status, filled quantity, fill count, or a fill price, quantity or maker/taker side).',
  different_decision:
    'The strategy made a different decision (side, price, quantity, time in force or order type) at this tick and slot.',
  only_a: 'The decision exists only in scenario A.',
  only_b: 'The decision exists only in scenario B.',
};

/**
 * The comparison's v1 `status` (`aligned` | `metrics_only`, in words with the served value as the
 * title) and, when the runs differ in an eligibility field, every reason with its effect and
 * explanation verbatim.
 */
export function EligibilityNotice({ cmp }: { cmp: Comparison }) {
  const { eligibility } = cmp;
  return (
    <div className={eligibility.reasons.length === 0 ? 'cmp-eligible' : 'cmp-warning'} role="note" aria-label="Comparison eligibility">
      <p className="cmp-eligibility-head">
        <Pill tone={cmp.status === 'aligned' ? 'ok' : 'warn'} title={`Comparison status as returned by the server: ${cmp.status}`}>
          {cmp.status.replace(/_/g, ' ')}
        </Pill>{' '}
        <Pill tone={eligibility.like_for_like ? 'ok' : 'warn'} title="eligibility.like_for_like">
          {eligibility.like_for_like ? 'like-for-like' : 'not like-for-like'}
        </Pill>{' '}
        {cmp.status === 'aligned' ? (
          eligibility.reasons.length === 0 ? (
            <>Orders are aligned by decision and the metric deltas are like-for-like.</>
          ) : (
            <>Orders are aligned, but metric deltas and pair differences also reflect the fields below.</>
          )
        ) : (
          <>
            <Badge kind="warn">no per-order alignment</Badge> The same order id does not name the same decision in both
            runs, so no pair is compared. Metric deltas are shown but are not like-for-like.
          </>
        )}
      </p>
      {eligibility.reasons.length === 0 ? null : (
        <table className="table cmp-eligibility">
          <thead>
            <tr>
              <th scope="col">Field</th>
              <th scope="col">A</th>
              <th scope="col">B</th>
              <th scope="col">Effect</th>
              <th scope="col">Explanation</th>
            </tr>
          </thead>
          <tbody>
            {eligibility.reasons.map((r) => (
              <tr key={r.field}>
                <th scope="row" title={r.field}>
                  {pathLabel(r.field)}
                </th>
                <ConfigValue path={r.field} value={r.a} />
                <ConfigValue path={r.field} value={r.b} />
                <td title={r.effect}>{EFFECT_LABEL[r.effect] ?? r.effect}</td>
                <td>{r.explanation}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
