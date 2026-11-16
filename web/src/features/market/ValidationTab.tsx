// Step 2, Validation: the reconstruction's own quality checks, failures first, each with its
// count, the server's explanation and its examples, plus the invalid and missing events.

import type { QualityCheck } from '../../api/types';
import { fmtInt } from '../../components/fmt';
import { Notice, Panel, Pill } from '../../components/ui';
import type { MarketSummary } from './api';
import { CheckPill } from './labels';
import { checkCounts, checkStatus, sortChecks } from './model';

/** What a non-passing outcome means for research on this source, by check family. */
function consequence(c: QualityCheck): string {
  const status = checkStatus(c);
  if (status === 'pass') return 'No occurrence found.';
  if (c.id.startsWith('book.') && c.severity === 'error')
    return 'These messages were not applied to the book; the book around them is missing their effect.';
  if (c.id === 'itch.framing') return 'The source is cut short: events after the last complete frame are missing.';
  if (c.id === 'itch.invalid_message' || c.id === 'itch.unknown_type') return 'These frames were skipped and counted, never interpreted.';
  if (c.id === 'itch.start_of_messages') return 'Orders entered before the first message are missing from the book.';
  if (c.id.startsWith('time.')) return 'Time-based measures near these events are unreliable; event order is still the file order.';
  if (c.id === 'book.crossed' || c.id === 'book.locked') return 'Spread and imbalance are not meaningful while the book is crossed or locked.';
  if (c.id.startsWith('l2.')) return 'The Level-2 rows derived from this source differ from the L3 book here.';
  return status === 'fail' ? 'Do not trust the affected interval.' : 'Read the explanation before relying on affected events.';
}

export function ValidationTab({ summary }: { summary: MarketSummary }) {
  const checks = sortChecks(summary.quality.checks);
  const counts = checkCounts(checks);
  const byId = new Map(checks.map((c) => [c.id, c]));
  const invalid = byId.get('itch.invalid_message')?.count ?? null;
  const unknown = byId.get('itch.unknown_type')?.count ?? null;
  const truncated = byId.get('itch.framing');
  // Quarantine outcomes are the error-severity `book.*` checks (book.unknown_order, ...).
  const quarantine = checks.filter((c) => c.id.startsWith('book.') && c.severity === 'error' && c.count > 0);
  return (
    <div className="mk-scroll">
      <div className="col mk-stack">
        <Panel
          title="Reconstruction quality"
          basis="observed"
          caption="Checks run by the ITCH decoder and order-book reconstruction while reading the source. They test the data and the reconstruction, not any trading model."
        >
          <div className="col">
            <p className="row" aria-label="Check outcomes">
              Overall {summary.quality.passed ? <Pill tone="ok">passed</Pill> : <Pill tone="error">not passed</Pill>}
              <span aria-hidden="true">{'\u00b7'}</span>
              <span>
                <CheckPill status="fail" /> <span className="mono">{fmtInt(counts.fail)}</span>
              </span>
              <span>
                <CheckPill status="warn" /> <span className="mono">{fmtInt(counts.warn)}</span>
              </span>
              <span>
                <CheckPill status="info" /> <span className="mono">{fmtInt(counts.info)}</span>
              </span>
              <span>
                <CheckPill status="pass" /> <span className="mono">{fmtInt(counts.pass)}</span>
              </span>
            </p>
            <p className="prose">
              <strong>Fail</strong> means an error-severity check found occurrences: affected messages were quarantined or
              skipped, never repaired or invented. <strong>Warn</strong> means a condition that can be legitimate (a crossed
              book during a halt) but changes how affected events should be read. <strong>Note</strong> is informational.
            </p>
          </div>
        </Panel>
        <Panel title="Invalid and missing events" basis="observed">
          <dl className="kv">
            <dt>Quarantined messages</dt>
            <dd className="mono">{fmtInt(summary.quality.quarantined_total)}</dd>
            <dt>Frames contradicting the specification</dt>
            <dd className="mono">{invalid === null ? '\u2014' : fmtInt(invalid)}</dd>
            <dt>Undefined message types</dt>
            <dd className="mono">{unknown === null ? '\u2014' : fmtInt(unknown)}</dd>
            <dt>Source cut inside a frame</dt>
            <dd>{truncated === undefined ? '\u2014' : truncated.passed ? 'no' : 'yes: events after the cut are missing'}</dd>
          </dl>
          {quarantine.length > 0 ? (
            <ul className="mk-list">
              {quarantine.map((c) => (
                <li key={c.id}>
                  <span className="mono">{fmtInt(c.count)}</span> {c.title.toLowerCase()} <span className="dim mono">({c.id})</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="note">No message was quarantined: every order-level message referred to a known order with valid values.</p>
          )}
          <p className="note">
            A quarantined message was not applied to the book and nothing was invented in its place. In the event timeline its
            outcome reads {'\u201c'}quarantined{'\u201d'}.
          </p>
        </Panel>
        {checks.length === 0 ? (
          <Notice tag="No checks" tone="error">
            The artifact carries no quality checks, so the reconstruction cannot be judged.
          </Notice>
        ) : (
          <Panel
            title={`Checks (${fmtInt(checks.length)})`}
            basis="observed"
            caption="Failures first. Under each check: first what the outcome means for research (this console's reading), then the reconstruction's own explanation in grey."
          >
            <ul className="mk-checks">
              {checks.map((c) => (
                <li key={c.id} className="mk-check">
                  <div className="row">
                    <CheckPill status={checkStatus(c)} />
                    <strong>{c.title}</strong>
                    <span className="dim mono">{c.id}</span>
                    <span className="spacer" />
                    <span className="mono" title="Occurrences">
                      {fmtInt(c.count)}
                    </span>
                    <span className="dim">{c.severity}</span>
                  </div>
                  <p>{consequence(c)}</p>
                  <p className="note">{c.detail}</p>
                  {c.examples.length > 0 ? (
                    <details>
                      <summary>Examples ({fmtInt(c.examples.length)} shown)</summary>
                      <ul className="mk-list mono">
                        {c.examples.map((x) => (
                          <li key={x}>{x}</li>
                        ))}
                      </ul>
                    </details>
                  ) : null}
                </li>
              ))}
            </ul>
          </Panel>
        )}
      </div>
    </div>
  );
}
