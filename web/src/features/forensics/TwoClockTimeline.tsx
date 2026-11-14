import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { FlightEvent, InstrumentMeta, Side } from "../../api/types";
import { Badge } from "../../components/ui";
import { ClockText } from "./evidence";
import { eventFields, eventTitle } from "./eventFormat";
import { fmtSince } from "./labels";
import { TimelineRenderer, buildTimeline } from "./timeline";
import type { AxisMode, TimelineMarker } from "./timeline";

interface Hover {
  marker: TimelineMarker;
  x: number;
  y: number;
}

interface Drag {
  startX: number;
  lastX: number;
  moved: boolean;
}

export interface TwoClockTimelineProps {
  events: FlightEvent[];
  /** The decision time; the axis starts here. */
  originNs: number;
  instrument: InstrumentMeta;
  side: Side;
  mode: AxisMode;
  selectedSeq: number | null;
  citedSeqs: ReadonlySet<number>;
  onSelect: (seq: number | null) => void;
}

export function TwoClockTimeline({
  events,
  originNs,
  instrument,
  side,
  mode,
  selectedSeq,
  citedSeqs,
  onSelect,
}: TwoClockTimelineProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const rendererRef = useRef<TimelineRenderer | null>(null);
  const dragRef = useRef<Drag | null>(null);
  const [hover, setHover] = useState<Hover | null>(null);
  const [width, setWidth] = useState(0);
  const model = useMemo(() => buildTimeline(events, originNs), [events, originNs]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const renderer = new TimelineRenderer(canvas);
    rendererRef.current = renderer;

    const observer = new ResizeObserver(() => {
      const r = canvas.getBoundingClientRect();
      renderer.resize(r.width, r.height, window.devicePixelRatio || 1);
      setWidth(r.width);
    });
    observer.observe(canvas);

    // React registers wheel listeners as passive, which cannot stop the page from scrolling
    // while the pointer is zooming the axis.
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const r = canvas.getBoundingClientRect();
      renderer.zoomAt(e.clientX - r.left, Math.exp(e.deltaY * 0.0015));
      setHover(null);
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });

    return () => {
      observer.disconnect();
      canvas.removeEventListener("wheel", onWheel);
      renderer.destroy();
      rendererRef.current = null;
    };
  }, []);

  useEffect(() => {
    rendererRef.current?.setModel(model, side);
  }, [model, side]);

  useEffect(() => {
    rendererRef.current?.setMode(mode);
  }, [mode]);

  useEffect(() => {
    const renderer = rendererRef.current;
    if (!renderer) return;
    renderer.setMarks(selectedSeq, citedSeqs);
    if (selectedSeq !== null) renderer.reveal(selectedSeq);
  }, [selectedSeq, citedSeqs, model, mode]);

  const zoomCentre = (factor: number) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const r = canvas.getBoundingClientRect();
    rendererRef.current?.zoomAt(r.width / 2 + 60, factor);
  };

  const step = (delta: 1 | -1) => {
    if (events.length === 0) return;
    const at = selectedSeq === null ? -1 : events.findIndex((e) => e.seq === selectedSeq);
    const next =
      at < 0
        ? delta > 0
          ? 0
          : events.length - 1
        : Math.min(events.length - 1, Math.max(0, at + delta));
    const target = events[next];
    if (target) onSelect(target.seq);
  };

  return (
    <div className="fx-timeline">
      <div className="fx-timeline-bar">
        <span className="dim">
          {mode === "linear"
            ? "Linear time: distances are proportional to elapsed time."
            : "Event order: each distinct instant is one step; distances do not show elapsed time."}
        </span>
        <span className="fx-timeline-zoom">
          <button
            type="button"
            className="btn-icon"
            onClick={() => zoomCentre(0.5)}
            aria-label="Zoom in"
            title="Zoom in"
          >
            +
          </button>
          <button
            type="button"
            className="btn-icon"
            onClick={() => zoomCentre(2)}
            aria-label="Zoom out"
            title="Zoom out"
          >
            {"\u2212"}
          </button>
          <button
            type="button"
            onClick={() => rendererRef.current?.resetView()}
            title="Fit the whole order"
          >
            Fit
          </button>
        </span>
      </div>
      <div
        className="fx-timeline-stage"
        tabIndex={0}
        role="group"
        aria-label="Two-clock timeline. Left and right arrow keys step through events, plus and minus zoom, zero fits the whole order, Escape clears the selection. The same events are listed in the Flight recorder tab."
        onKeyDown={(e) => {
          if (e.key === "ArrowRight") step(1);
          else if (e.key === "ArrowLeft") step(-1);
          else if (e.key === "+" || e.key === "=") zoomCentre(0.5);
          else if (e.key === "-") zoomCentre(2);
          else if (e.key === "0") rendererRef.current?.resetView();
          else if (e.key === "Escape") onSelect(null);
          else return;
          e.preventDefault();
        }}
      >
        <canvas
          ref={canvasRef}
          className={hover ? "fx-timeline-canvas is-hit" : "fx-timeline-canvas"}
          aria-hidden="true"
          onPointerDown={(e) => {
            dragRef.current = { startX: e.clientX, lastX: e.clientX, moved: false };
            e.currentTarget.setPointerCapture(e.pointerId);
          }}
          onPointerMove={(e) => {
            const renderer = rendererRef.current;
            if (!renderer) return;
            const drag = dragRef.current;
            if (drag) {
              if (!drag.moved && Math.abs(e.clientX - drag.startX) <= 3) return;
              drag.moved = true;
              renderer.panBy(e.clientX - drag.lastX);
              drag.lastX = e.clientX;
              renderer.setHover(null);
              setHover(null);
              return;
            }
            const r = e.currentTarget.getBoundingClientRect();
            const x = e.clientX - r.left;
            const y = e.clientY - r.top;
            const marker = renderer.hitTest(x, y);
            renderer.setHover(marker ? marker.seq : null);
            setHover(marker ? { marker, x, y } : null);
          }}
          onPointerUp={(e) => {
            const drag = dragRef.current;
            dragRef.current = null;
            const renderer = rendererRef.current;
            if (!renderer || !drag || drag.moved) return;
            const r = e.currentTarget.getBoundingClientRect();
            const marker = renderer.hitTest(e.clientX - r.left, e.clientY - r.top);
            onSelect(marker ? marker.seq : null);
          }}
          onPointerCancel={() => {
            dragRef.current = null;
          }}
          onPointerLeave={() => {
            if (dragRef.current) return;
            rendererRef.current?.setHover(null);
            setHover(null);
          }}
          onDoubleClick={() => rendererRef.current?.resetView()}
        />
        {hover ? (
          <div
            className="fx-tooltip"
            role="presentation"
            style={
              hover.x > width / 2
                ? { right: width - hover.x + 14, top: hover.y + 14 }
                : { left: hover.x + 14, top: hover.y + 14 }
            }
          >
            <div className="fx-tooltip-head">
              <span className="mono">#{hover.marker.seq}</span>
              <Badge kind={hover.marker.event.source}>{hover.marker.event.source}</Badge>
              <span className="fx-clock-tag" data-clock={hover.marker.event.clock}>
                {hover.marker.event.clock} clock
              </span>
            </div>
            <div className="fx-tooltip-title">{eventTitle(hover.marker.event)}</div>
            <div className="dim">
              <ClockText ns={hover.marker.event.ts} />{" "}
              <span className="mono">({fmtSince(hover.marker.event.ts, originNs)} from decision)</span>
            </div>
            <dl className="fx-fields">
              {eventFields(hover.marker.event, instrument).map(([label, value]) => (
                <div key={label} className="fx-field">
                  <dt>{label}</dt>
                  <dd className="mono">{value}</dd>
                </div>
              ))}
            </dl>
          </div>
        ) : null}
      </div>
      <p className="fx-hint dim">
        Wheel zooms, drag pans, double-click fits. Click a marker to select it everywhere on this
        page. With the timeline focused: left and right arrows step through events, + and - zoom,
        0 fits, Escape clears the selection.
      </p>
      <TimelineLegend side={side} />
    </div>
  );
}

interface LegendEntry {
  label: string;
  shape: "diamond" | "down" | "up" | "circle" | "ring" | "cross" | "tick" | "square" | "slant";
  tone: "fg" | "modeled" | "observed" | "warn" | "error" | "muted" | "side";
  hollow?: boolean;
  dashed?: boolean;
}

const LEGEND: LegendEntry[] = [
  { label: "strategy decision", shape: "diamond", tone: "fg" },
  { label: "strategy cancel decision", shape: "diamond", tone: "warn", hollow: true },
  { label: "risk rule block", shape: "diamond", tone: "error" },
  { label: "request sent", shape: "down", tone: "modeled" },
  { label: "cancel request sent", shape: "down", tone: "warn", hollow: true },
  { label: "request refused by engine", shape: "cross", tone: "error" },
  { label: "accepted to rest (queue initialised)", shape: "circle", tone: "modeled", hollow: true },
  { label: "market trade at the order's price", shape: "tick", tone: "observed" },
  { label: "displayed quantity change at the price", shape: "tick", tone: "muted" },
  { label: "simulated fill", shape: "ring", tone: "side" },
  { label: "exchange response emitted", shape: "up", tone: "modeled" },
  { label: "cancel response emitted", shape: "up", tone: "warn", hollow: true },
  { label: "response received locally", shape: "circle", tone: "fg" },
  { label: "cancel acknowledgement received", shape: "circle", tone: "warn", hollow: true },
  { label: "recorder marker", shape: "square", tone: "muted", hollow: true },
  { label: "modeled latency (send to scheduled arrival)", shape: "slant", tone: "modeled" },
  { label: "modeled latency of a cancel", shape: "slant", tone: "warn", dashed: true },
];

function LegendGlyph({ entry, side }: { entry: LegendEntry; side: Side }) {
  const tone = entry.tone === "side" ? side : entry.tone;
  const cls = `fx-glyph fx-tone-${tone}${entry.hollow ? " is-hollow" : ""}`;
  let shape: ReactNode = null;
  switch (entry.shape) {
    case "diamond":
      shape = <path className={cls} d="M9 2 L16 9 L9 16 L2 9 Z" />;
      break;
    case "down":
      shape = <path className={cls} d="M9 15 L15 4 L3 4 Z" />;
      break;
    case "up":
      shape = <path className={cls} d="M9 3 L15 14 L3 14 Z" />;
      break;
    case "circle":
      shape = <circle className={cls} cx="9" cy="9" r="4.5" />;
      break;
    case "ring":
      shape = (
        <>
          <circle className={cls} cx="9" cy="9" r="4.5" />
          <circle className={`${cls} is-hollow`} cx="9" cy="9" r="7.5" />
        </>
      );
      break;
    case "cross":
      shape = <path className={`${cls} is-hollow`} d="M4 4 L14 14 M14 4 L4 14" />;
      break;
    case "tick":
      shape = <path className={`${cls} is-hollow`} d="M9 2 L9 16" />;
      break;
    case "square":
      shape = <rect className={cls} x="4.5" y="4.5" width="9" height="9" />;
      break;
    case "slant":
      shape = (
        <path
          className={`${cls} is-hollow`}
          d="M2 3 L16 15"
          strokeDasharray={entry.dashed ? "4 3" : undefined}
        />
      );
      break;
  }
  return (
    <svg className="fx-glyph-svg" width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
      {shape}
    </svg>
  );
}

function TimelineLegend({ side }: { side: Side }) {
  return (
    <ul className="fx-glyph-legend" aria-label="Timeline marker legend">
      {LEGEND.map((entry) => (
        <li key={entry.label}>
          <LegendGlyph entry={entry} side={side} />
          <span>{entry.label}</span>
        </li>
      ))}
    </ul>
  );
}
