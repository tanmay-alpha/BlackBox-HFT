// Modal dialog (APG dialog pattern) on the native <dialog> element: showModal() makes the rest of
// the page inert (focus is trapped), Esc closes it, and focus returns to the opener.

import { useEffect, useId, useRef } from 'react';
import type { ReactNode } from 'react';
import { X } from 'lucide-react';

export interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  className?: string;
}

export function Dialog({ open, onClose, title, children, className }: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    if (!open && d.open) d.close();
  }, [open]);
  return (
    <dialog
      ref={ref}
      className={className ? `dialog ${className}` : 'dialog'}
      aria-labelledby={titleId}
      onClose={onClose}
    >
      {open ? (
        <>
          <header className="dialog-head">
            <h2 id={titleId}>{title}</h2>
            <button type="button" className="btn-icon" aria-label="Close" title="Close (Esc)" onClick={onClose}>
              <X size={16} strokeWidth={1.5} aria-hidden="true" />
            </button>
          </header>
          <div className="dialog-body">{children}</div>
        </>
      ) : null}
    </dialog>
  );
}
