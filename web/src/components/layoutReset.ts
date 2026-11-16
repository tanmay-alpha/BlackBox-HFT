// "Reset layout" (the workspace View menu) bumps this number; every SplitPane remounts with its
// default sizes. Kept apart from SplitPane so the workspace frame, which is in the entry chunk,
// does not pull react-resizable-panels into pages that have no resizable panes (PHASE32_PERFORMANCE).

import { createContext } from 'react';

export const LayoutResetContext = createContext(0);
