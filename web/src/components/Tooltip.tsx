// Tooltip and quant-term tooltip (UI_DESIGN_SPEC.md §6.11, §6.14). Appears after 500 ms of hover
// or at once on keyboard focus, stays while the pointer is over it, and closes on Esc
// (WCAG 1.4.13). Supplementary text only: the same text is reachable in the inspector glossary.

import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { createPortal } from 'react-dom';

import { GLOSSARY, type GlossaryKey } from './glossary';

const HOVER_DELAY_MS = 500;
const MAX_WIDTH = 360;

interface TooltipProps {
  content: ReactNode;
  children: ReactNode;
  /** Make the wrapper itself focusable (for plain text triggers such as terms). */
  focusable?: boolean;
  className?: string;
}

/**
 * Wraps a trigger. Focus events bubble from focusable children (a sort button, a link), so the
 * wrapper only needs its own tab stop when the trigger is plain text.
 */
export function Tooltip({ content, children, focusable = false, className }: TooltipProps) {
  const id = useId();
  const anchor = useRef<HTMLSpanElement>(null);
  const tip = useRef<HTMLDivElement>(null);
  const timer = useRef<number | undefined>(undefined);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  const clear = () => window.clearTimeout(timer.current);
  const show = (delay: number) => {
    clear();
    if (delay === 0) setOpen(true);
    else timer.current = window.setTimeout(() => setOpen(true), delay);
  };
  const hide = () => {
    clear();
    setOpen(false);
    setPos(null);
  };

  useEffect(() => clear, []);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        hide();
      }
    };
    document.addEventListener('keydown', onKey, true);
    return () => document.removeEventListener('keydown', onKey, true);
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !anchor.current || !tip.current) return;
    const a = anchor.current.getBoundingClientRect();
    const t = tip.current.getBoundingClientRect();
    const left = Math.max(4, Math.min(a.left, window.innerWidth - t.width - 4));
    const below = a.bottom + 4;
    const top = below + t.height > window.innerHeight - 4 ? Math.max(4, a.top - t.height - 4) : below;
    setPos({ left, top });
  }, [open]);

  return (
    <span
      ref={anchor}
      className={className ? `tooltip-anchor ${className}` : 'tooltip-anchor'}
      tabIndex={focusable ? 0 : undefined}
      aria-describedby={open ? id : undefined}
      onPointerEnter={() => show(HOVER_DELAY_MS)}
      onPointerLeave={(e) => {
        if (tip.current && e.relatedTarget instanceof Node && tip.current.contains(e.relatedTarget)) return;
        hide();
      }}
      onFocus={() => show(0)}
      onBlur={hide}
    >
      {children}
      {open
        ? createPortal(
            <div
              ref={tip}
              id={id}
              role="tooltip"
              className="tooltip"
              style={{ maxWidth: MAX_WIDTH, left: pos?.left ?? -9999, top: pos?.top ?? -9999 }}
              onPointerLeave={hide}
            >
              {content}
            </div>,
            document.body,
          )
        : null}
    </span>
  );
}

/** Body of a glossary tooltip: the term, its definition and the document it comes from. */
export function Definition({ term }: { term: GlossaryKey }) {
  const entry = GLOSSARY[term];
  return (
    <>
      <strong className="tooltip-title">{entry.term}</strong>
      <span>{entry.definition}</span>
      <span className="tooltip-source">{entry.source}</span>
    </>
  );
}

/** A quant term with a dotted underline and its definition on hover or focus. */
export function Term({ term, children }: { term: GlossaryKey; children?: ReactNode }) {
  return (
    <Tooltip content={<Definition term={term} />} focusable className="term">
      {children ?? GLOSSARY[term].term}
    </Tooltip>
  );
}
