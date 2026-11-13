// The hosted read-only viewer (`VITE_STATIC_DATA=1` builds) announces itself on every page and
// explains every action it cannot perform.

import { isStaticViewer, STATIC_VIEWER_MESSAGE } from '../api/client';
import { Badge } from './ui';

const BANNER =
  'Hosted viewer: precomputed simulations on synthetic data. Not market data. Run locally to simulate.';

/** Persistent banner of the hosted viewer; renders nothing in the local console. */
export function StaticViewerBanner() {
  if (!isStaticViewer()) return null;
  return (
    <div className="shell-banner" role="note" aria-label="Hosted read-only viewer">
      <Badge kind="warn">hosted viewer</Badge>
      <span>{BANNER}</span>
    </div>
  );
}

/** Why an action is disabled in the hosted viewer; renders nothing in the local console. */
export function StaticViewerNote() {
  if (!isStaticViewer()) return null;
  return <p className="note">{STATIC_VIEWER_MESSAGE}</p>;
}
