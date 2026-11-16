import { Fragment } from "react";
import { DASH } from "../../components/fmt";
import { BasisBadge, ClockText } from "../../components/ui";
import type { LedgerRow, LedgerTarget } from "./ledger";
import "./forensics.css";

/** How to read the ledger; shown above it, or in the header of a docked panel. */
export const LEDGER_HINT =
  "One row per kind of evidence, from recorded market data to what no replay can know. Expand a row for its values, the assumption it rests on and the tape events behind it.";

/**
 * "What the simulator knows vs. estimates": one collapsed row per evidence class, details on
 * expand. Rows point at the existing panels (fills, latency, decision context, queue chart)
 * instead of repeating them.
 */
export function EvidenceLedger({
  rows,
  selectedSeq,
  onSelect,
  targets,
  intro = true,
}: {
  rows: LedgerRow[];
  selectedSeq: number | null;
  onSelect: (seq: number) => void;
  /** Views this layout can open; rows whose target is missing show no link. */
  targets: Partial<Record<LedgerTarget, () => void>>;
  /** The how-to-read sentence; a docked panel moves it into its header instead. */
  intro?: boolean;
}) {
  return (
    <section className="fx-ledger" aria-label="What the simulator knows vs. estimates">
      {intro ? <p className="note">{LEDGER_HINT}</p> : null}
      <ol className="fx-ledger-list">
        {rows.map((row) => {
          const open = row.link ? targets[row.link.target] : undefined;
          return (
            <li key={row.id} className="fx-ledger-row" data-basis={row.basis} data-ledger={row.id}>
              <details>
                <summary className="fx-ledger-sum">
                  <span className="fx-ledger-chev" aria-hidden="true">
                    {"\u203a"}
                  </span>
                  <span className="fx-ledger-class">
                    <BasisBadge basis={row.basis} />
                    {row.evidence !== row.basis ? (
                      <span className="fx-ledger-ev">{row.evidence}</span>
                    ) : null}
                  </span>
                  <span className="fx-ledger-title">{row.title}</span>
                  <span className="fx-ledger-summary">{row.summary}</span>
                  <span className="fx-ledger-rests">
                    <span className="dim">rests on </span>
                    {row.restsOn}
                  </span>
                </summary>
                <div className="fx-ledger-body">
                  <p>{row.what}</p>
                  <dl className="fx-ledger-facts">
                    {row.facts.map((fact) => (
                      <Fragment key={fact.label}>
                        <dt>{fact.label}</dt>
                        <dd>
                          {fact.time ? (
                            <>
                              <ClockText ns={fact.time.ns} />{" "}
                              <span className="fx-clock-tag" data-clock={fact.time.clock}>
                                {fact.time.clock} clock
                              </span>{" "}
                            </>
                          ) : null}
                          {fact.value !== null ? (
                            <span className={fact.prose ? undefined : "mono"}>{fact.value}</span>
                          ) : row.basis === "unobservable" ? null : (
                            <span className="mono dim">{DASH}</span>
                          )}
                          {fact.note ? <span className="fx-ledger-note">{fact.note}</span> : null}
                        </dd>
                      </Fragment>
                    ))}
                  </dl>
                  <p className="fx-ledger-assume">
                    <span className="fx-ledger-assume-tag">Rests on</span> {row.assumption}
                  </p>
                  {row.seqs.length > 0 || open ? (
                    <div className="fx-ledger-actions">
                      {row.seqs.length > 0 ? (
                        <span className="fx-ledger-seqs" role="group" aria-label={`Tape events behind ${row.title}`}>
                          <span className="dim">Events</span>
                          {row.seqs.map((seq) => (
                            <button
                              key={seq}
                              type="button"
                              className="btn-link mono"
                              aria-pressed={selectedSeq === seq}
                              title={`Select event #${seq} on the timeline`}
                              onClick={() => onSelect(seq)}
                            >
                              #{seq}
                            </button>
                          ))}
                        </span>
                      ) : null}
                      {open && row.link ? (
                        <button type="button" onClick={open}>
                          {row.link.label}
                        </button>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              </details>
            </li>
          );
        })}
      </ol>
    </section>
  );
}
