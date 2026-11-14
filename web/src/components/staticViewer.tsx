// The hosted read-only viewer (`VITE_STATIC_DATA=1` builds) announces itself in the status bar
// mode segment and in Overview, and explains every action it cannot perform.

import { isStaticViewer, STATIC_VIEWER_MESSAGE } from '../api/client';
import { Notice } from './ui';

export const STATIC_VIEWER_BANNER =
  'Hosted viewer: precomputed simulations on synthetic data. Not market data. Run locally to simulate.';

/** Notice of the hosted viewer; renders nothing in the local console. */
export function StaticViewerNotice() {
  if (!isStaticViewer()) return null;
  return (
    <Notice tag="Precomputed bundle" tone="info">
      {STATIC_VIEWER_BANNER}
    </Notice>
  );
}

/** Why an action is disabled in the hosted viewer; renders nothing in the local console. */
export function StaticViewerNote() {
  if (!isStaticViewer()) return null;
  return <p className="note">{STATIC_VIEWER_MESSAGE}</p>;
}
