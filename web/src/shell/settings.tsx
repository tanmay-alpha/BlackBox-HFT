// Arrangement preferences (UI_DESIGN_SPEC.md §9: how data is arranged lives in localStorage, never
// in the URL). Applied to <html> as data attributes the tokens read (`data-side-palette`,
// `data-density`).

import { createContext, useCallback, useContext, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';

export type Density = 'comfortable' | 'default' | 'compact';

export interface Settings {
  /** Single-character shortcuts (WCAG 2.1.4: can be turned off). */
  singleKeys: boolean;
  /** Side colours: default green/red or the colour-vision-deficiency blue/orange palette. */
  sidePalette: 'default' | 'cvd';
  railOpen: boolean;
  inspectorOpen: boolean;
}

const SETTINGS_KEY = 'bb.settings.v2';
const DENSITY_KEY = 'bb.density.v1';

// The labelled rail starts open on desktop widths; the inspector starts closed everywhere and
// opens on demand (`i`, or a selection that publishes inspector content).
const DEFAULTS: Settings = {
  singleKeys: true,
  sidePalette: 'default',
  railOpen: window.innerWidth >= 1280,
  inspectorOpen: false,
};

function read<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? fallback : { ...fallback, ...(JSON.parse(raw) as T) };
  } catch {
    return fallback;
  }
}

function readDensity(): Density {
  const v = localStorage.getItem(DENSITY_KEY);
  return v === 'comfortable' || v === 'compact' ? v : 'default';
}

interface SettingsApi {
  settings: Settings;
  density: Density;
  update: (patch: Partial<Settings>) => void;
  setDensity: (d: Density) => void;
}

const SettingsContext = createContext<SettingsApi | null>(null);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<Settings>(() => read(SETTINGS_KEY, DEFAULTS));
  const [density, setDensityState] = useState<Density>(readDensity);

  useEffect(() => {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
    document.documentElement.dataset.sidePalette = settings.sidePalette;
  }, [settings]);

  useEffect(() => {
    localStorage.setItem(DENSITY_KEY, density);
    document.documentElement.dataset.density = density;
  }, [density]);

  const update = useCallback((patch: Partial<Settings>) => setSettings((s) => ({ ...s, ...patch })), []);
  const api = useMemo(() => ({ settings, density, update, setDensity: setDensityState }), [settings, density, update]);
  return <SettingsContext.Provider value={api}>{children}</SettingsContext.Provider>;
}

export function useSettings(): SettingsApi {
  const api = useContext(SettingsContext);
  if (!api) throw new Error('useSettings outside SettingsProvider');
  return api;
}
