// Phone layout of a docked workspace: below 600 px wide, panes that a wide screen shows side by
// side (and a 768 px screen stacks) become tabs, each filling the workspace body, instead of a
// tall stack of short panes that each scroll on their own (FINAL_REVIEW J, UI_AUDIT).

import { useSyncExternalStore } from 'react';
import type { ReactNode } from 'react';

import { Tabs } from './Tabs';

export const PHONE_QUERY = '(max-width: 599px)';

function subscribe(cb: () => void): () => void {
  const mq = window.matchMedia(PHONE_QUERY);
  mq.addEventListener('change', cb);
  return () => mq.removeEventListener('change', cb);
}

/** True on a phone-width viewport (the `max-width: 599px` breakpoint of shell.css). */
export function usePhone(): boolean {
  return useSyncExternalStore(subscribe, () => window.matchMedia(PHONE_QUERY).matches);
}

export interface PaneTab<K extends string> {
  id: K;
  label: ReactNode;
  count?: ReactNode;
  content: ReactNode;
}

/** One pane at a time, chosen by a tab row that stays at the top of the workspace body. */
export function PaneTabs<K extends string>({
  label,
  tabs,
  active,
  onChange,
}: {
  label: string;
  tabs: PaneTab<K>[];
  active: K;
  onChange: (id: K) => void;
}) {
  const current = tabs.find((t) => t.id === active) ?? tabs[0];
  return (
    <Tabs<K> label={label} className="pane-tabs" active={current.id} onChange={onChange} tabs={tabs.map(({ id, label: l, count }) => ({ id, label: l, count }))}>
      {current.content}
    </Tabs>
  );
}
