import { Badge, BASIS_LABEL } from "../../components/ui";
import { BASES, BASIS_MEANING, SOURCES, SOURCE_MEANING } from "./labels";
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
            <Badge kind={basis}>{BASIS_LABEL[basis]}</Badge>
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

// The comparison workspace imports the shared clock from here; one implementation lives in ui.tsx.
export { ClockText } from "../../components/ui";
