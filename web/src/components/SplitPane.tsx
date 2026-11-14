// Resizable slots of a workspace (UI_DESIGN_SPEC.md §5.3, §6.7) on react-resizable-panels.
// Separators are keyboard-operable window splitters (role="separator", arrows, Home/End, Enter
// collapses a collapsible pane, double click resets). Sizes persist per workspace in localStorage
// under `bb.layout.<id>.v1`; "Reset layout" (the workspace toolbar) clears them. Below 960 px wide
// the panes stack vertically.

import { Fragment, createContext, useContext, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { Group, Panel, Separator, type Layout } from 'react-resizable-panels';

export interface Pane {
  id: string;
  /** Names the pane in its separators' accessible labels. */
  label: string;
  /** Percentage of the group (`"30%"`) or pixels (`320`). */
  defaultSize?: number | string;
  minSize?: number | string;
  maxSize?: number | string;
  collapsible?: boolean;
  content: ReactNode;
}

/** Bumped by "Reset layout"; every SplitPane remounts with its default sizes. */
export const LayoutResetContext = createContext(0);

const STACK_QUERY = '(max-width: 959px)';

function useNarrow(): boolean {
  const [narrow, setNarrow] = useState(() => window.matchMedia(STACK_QUERY).matches);
  useEffect(() => {
    const mq = window.matchMedia(STACK_QUERY);
    const on = () => setNarrow(mq.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  }, []);
  return narrow;
}

export function layoutKey(id: string): string {
  return `bb.layout.${id}.v1`;
}

function loadLayout(id: string, panes: Pane[]): Layout | undefined {
  try {
    const raw = localStorage.getItem(layoutKey(id));
    if (!raw) return undefined;
    const parsed = JSON.parse(raw) as Layout;
    const ids = panes.map((p) => p.id);
    const keys = Object.keys(parsed);
    if (keys.length !== ids.length || !ids.every((k) => typeof parsed[k] === 'number')) return undefined;
    return parsed;
  } catch {
    return undefined;
  }
}

export function SplitPane({ id, orientation = 'horizontal', panes, className }: { id: string; orientation?: 'horizontal' | 'vertical'; panes: Pane[]; className?: string }) {
  const reset = useContext(LayoutResetContext);
  const narrow = useNarrow();
  const dir = narrow ? 'vertical' : orientation;
  // Read once per mount and again after "Reset layout" (which cleared the stored sizes).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const defaultLayout = useMemo(() => loadLayout(id, panes), [id, reset]);
  return (
    <Group
      key={`${reset}-${dir}`}
      id={id}
      orientation={dir}
      className={className ? `split ${className}` : 'split'}
      defaultLayout={narrow ? undefined : defaultLayout}
      onLayoutChanged={(layout, meta) => {
        if (!meta.isUserInteraction || narrow) return;
        localStorage.setItem(layoutKey(id), JSON.stringify(meta.requestedLayout ?? layout));
      }}
    >
      {panes.map((p, i) => (
        <Fragment key={p.id}>
          {i > 0 ? <Separator className="split-separator" aria-label={`Resize ${panes[i - 1].label} and ${p.label}`} /> : null}
          <Panel
            id={p.id}
            className="split-pane"
            defaultSize={p.defaultSize}
            minSize={p.minSize}
            maxSize={p.maxSize}
            collapsible={p.collapsible}
          >
            {p.content}
          </Panel>
        </Fragment>
      ))}
    </Group>
  );
}
