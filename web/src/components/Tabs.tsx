// Tabs (UI_DESIGN_SPEC.md §6.5). Panel-level tabs follow the APG tabs pattern with automatic
// activation; workspace-level tabs are routes and render as links (`RouteTabs`).

import { useId, useRef } from 'react';
import type { KeyboardEvent, ReactNode } from 'react';
import { NavLink } from 'react-router-dom';

export interface TabItem<K extends string> {
  id: K;
  label: ReactNode;
  /** Shown after the label (`Orders 2,060`). */
  count?: ReactNode;
}

export interface TabsProps<K extends string> {
  label: string;
  tabs: TabItem<K>[];
  active: K;
  onChange: (id: K) => void;
  /** Content of the active tab. */
  children: ReactNode;
  className?: string;
}

export function Tabs<K extends string>({ label, tabs, active, onChange, children, className }: TabsProps<K>) {
  const base = useId();
  const refs = useRef(new Map<K, HTMLButtonElement>());
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const i = tabs.findIndex((t) => t.id === active);
    let next = -1;
    if (e.key === 'ArrowRight') next = (i + 1) % tabs.length;
    else if (e.key === 'ArrowLeft') next = (i - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = tabs.length - 1;
    if (next < 0) return;
    e.preventDefault();
    onChange(tabs[next].id);
    refs.current.get(tabs[next].id)?.focus();
  };
  return (
    <div className={className ? `tabs ${className}` : 'tabs'}>
      <div role="tablist" aria-label={label} className="tablist" onKeyDown={onKeyDown}>
        {tabs.map((t) => (
          <button
            key={t.id}
            ref={(el) => {
              if (el) refs.current.set(t.id, el);
              else refs.current.delete(t.id);
            }}
            type="button"
            role="tab"
            id={`${base}-tab-${t.id}`}
            aria-selected={t.id === active}
            aria-controls={`${base}-panel`}
            tabIndex={t.id === active ? 0 : -1}
            className="tab"
            onClick={() => onChange(t.id)}
          >
            {t.label}
            {t.count !== undefined ? <span className="tab-count">{t.count}</span> : null}
          </button>
        ))}
      </div>
      <div role="tabpanel" id={`${base}-panel`} aria-labelledby={`${base}-tab-${active}`} className="tabpanel" tabIndex={0}>
        {children}
      </div>
    </div>
  );
}

/** Workspace-level tabs: each tab is a route. */
export function RouteTabs({ label, items }: { label: string; items: { to: string; label: string; end?: boolean }[] }) {
  return (
    <nav aria-label={label} className="route-tabs">
      {items.map((it) => (
        <NavLink key={it.to} to={it.to} end={it.end} className="tab">
          {it.label}
        </NavLink>
      ))}
    </nav>
  );
}
