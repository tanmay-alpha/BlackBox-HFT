# BlackBox HFT design system

One dark, restrained system for every workspace. Colour values live only in
`web/src/styles/tokens.css`; shared components in `base.css` (elements, controls, badges, static
tables, notices) and `shell.css` (shell, panels, DataTable, tabs, tooltip, dialogs, status bar).
Feature CSS composes these and does not redefine them. The full rationale is in
`docs/engineering/UI_DESIGN_SPEC.md` §4–§6.

## Tokens

| Group | Tokens | Rule |
|---|---|---|
| Surfaces | `--bg-0` page/inputs, `--bg-1` panels/chrome, `--bg-2` controls/hover, `--bg-3` pressed, `--bg-selected` | Depth comes from surfaces and spacing, not outlines or shadow. |
| Lines | `--border-subtle` dividers, `--border` table header rule, `--border-control` inputs/buttons, `--border-strong` menus | One 1 px line per edge. |
| Text | `--fg-0` primary, `--fg-1` labels, `--fg-2` captions/notes/column headers, `--fg-disabled` | Never below `--fg-2` for readable text. |
| Accent | `--accent`, `--accent-hover`, `--on-accent`, `--focus-ring` | The only interaction colour: links, focus, selection, the one primary fill. |
| Meaning | `--bid/--ask`, `--up/--down`, `--observed/--modeled/--derived/--unobservable`, `--ok/--warn/--error` | Colour carries meaning only; a CVD palette swaps side colours. |
| Type | `--font-sans` (Inter), `--font-mono` (JetBrains Mono); `--text-2xs` 11 … `--text-xl` 24 | Body and explanatory prose are 14 px (`--text-sm`). 11–12 px only for badges, column headers, meta. |
| Space | `--sp-1` 4 … `--sp-8` 32 | 4 px grid. Panels pad 16 (12 on phones). |
| Size | `--h-control` 32 (40 on phones/touch), `--h-touch` 40, `--h-tab`, `--h-row` 36 (28 compact, 44 comfortable), `--h-statusbar` | Every control on a phone is ≥ 40 px. |
| Radius | `--radius-1` 6 controls/badges, `--radius-2` 8 panels/menus/dialogs | Nothing rounder; no pills-as-shapes. |
| Motion | `--dur-fast` 120 ms (0 under `prefers-reduced-motion`) | Colour/opacity transitions only; no fake-live animation. |

## Typography rule

Inter for everything people read: prose, labels, badges, pills, notice tags, column headers,
captions, `.note`, tooltips, the status bar. JetBrains Mono (tabular, slashed zero) only for
numbers, prices, quantities, timestamps, hashes and ids, opted in with `.mono` / `.num`, numeric
DataTable cells, numeric inputs and `<code>/<kbd>`. Key/value values (`.kv dd`) are Inter unless
the dd or a span carries `.mono`.

## Components

- **Buttons** — default (secondary): `--bg-2` + control border. `.btn-primary`: the single solid
  accent fill for the main action of a view (dark text). `.btn-ghost`: no chrome, for toolbars and
  panel actions. `.btn-danger`: error outline, tinted on hover, never filled. `.btn-link`: inline
  text action. `.btn-icon`: square, `--h-control`. Toggles use `aria-pressed` (accent outline +
  tint), visibly distinct from primary.
- **Inputs** — `--bg-0`, control border, accent border on focus, error border with
  `aria-invalid`. Numeric inputs are mono. Wrap in `.field` (label, control, unit, help, error).
- **Focus** — 2 px `--focus-ring`, offset 2 px; inset (−2 px) inside scrollers (tabs, rows,
  status bar). Never removed.
- **Tabs** — `Tabs` (in-page), `RouteTabs` (URL), `PaneTabs` (phone panes, full width, 40 px).
  Selected = accent underline + `--fg-0`.
- **Badges** (`Badge`, `BasisBadge`, `SideBadge`) — 11 px Inter semibold uppercase. Evidence bases
  differ by border style too: observed solid, modeled dashed, derived double, unobservable
  dotted. **Pills** (`Pill`) add a shape per state (square ok, ring warn, triangle error).
- **Panel** — `--bg-1`, 8 px radius (flush inside split panes); header: 14 px semibold title,
  optional basis badge, actions right; `caption` is 14 px `--fg-2` prose.
- **DataTable / `.table`** — sticky header in 12 px `--fg-2` Inter over a `--border` rule;
  `--h-row` rows, hover `--bg-2`, selection `--bg-selected` + accent edge; numeric cells right
  aligned mono; footer 12 px Inter.
- **Notice** — 2 px left rule in the tone colour, uppercase Inter tag, one sentence. **ErrorBox**
  prints the server's message verbatim in Inter; facts stay mono.
- **Status bar** — desktop: API, worker, build, access, verbatim research banner, UTC clock on
  one 32 px line (build/clock/queue hide below 1280 so the banner is never cut). Below 960 px:
  one 40 px line `API · worker · SIMULATED Research only` with a disclosure (`Status details`)
  that reveals the full banner, queue counts, build, access and clock. The verbatim banner is
  always in the DOM (title + screen-reader text).
- **Top bar on phones** — Menu button with the workspace name, then 40 px ghost icon actions
  (search, inspector, help) and the access badge (24 px visual, 40 px hit area). Sign out is in
  the command palette and System status.

## Usage rules

1. No decorative KPI cards, gradients, glows, or shadows except floating menus/dialogs.
2. One primary button per view; everything else secondary or ghost.
3. Keep Observed/Modeled/Derived/Unobservable labels and SYNTHETIC/SIMULATED warnings visible;
   never colour-only.
4. Hidden-but-announced text uses `.visually-hidden` (pinned to its containing block so it can
   never widen a scroll area).
5. Check 1440×900, 1024×768, 768×1024, 390×844, 360×800: no horizontal page overflow, Tab
   reaches every primary action, phone targets ≥ 40 px.
