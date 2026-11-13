import { Badge } from "../../components/ui";
import { fmtClock } from "../../components/fmt";
import { BASES, BASIS_MEANING, EM_DASH, SOURCES, SOURCE_MEANING } from "./labels";
import "./forensics.css";

/**
 * Legend for the evidence bases and, optionally, the event sources. Every view that shows a basis
 * or source badge renders this so the colours are never left unexplained.
 */
export function EvidenceLegend({ sources = false }: { sources?: boolean }) {
  return (
    <section className="ev-legend" aria-label="Legend">
      <dl className="ev-legend-group">
        <dt className="ev-legend-title">Evidence basis</dt>
        {BASES.map((basis) => (
          <dd key={basis} className="ev-legend-item">
            <Badge kind={basis}>{basis}</Badge>
            <span className="dim">{BASIS_MEANING[basis]}</span>
          </dd>
        ))}
      </dl>
      {sources ? (
        <dl className="ev-legend-group">
          <dt className="ev-legend-title">Event source</dt>
          {SOURCES.map((source) => (
            <dd key={source} className="ev-legend-item">
              <Badge kind={source}>{source}</Badge>
              <span className="dim">{SOURCE_MEANING[source]}</span>
            </dd>
          ))}
        </dl>
      ) : null}
    </section>
  );
}

/** A dataset-relative timestamp as `mm:ss.mmm` with the microseconds dimmed. */
export function ClockText({ ns }: { ns: number | null }) {
  if (ns === null) return <span className="mono dim">{EM_DASH}</span>;
  const text = fmtClock(ns);
  return (
    <span className="mono">
      {text.slice(0, -3)}
      <span className="dim">{text.slice(-3)}</span>
    </span>
  );
}
