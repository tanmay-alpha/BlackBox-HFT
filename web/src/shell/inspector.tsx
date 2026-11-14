// The inspector (UI_DESIGN_SPEC.md §5.3): an optional right slot that shows details of the current
// selection, quant-term definitions and longer explanations. A workspace publishes its content
// with `useInspector`; without content the inspector shows the evidence-basis legend and the
// glossary. Open/closed is an arrangement setting (`i`, the palette, or its close button).

import { createContext, useContext, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';

interface InspectorContent {
  title: string;
  body: ReactNode;
}

interface InspectorApi {
  content: InspectorContent | null;
  set: (content: InspectorContent | null) => void;
}

const InspectorContext = createContext<InspectorApi | null>(null);

export function InspectorProvider({ children }: { children: ReactNode }) {
  const [content, set] = useState<InspectorContent | null>(null);
  const api = useMemo(() => ({ content, set }), [content]);
  return <InspectorContext.Provider value={api}>{children}</InspectorContext.Provider>;
}

export function useInspectorContent(): InspectorContent | null {
  return useContext(InspectorContext)?.content ?? null;
}

/**
 * Shows `body` in the inspector while the caller is mounted. `deps` decide when it is re-published
 * (like `useMemo`); pass the values the body renders.
 */
export function useInspector(title: string, body: ReactNode, deps: unknown[]): void {
  const ctx = useContext(InspectorContext);
  const set = ctx?.set;
  useEffect(() => {
    set?.({ title, body });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [set, title, ...deps]);
  useEffect(() => () => set?.(null), [set]);
}
