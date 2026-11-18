// The frame of every workspace (UI_DESIGN_SPEC.md §5.4): a toolbar row (context pickers and at
// most one primary action on the left; a "View" disclosure with row density and "Reset layout" on
// the right) above the workspace body. The disclosure keeps those rarely used settings from
// taking a toolbar row of their own on narrow screens and labels the density select visibly.
// The workspace name is in the switcher and in `document.title` as
// `<Workspace> · <context> — BlackBox HFT`.

import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { useLocation } from 'react-router-dom';

import { LayoutResetContext } from '../components/layoutReset';
import { Toolbar } from '../components/Toolbar';
import { Spinner } from '../components/ui';
import { useSettings, type Density } from './settings';
import { workspaceFor } from './workspaces';

const APP_TITLE = 'BlackBox HFT';

/** Sets `document.title` while mounted. */
export function useDocumentTitle(workspace: string, context?: string | null): void {
  useEffect(() => {
    document.title = `${workspace}${context ? ` \u00b7 ${context}` : ''} \u2014 ${APP_TITLE}`;
    return () => {
      document.title = APP_TITLE;
    };
  }, [workspace, context]);
}

export interface WorkspaceProps {
  /** The entity in view (dataset id, run id, order), for the document title. */
  context?: string | null;
  /** Left side of the toolbar row. */
  toolbar?: ReactNode;
  /** The workspace arranges resizable panes; offers "Reset layout". */
  layout?: boolean;
  children: ReactNode;
  /** The body scrolls as one document instead of filling the area with docked panes. */
  scroll?: boolean;
  /** The page renders its own visible `h1`. Otherwise the frame supplies one for screen readers
   *  ("<Workspace> · <context>"), so every page has exactly one top-level heading. */
  ownHeading?: boolean;
}

const DENSITIES: { id: Density; label: string }[] = [
  { id: 'comfortable', label: 'Comfortable' },
  { id: 'default', label: 'Default' },
  { id: 'compact', label: 'Compact' },
];

export function Workspace({ context, toolbar, layout = false, children, scroll = false, ownHeading = false }: WorkspaceProps) {
  const { pathname } = useLocation();
  const def = workspaceFor(pathname);
  const name = def?.name ?? 'BlackBox HFT';
  useDocumentTitle(name, context);
  const { density, setDensity } = useSettings();
  const [reset, setReset] = useState(0);

  const resetLayout = () => {
    const prefix = `bb.layout.${def?.id ?? 'ws'}.`;
    for (const key of Object.keys(localStorage)) {
      if (key.startsWith(prefix)) localStorage.removeItem(key);
    }
    setReset((n) => n + 1);
  };

  return (
    <div className="ws">
      {ownHeading ? null : (
        <h1 className="visually-hidden">
          {name}
          {context ? ` \u00b7 ${context}` : ''}
        </h1>
      )}
      <Toolbar label={`${name} toolbar`} className="ws-toolbar">
        {toolbar}
        <span className="spacer" />
        <details className="ws-more ws-view ws-density">
          <summary title="Row density and pane layout">View</summary>
          <div className="ws-more-body">
            <label className="ws-view-field">
              Row density
              <select value={density} onChange={(e) => setDensity(e.target.value as Density)}>
                {DENSITIES.map((d) => (
                  <option key={d.id} value={d.id}>
                    {d.label}
                  </option>
                ))}
              </select>
            </label>
            {layout ? (
              <button type="button" className="ws-reset" onClick={resetLayout} title="Restore the default pane sizes of this workspace">
                Reset layout
              </button>
            ) : null}
          </div>
        </details>
      </Toolbar>
      <LayoutResetContext.Provider value={reset}>
        <div className={scroll ? 'ws-body ws-body-scroll' : 'ws-body'}>{children}</div>
      </LayoutResetContext.Provider>
    </div>
  );
}

/**
 * Shown in the workspace area while a workspace's code chunk loads: the same frame as
 * `Workspace` (an empty toolbar row of the same height above the body), so the page's first rows
 * do not move when the workspace replaces it. It shows no data; it is marked busy and announces
 * the load (with the elapsed time and the API's waking state, as every loading line does).
 */
export function WorkspaceLoading() {
  return (
    <div className="ws" aria-busy="true">
      <div className="toolbar ws-toolbar" aria-hidden="true" />
      <div className="ws-body">
        <div className="state-line">
          <Spinner label="Loading workspace" />
        </div>
      </div>
    </div>
  );
}
