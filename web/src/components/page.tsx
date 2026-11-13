// Page-level chrome shared by the routed pages.

import { useEffect } from 'react';
import type { ReactNode } from 'react';

const APP_TITLE = 'BlackBox HFT';

/** Sets the document title while the calling page is mounted. */
export function usePageTitle(title: string): void {
  useEffect(() => {
    document.title = title ? `${title} \u00b7 ${APP_TITLE}` : APP_TITLE;
    return () => {
      document.title = APP_TITLE;
    };
  }, [title]);
}

export interface PageHeaderProps {
  title: ReactNode;
  /** One or two sentences under the title. */
  lead?: ReactNode;
  actions?: ReactNode;
}

export function PageHeader({ title, lead, actions }: PageHeaderProps) {
  return (
    <header className="page-header">
      <div className="page-header-text">
        <h1>{title}</h1>
        {lead ? <p className="page-lead">{lead}</p> : null}
      </div>
      {actions ? <div className="page-header-actions">{actions}</div> : null}
    </header>
  );
}
