// Canvas-based views (heatmaps, timelines, run charts) cannot use CSS custom properties directly,
// so they read the resolved token values from `:root` (styles/tokens.css, the only place colours
// are defined). Read at draw time, so the side-colour palette setting applies on the next draw.

/** Resolved value of a design token on `:root`. Throws for a name tokens.css does not define. */
export function cssVar(name: `--${string}`): string {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  if (value === '') throw new Error(`design token ${name} is not defined in styles/tokens.css`);
  return value;
}
