// Canvas-based views (the heatmap, the run charts) cannot use CSS custom properties directly, so
// they read the resolved values from `:root` and stay in step with `styles/theme.css`.

/** Resolved value of a custom property on `:root`, or `fallback` when it is not defined. */
export function cssVar(name: string, fallback: string): string {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value === '' ? fallback : value;
}
