// The frame of every workspace (UI_DESIGN_SPEC.md §5.4): a 32 px toolbar row (context pickers and
// at most one primary action on the left; row density and "Reset layout" on the right) above the
// workspace body. There is no page heading: the workspace name is in the switcher and in
// `document.title` as `<Workspace> · <context> — BlackBox HFT`.

import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { useLocation } from 'react-router-dom';

import { LayoutResetContext } from '../components/SplitPane';
import { Toolbar, ToolbarDivider } from '../components/Toolbar';
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
}

const DENSITIES: { id: Density; label: string }[] = [
  { id: 'comfortable', label: 'Comfortable' },
  { id: 'default', label: 'Default' },
  { id: 'compact', label: 'Compact' },
];

export function Workspace({ context, toolbar, layout = false, children, scroll = false }: WorkspaceProps) {
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
      <Toolbar label={`${name} toolbar`} className="ws-toolbar">
        {toolbar}
        <span className="spacer" />
        <label className="ws-density">
          <span className="visually-hidden">Row density</span>
          <select value={density} onChange={(e) => setDensity(e.target.value as Density)} title="Row density">
            {DENSITIES.map((d) => (
              <option key={d.id} value={d.id}>
                {d.label}
              </option>
            ))}
          </select>
        </label>
        {layout ? (
          <>
            <ToolbarDivider className="ws-reset" />
            <button type="button" className="ws-reset" onClick={resetLayout} title="Restore the default pane sizes of this workspace">
              Reset layout
            </button>
          </>
        ) : null}
      </Toolbar>
      <LayoutResetContext.Provider value={reset}>
        <div className={scroll ? 'ws-body ws-body-scroll' : 'ws-body'}>{children}</div>
      </LayoutResetContext.Provider>
    </div>
  );
}
