// Resizable slots of a workspace (UI_DESIGN_SPEC.md §5.3, §6.7) on react-resizable-panels.
// Separators are keyboard-operable window splitters (role="separator", arrows, Home/End, Enter
// collapses a collapsible pane, double click resets). Sizes persist per workspace in localStorage
// under `bb.layout.<id>.v1`; "Reset layout" (the workspace toolbar) clears them. Below 960 px wide
// the panes are not resizable: they render as one vertical flow that scrolls as a whole, each leaf
// pane at a fixed `stackHeight` (nested SplitPanes take their content height).

import { Fragment, createContext, isValidElement, useContext, useEffect, useMemo, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { Group, Panel, Separator, type Layout } from 'react-resizable-panels';

import { LayoutResetContext } from './layoutReset';

export interface Pane {
  id: string;
  /** Names the pane in its separators' accessible labels. */
  label: string;
  /** Percentage of the group (`"30%"`) or pixels (`320`). */
  defaultSize?: number | string;
  minSize?: number | string;
  maxSize?: number | string;
  collapsible?: boolean;
  /** Height of the pane in the narrow (< 960 px) vertical flow; CSS length or pixels. */
  stackHeight?: number | string;
  content: ReactNode;
}

/** Default leaf height in the narrow flow: enough for a table or chart to be usable. */
const STACK_HEIGHT = 'max(240px, 45dvh)';

/** True inside a narrow vertical flow: nested flows grow with their content instead of scrolling. */
const StackContext = createContext(false);

/** The outermost narrow flow fills the workspace slot and scrolls; nested flows grow with content. */
const STACK_ROOT: CSSProperties = { display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0, overflowY: 'auto', overflowX: 'hidden' };
const STACK_NESTED: CSSProperties = { display: 'flex', flexDirection: 'column', height: 'auto' };

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
  const nested = useContext(StackContext);
  // Read once per mount and again after "Reset layout" (which cleared the stored sizes).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const defaultLayout = useMemo(() => loadLayout(id, panes), [id, reset]);
  if (narrow) {
    return (
      <div id={id} className={className ? `split split-stack ${className}` : 'split split-stack'} style={nested ? STACK_NESTED : STACK_ROOT}>
        {panes.map((p, i) => {
          const container = isValidElement(p.content) && p.content.type === SplitPane;
          const style: CSSProperties = {
            flex: 'none',
            height: container ? 'auto' : (p.stackHeight ?? STACK_HEIGHT),
            overflow: container ? 'visible' : 'hidden',
            borderTop: i > 0 ? '1px solid var(--border)' : undefined,
          };
          return (
            <div key={p.id} className="split-pane" data-pane={p.id} style={style}>
              {container ? <StackContext.Provider value>{p.content}</StackContext.Provider> : p.content}
            </div>
          );
        })}
      </div>
    );
  }
  return (
    <Group
      key={reset}
      id={id}
      orientation={orientation}
      className={className ? `split ${className}` : 'split'}
      defaultLayout={defaultLayout}
      onLayoutChanged={(layout, meta) => {
        if (!meta.isUserInteraction) return;
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
