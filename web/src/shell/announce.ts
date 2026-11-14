// The shell's polite live region (UI_DESIGN_SPEC.md §10.4: one announcement per user-caused
// transition). Workspaces announce through this instead of rendering their own regions.

import { createContext, useContext } from 'react';

export const AnnounceContext = createContext<(text: string) => void>(() => undefined);

export function useAnnounce(): (text: string) => void {
  return useContext(AnnounceContext);
}
