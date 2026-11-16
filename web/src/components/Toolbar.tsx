// Toolbar (UI_DESIGN_SPEC.md §6.4, APG toolbar): the buttons, links and disclosure buttons share
// one tab stop and Left/Right/Home/End move between them. Text fields and selects keep their own
// tab stop and their own arrow keys, so a value can always be reached and edited from the keyboard.
// Escape closes the disclosure that holds focus and returns focus to its button; a disclosure
// shown as a menu (`.ws-view`) also closes on a pointer press outside it. While a roving member
// has keyboard focus, a hint says how the arrow keys work.

import { useEffect, useLayoutEffect, useRef } from 'react';
import type { FocusEvent, KeyboardEvent, ReactNode } from 'react';

const FOCUSABLE =
  'button:not([disabled]), a[href], summary, input:not([disabled]), select:not([disabled]), textarea:not([disabled])';

function ownsArrows(el: HTMLElement): boolean {
  if (el instanceof HTMLSelectElement || el instanceof HTMLTextAreaElement) return true;
  return el instanceof HTMLInputElement && !['checkbox', 'radio', 'button', 'submit'].includes(el.type);
}

/** The roving members: buttons, links, checkboxes of this toolbar (not of a nested one, nor in a
 *  closed disclosure). */
function items(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => {
    if (el.closest('[role="toolbar"]') !== root || ownsArrows(el)) return false;
    // Inside a closed disclosure only its own button (summary) is reachable.
    const closed = el.closest('details:not([open])');
    return closed === null || (el.tagName === 'SUMMARY' && el.parentElement === closed);
  });
}

export function Toolbar({ label, children, className }: { label: string; children: ReactNode; className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const active = useRef(0);

  // Re-applied after every render because the controls change with the data.
  useLayoutEffect(() => {
    const root = ref.current;
    if (!root) return;
    const list = items(root);
    if (active.current >= list.length) active.current = 0;
    list.forEach((el, i) => {
      el.tabIndex = i === active.current ? 0 : -1;
    });
  });

  const focusAt = (list: HTMLElement[], i: number) => {
    const next = (i + list.length) % list.length;
    list.forEach((el, j) => {
      el.tabIndex = j === next ? 0 : -1;
    });
    active.current = next;
    list[next].focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const root = ref.current;
    if (!root) return;
    if (e.key === 'Escape') {
      const open = (e.target as HTMLElement).closest('details[open]');
      if (open && root.contains(open)) {
        open.removeAttribute('open');
        open.querySelector<HTMLElement>(':scope > summary')?.focus();
        e.preventDefault();
        e.stopPropagation();
      }
      return;
    }
    const list = items(root);
    const i = list.indexOf(e.target as HTMLElement);
    if (i < 0) return;
    if (e.key === 'ArrowRight') focusAt(list, i + 1);
    else if (e.key === 'ArrowLeft') focusAt(list, i - 1);
    else if (e.key === 'Home') focusAt(list, 0);
    else if (e.key === 'End') focusAt(list, list.length - 1);
    else return;
    e.preventDefault();
  };

  // Menu-style disclosures close on a press outside them.
  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      const root = ref.current;
      if (!root) return;
      for (const d of root.querySelectorAll<HTMLDetailsElement>('details.ws-view[open]')) {
        if (!d.contains(e.target as Node)) d.open = false;
      }
    };
    document.addEventListener('pointerdown', onDown);
    return () => document.removeEventListener('pointerdown', onDown);
  }, []);

  const onFocus = (e: FocusEvent<HTMLDivElement>) => {
    const root = ref.current;
    if (!root) return;
    const i = items(root).indexOf(e.target as HTMLElement);
    if (i >= 0) active.current = i;
  };

  return (
    <div
      ref={ref}
      role="toolbar"
      aria-label={label}
      className={className ? `toolbar ${className}` : 'toolbar'}
      onKeyDown={onKeyDown}
      onFocus={onFocus}
    >
      {children}
      <span className="toolbar-hint" aria-hidden="true">
        {'\u2190 \u2192'} move between buttons {'\u00b7'} Tab leaves the toolbar
      </span>
    </div>
  );
}

/** A 1 px vertical divider between toolbar groups. */
export function ToolbarDivider({ className }: { className?: string } = {}) {
  return <span className={className ? `toolbar-divider ${className}` : 'toolbar-divider'} aria-hidden="true" />;
}
