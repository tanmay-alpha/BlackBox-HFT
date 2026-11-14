// Toolbar (UI_DESIGN_SPEC.md §6.4, APG toolbar): the buttons and links share one tab stop and
// Left/Right/Home/End move between them. Text fields and selects keep their own tab stop and
// their own arrow keys, so a value can always be reached and edited from the keyboard.

import { useLayoutEffect, useRef } from 'react';
import type { FocusEvent, KeyboardEvent, ReactNode } from 'react';

const FOCUSABLE = 'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled])';

function ownsArrows(el: HTMLElement): boolean {
  if (el instanceof HTMLSelectElement || el instanceof HTMLTextAreaElement) return true;
  return el instanceof HTMLInputElement && !['checkbox', 'radio', 'button', 'submit'].includes(el.type);
}

/** The roving members: buttons, links, checkboxes of this toolbar (not of a nested one). */
function items(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => el.closest('[role="toolbar"]') === root && !ownsArrows(el),
  );
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
    </div>
  );
}

/** A 1 px vertical divider between toolbar groups. */
export function ToolbarDivider() {
  return <span className="toolbar-divider" aria-hidden="true" />;
}
