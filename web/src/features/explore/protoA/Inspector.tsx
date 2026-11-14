// One order's lifecycle in both scenarios side by side, from each run's stored order record and
// flight-recorder events, with links to the full forensics and the stored comparison.

import { Link } from 'react-router-dom';

import type { OrderDetail } from '../../../api/model';
import type { InstrumentMeta, OrderPair, OrderRecord } from '../../../api/types';
import { fmtDuration, fmtPrice, fmtQty } from '../../../components/fmt';
import { ErrorBox, Spinner, useAsync } from '../../../components/ui';
import { eventTitle } from '../../forensics/eventFormat';
import { paths } from '../../../shell/paths';
import type { LatencyStory } from '../story';
import { differenceWords, fmtSec, loadLifecycles, outcomeWords, pairVerdict, type Which } from './data';

interface Stage {
  label: string;
  clock: string;
  ts: (o: OrderRecord) => number | null;
}

const STAGES: Stage[] = [
  { label: 'Strategy decides', clock: 'strategy clock', ts: (o) => o.decision_ts },
  { label: 'Order arrives at the exchange', clock: 'exchange clock', ts: (o) => o.exch_arrival_ts },
  { label: 'Exchange response reaches the strategy', clock: 'strategy clock', ts: (o) => o.first_response_ts },
  { label: 'First fill', clock: 'exchange clock', ts: (o) => o.first_fill_ts },
  { label: 'Cancel sent', clock: 'strategy clock', ts: (o) => o.cancel_sent_ts },
  { label: 'Order finished', clock: 'exchange clock', ts: (o) => o.terminal_ts },
];

function Cell({ o, stage, sinceDecision }: { o: OrderRecord | null; stage: Stage; sinceDecision: boolean }) {
  if (!o) return <td className="pa-muted">not placed</td>;
  const t = stage.ts(o);
  if (t === null) return <td className="pa-muted">—</td>;
  return (
    <td className="pa-num" title={`${t} ns after the dataset origin`}>
      {fmtSec(t, 6)}
      {sinceDecision ? <span className="pa-after"> +{fmtDuration(t - o.decision_ts)}</span> : null}
    </td>
  );
}

function tifWords(tif: string): string {
  return tif === 'gtx' ? 'post-only' : tif === 'ioc' ? 'immediate-or-cancel' : tif === 'fok' ? 'fill-or-kill' : 'good-till-cancel';
}

export interface InspectorProps {
  story: LatencyStory;
  orderId: number;
  pair: OrderPair | null;
  /** Stored pairs cover only the earliest orders; `pair` is null beyond them. */
  pairsCover: boolean;
  labels: Record<Which, string>;
  inst: InstrumentMeta;
  onClose: () => void;
  onShowInChart: (t: number) => void;
}

export function Inspector({ story, orderId, pair, pairsCover, labels, inst, onClose, onShowInChart }: InspectorProps) {
  const res = useAsync((signal) => loadLifecycles(story, orderId, signal), [story, orderId]);
  const d: Record<Which, OrderDetail | null> | null = res.data;
  const ref = d ? (d.fast ?? d.slow)?.order ?? null : null;

  return (
    <section className="pa-inspector" aria-labelledby="pa-inspector-title">
      <header className="pa-inspector-head">
        <h2 id="pa-inspector-title" tabIndex={-1}>
          Order #{orderId} in both scenarios
        </h2>
        <button type="button" className="pa-btn" onClick={onClose} aria-label="Close order inspector">
          Close
        </button>
      </header>
      {res.loading && !d ? <Spinner label="Loading the order in both runs" /> : null}
      {res.error ? <ErrorBox error={res.error} onRetry={res.reload} /> : null}
      {d && !d.fast && !d.slow ? <p className="pa-muted">Neither run placed an order with this id.</p> : null}
      {d && ref ? (
        <>
          <p className="pa-lead">
            <span className={ref.side === 'buy' ? 'pa-buy' : 'pa-sell'}>{ref.side === 'buy' ? 'Buy' : 'Sell'}</span> {fmtQty(ref.qty, inst)} at{' '}
            <span className="pa-num">{fmtPrice(ref.price_tick, inst)}</span>, {tifWords(ref.tif)} limit order.
          </p>
          {pair ? (
            <p className="pa-verdict">
              <strong>{pairVerdict(story, pair)}.</strong>
              {pair.differences.length > 0 ? ` Differs in: ${pair.differences.map(differenceWords).join(', ')}.` : null}
            </p>
          ) : !d.fast || !d.slow ? (
            <p className="pa-verdict">
              <strong>Placed only in the {d.fast ? labels.fast : labels.slow} scenario.</strong>
            </p>
          ) : !pairsCover ? (
            <p className="pa-muted">The stored comparison classifies only the earliest orders; compare the times below.</p>
          ) : null}
          <div className="pa-scroll-x">
            <table className="pa-table pa-life">
              <caption className="visually-hidden">Order lifecycle in both scenarios</caption>
              <thead>
                <tr>
                  <th scope="col">Step</th>
                  <th scope="col">
                    <span className="pa-swatch pa-swatch-fast" aria-hidden="true" /> {labels.fast}
                  </th>
                  <th scope="col">
                    <span className="pa-swatch pa-swatch-slow" aria-hidden="true" /> {labels.slow}
                  </th>
                </tr>
              </thead>
              <tbody>
                {STAGES.map((s) => {
                  const a = d.fast ? s.ts(d.fast.order) : null;
                  const b = d.slow ? s.ts(d.slow.order) : null;
                  const differs = a !== b;
                  return (
                    <tr key={s.label} className={differs ? 'pa-life-differs' : undefined}>
                      <th scope="row">
                        {s.label}
                        <span className="pa-clock">{s.clock}</span>
                      </th>
                      <Cell o={d.fast?.order ?? null} stage={s} sinceDecision={s !== STAGES[0]} />
                      <Cell o={d.slow?.order ?? null} stage={s} sinceDecision={s !== STAGES[0]} />
                    </tr>
                  );
                })}
                <tr>
                  <th scope="row">Outcome</th>
                  <td>{d.fast ? outcomeWords(d.fast.order) : '—'}</td>
                  <td>{d.slow ? outcomeWords(d.slow.order) : '—'}</td>
                </tr>
                <tr>
                  <th scope="row">Fees</th>
                  <td className="pa-num">{d.fast ? d.fast.order.fees.toFixed(4) : '—'}</td>
                  <td className="pa-num">{d.slow ? d.slow.order.fees.toFixed(4) : '—'}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <p className="pa-muted">Times are seconds after the start of the dataset; “+” is the time since the decision. Highlighted rows differ.</p>
          <div className="pa-actions">
            <button type="button" className="pa-btn" onClick={() => onShowInChart(ref.decision_ts)}>
              Centre the chart on this order
            </button>
          </div>
          <ul className="pa-links">
            {(['fast', 'slow'] as const).map((w) =>
              d[w] ? (
                <li key={w}>
                  <Link to={paths.order(story[w].runId, orderId)}>Full forensics of this order, {labels[w]} run</Link>
                </li>
              ) : null,
            )}
            <li>
              <Link to={paths.compareSet({ runs: [story.fast.runId, story.slow.runId], demo: story.demo.id })}>Full comparison of both runs</Link>
            </li>
          </ul>
          <details className="pa-details">
            <summary>Every recorded event of this order</summary>
            {(['fast', 'slow'] as const).map((w) => (
              <div key={w}>
                <h3>{labels[w]} scenario</h3>
                {d[w] ? (
                  <ol className="pa-events">
                    {d[w].events.map((ev) => (
                      <li key={ev.seq}>
                        <span className="pa-num">{fmtSec(ev.ts, 6)}</span> {eventTitle(ev)} <span className="pa-clock">{ev.clock} clock</span>
                      </li>
                    ))}
                  </ol>
                ) : (
                  <p className="pa-muted">Not placed in this scenario.</p>
                )}
              </div>
            ))}
          </details>
        </>
      ) : null}
    </section>
  );
}
