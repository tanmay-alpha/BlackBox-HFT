# UI design specification — BlackBox HFT

**BlackBox HFT — Market Microstructure & Execution Research Laboratory**

Status: Phase 0 design specification (audit + research). It describes the target console, not
the current one. Implementers should be able to build the shell, the tokens and every workspace
from this document without making further design decisions. Where a decision depends on a backend
contract that another Phase 0 document owns (job API, auth header, health fields), this document
lists exactly what the UI needs and marks the dependency.

Evidence conventions used below:

- `path:line` is a file in this repository at HEAD `5fce27d`.
- `[S1]`… are external sources, listed with URLs in [§17 Sources](#17-sources). All were read on
  2026-11-14.
- `[INFERENCE]` marks a conclusion of mine that no source states directly.
- Contrast ratios in §4 were computed with the WCAG 2.2 relative-luminance formula [S20] by a script
  I ran during this audit. The script and its output are reproduced in [§4.3](#43-computed-contrast-ratios).

Decisions taken by the user that this spec applies (relayed by Main, 2026-11-14):

1. The hosted system has a **public read-only demo workspace**. Anyone can browse precomputed and
   stored experiments.
2. **Authenticated research access uses API keys.** The user pastes a key, and the UI keeps it in
   `sessionStorage`. Mutating actions (create experiment, cancel, delete, sweeps) work only when
   authenticated. Without a key they are still shown, but disabled with the reason.
3. **Jobs are asynchronous**: `created → queued → running → completed | failed`, and
   `→ cancelling → cancelled`, plus `expired`. They run on a slow free host: Render Free gives
   0.1 CPU, and a service spun down after idle takes about one minute to start [S24]. The
   Simulation Workbench and the status bar must show queue position, progress, heartbeat and the
   cold-start state honestly.
4. No Lovable or v0 credentials exist in this environment.

---

## Contents

1. [Current console inventory](#1-current-console-inventory)
2. [Institutional interface principles (research)](#2-institutional-interface-principles-research)
3. [Lovable and v0 evaluation](#3-lovable-and-v0-evaluation)
4. [Identity and design tokens](#4-identity-and-design-tokens)
5. [Layout shell](#5-layout-shell)
6. [Component inventory](#6-component-inventory)
7. [Numeric and time formatting](#7-numeric-and-time-formatting)
8. [Keyboard model](#8-keyboard-model)
9. [URL state conventions](#9-url-state-conventions)
10. [Connection, authentication and job states](#10-connection-authentication-and-job-states)
11. [Workspaces and wireframes](#11-workspaces-and-wireframes)
12. [Existing page → workspace mapping and reuse](#12-existing-page--workspace-mapping-and-reuse)
13. [Anti-pattern checklist](#13-anti-pattern-checklist)
14. [Accessibility checklist](#14-accessibility-checklist)
15. [Implementation order and acceptance](#15-implementation-order-and-acceptance)
16. [Backend dependencies the UI needs](#16-backend-dependencies-the-ui-needs)
17. [Sources](#17-sources)

---

## 1. Current console inventory

### 1.1 Stack

- React 19, React Router 7, Vite 7, TypeScript 5.9 and `lightweight-charts` 5.3 (`web/package.json`).
  There is no CSS framework and no component library. Styling is plain CSS with custom properties:
  `web/src/styles/theme.css` and `web/src/styles/app.css`, plus one sheet per feature
  (`features/*/*.css`).
- Hosting: the static viewer is on Vercel, and `web/vercel.json:17-18` sets a strict CSP
  (`default-src 'self'; … font-src 'self'; connect-src 'self'`). Fonts must therefore be
  self-hosted. A cross-origin API (Render) will need `connect-src` widened, or a same-origin
  rewrite (see §16).

### 1.2 Routes (`web/src/App.tsx:187-208`) and primary navigation (`App.tsx:34-42`)

| Route | Component | Nav label | What it is (from code + `docs/screenshots/*.png`) |
|---|---|---|---|
| `/` | `features/mission/MissionControlPage.tsx` | Mission Control | Three "canonical experiment" cards (cost of latency, queue position uncertainty, gross vs net) with **Run demo**, then a table of recent runs (`mission.png`). |
| `/datasets` | `features/datasets/DatasetsPage.tsx` | Datasets | Per-dataset identity, events, source, timestamp provenance, input files, data-quality report with 15 checks (`datasets.png`). |
| `/replay/:datasetId` | `features/replay/ReplayPage.tsx` (+ `HeatmapView.tsx`, `heatmap.ts`, `DepthLadder.tsx`) | (under Datasets) | Market-only order book heatmap with playback, zoom and a depth ladder at the cursor. Query `t0`, `len`, `levels` (`ReplayPage.tsx:53-60`; `replay.png`). |
| `/lab` | `features/lab/LabPage.tsx` (+ `FieldInput.tsx`, `fields.ts`, `validate.ts`, `ScriptedOrdersEditor.tsx`) | Strategy Lab | Long numbered form: 1 Dataset, 2 Presets, 3 Configuration, 4 Scenario A, 5 Run. Query `dataset`, `from` (`LabPage.tsx:137-139`; `lab.png`). |
| `/runs` | `features/runs/RunsPage.tsx` | Runs | Stored runs table, pick two to compare. |
| `/runs/:runId` | `features/runs/RunOverviewPage.tsx` (+ `TimeSeriesChart.tsx`) | (under Runs) | Reproducibility manifest, instrument, engine cross-check, run metrics, account charts, orders and fills tables (`run.png`). |
| `/runs/:runId/replay` | `features/terminal/ReplayTerminalPage.tsx` (+ `TerminalHeatmap.tsx`, `TerminalLadder.tsx`, `EventLog.tsx`, `overlay.ts`) | (under Runs) | Heatmap with own orders and fills overlaid, depth at the cursor, account at the cursor, event log, orders alive. Query `t`, `order` (`ReplayTerminalPage.tsx:176-178`; `terminal.png`). |
| `/runs/:runId/orders/:orderId` | `features/forensics/OrderForensicsPage.tsx` (+ `TwoClockTimeline.tsx`, `QueueChart.tsx`, `panels.tsx`, `timeline.ts`, `evidence.tsx`, `eventFormat.ts`, `labels.ts`) | (under Runs) | Two-clock timeline, modeled queue position, explanation, decision context, latency breakdown, fills, flight-recorder events (`forensics.png`). |
| `/latency` | `features/latency/LatencyLabPage.tsx` | Latency Lab | Baseline vs N runs: varied vs controlled, key metrics with deltas, decision alignment, gross to net. Query `runs`, `demo` (`LatencyLabPage.tsx:133-135`; `latency.png`). |
| `/compare` | `features/compare/ComparePage.tsx` (+ `EquityOverlay.tsx`) | Compare | A/B runs: what changed, measured outcomes, order alignment, first divergence, equity overlay. Query `a`, `b` (`ComparePage.tsx:60-62`; `compare.png`). |
| `/grid` | `features/grid/GridPage.tsx` (+ `gridModel.ts`) | Sensitivity Grid | Two-axis parameter sweep (≤ 100 cells), diverging-colour result matrix, cells table. Query `base` (`GridPage.tsx:48-51`; `grid.png`). |
| `*` | `NotFoundPage` (`App.tsx:172-185`) | — | Not found. |

### 1.3 Shared building blocks worth keeping

| Module | Contents | Reuse decision |
|---|---|---|
| `components/fmt.ts` | Pure, locale-independent formatters: `fmtDuration` (3 significant digits, ns/µs/ms/s; `fmt.ts:21-37`), `fmtDurationExact`, `fmtClock` (`mm:ss.mmmuuu` relative to origin; `fmt.ts:69-85`), `fmtPrice`/`fmtQty` with decimals derived from `tick_size`/`lot_size` (`fmt.ts:97-117`), `fmtSigned`, `fmtPct`, `fmtBytes`, `shortHash`, and `fmtOriginUtc` using BigInt over the decimal-string origin (`fmt.ts:170-185`). | **Keep and extend.** It becomes the single formatting module of §7. The ns-string functions in §7.3 are added here. |
| `components/ui.tsx` | `Panel`, `Badge` (13 kinds; `ui.tsx:33-46`), `BasisBadge`/`BasisLegend` (evidence bases; `ui.tsx:63-95`), `Stat`, `Spinner`, `ErrorBox` (server error code + message verbatim; `ui.tsx:131-155`), `Empty`, `ClockText`, `HashText`, and the race-safe `useAsync` (`ui.tsx:213-271`). | **Keep the API and restyle.** `Panel` gains a toolbar slot and a menu (§6.3). |
| `components/describe.ts` | Plain-language labels and help text for every configuration option, worded to match `crates/bb-core/src/config.rs` (`describe.ts:1-3`). | **Keep.** It is the source of the quant-term tooltips (§6.14). |
| `components/data.tsx` | `QuantileTable`, `JsonBlock`, `Pager`. | Superseded by `DataTable` (§6.1). `JsonBlock` is kept. |
| `components/page.tsx` | `usePageTitle`, `PageHeader`. | `usePageTitle` is kept. `PageHeader` is replaced by the workspace header strip (§5.4). |
| Canvas renderers | `replay/heatmap.ts`, `terminal/TerminalHeatmap.tsx`, `forensics/TwoClockTimeline.tsx`, `forensics/QueueChart.tsx`, `compare/EquityOverlay.tsx`, `runs/TimeSeriesChart.tsx` | **Keep.** Only their colours change, through `components/theme.ts` `cssVar()`. |
| Evidence-basis system | Observed / modeled / derived / unobservable, each with its own colour and a distinct border style: solid, dashed, double, dotted (`theme.css:1-8`). | **Keep.** It is the product's honesty mechanism and works without colour vision. |
| Research banner | `RESEARCH_BANNER` text "fixed by the product's honesty rules" (`App.tsx:22-24`). | **Keep the exact wording** and move it into the status bar (§5.5). |

### 1.4 Current tokens (`web/src/styles/theme.css:10-37`)

`--bg #0f1113`, `--surface #15181b`, `--surface-2 #1d2125`, `--border #2c3238`, `--fg #d7dbe0`,
`--muted #929ba5`, `--accent #9db8dc`, `--buy #4fbf8d`, `--sell #ee7468`, `--observed #58c4dd`,
`--modeled #e2a83a`, `--derived #b596ee`, `--unobservable #a3a9b1`, `--warn #d8c64e`,
`--error #ff6157`, `--radius 3px`, `--gap 12px`. Body text is 13 px with a system sans
(`theme.css:49-58`). Tables are 12 px with 3×8 px cell padding and sticky uppercase 11 px headers
(`theme.css:250-282`). Focus is a 2 px accent outline at offset 2 (`theme.css:143-147`).
Reduced-motion is handled for the spinner (`theme.css:727-733`).

### 1.5 Audit findings (from the code and from the 10 screenshots in `docs/screenshots/`)

What already fits the target identity: a dark neutral palette, 3 px radii, 1 px borders, no
gradients or glow, mono numerics with `tabular-nums`, uppercase panel titles, and dense tables. Colour
is reserved for meaning (`theme.css:4-7`). Explicit "downsampled" and "events truncated" notices
appear in `terminal.png` and `replay.png`.

What does not fit:

1. **Document pages, not a workspace.** Every route is one long vertical scroll of full-width
   panels. `lab.png` shows a numbered five-step form, and `run.png` stacks manifest, cross-check,
   metrics, charts, orders and fills. Users cannot see two related views side by side unless the
   page author put them there. Nothing can be resized or rearranged, and `.shell-main` caps width at
   2000 px (`app.css:181-183`).
2. **Navigation is by page, not by research task.** There are seven top-level links
   (`App.tsx:34-42`). Run-scoped views (replay terminal, order forensics) are reached only through
   links inside other pages, and the active run or dataset context is not persistent.
3. **Health is buried in the footer.** `ServerStatus` (`App.tsx:73-115`) sits below the fold and
   shows no worker or queue state, because none exists yet: `POST /api/runs` is synchronous
   (`docs/API.md:48-51`) and nothing can be cancelled (`TODO.md:69-70`).
4. **Prose in the work surface.** Long explanatory paragraphs sit inside panels (`mission.png`,
   `grid.png`, `forensics.png`). They are valuable for honesty, but they push data below the fold.
   Target: one-line captions with the full text in an inspector or tooltip. The honesty wording
   stays verbatim.
5. **Inconsistent units and precision across pages.** `compare.png` shows P&L with 4 dp,
   `mission.png` shows 2 dp, `latency.png` shows raw `1000000` ns in the varied/controlled table
   while `compare.png` humanises it, and both "bp" and "bps" appear. §7 fixes one rule per quantity.
6. **No shared table component.** Each page hand-rolls `<table className="table">`. Tables have no
   virtualization, sorting, column units or keyboard navigation, and pagination differs per page.
7. **Nanosecond values arrive as JS numbers** for every relative timestamp (`docs/API.md:8-9`);
   only `origin_ns` is a string. The backend will move all ns values to decimal strings, and §7.3
   defines formatting that never converts them to floating point.

---

## 2. Institutional interface principles (research)

Only public documentation and marketing pages were read. The table records the principles. No
asset, layout or branding is copied.

| # | Principle | Evidence | Rule for BlackBox |
|---|---|---|---|
| P1 | **One window, many tools, user-arranged.** TWS Mosaic is "completely customizable, with a drag and snap functionality", combining order entry, order monitor, portfolio, charts and watchlists in one window. LSEG Workspace layouts show "multiple applications side by side", with up to eight tabs per window. | TWS Mosaic layout [S1]; LSEG Workspace quick start, "Layouts" and "Tabs" [S4] | Each workspace is a set of resizable panels with tab groups (§5.3). Panels can move between the slots of a workspace. Layouts persist per user. |
| P2 | **Linked context ("colour groups").** In TWS, panels assigned to the same colour group follow the instrument selected in any one of them. A window is a source, a destination, or both. TradingView multi-chart layouts sync symbol, interval, crosshair, time and date range. | TWS colour grouping [S2]; TradingView sync help [S6]; Trading Platform multiple-chart layout [S5] | Every panel in a workspace follows one **context** (dataset, run, order, cursor time) held in the URL (§9). There is no colour-group UI, because one research context per workspace is enough and avoids colour-only coding `[INFERENCE]`. Time cursor and zoom are synced across all time-axis panels of a workspace. |
| P3 | **Fixed preset layouts first, free docking second.** TradingView names a fixed vocabulary of layouts (`2h`, `3s`, `1-2`, …), and splitters resize within them. | [S5] | Each workspace ships a named default layout (§11). Users resize splitters and move panels between the slots. Arbitrary floating windows are not provided. |
| P4 | **Keyboard-first with discoverable, remappable shortcuts.** TWS lets users record hot keys for actions, shows mapped keys on the status line, and refuses reserved keys. TradingView documents a shortcut table (Ctrl+K quick search, arrows step bars, Ctrl+↑/↓ zoom, Alt+G go to date, Alt+R reset) and an opt-in keyboard-navigation mode. | TWS hot keys [S3]; TradingView shortcuts [S7] | §8: Ctrl+K command palette, a `?` overlay that lists every binding, single-key shortcuts that can be disabled (WCAG 2.1.4 [S20]), and browser-reserved keys never overridden. |
| P5 | **Bid/ask colour is a convention, not a universal.** IBKR BookTrader shows the best bid in **yellow** and the best ask in **green**. Common web practice (and the current BlackBox console) uses green bid / red ask. | BookTrader colours [S8]; `theme.css:21-22` | Keep green-bid/red-ask (existing users, chart conventions). Never rely on colour alone: the ask sits above the bid, every ladder row carries a side, and a colour-vision-deficiency palette is selectable (§4.2). |
| P6 | **Order lifecycle states are first-class blotter columns.** EMSX documents explicit order and route states (`NEW`, `SENT`, `WORKING`, `PARTFILLED`, `FILLED`, `CXLREQ`, `CANCEL`, `REJECTED`, `EXPIRED` …) and transition tables. NautilusTrader's order report carries `status`, `quantity`, `filled_qty` and `avg_px`. | EMSX subscription [S9]; NautilusTrader reports [S10] | The Execution Forensics blotter (§11.6) shows lifecycle status as a text pill plus columns for filled/remaining quantity. Job states (§10.4) are modelled the same way: explicit, transition-checked, never inferred. |
| P7 | **Exact values travel as strings; nanosecond timestamps are first-class.** The NautilusTrader reports give quantities and prices as **strings** and timestamps as **Unix nanoseconds**. Its data model distinguishes `ts_event` from `ts_init`. | [S10], [S11] | §7: formatting never goes through float for ns values. Two clocks (exchange/local) stay labelled everywhere (already true in `TwoClockTimeline.tsx`). |
| P8 | **Backtest results page = runtime banner + charts + orders + logs + overview stats + report tab.** QuantConnect shows runtime statistics in a banner, built-in charts whose zoom applies to all charts, an orders tab where clicking an order reveals its events, and a downloadable report. | QuantConnect backtest results [S12] | Risk & Performance (§11.7) uses a KPI strip, synced charts and drill-down. Research Reports (§11.9) is a separate workspace. BlackBox adds evidence-basis badges, which QuantConnect does not have. |
| P9 | **Downsampling is disclosed.** QuantConnect states its chart-point quotas per tier and when data is skipped. BlackBox already shows `DOWNSAMPLED` and `EVENTS TRUNCATED` notices. | [S12]; `terminal.png` | Every chart that downsamples or truncates shows an inline notice with the ratio (§6.10). |
| P10 | **Accessibility in professional charting is still partial.** TradingView states it is "actively working" toward WCAG 2.2. | [S7] (accessibility page linked from the shortcuts docs) | Canvas charts must have a keyboard path and a tabular alternative (§14). |

---

## 3. Lovable and v0 evaluation

### 3.1 Lovable

| Question | Finding | Source |
|---|---|---|
| Can it attach to an **existing** GitHub repo and edit it in place? | **No.** "Git sync does not import a repository. When you connect a project to GitHub, GitLab, or Bitbucket, Lovable creates a new repository from the project, and you cannot connect an existing one." Linking a project creates a new repo, private by default. | Lovable FAQ [S13]; GitHub sync docs [S14] |
| Can it import existing code at all? | Only as a **ZIP upload** (≤ 250 MB). Lovable then "rebuilds the app the way it builds new Lovable apps, on TanStack Start", moves the database and users to Lovable's built-in backend, and replaces the app's sign-in. | Import a project [S15] |
| Stack | New apps since 2026-05-13 use TanStack Start. Older apps use React + Vite. There is no choice of framework. Backends are Lovable's built-in backend or Supabase. | FAQ [S13] |
| Free tier | 5 daily build credits capped at 30/month, a daily chat allowance, Git sync. **No code editing, no code download, no custom domain on Free.** | Subscription plans [S16] |
| Paid | Pro from $25/month for 100 credits. Business from $50/month. | [S16] |
| Ownership / export | "Your apps, code, and the content you create with Lovable are yours." Download is on paid plans; Git sync is on any plan. | FAQ [S13] |
| Training | Since 2026-09-09, Free and Pro data may be used for model training unless the user opts out. | FAQ [S13] |

**Fit for BlackBox: poor.** An import would rewrite the Vite SPA onto TanStack Start and push a
Lovable/Supabase backend into a project whose API is a Rust/axum service (`docs/ARCHITECTURE.md`).
That breaks the "browser only renders what the API returns" boundary (`docs/ARCHITECTURE.md:156`).
It also cannot sync with `tanmay-alpha/BlackBox-HFT` itself, so every change would need a manual
port back.

### 3.2 Vercel v0

| Question | Finding | Source |
|---|---|---|
| Existing repo? | **Yes.** It imports public repositories and private ones the Vercel GitHub App can access. It creates a chat, a Vercel Sandbox with the repo checked out, an isolated working branch on the first change, and a preview deployment. It never pushes to the base branch directly; publishing goes through a pull request. | Import from GitHub [S17]; GitHub docs [S18] |
| Output stack | "v0 uses Next.js, React, TypeScript, Tailwind CSS, and shadcn/ui." | v0 FAQ [S19] |
| Free tier | $0, 7 messages/day, a one-time $5 credit for new users, GitHub sync, Design Mode. | v0 pricing docs [S21]; pricing page [S22] |
| Paid | Plus $30/user/month, Business $100/user/month. Data opt-out only on Business and Enterprise. | [S21] |
| Ownership | "Vercel doesn't own the code generated based on your queries and prompts." The user is responsible for checking that output is not similar to third-party IP. | v0 FAQ [S19] |

**Fit for BlackBox: possible, but low value.** v0 can work on this repo through branches and PRs.
However, its default idiom (Tailwind + shadcn/ui) would be a second styling convention beside
the plain-CSS-token system. The repo rules forbid adding a second convention. With 7 messages/day
on Free, it cannot build a ten-workspace console.

### 3.3 Recommendation

1. **Do not use Lovable** for this project. It cannot edit the existing repository, it rebuilds
   onto a different framework and backend, and Free has no code export.
2. **Do not plan on v0 either.** At most, a human with their own account may use v0 in a
   throwaway chat (not an import of this repo) to explore the look of an isolated component.
   The result is then re-implemented by hand against the tokens in §4. No Tailwind or shadcn code
   enters `web/`.
3. **Primary path:** implementation agents build the shell and components by hand from this spec,
   in the existing React + Vite + plain-CSS stack, adding only the three small MIT/ISC/OFL
   dependencies named in §6.
4. **No Lovable or v0 credentials are available to agents in this environment.** Any plan that
   needs either tool is blocked by construction.

---

## 4. Identity and design tokens

### 4.1 Identity

- Name in the top bar: **BlackBox HFT**. Subtitle, shown only in the System workspace and the
  document `<title>` suffix: *Market Microstructure & Execution Research Laboratory*.
- Mark: a 12×12 px square outline (1 px `--fg-0`) containing a 4×4 px filled square. This is the
  current `brand-mark` idea (`app.css:41-47`) at 12 px. It has no gradient and no animation.
- Tone: instrument panel. Neutral charcoal and graphite surfaces carry no meaning. Colour appears
  only for side (bid/ask), sign (up/down), evidence basis, and status.

### 4.2 Colour tokens (exact values)

Replace `:root` in `theme.css` with the following. Old names are mapped in the right-hand comment
for a mechanical migration. Remove the aliases after migration.

```css
:root {
  color-scheme: dark;

  /* Surfaces: charcoal → graphite. Lighter = closer to the user. */
  --bg-0: #0E0F11;          /* app background, input fill, chart canvas   (was --bg)        */
  --bg-1: #141518;          /* panel body                                 (was --surface)   */
  --bg-2: #1B1D21;          /* panel header, table header, row hover, popover (was --surface-2) */
  --bg-3: #23262B;          /* pressed control, active tab                                   */
  --bg-selected: #1A2636;   /* selected row / selected cell range                            */

  /* Lines */
  --border-subtle: #25282D; /* table row rules, inner dividers                               */
  --border: #30343A;        /* panel edges, splitters at rest              (was --border)    */
  --border-strong: #454B53; /* popover/dialog edge, splitter hover                           */
  --border-control: #646B74;/* input, select, checkbox, button outline (≥3:1, WCAG 1.4.11)   */

  /* Text */
  --fg-0: #E4E6E9;          /* primary text, numbers                       (was --fg)        */
  --fg-1: #A9AFB7;          /* secondary text, labels                                        */
  --fg-2: #868D96;          /* tertiary: units, captions, axis labels      (was --muted)     */
  --fg-disabled: #5B6169;   /* disabled text only (exempt from 1.4.3; never for information)  */

  /* Interaction */
  --accent: #7FA7D9;        /* focus ring, links, active tab bar, cursor line (was --accent) */

  /* Market semantics (default palette) */
  --bid: #3FB68B;   --ask: #EB6F66;       /* side                    (was --buy / --sell) */
  --up: #3FB68B;    --down: #EB6F66;      /* sign of P&L / delta (separate tokens on purpose) */
  --bid-weak: #1C322D; --ask-weak: #3B2526; /* row tint behind best bid/ask, 18 % mix on --bg-1 */
  --bid-bar: #2C6E57;  --ask-bar: #8A4643;  /* depth bars, 55 % mix on --bg-1 */

  /* Evidence bases (product semantics; border style also differs, see §6.6) */
  --observed: #62B8D6;
  --modeled: #D9A441;
  --derived: #A99CDB;
  --unobservable: #9AA0A8;

  /* Status */
  --ok: #74BE87;
  --warn: #D8C25A;
  --error: #F2675C;
  --info: #7FA7D9;          /* same value as --accent */
}

/* Colour-vision-deficiency palette: user setting "Side colours: blue/orange". */
:root[data-side-palette='cvd'] {
  --bid: #5AA2E6;  --ask: #E8953A;
  --up:  #5AA2E6;  --down: #E8953A;
  --bid-weak: #212E3D; --ask-weak: #3A2C1E;   /* 18 % mix on --bg-1 (same rule as default) */
  --bid-bar: #3B6389;  --ask-bar: #895B2B;    /* 55 % mix on --bg-1 */
}
```

Rules:

- Use `--up`/`--down` only for the sign of P&L, deltas and returns. Use `--bid`/`--ask` only for
  side. They share a value by default, but are separate tokens so that a palette can split them.
- Never colour quantities, prices, counts or durations except the best bid/ask price in a ladder.
- Each evidence colour appears only as badge text and border colour, never as a fill, glow or
  background.
- `--modeled` and `--warn` are close (1.26:1 between them). They are told apart by the dashed
  border and the uppercase label `MODELED` vs the `WARN` prefix. Never place a modeled badge
  inside a warning notice.
- `--ask`, `--down` and `--error` are all reds. Errors always carry the text prefix `ERROR` or
  `FAILED`, never colour alone.

### 4.3 Computed contrast ratios

Computed with WCAG relative luminance [S20]. The script (Python, run in this audit):

```python
def lum(h):
    r,g,b=[int(h[i:i+2],16)/255 for i in (1,3,5)]
    f=lambda c: c/12.92 if c<=0.04045 else ((c+0.055)/1.055)**2.4
    return 0.2126*f(r)+0.7152*f(g)+0.0722*f(b)
cr=lambda a,b:(max(lum(a),lum(b))+0.05)/(min(lum(a),lum(b))+0.05)
```

Text tokens against surfaces. The threshold is 4.5:1 for normal text (SC 1.4.3) [S20]:

| Token | Hex | on bg-0 | on bg-1 | on bg-2 | on bg-3 | on bg-selected |
|---|---|---|---|---|---|---|
| fg-0 | #E4E6E9 | 15.34 | 14.60 | 13.50 | 12.14 | 12.22 |
| fg-1 | #A9AFB7 | 8.68 | 8.26 | 7.64 | 6.87 | 6.91 |
| fg-2 | #868D96 | 5.72 | 5.45 | 5.03 | 4.53 | 4.56 |
| fg-disabled | #5B6169 | 3.07 | 2.92 | 2.70 | 2.43 | — |
| accent / info | #7FA7D9 | 7.70 | 7.33 | 6.78 | 6.09 | 6.13 |
| bid / up | #3FB68B | 7.56 | 7.19 | 6.65 | 5.98 | 6.02 |
| ask / down | #EB6F66 | 6.39 | 6.08 | 5.62 | 5.06 | 5.09 |
| bid (cvd) | #5AA2E6 | 7.07 | 6.73 | 6.22 | 5.60 | — |
| ask (cvd) | #E8953A | 8.03 | 7.64 | 7.07 | 6.35 | — |
| observed | #62B8D6 | 8.53 | 8.13 | 7.51 | 6.76 | — |
| modeled | #D9A441 | 8.53 | 8.12 | 7.50 | 6.75 | 6.79 |
| derived | #A99CDB | 7.73 | 7.36 | 6.80 | 6.12 | — |
| unobservable | #9AA0A8 | 7.28 | 6.93 | 6.40 | 5.76 | — |
| ok | #74BE87 | 8.64 | 8.22 | 7.60 | 6.84 | — |
| warn | #D8C25A | 10.76 | 10.25 | 9.47 | 8.52 | — |
| error | #F2675C | 6.27 | 5.97 | 5.52 | 4.97 | 5.00 |

Other measured pairs:

- `--fg-0` on `--bid-weak`: 10.88. `--fg-0` on `--ask-weak`: 11.36. `--bid` on `--bid-weak`: 5.36.
  `--ask` on `--ask-weak`: 4.73.
- Non-text (SC 1.4.11, threshold 3:1): `--border-control` #646B74 is 3.56 on bg-0, 3.39 on bg-1
  and 3.13 on bg-2. Inputs therefore sit on bg-1 or bg-2 only, never on bg-3.
- `--accent` as a focus ring is 7.33 on bg-1. Depth bars (`--bid-bar` 3.02, `--ask-bar` 2.64 on
  bg-1) are decorative magnitude cues: the quantity is always printed beside them.
- `--border`/`--border-subtle` (1.46/1.23 on bg-1) are deliberately low. They separate regions
  that also differ in content. They are never the only boundary of an interactive control.
- `bid` vs `ask` against each other: 1.18 (default) and 1.14 (cvd). **They are distinguishable by
  hue only**, so position and labels are mandatory (§14).

Usage constraints derived from the table:

1. `fg-2` is allowed on bg-0…bg-3 and bg-selected (all ≥ 4.5). It is not allowed on `--bid-weak`
   or `--ask-weak`; use fg-1 there (fg-1 measures 6.16/6.43 on the default tints and 6.24/6.10 on
   the CVD tints).
2. `fg-disabled` is used only for disabled controls, which SC 1.4.3 exempts. A disabled action that
   needs an explanation (unauthenticated mutations, §10.2) keeps its label in `fg-1` and gets a
   lock glyph, so the reason stays readable.

### 4.4 Typography

| Role | Face | Licence / package (version read from npm registry 2026-11-14) |
|---|---|---|
| UI text (labels, prose, headings) | **Inter**, fallback `system-ui, 'Segoe UI', Roboto, sans-serif` | OFL-1.1, `@fontsource/inter` 5.3.0 |
| All numbers, timestamps, ids, hashes, config paths, code | **JetBrains Mono**, fallback `'Cascadia Mono', ui-monospace, Consolas, monospace` | OFL-1.1, `@fontsource/jetbrains-mono` 5.3.0 |

Both fonts are self-hosted from `/assets`, as CSP `font-src 'self'` requires (`web/vercel.json:18`).
Load only these weights: Inter 400/500/600, latin subset; JetBrains Mono 400/600, latin subset.
Use `font-display: swap`. A monospaced face gives every digit the same advance by construction.
`font-variant-numeric: tabular-nums slashed-zero` is still set on `.num` for the fallback faces.

Scale (px size / line height). Six steps; no other sizes may appear:

| Token | Size / LH | Face | Use |
|---|---|---|---|
| `--text-2xs` | 11 / 14 | mono | badges, status bar, chart axis labels, units under KPI values |
| `--text-xs` | 11 / 16 | sans | panel titles (600, uppercase, tracking 0.06em), table header labels (500, uppercase, 0.04em), captions |
| `--text-sm` | 12 / 16 | sans + mono | **default for data**: table cells, controls, tabs, inspector key/values |
| `--text-md` | 13 / 18 | sans | prose in inspectors and reports, form labels, dialogs |
| `--text-lg` | 15 / 20 | mono | KPI values in the KPI strip |
| `--text-xl` | 18 / 24 | sans 600 | title of a Research Report document only |

Weights: 400 body, 500 labels and headers, 600 titles and the best-bid/ask row. Never use 700 or
more. Never use italics for data.

### 4.5 Spacing, sizes, borders, radii, elevation, motion

**4 px grid.** Every margin, padding, gap and fixed size is a multiple of 4. The only exceptions
are 1 px borders, 2 px focus/indicator lines and the 2 px focus offset.

```css
:root {
  --sp-1: 4px;  --sp-2: 8px;  --sp-3: 12px; --sp-4: 16px;
  --sp-5: 20px; --sp-6: 24px; --sp-8: 32px;

  --h-topbar: 36px;   --h-statusbar: 24px;
  --w-rail: 48px;     --w-rail-open: 208px;
  --h-panel-head: 28px; --h-toolbar: 32px; --h-tab: 28px;
  --h-control: 24px;  /* buttons, inputs, selects (min target 24×24, WCAG 2.5.8) */
  --h-row: 24px;      /* table rows, default density */
  --h-row-compact: 20px; /* opt-in density; rows are not the only target (see §14) */
  --w-inspector: 320px;

  --radius-0: 0;      /* panels, tables, bars, status bar: docked surfaces abut */
  --radius-1: 2px;    /* buttons, inputs, badges, tabs, pills */
  --radius-2: 3px;    /* popovers, menus, tooltips, dialogs, toasts — the maximum */

  --shadow-float: 0 4px 12px rgba(0, 0, 0, 0.45); /* floating layers only; never coloured */
  --scrim: rgba(0, 0, 0, 0.55);                   /* modal backdrop; no blur */

  --dur-fast: 120ms;  /* opacity of tooltip/popover; nothing else animates by default */
}
@media (prefers-reduced-motion: reduce) { :root { --dur-fast: 0ms; } }
```

Border rules:

- All borders are 1 px solid, with three exceptions. The 2 px `--accent` focus outline sits at
  offset 2 (offset −2 inside table cells). The active tab has a 2 px `--accent` bottom bar. Evidence
  badges use dashed/double/dotted borders.
- Panels in the docked layout have no radius and share a 1 px `--border` gutter. There are no gaps
  between docked panels; the splitter is the gap.
- No `box-shadow` on anything docked. `--shadow-float` is allowed only on menus, popovers,
  tooltips, dialogs and toasts.
- **No gradients, no `backdrop-filter`, no glow, no coloured shadows, no translucent panels.** The
  only translucency is the modal scrim.

Motion rules:

- Hover and focus change colour instantly. Tooltips and popovers fade in over `--dur-fast`, with
  opacity only and no transform.
- The only continuous animation is **user-started replay playback**. Loading uses static text with
  an elapsed-time counter. The spinner (`ui.tsx:122-129`) may remain, but it is replaced by text
  under reduced motion, as today.
- No skeleton shimmer, count-up numbers, parallax or entrance animation.

Icons: `lucide-react` (ISC, 1.72.0) at 16 px, stroke width 1.5, `currentColor`. Icons never carry
meaning alone: every icon button has a visible text label or a tooltip **and** an `aria-label`.

---

## 5. Layout shell

### 5.1 Overall structure

```
+--------------------------------------------------------------------------------------------------+
| [#] BlackBox HFT | Order Book Lab v | ctx: synthetic-demo > run 253bbc23b342 > t +00:20.100 000   |
|                  |                  |  [Search / command  Ctrl K]  [DEMO read-only] [API key] [?]  |  36 px top bar
+----+---------------------------------------------------------------------------------------------+
| OV | workspace toolbar: [layout: Default v] [Reset layout]   ...workspace-specific tools...      |  32 px
| MD +-------------------------------------------------------------+-------------------------------+
| OB | PANEL A TITLE          [tab][tab]          [tools] [..] [^]  | INSPECTOR              [x]   |
| SL |                                                             |                               |
| SW |                                                             |  selection details,           |
| EF |                                                             |  definitions, raw JSON        |
| RP +=============================================================+  (collapsible, 320 px)       |
| EC | PANEL B TITLE                                    [tools][..]|                               |
| RR |                                                             |                               |
| SA |                                                             |                               |
+----+-------------------------------------------------------------+-------------------------------+
| API ok 42ms | WORKER running 1, queued 3, hb 2s | v0.1.0 contract v1 hftbacktest 0.9.4 |          |
|   SIMULATED  Simulated on historical replay - research only. Simulated orders ... | 12:34:56 UTC  |  24 px status bar
+--------------------------------------------------------------------------------------------------+
```

- Minimum supported viewport: 1280×720. Below 1280 px wide the rail collapses to icons and the
  inspector becomes an overlay drawer. Below 960 px, panels stack vertically in slot order and each
  panel is collapsible. This satisfies reflow (SC 1.4.10 [S20]); dense desktop use is the target.
- The shell is `height: 100vh` with no page scroll. Panels scroll internally. The only exception is
  the Research Reports document view (§11.9), whose main panel is one scrolling article.

### 5.2 Top bar (36 px, `--bg-1`, bottom `--border`)

Left to right:

1. **Mark + "BlackBox HFT"** (sans 600, 13 px). Links to Overview.
2. **Workspace switcher**: a button showing the current workspace name with ▾. It opens a menu
   listing all ten workspaces with their key chords (§8). It mirrors the rail and exists for
   keyboard users and for collapsed-rail widths.
3. **Context breadcrumb**: `dataset > run > order > t` for whatever the URL holds (§9), each part
   in mono 12 px and clickable to clear the deeper parts. `t` uses the relative clock format
   (§7.3). Empty context shows nothing.
4. **Command palette trigger**: a field-like button with the placeholder "Search or run a
   command" and the hint `Ctrl K` (§6.12).
5. **Access badge**: `DEMO · read-only` (neutral badge) when no key is set, or
   `RESEARCH · key …a1b2` (ok badge; last 4 characters of the key) when one is. It opens the API
   key dialog (§10.2).
6. **Help `?`**: opens the shortcut overlay and the glossary.

The research banner is not in the top bar. It lives permanently in the status bar (§5.5) so that it
never costs a row of height and is never dismissible.

### 5.3 Left rail and workspace area

- Rail (48 px, `--bg-1`, right `--border`): ten 40×40 px buttons, each with a 16 px icon above a
  two-letter mnemonic (`OV`, `MD`, `OB`, `SL`, `SW`, `EF`, `RP`, `EC`, `RR`, `SA`). The mnemonic
  matches the `g`-chord letters in §8. The tooltip shows the full name and chord. The active
  workspace has a 2 px `--accent` left bar plus `aria-current="page"`. The `SW` button carries a
  numeric badge with the count of the user's non-terminal jobs, in text (`3`), not as a dot.
  `\` (or the palette command "Toggle rail") expands the rail to 208 px with full names.
- **Workspace area** = toolbar row (32 px) + **panel grid**. The grid is built with
  `react-resizable-panels` (MIT, 4.22.1; components `Group`, `Panel`, `Separator`). The library
  renders `role="separator"` with the required WAI-ARIA properties, supports keyboard resizing
  through separators, and supports per-panel min/max and collapsible sizes, which matches the APG
  window-splitter pattern [S23]. Each workspace defines named **slots** (e.g. `main`, `side`,
  `bottom`) in a fixed `Group` tree; §11 gives the default for each.
- **Docking model.** Each slot holds a **tab group** of one or more panels. Users can:
  - resize slots by dragging or keyboard on the separators;
  - collapse a slot to its 28 px header;
  - maximise one panel to the full workspace area (Alt+Enter; same again or Esc restores);
  - move a panel to another slot via the panel menu `[..] → Move to → main | side | bottom` or
    by dragging its tab onto another tab strip;
  - close optional panels and reopen them from the toolbar's `Panels v` menu.

  Floating windows and arbitrary nested splits are **not** provided (P3). The layout (slot sizes,
  tab membership, active tab, collapsed state) is saved per workspace in `localStorage` under
  `bb.layout.<workspace>.v1`. **Reset layout** restores the default. Layout is never put in the URL
  (§9).
- **Inspector**: an optional right slot, 320 px default (min 240, max 560) and collapsible. It shows
  details of the current selection (row, chart marker, cell) as key/value lists, definitions of
  quant terms (§6.14) and a `Raw JSON` disclosure. Toggle with `i` or the palette command
  "Toggle inspector".

### 5.4 Workspace toolbar (32 px, `--bg-1`, bottom `--border-subtle`)

Left: workspace-scoped context pickers (e.g. dataset select, run select), then the primary
action of the workspace (at most one `btn-primary`). Right: `Panels v`, `Layout v` (Reset layout)
and a density toggle (`Comfortable | Default | Compact`, stored in `localStorage`
`bb.density.v1`). There is no page title heading; the workspace name is in the switcher and in
`document.title` as `<Workspace> · <context> — BlackBox HFT`.

### 5.5 Status bar (24 px, `--bg-1`, top `--border`, mono 11 px)

Segments are separated by 1 px `--border-subtle` and each is focusable. From left to right:

| Segment | Content | States |
|---|---|---|
| **API** | dot + `API` + state + last round-trip of `/api/health` in ms | `ok` (ok colour), `waking` (warn, with elapsed seconds), `degraded` (warn), `unreachable` (error), `contract mismatch` (warn). See §10.1. |
| **Worker** | `WORKER` + state + `running n · queued m` + `hb <age>` | `idle`, `running`, `stale heartbeat` (warn: last heartbeat older than 3× the expected interval), `offline` (error), `unknown` (fg-2, when health does not report it). |
| **Build** | `v<blackbox_version> · contract v<n> · <engine name> <engine version>` | Static. Click copies it. |
| **Mode** | `DEMO read-only`, `RESEARCH`, or `PRECOMPUTED BUNDLE` (static viewer, `App.tsx:93-96`) | — |
| **Research banner** | `SIMULATED` modeled badge + the exact `RESEARCH_BANNER` text (`App.tsx:23-24`) | Truncates with ellipsis. The full text is in the tooltip and is always in the accessibility tree. Never hidden. |
| **Clock** | `HH:MM:SS UTC` from the browser, updated every second | Tooltip shows local time and, if health reports `server_time_ns`, the skew `server − browser`. |

Clicking API or Worker opens System/Admin (§11.10). Status changes are announced through one
polite `aria-live` region inside the status bar. Announcements fire only on **state changes**, not
on every poll.

---

## 6. Component inventory

All components live in `web/src/components/` and use only the tokens of §4. Each entry gives its
anatomy, its states, and what it replaces.

### 6.1 DataTable (virtualized)

- Built on `@tanstack/react-virtual` (MIT, 3.14.26), which describes itself as "Headless UI for
  virtualizing scrollable elements". Virtualize when there are more than 200 rows. Overscan is 10
  rows. Row height is fixed by density (`--h-row`), so measurement is free.
- Semantics: `role="grid"` with `aria-rowcount`/`aria-colcount` set to the **total** counts and
  `aria-rowindex` on each rendered row, following the APG grid pattern [S23]. Header cells carry
  `aria-sort`.
- Header: sticky, `--bg-2`, uppercase `--text-xs` 500 `--fg-1`. The **unit** appears in the header
  in `--fg-2`, never in cells (`NET P&L  USDT`, `LATENCY  ms`). Exceptions are mixed-unit columns
  such as "value" in a metrics table: there the unit sits in its own `UNIT` column (as
  `compare.png` already does).
- Cells: 12 px. Numbers use mono, right alignment and fixed decimals per column (§7). Text is left
  aligned with ellipsis, and the full value is shown in the tooltip after 500 ms hover or on
  focus. Row rule is 1 px `--border-subtle`, hover is `--bg-2`, selection is `--bg-selected` with
  a 2 px `--accent` left bar on the first cell. There is no zebra striping.
- Features: sort (click or Enter on a header; Shift adds a secondary key), column resize (drag the
  header edge; Alt+←/→ on a focused header), column show/hide menu, pinned first column, a filter
  row toggled by `/`, copy (Ctrl+C copies the selected cells as TSV with **raw** values: ASCII
  minus, no thousands separators, full precision, ns as decimal strings).
- Server paging: when the API pages (`OrdersPage`, `TapePage`), the table reports
  `showing 1–5,000 of 12,591` in its footer. It fetches further pages on scroll and **never
  implies completeness**. A truncated result shows the truncation notice (§6.10).
- States: loading (header rendered, body shows `Loading orders… 3 s`), empty (§6.9), error (§6.9),
  partial (truncation notice).
- Replaces: every `<table className="table">` and `components/data.tsx` `Pager`.

### 6.2 KPI strip

A single row of `Stat` cells (`ui.tsx:104-118`) with 1 px separators. Each cell has a label
(`--text-xs`, fg-1), a value (`--text-lg` mono) and a unit or basis line (`--text-2xs` fg-2 plus an
evidence badge). It holds at most 8 cells, and overflow scrolls horizontally. Values follow §7 and
expose the exact value on hover. It replaces the current `.stats` grid. **Never** use it for
decorative counts ("12 strategies!"); every KPI must come from the API for the current context.

### 6.3 Panel

- Header (28 px, `--bg-2`): uppercase title (`--text-xs` 600), optional evidence badge, tab strip
  (if the slot has more than one panel), right-aligned tool buttons (icon buttons, 24 px), the
  panel menu `[..]` (Move to, Maximise, Copy link to this view, Export CSV where applicable) and
  collapse `^`.
- Body: `--bg-1`. Padding is 0 for tables and charts and `--sp-2` for forms and inspectors.
- Caption line (optional, 20 px, `--text-xs` fg-2) under the header for the one-line honesty
  caption (e.g. "Observed L2. Downsampled 1:200 ms."). Longer explanations go to the inspector
  "About this panel" section, keeping the current wording verbatim.
- Replaces `Panel` in `ui.tsx:19-29`. The props stay compatible: `title`, `actions`, `children`,
  `className`. New: `tools`, `menu`, `caption`, `basis`.

### 6.4 Toolbar

A horizontal row of 24 px controls with `--sp-1` gaps, grouped by 1 px vertical dividers.
`role="toolbar"` with roving tabindex: Tab enters the toolbar, ←/→ move between controls.
Toggle buttons use `aria-pressed`, as today (`theme.css:446-450`). Primary buttons
(`--bg-3` fill, `--accent` 1 px border) appear only in the workspace toolbar.

### 6.5 Tabs

APG tabs pattern [S23] with automatic activation. 28 px tall, 12 px sans. The active tab uses
`--fg-0` with a 2 px `--accent` bottom bar; inactive tabs use `--fg-1`. Tabs never use pill or
rounded-card styling. A tab can show a count (`Orders 2,060`). Workspace-level tabs (e.g. Experiment
Comparison: Pair / Set / Sweep) are routes (§9). Panel-level tabs are layout state.

### 6.6 Badge and status pill

- **Evidence badge** (existing `BasisBadge`). Uppercase mono 11 px, 16 px tall, `--radius-1`,
  transparent fill, text and border in the basis colour. Border style differs per basis: observed
  solid, modeled dashed, derived double (3 px), unobservable dotted. Keep `BASIS_MEANING` as the
  tooltip text.
- **Side badge**: `BUY`/`SELL` (or `BID`/`ASK` in book contexts) as text in `--bid`/`--ask`.
- **Status pill** (job and order lifecycle): uppercase mono 11 px text in the status colour with a
  1 px border, plus a leading 8 px shape. The **shape** differs per state so that state never
  depends on colour alone:

| State | Shape (CSS-drawn) | Colour |
|---|---|---|
| created | hollow square | fg-2 |
| queued | hollow circle | fg-1 |
| running | filled circle (static; no pulse) | accent |
| completed | filled square | ok |
| failed | filled triangle | error |
| cancelling | half-filled circle | warn |
| cancelled | square with diagonal | fg-2 |
| expired | hollow triangle | warn |

### 6.7 Split panes

These are the `react-resizable-panels` separators of §5.3. A separator is 1 px `--border` at rest,
with an 8 px invisible hit area. On hover or focus it shows a 1 px `--border-strong` line plus the
2 px `--accent` focus outline. Keyboard: ←/→ (or ↑/↓) move by 4 % (Shift: 1 %), Home/End snap to
min/max, Enter collapses or expands, and double-click resets to the default. Separators are
labelled `aria-label="Resize <left panel> and <right panel>"`.

### 6.8 Form controls

- Inputs and selects: 24 px tall, `--bg-0` fill, 1 px `--border-control`, `--radius-1`, 12 px text
  (mono for numeric fields). Hover changes the border to `--fg-2`. Focus adds the 2 px accent
  outline. Invalid state: `aria-invalid="true"`, a 1 px `--error` border and an error line below
  in `--error` with an `ERROR` prefix glyph.
- **Numeric fields always show their unit** as a suffix inside the field (`[ 50  ms ]`) and the
  canonical value sent to the API below, as the grid page already does
  ("2 ms = 2,000,000 ns", `grid.png`). Units are selectable where the API accepts several
  (ns/µs/ms/s for latencies; bp/fraction for fees). The canonical integer is computed with integer
  arithmetic on the typed decimal string, never through float (e.g. `"0.5" ms → 500000` ns).
- Field layout: label above (12 px fg-1, `optional` tag in fg-2), control, help line (11 px fg-2).
  Evidence badge after the label when the field is a modelling assumption (current `FieldInput`
  behaviour).
- Checkbox and radio: 14 px box, 1 px `--border-control`, accent check. Segmented control: a row
  of toggle buttons with `aria-pressed`. Slider: only for playback scrubbing, always paired with a
  typed time field.
- Submit: Ctrl+Enter from any field of a form submits it (§8).

### 6.9 Empty, loading and error states

| State | Rendering | Rules |
|---|---|---|
| Loading | Panel body shows a single line `Loading <thing>… <elapsed s>` in fg-2, top-left aligned. After 10 s it appends `— the API may be waking up` if the API status is `waking`. | No skeleton shimmer. Keep the last data visible (dimmed to fg-2 with a `STALE` badge) while reloading. |
| Empty | One line stating what is absent and why, plus at most one action: "No runs on this dataset yet. [Open Strategy Lab]". In demo mode without auth, the action is shown disabled with its reason (§10.2). | No illustrations, emojis or marketing copy. |
| Error | `ErrorBox` (`ui.tsx:131-155`) restyled: 1 px `--error` left border, `ERROR <code>` header in mono, server message verbatim (multi-line for `invalid_config`), `[Retry]` and `[Copy details]`. | Never paraphrase server errors. Never show a stack trace. A 401 opens the key dialog (§10.2). |
| Partial | Inline notice (§6.10) | — |

### 6.10 Notices (inline)

A 24 px-min row with a 2 px left border in the notice colour, a mono uppercase tag
(`DOWNSAMPLED`, `TRUNCATED`, `MODELED`, `STALE`, `WAKING`), and one sentence. Notices are not
dismissible when they qualify the data. Examples kept from the current UI: "This window holds
12,591 tape events; only the earliest 5,000 were returned" (`terminal.png`).

### 6.11 Tooltip and popover

- A tooltip appears after 500 ms hover or immediately on keyboard focus. It is dismissed with Esc
  (SC 1.4.13 [S20]) and stays while the pointer is over it. Maximum width 360 px, `--bg-2`, 1 px
  `--border-strong`, `--radius-2`, `--shadow-float`, 12 px text.
- Tooltips hold supplementary text only. Any action or essential value must also be reachable in
  the inspector.

### 6.12 Command palette (Ctrl+K)

A modal dialog (APG dialog [S23]) with one combobox input and a result list grouped by: Workspaces,
Datasets, Runs (by id prefix or label), Orders (`#51201` in the current run), Commands ("Reset
layout", "Toggle CVD palette", "Copy link", "Set API key", "Sign out"). Fuzzy matching is
client-side over loaded lists. Enter executes. Commands that require auth appear with a lock glyph
and the reason. Placement: centred, 560 px wide, at 20 % from the top.

### 6.13 Dialogs and toasts

- Dialogs are used only for confirmation of destructive actions (delete run, cancel job) and for
  the API key entry. The confirm button names the action ("Cancel job 7f3a…"). Focus is trapped,
  and Esc closes.
- Toasts appear bottom-right above the status bar, at most 3 at once, and auto-dismiss after 6 s
  (paused on hover or focus) except errors. They report job transitions the user caused ("Job
  7f3a… completed — open run"). They are also announced through the status bar live region.

### 6.14 Quant-term tooltips (glossary)

Any column header, KPI label or field label whose term appears below is rendered with a dotted
underline. Its tooltip (and the inspector "Definition" section) shows the definition. The text
lives in **one** module, extending `components/describe.ts`, and must be checked against the
linked document. Initial entries, with wording consistent with the repo docs:

| Term | Tooltip text (≤ 2 sentences) | Source of truth |
|---|---|---|
| Markout (h) | Signed move of the mid `h` after a fill, in ticks: side × (mid(t_fill + h) − fill price). Negative means the market moved against the fill (adverse selection). | `docs/PNL_ACCOUNTING.md:125-136` |
| Spread capture | Half-spread earned at the instant of each passive fill against the dataset mid then; taker fills are excluded. Not additive with net P&L. | `docs/PNL_ACCOUNTING.md:113-123,138-147` |
| Queue position | Quantity modeled ahead of the order at its price. L2 data does not show individual orders, so it is an estimate. | `docs/QUEUE_MODELS.md` |
| Entry / response latency | Modeled delay from the strategy's decision to the simulated exchange (entry), and from the exchange back to the strategy (response). | `docs/LATENCY_MODEL.md:72-80` |
| Round trip | Entry plus response latency for one request, as realised in the run. | `docs/LATENCY_MODEL.md:143` |
| Feed latency | Local receipt time minus exchange time of market data. | `docs/MARKET_DATA.md` |
| Fill ratio (orders / qty) | Orders with a fill ÷ orders submitted; filled ÷ submitted quantity. | `docs/PNL_ACCOUNTING.md:110` |
| Exchange clock / local clock | Time at the simulated matching engine vs time at the strategy host; every event names its clock. | `docs/ARCHITECTURE.md:161-167` |
| Maker / taker fee, rebate | Fee on passive / aggressive fills as a fraction of traded value; a negative maker fee is a rebate. | `components/describe.ts`, `grid.png` |
| Observed / modeled / derived / unobservable | `BASIS_MEANING` verbatim. | `components/ui.tsx:63-70` |
| L2 (market by price) | Aggregate displayed quantity per price level; individual orders and hidden liquidity are not visible. | `datasets.png`, `docs/MARKET_DATA.md` |
| Drawdown | Largest peak-to-trough fall of simulated equity over the run, in quote currency. | `docs/PNL_ACCOUNTING.md` |

---

## 7. Numeric and time formatting

All formatting goes through `components/fmt.ts`. Formatting is pure and locale-independent: `.` is
the decimal point and `,` is the thousands separator, regardless of browser locale. The same data
always renders the same text (`fmt.ts:1-2`).

### 7.1 General rules

1. Numbers are **right-aligned** and use mono and **fixed decimals per column**, so decimal points
   align. The precision rule is per quantity (table below), never per value.
2. **Minus is U+2212 `−`** in display (it has the same advance as `+` in the mono face). Copy and
   CSV export emit ASCII `-`.
3. Deltas, P&L and returns show an explicit `+` for positive values; zero has no sign (current
   `fmtSigned`, `fmt.ts:129-137`). Plain quantities and prices are unsigned except negative
   positions.
4. A missing or undefined value is an em dash `—` (`DASH`, `fmt.ts:7`), with a tooltip saying why
   when the API gives a reason. `NaN`, `Infinity`, `null` or `undefined` must never appear as text.
5. Tables never abbreviate (no `12.4K`). Abbreviation (K/M/B, SI) is allowed only on chart axis
   ticks.
6. Units: in the column header for tables, in a fg-2 suffix for KPI values (`−74.7096 USDT`), and
   inside the field for inputs. Spell units as `ns`, `µs`, `ms`, `s`, `bp`, `%`, `pp`, `ticks`,
   `lots`, and the instrument's base and quote asset codes. Use `bp` everywhere; `bps` is retired.
7. Colour applies only to signed P&L/delta values (`--up`/`--down`) and to side. Everything else is
   `--fg-0`.
8. The exact value is always reachable: the tooltip or inspector shows the unrounded value (and the
   raw integer for ticks/ns).

### 7.2 Precision per quantity

| Quantity (API form) | Display | Decimals | Unit placement | Example |
|---|---|---|---|---|
| Price (integer ticks × `tick_size`) | price | decimals of `tick_size` (`stepDecimals`, `fmt.ts:87-95`) | header: quote asset | `29998.9` |
| Mid / average fill price (fractional ticks) | price | tick decimals + 2 (current `fmtPrice` rule) | same | `29999.05` |
| Quantity / position (multiples of `lot_size`) | qty | decimals of `lot_size` | header: base asset | `0.010` |
| Notional, turnover, cash | money | 4 in tables, 2 in KPI strip (exact on hover) | header: quote asset | `372,305.2660` |
| P&L (realized, unrealized, net), fees, drawdown | signed money | 4 in tables, 2 in KPI strip | header: quote asset | `−74.7096` |
| Fee rates, slippage, spread in bp | bp | 2 | suffix ` bp` | `0.50 bp` |
| Spread, markout, spread capture in ticks | ticks | 3 | header: `ticks` | `−0.129` |
| Ratios shown as percent (fill ratios) | percent | 2 | suffix `%` | `60.24%` |
| Difference of two percentages | pp | 2 | suffix ` pp` | `−17.89 pp` |
| Dimensionless statistics (Sharpe-like, imbalance) | number | 2 (3 if \|x\| < 0.1) | — | `0.62` |
| Counts (orders, fills, events, rows) | integer | 0, with thousands separators | header: `count` where ambiguous | `12,591` |
| Durations / latencies (ns integer or string) | adaptive | 3 significant digits in ns, µs, ms or s (`fmtDuration`); exact in tooltip (`fmtDurationExact`) | inline suffix | `50.0 ms` |
| Latency configuration values | exact | exact (`fmtDurationExact`) + raw ns in fg-2 | inline | `1 ms (1,000,000 ns)` |
| Byte sizes | binary units | 1 decimal (`fmtBytes`) | inline | `872 KiB` |
| Hashes / run ids | hex | first 12 characters; full value on hover and on copy (`HashText`) | — | `253bbc23b342` |
| Seeds, order ids, tick indices | integer | 0, **no** thousands separators (identifiers) | — | `51201` |
| Job progress | percent | 0 decimals, only when the server reports a total | suffix `%` | `42%` |

### 7.3 Timestamps: nanosecond decimal strings, never floats

The backend is moving every nanosecond value (absolute epoch ns, relative ns, durations) to
**string-encoded i64** decimal strings, because they exceed the 2^53 integer range of a JS number
(`docs/API.md:8-9`). The UI rule:

> **A nanosecond value is parsed from its decimal string into a `BigInt` (or kept as a string) and
> formatted with integer arithmetic. It is never passed through `Number(...)`, `parseFloat`,
> `new Date(ns / 1e6)` or any floating-point operation.**

Specification for the `fmt.ts` additions:

```ts
/** Strict parse: optional '-', 1–19 digits, i64 range. Throws on anything else. */
export function parseNs(s: string): bigint;

/** Absolute UTC: 'YYYY-MM-DD HH:MM:SS.fffffffff UTC', fraction digits limited to `digits`. */
export function fmtNsUtc(epochNs: string, digits: 0 | 3 | 6 | 9): string;

/** Relative to dataset origin: '+mm:ss.mmm uuu nnn' (minutes not wrapped), sign always shown. */
export function fmtNsRel(relNs: string, digits: 3 | 6 | 9): string;

/** Duration: adaptive 3-significant-digit unit from a string (`fmtDuration` semantics). */
export function fmtNsDuration(ns: string): string;

/** Ordering without conversion: compare as BigInt. */
export function cmpNs(a: string, b: string): -1 | 0 | 1;
```

Algorithm for `fmtNsUtc` (extends the existing BigInt logic in `fmtOriginUtc`, `fmt.ts:170-185`):

1. `n = parseNs(s)`; `sec = n / 1_000_000_000n` (floor toward −∞ for negatives);
   `frac = n − sec × 1_000_000_000n` (0 ≤ frac < 1e9).
2. `sec` is converted to `Number` **only for seconds**. Epoch seconds for any realistic date are
   < 2^53, so the conversion is exact. Call `new Date(Number(sec) * 1000)` and read the UTC fields.
   Assert `-8.64e12 ≤ sec ≤ 8.64e12` and throw outside that range.
3. Fraction = `frac.toString().padStart(9, '0').slice(0, digits)`.
4. Output `2026-11-14 09:30:00.123456789 UTC`. In the UI, render the fraction in groups of 3
   separated by a thin space (U+2009), `.123 456 789`. Groups below millisecond are dimmed (fg-2),
   continuing the current `ClockText` dimming of µs digits (`ui.tsx:163-173`). Copy yields the
   undecorated string.

Precision rule: `digits` comes from the dataset's declared timestamp resolution (the dataset
manifest's "Source unit", e.g. `nanos` in `datasets.png`). µs-resolution sources therefore show 6
digits. The UI **never pads digits beyond the source resolution with zeros that look measured**.
Synthetic datasets show their declared unit.

Display formats:

| Context | Format | Example |
|---|---|---|
| Cursor, event times, tables in a run/dataset | relative to `origin_ns`, `+mm:ss.mmm uuu` (`nnn` when source is ns) | `+00:20.100 000` |
| Absolute times (dataset origin, run creation, exports) | UTC, as above | `2023-11-14 22:13:20.000 UTC` |
| Durations | adaptive (`fmtNsDuration`), exact in tooltip | `2.78 ms` |
| Job timestamps (created/started/finished) | UTC to the second; relative age in fg-2 | `2026-11-14 12:30:05 UTC · 2 min ago` |
| Typed time input (jump to time) | accepts `ss.fffffffff`, `mm:ss.fff…`, or `+<n>ns`/`ms`/`s`; converted with integer math to a relative-ns string | `20.1` → `"20100000000"` |

During the transition, relative ns still arrive as JS numbers (current API). Formatters accept
`string | number`. Numbers are converted with `BigInt(Math.trunc(x))` only after asserting
`Number.isSafeInteger(x)`, and the formatter fails loudly (shows `—` and logs once) otherwise.

---

## 8. Keyboard model

Principles: every action is reachable by keyboard. Global shortcuts use a modifier or a
`g`-prefixed chord. Single-character shortcuts are active only when focus is not in a text field,
and they can be turned off in Settings (WCAG 2.1.4 [S20]; P4). Browser and OS-reserved bindings
are never overridden: Ctrl+W/T/N/L/R/D/P/F/B/I/U/H/J, Ctrl+Shift+C/I/J/B/Del (developer tools,
bookmarks), Ctrl+Tab, Ctrl+1…9, F5, F11, F12, Alt+F4, Alt+←/→ (history). Ctrl+K and Ctrl+S are
deliberately taken over (command palette, save draft), as TradingView does [S7]. Every
single-key binding below also exists as a command-palette entry, so turning single keys off loses
no function.

### 8.1 Global

| Keys | Action |
|---|---|
| Ctrl+K | Command palette |
| `?` | Shortcut overlay (lists every binding active in the current focus context) |
| `g` then `o` / `m` / `b` / `l` / `w` / `f` / `r` / `c` / `p` / `a` | Go to Overview / Market Data / Order Book Lab / Strategy Lab / Simulation Workbench / Execution Forensics / Risk & Performance / Experiment Comparison / Research Reports / System-Admin (rail mnemonics OV MD OB SL SW EF RP EC RR SA) |
| F6 / Shift+F6 | Move focus to next / previous region: top bar → rail → toolbar → each slot → inspector → status bar |
| `\` | Expand/collapse rail (single key) |
| `i` | Toggle inspector (single key) |
| Alt+Enter | Maximise/restore the focused panel |
| `y` | Copy a link to the current view (URL with context, §9) (single key) |
| Esc | Close the topmost popover/dialog → clear selection → restore maximised panel (in that order) |

### 8.2 Tables (APG grid [S23])

↑/↓/←/→ move the cell focus. Home/End go to the first/last cell in the row; Ctrl+Home/End go to the
first/last row. PgUp/PgDn move one page. Enter opens the row in the inspector, or follows its
primary link for run and order rows. Space selects the row; Shift+↑/↓ extends the selection.
Ctrl+A selects all loaded rows. Ctrl+C copies raw TSV (§6.1). `/` focuses the filter row. Enter on
a header sorts; Shift+Enter adds a secondary sort.

### 8.3 Time-axis views (heatmap, two-clock timeline, queue chart, equity)

These keep the bindings already implemented (`ReplayTerminalPage.tsx:438-448`,
`TwoClockTimeline.tsx:158-163`, `HeatmapView.tsx:271`), active while the chart has focus:

| Keys | Action |
|---|---|
| Space | Play/pause replay (Order Book Lab only) |
| ← / → | Previous/next event (respects event filters); Shift: ×10 |
| `+` / `-` (also `=`) | Zoom in/out around the cursor |
| `0` | Reset/fit view |
| `j` | Focus "jump to time" field |
| `o` | Focus "jump to order" field |
| `[` / `]` | Previous/next order of the run at or after the cursor |
| Esc | Clear marker selection |

The cursor and zoom are shared by all time-axis panels in the workspace (P2).

### 8.4 Forms and jobs

| Keys | Context | Action |
|---|---|---|
| Ctrl+Enter | any form | Submit (Run / Create experiment / Save). Disabled without auth, with the reason announced. |
| Ctrl+S | Strategy Lab | Save configuration draft to `localStorage` (never to the server) |
| Delete | selected job (Workbench) | Opens the "Cancel job" confirm dialog. Never cancels directly. |
| Shift+Delete | selected run | Opens the "Delete run" confirm dialog (auth only) |

---

## 9. URL state conventions

**Rule:** everything that changes *which data* is shown is in the URL. Everything that changes
*how it is arranged* (slot sizes, tabs, density, palette, inspector open) is in `localStorage`.
Secrets are never in the URL.

- **Path** = workspace + primary entity. **Query** = view parameters.
- ns values in the query are **decimal strings of relative ns** (`t=20100000000`), validated with
  `parseNs`. Run, order and dataset ids are used as given.
- Continuous changes (cursor scrub, zoom, typing in a filter) use `history.replaceState`, debounced
  300 ms. Discrete navigation (select a run/order, change tab, submit) uses `pushState`. This
  matches the current Replay Terminal sync (`setSearchParams(…, { replace: true })` after a 300 ms
  timer, `ReplayTerminalPage.tsx:300-313`).
- Unknown query parameters are ignored and preserved. Invalid values are dropped with an inline
  notice ("Ignored t=abc: not a nanosecond integer").
- Lists are comma-separated (`runs=a,b,c`). Filters use `f.<column>=<value>` and sorting uses
  `sort=<column>:asc|desc[,…]`.

### 9.1 Route table

| Workspace | Paths | Query |
|---|---|---|
| Overview | `/` | — |
| Market Data Explorer | `/data`, `/data/:datasetId` | `tab=identity\|quality\|files`, `check=<check id>` |
| Order Book Lab | `/book/data/:datasetId` (market only), `/book/runs/:runId` (with simulated orders) | `t0`, `len`, `levels`, `t`, `order`, `speed`, `brightness` |
| Strategy Lab | `/strategy` | `dataset`, `from` (run id to load), `preset` |
| Simulation Workbench | `/sim` (jobs), `/sim/jobs/:jobId`, `/sim/runs`, `/sim/runs/:runId`, `/sim/sweep` | `state=<job states>`, `mine=1`, `base` (sweep base run) |
| Execution Forensics | `/forensics/:runId` (blotter), `/forensics/:runId/orders/:orderId` | `outcome`, `side`, `sort`, `ev=<selected event index>`, `scale=linear\|order` |
| Risk & Performance | `/risk/:runId` | `from`, `to` (relative ns strings), `series` |
| Experiment Comparison | `/compare` (picker), `/compare/pair`, `/compare/set`, `/compare/experiments/:experimentId` | pair: `a`, `b`; set: `base`, `runs`, `demo`; experiment: `metric` |
| Research Reports | `/reports`, `/reports/:runId` | — |
| System/Admin | `/system` | `tab=health\|queue\|storage\|access\|about` |

### 9.2 Legacy redirects (required, preserve query)

| Old | New |
|---|---|
| `/datasets` | `/data` |
| `/replay/:datasetId?t0&len&levels` | `/book/data/:datasetId?t0&len&levels` |
| `/lab?dataset&from` | `/strategy?dataset&from` |
| `/runs` | `/sim/runs` |
| `/runs/:runId` | `/sim/runs/:runId` |
| `/runs/:runId/replay?t&order` | `/book/runs/:runId?t&order` |
| `/runs/:runId/orders/:orderId` | `/forensics/:runId/orders/:orderId` |
| `/latency?runs&demo` | `/compare/set?runs&demo` (the first run becomes `base`) |
| `/compare?a&b` | `/compare/pair?a&b` |
| `/grid?base` | `/sim/sweep?base` |

The static viewer (`isStaticViewer()`) uses the same routes. `web/vercel.json:6-11` already
rewrites every non-asset path to `index.html`.

---

## 10. Connection, authentication and job states

### 10.1 API connection and cold start

The hosted API runs on a Render Free web service. Render spins it down after 15 minutes without
inbound traffic, spin-up "takes about one minute", and "Render displays a loading page to
connecting browsers while a service is spinning up" [S24]. A Free instance has 0.1 CPU and
512 MB RAM [S24]. The UI therefore treats slowness as a normal, labelled state:

```
             first request
  (start) ─────────────────► CONNECTING ──200 JSON──► OK ◄──────────────┐
                                  │                    │                │
                       >3 s, or 502/503/504,           │ 3 failures     │ 200 JSON
                       or non-JSON (HTML) body         ▼ in a row       │
                                  ▼               UNREACHABLE ──────────┘
                               WAKING ──200 JSON──► OK     ▲  (retry button + backoff)
                                  │                        │
                                  └── >120 s ──────────────┘
```

- **WAKING**: status bar `API waking 37 s` (warn). Panels that are waiting show `Loading… 37 s —
  the API is waking up (free host; first response can take about a minute)`. Retries back off
  2, 4, 8, 15, 15… s. A non-JSON `/api/health` body (the host's loading page) counts as waking,
  not as an error `[INFERENCE from S24]`.
- **OK**: health is polled every 30 s while the tab is visible, and paused when it is hidden
  (`document.visibilityState`). `contract_version` mismatch → `contract mismatch` warn badge
  (current behaviour, `App.tsx:104-111`).
- **UNREACHABLE**: error segment, with a Retry action and the last error message verbatim.
- Data already on screen is never cleared on a connection drop. It is marked `STALE`.
- The UI does not ping the API just to keep it warm. The free host's hours are limited [S24]
  `[INFERENCE: keep-alive pings would spend them]`.

### 10.2 Access: public demo vs API key

| Mode | Shown as | Allowed |
|---|---|---|
| Demo (no key) | top bar `DEMO · read-only`; status bar `DEMO read-only` | All reads of stored/precomputed datasets, runs, experiments, reports. |
| Research (key present and accepted) | `RESEARCH · key …a1b2` | Reads + create experiment/run, sweeps, cancel, delete. |
| Precomputed bundle (static viewer) | `PRECOMPUTED BUNDLE` | Reads of the bundle only; every mutation disabled with the reason "No server: this is a precomputed bundle". |

API key dialog (opened from the access badge, the palette, or a 401):

```
+-- Research access ------------------------------------------- [x] --+
| Paste an API key to create experiments, run sweeps and cancel jobs.  |
| The key is kept in this browser tab only (sessionStorage) and is     |
| sent to the BlackBox API with each request. It is never put in a URL |
| or saved to disk by this page.                                       |
|                                                                      |
| API key  [ ••••••••••••••••••••••••••••••••••••••  ] [Show]          |
|                                                                      |
| ERROR 401 invalid_key: key not recognised        (only after submit) |
|                                                     [Cancel] [Use key]|
+----------------------------------------------------------------------+
```

- Storage: `sessionStorage['bb.apiKey']` only. It is cleared on **Sign out** (palette and access
  badge menu) and automatically on a 401 response. It is never written to `localStorage`, the URL,
  logs, error reports, `Copy details` or exported files.
- The key is validated by a single authenticated read (endpoint and header defined by the API
  contract, §16) before the dialog closes.
- **Disabled mutations**: every mutating control is rendered in place, with `aria-disabled="true"`
  rather than `disabled` so it stays focusable and its reason is readable. It shows a lock icon,
  label text in fg-1, and the tooltip/inspector reason: "Requires an API key — the public demo is
  read-only. [Enter key]". On activation it opens the key dialog instead of acting.
- A 403 response shows `ERROR 403 forbidden` verbatim. It does not clear the key.

### 10.3 What counts as mutating

Create run/experiment (Strategy Lab Run, Overview "Run demo"), sweep submission, cancel job, delete
run, delete experiment, and any admin action in System. Saving a **local** draft is not mutating.

### 10.4 Job states (Simulation Workbench, status bar, toasts)

States and transitions as decided:

```
 created ──► queued ──► running ──► completed
    │          │           │
    │          │           └──────► failed
    │          │           │
    └──────────┴───────────┴──► cancelling ──► cancelled
                                                      
 created/queued/running ──► expired      (server-decided; reason shown verbatim)
```

| State | Terminal | Shown fields | Actions (auth) | Copy rules |
|---|---|---|---|---|
| created | no | created at, submitter (key label if provided), config hash | Cancel | "Accepted, not yet queued." |
| queued | no | **queue position** `#3 of 5`, time in queue | Cancel | Position only if the server reports it, otherwise `position unknown`. No client-side ETA. |
| running | no | started at, elapsed, **progress** (`events 1,204,331 / 2,867,000 · 42%` if the server reports both counts; otherwise an indeterminate bar labelled `running — no progress reported`), **last heartbeat age** | Cancel | Heartbeat older than 3× the server-declared interval → `STALE HEARTBEAT` warn notice: "The worker has not reported for 47 s. The job may be stuck or the host may be restarting." Never extrapolate progress. |
| cancelling | no | requested at, by | — | "Cancellation requested; the worker stops at its next checkpoint." |
| completed | yes | finished at, wall time, result hash, run id(s) | Open run, Compare, Report | — |
| failed | yes | finished at, error code + message verbatim | Retry (re-submit same config), Copy details | — |
| cancelled | yes | cancelled at | Re-submit | — |
| expired | yes | expired at, server reason verbatim | Re-submit | The UI does not guess the reason. |

Update strategy: while any visible job is non-terminal, poll the job list every 2 s (5 s when the
API is `waking` or the tab is hidden), or subscribe to a server event stream if the API offers one
(§16). Transitions are applied only in the directions of the diagram. An out-of-order update is
ignored and logged once. Each user-caused transition raises one toast and one live-region
announcement.

---

## 11. Workspaces and wireframes

Every wireframe shows the default layout at 1440×900. `[..]` is the panel menu, `^` collapse,
`[x]` close. Labels in `<>` are data. Numbers shown in the wireframes are layout placeholders, not
measurements.

### 11.1 Overview (`/`, rail `OV`)

Purpose: orient the user, show system state, and give fast entry into recent work. No hero, no
marketing.

```
+----+------------------------------------------------------------------------------------------+
| OV | Toolbar: Dataset [synthetic-demo v]                                                      |
|    +--------------------------------------------------+---------------------------------------+
|    | CANONICAL EXPERIMENTS                    [..] ^  | SYSTEM                         [..] ^ |
|    | Experiment            Varied               Runs  | API          ok   42 ms               |
|    | Cost of latency       order_latency.*      2  [>]| Worker       running 1 · queued 3     |
|    | Queue uncertainty     queue_model.*        3  [>]| Heartbeat    2 s ago                  |
|    | Gross versus net      fees.*               3  [>]| Contract     v1 (console v1)          |
|    |   [>] = Open results (read) | [Run] (auth)       | Engine       hftbacktest 0.9.4        |
|    +--------------------------------------------------+ Access       DEMO read-only [Enter key]|
|    | RECENT RUNS                         [All runs] ^ +---------------------------------------+
|    | Run          Label        Strategy  Orders Fills | MY JOBS                        [..] ^ |
|    |              (12 rows, virtualized)    Net P&L   | (shown when authenticated)            |
|    | 4bde4759b784 grid x=2..   touch_m.  2,050 1,225  | 7f3a.. RUNNING  42%  hb 2 s           |
|    |                                     −18.6500     | 91c0.. QUEUED   #2 of 3               |
|    +--------------------------------------------------+---------------------------------------+
```

Reuse: `MissionControlPage.tsx` demo list and recent-runs data. The three demo cards
(`mission.png`) become rows of one table. "Reproduce from the command line" moves to the inspector
of the selected row.

### 11.2 Market Data Explorer (`/data/:datasetId`, rail `MD`)

```
+----+------------------------------------------------------------------------------------------+
| MD | Toolbar: [Open in Order Book Lab] [Open in Strategy Lab]                                 |
|    +--------------------------+---------------------------------------------+-----------------+
|    | DATASETS           [..] ^| synthetic-demo  SYNTHETIC-NOT MARKET DATA    | INSPECTOR       |
|    | Id           Level  Span | [Identity] [Quality 1/15 failed] [Files]     | check: time.    |
|    | synthetic-.. L2+tr 120 s | Identity                                     | rows_split_for_ |
|    | <other ids>              |  Dataset id   synthetic-demo                 | ordering        |
|    |                          |  Span         +00:00.000 -> +01:59.998 088   | Severity INFO   |
|    |                          |  Origin       2023-11-14 22:13:20.000 UTC    | Occurrences     |
|    |                          |  Tick / lot   0.1 / 0.001                    |   5,289         |
|    |                          |  Source unit  nanos                          | Detail (verbat.)|
|    |                          | Quality                                      | Examples (5)    |
|    |                          |  Result Sev  Check            Occur.  Detail |                 |
|    |                          |  PASS   WARN time.exch_out..       0  ...    |                 |
|    |                          |  FAIL   INFO time.rows_split..  5,289 ...    |                 |
|    |                          |  Measure       N    min  p50  p90  p99 max   |                 |
+----+--------------------------+---------------------------------------------+-----------------+
```

Reuse: `DatasetsPage.tsx` (identity, events, source, timestamp provenance, input files, quality
report, quantile table → DataTable). The licence and vendor notes stay verbatim. **Vendor
datasets:** this workspace shows aggregate manifest statistics only and never raw rows, following
the Tardis terms recorded in `TODO.md:68` and `docs/OPEN_SOURCE_RESEARCH.md`. The public demo lists
synthetic datasets only.

### 11.3 Order Book Lab (`/book/data/:id`, `/book/runs/:runId`, rail `OB`)

```
+----+------------------------------------------------------------------------------------------+
| OB | Toolbar: [> Play] [|< prev] [next >|] Speed[1x v] t [+00:20.100 000] [Go] Order[51201][Go]|
|    |   Window +00:15.1 -> +01:15.1  Levels[25 v]  Brightness[sqrt v]   [Market only|With run] |
|    +-----------------------------------------------------------------+------------------------+
|    | BOOK HEATMAP  OBSERVED L2  OWN ORDERS MODELED    [+][-][0] [..] | DEPTH AT CURSOR  [..]  |
|    | DOWNSAMPLED best bid/ask 1:200 ms                               | Side Price   Qty  Own  |
|    | ask ##########...........                                30002.5| ASK 30000.0 0.913      |
|    |      ######.......####                                         | ASK 29999.9 7.471      |
|    | ---------------------|cursor------------------------- 29999.0 | ASK 29999.0 0.122 <best|
|    | bid ######......####   o o x                                    | spread 1 tick mid ...  |
|    |                                                         29997.0 | BID 29998.9 0.298 <best|
|    |  +00:18.5   +00:19.0   +00:19.5   +00:20.0   +00:20.5           | BID 29998.7 0.017 #50944|
|    +-----------------------------------------------------------------+------------------------+
|    | [Event log] [Orders alive 1,031] [Account at cursor]       [..] | INSPECTOR (event/order)|
|    | t (rel)          Clock Src     Event          Order  Px    Qty   |                        |
|    | +00:20.099 812   EXCH  engine  fill           50944  29998.7 .. |                        |
+----+-----------------------------------------------------------------+------------------------+
```

- The market-only mode reuses `ReplayPage.tsx`, `HeatmapView.tsx`, `heatmap.ts` and
  `DepthLadder.tsx`. The run mode reuses `ReplayTerminalPage.tsx`, `TerminalHeatmap.tsx`,
  `TerminalLadder.tsx`, `EventLog.tsx` and `overlay.ts`. The two pages merge into one workspace with
  a `Market only | With run` segmented control. Both share one heatmap renderer.
- The ladder shows a `Side` column (`ASK`/`BID`), with asks above bids, and best rows in 600
  weight on `--ask-weak`/`--bid-weak`. A depth bar is decorative; the quantity is always printed.
- The `DOWNSAMPLED` and `TRUNCATED` notices are kept (§6.10).

### 11.4 Strategy Lab (`/strategy`, rail `SL`)

```
+----+------------------------------------------------------------------------------------------+
| SL | Toolbar: Dataset [synthetic-demo v]  Preset [Touch maker: baseline v]  [Validate]       |
|    |          [Run simulation  Ctrl+Enter]  (aria-disabled in DEMO: "Requires an API key")   |
|    +------------------------+-------------------------------------------+---------------------+
|    | PRESETS          [..] ^| CONFIGURATION                     [..]    | VALIDATION / DIFF   |
|    | Name         Strategy  | [Strategy] [Execution] [Fees] [Risk] [Scripted]| 0 errors        |
|    | Touch maker  touch_m.  | Strategy                                  | Changed vs preset:  |
|    | Slower line  touch_m.  |  Kind         [touch_maker v]             |  order_latency.     |
|    | Conservative q.        |  Interval     [ 100 ][ms v] = 100,000,000 ns| entry_ns 1 ms->50ms|
|    | Maker rebate           | Execution  MODELED                        | Run label (opt.)    |
|    | Avellaneda-Stoikov     |  Exchange model [no_partial_fill v]  (i)  | [               ]   |
|    | Imbalance taker        |  Queue model    [power_prob3 v] n [3]  (i)| Seed [1]            |
|    | Forensics probe        |  Entry latency  [ 1 ][ms v]               | Config hash (local  |
|    |  (description in insp.)|  Response lat.  [ 1 ][ms v]               |  preview) 253bbc..  |
+----+------------------------+-------------------------------------------+---------------------+
```

- Reuse: `LabPage.tsx` data flow, `fields.ts`, `FieldInput.tsx`, `validate.ts` and
  `ScriptedOrdersEditor.tsx`. The five numbered sections become a preset list, a tabbed
  configuration and a validation/diff panel. "Scenario A (optional baseline)" becomes "Compare
  with…" in the diff panel.
- Submitting in research mode creates a **job** and navigates to `/sim/jobs/:jobId`. It does not
  block the form. A content-addressed resubmission is reported honestly as "Already simulated —
  stored run 253bbc… returned" (current idempotence, `docs/API.md:48-51`).

### 11.5 Simulation Workbench (`/sim`, rail `SW`)

```
+----+------------------------------------------------------------------------------------------+
| SW | Toolbar: [Jobs] [Runs] [Sweep]   Filter state [non-terminal v]  [x] Mine only            |
|    |          API waking 37 s - free host; first response can take about a minute (WAKING)    |
|    +-----------------------------------------------------------+------------------------------+
|    | JOB QUEUE   worker running 1 · queued 3 · hb 2 s     [..] | JOB 7f3a91c0d2e4      [..]   |
|    | State          Job           Kind    Label     Created  Pos| State     RUNNING            |
|    | [*] RUNNING    7f3a91c0d2e4  run     slower..  12:30:05  - | Progress  events 1,204,331 / |
|    |                 42% =========-----------  hb 2 s           |           2,867,000 · 42%    |
|    | (o) QUEUED     91c0aa12b7e3  sweep   lat x fee 12:31:10 #1 | Heartbeat 2 s ago (every 5 s)|
|    | (o) QUEUED     a2e3...       run     ...       12:31:40 #2 | Started   2026-11-14 12:30:41|
|    | [] CREATED     c0d1...       run     ...       12:32:02  - | Elapsed   1 min 12 s         |
|    | (/) CANCELLING 55aa...       run     ...       12:20:00  - | Config    253bbc.. [view]    |
|    | [#] COMPLETED  4bde...       run     grid x=2  12:10:44  - | Dataset   synthetic-demo     |
|    | /!\ FAILED     9f01...       run     ...       12:05:13  - |                              |
|    | [\] CANCELLED  ...                                          | [Cancel job  Del] (auth)     |
|    | /_\ EXPIRED    ...  reason: <server text>                   |                              |
|    +-----------------------------------------------------------+------------------------------+
|    | RUN DETAIL (selected completed job)  [Manifest][Engine cross-check][Logs]  [Open run ->] |
|    | Run id 4bde4759b7843c27 · Config SHA-256 ... · Result SHA-256 ... · Wall time 29.0 ms     |
+----+------------------------------------------------------------------------------------------+
```

- The **Jobs** tab is the queue above. It shows queue position, progress and heartbeat exactly as
  §10.4 specifies. Rows sort by state group (running → cancelling → queued → created →
  terminal), then by creation time.
- The **Runs** tab reuses `RunsPage.tsx` as a DataTable of stored runs, with multi-select →
  "Compare (2)" / "Compare set (n)".
- The **Sweep** tab reuses `GridPage.tsx` axis definitions and `gridModel.ts`. In research mode it
  submits one sweep job that creates up to 100 run jobs (the current `MAX_GRID_CELLS`,
  `docs/API.md:50`). The results open in Experiment Comparison.
- Run detail reuses the `RunOverviewPage.tsx` manifest and engine cross-check panels.
- Cold start: when the API status is `waking`, a non-dismissible `WAKING` notice is shown in the
  toolbar row. The table keeps its last data with `STALE` until the first response.

### 11.6 Execution Forensics (`/forensics/:runId[/orders/:orderId]`, rail `EF`)

```
+----+------------------------------------------------------------------------------------------+
| EF | Toolbar: Run [253bbc23b342 touch_maker v]  Outcome[all v] Side[all v]  [Open in Book Lab]|
|    +----------------------------------+-----------------------------------------------------+
|    | ORDER BLOTTER  MODELED      [..] | TWO-CLOCK TIMELINE  order 6400      [Linear|Order]   |
|    | Order  Side Px      Qty   Outcome| LOCAL     <>-o                                  o    |
|    | 6400   BUY  29998.8 0.010 filled | EXCHANGE      ^-x            |  |              (O)  |
|    | 6401   SELL 29999.0 0.010 cancel.|          decision  +20 ms   +40 ms  ...   +100 ms    |
|    | 6402   BUY  ...        working.. | (selected event -> inspector)                       |
|    |  Filled/Remaining, Entry lat,    +-----------------------------------------------------+
|    |  Time to first fill, Markout 1s  | MODELED QUEUE POSITION  power_prob3(n=3)  MODELED   |
|    |  (virtualized, 2,060 rows)       | qty ahead  ____________|_______                     |
|    |                                  |                        |____________(O)             |
|    +----------------------------------+-----------------------------------------------------+
|    | [Explanation] [Decision context] [Latency breakdown] [Fills] [Flight recorder]   [..]  |
|    | 1 DERIVED  At t=2.600 s the strategy decided to buy 0.010 at 29998.8 ...               |
+----+------------------------------------------------------------------------------------------+
```

Reuse: `OrderForensicsPage.tsx` and the whole `features/forensics/*` (timeline, queue chart,
panels, evidence, labels), plus the `RunOverviewPage.tsx` orders/fills tables for the blotter.
The blotter's outcome column uses the current outcome vocabulary (`describe.ts`, `OrderOutcome`)
as text status pills (P6). Selecting a blotter row drives every other panel through the URL.

### 11.7 Risk & Performance (`/risk/:runId`, rail `RP`)

```
+----+------------------------------------------------------------------------------------------+
| RP | Toolbar: Run [253bbc23b342 v]  Range [all v]                       [Download run .json] |
|    +------------------------------------------------------------------------------------------+
|    | KPI  Net P&L −74.71 USDT | Fees 74.46 | Max DD 74.71 | Fill ratio 60.24% | Markout 1 s  |
|    |      DERIVED               ...                                        −0.137 ticks      |
|    +---------------------------------------------------+--------------------------------------+
|    | EQUITY AND P&L  DERIVED (synced zoom)        [..] | METRICS TABLE  DERIVED          [..] |
|    |  ~~~~~~~~~~\___________                           | Metric             Value    Unit     |
|    +---------------------------------------------------+ Orders submitted   2,060    count    |
|    | POSITION / INVENTORY  DERIVED                [..] | Spread capture     −0.076   ticks    |
|    |  _|-|_|--|_                                       | Markout 100 ms     −0.129   ticks    |
|    +---------------------------------------------------+ Realized P&L       −0.2485  USDT     |
|    | DRAWDOWN  DERIVED                            [..] | ...                                  |
+----+---------------------------------------------------+--------------------------------------+
```

Reuse: `RunOverviewPage.tsx` "Run metrics" and "Account charts" panels and `TimeSeriesChart.tsx`.
All charts share the time cursor and zoom (P8). The engine cross-check summary stays one click
away (Workbench run detail). Lightweight Charts attribution moves from the footer to the System →
About tab and to an "i" link on each chart panel menu. The attribution text is unchanged from
`App.tsx:160-166`.

### 11.8 Experiment Comparison (`/compare/...`, rail `EC`)

```
+----+------------------------------------------------------------------------------------------+
| EC | Toolbar: [Pair] [Set] [Experiment]   A [253bbc.. v] B [d6a57a.. v] [Swap A/B]             |
|    +--------------------------------------------+---------------------------------------------+
|    | WHAT CHANGED (varied)               [..]  | MEASURED OUTCOMES  DERIVED            [..]  |
|    | Path                          A      B     | Metric        A        B        Δ (B−A) Unit|
|    | execution.order_latency.entry 1 ms  50 ms  | Fills         1,241    745      −496  count |
|    | execution.order_latency.resp. 1 ms  50 ms  | Fill ratio    60.24%   42.35% −17.89 pp     |
|    | Controlled: 12 settings identical [show]   | Net P&L      −74.7096 −44.8362 +29.8734 USDT|
|    +--------------------------------------------+---------------------------------------------+
|    | EQUITY OVERLAY  DERIVED  A ── B - -                          | FIRST DIVERGENCE       |
|    |                                                             | order 6412 at +02.700  |
|    +-------------------------------------------------------------+------------------------+
|    | [Order alignment] [Diverging decisions] [Gross to net] [Interpretation (verbatim)]     |
+----+------------------------------------------------------------------------------------------+

Experiment tab (sweep result):
|    | MATRIX  Net P&L  MODELED   metric[Net P&L v]  | CELLS                                  |
|    |  maker fee \ latency   2.00 ms     10.0 ms    | Latency  Fee     Run          Net P&L  |
|    |  0.50 bp              −18.6538    −17.1738    | 2.00 ms  0.50 bp 4bde4759b784 −18.6538 |
|    |  1.50 bp              −55.4043    −51.0443    | (state pill per cell job)              |
|    |  diverging scale centred on 0; missing = hatched + "not completed"                     |
```

Reuse: `ComparePage.tsx` + `EquityOverlay.tsx` (Pair), `LatencyLabPage.tsx` (Set: baseline + N),
and `GridPage.tsx` result matrix + cells table (Experiment). Matrix cells print their value and
link to their run. Colour is a secondary cue; a missing cell is hatched **and** labelled.

### 11.9 Research Reports (`/reports`, `/reports/:runId`, rail `RR`)

```
+----+------------------------------------------------------------------------------------------+
| RR | Toolbar: [Download .md] [Download run .json] [Copy link]                                  |
|    +---------------------------+--------------------------------------------------------------+
|    | REPORTS             [..]  | Cost of latency: 1 ms entry and response latency  (18 px)  |
|    | Run          Label  Date  | SIMULATED on historical replay - research only ...           |
|    | 253bbc23b342 Cost.. 11-13 | 1. Reproducibility manifest (table)                          |
|    | d6a57aed972e Cost.. 11-13 | 2. Execution assumptions  MODELED                            |
|    | ...                       | 3. Results  DERIVED  (tables, figures from API)              |
|    |                           | 4. Limitations (verbatim from docs/LIMITATIONS.md)           |
|    |                           |  max line length 100ch, prose 13 px, tables 12 px            |
+----+---------------------------+--------------------------------------------------------------+
```

The report body is the server's markdown report (`GET /api/runs/{id}/report`, `docs/API.md:44`),
rendered with a sanitising markdown renderer: no raw HTML, links `rel="noreferrer"`. This is the
only workspace whose main panel scrolls as a document.

### 11.10 System / Admin (`/system`, rail `SA`)

```
+----+------------------------------------------------------------------------------------------+
| SA | Toolbar: [Health] [Queue] [Storage] [Access] [About]                                      |
|    +------------------------------------------------------------------------------------------+
|    | HEALTH                                                                                   |
|    | Component  State     Detail                         Checked                              |
|    | API        ok        42 ms round trip               12:34:50 UTC                         |
|    | Worker     running   heartbeat 2 s ago (every 5 s)  12:34:50 UTC                         |
|    | Queue      3 queued  oldest queued 1 min 20 s       12:34:50 UTC                         |
|    | Database   <from health, if reported>                                                    |
|    | Storage    <from health, if reported>                                                    |
|    | Host note: free tier — spins down after 15 min idle; spin-up about 1 min [Render docs]   |
|    +------------------------------------------------------------------------------------------+
|    | ACCESS: mode DEMO read-only  [Enter API key]   |  ABOUT: version, contract, engine,      |
|    | (key management itself is server-side; no     |  licences, Lightweight Charts attribution|
|    |  key listing/creation UI in Phase 1)          |  research disclaimer (Health.disclaimer) |
+----+------------------------------------------------------------------------------------------+
```

Rows for components the health endpoint does not report are **omitted**, not shown as "ok". The
Queue tab is the Workbench job table without the detail panel, plus queue-wide counts.

---

## 12. Existing page → workspace mapping and reuse

| Existing route / file | Target workspace(s) | Reused as-is | Restyled / restructured | Retired |
|---|---|---|---|---|
| `/` `MissionControlPage.tsx` | Overview | demo + runs data calls | cards → table rows; CLI snippets → inspector | card layout `mission.css` |
| `/datasets` `DatasetsPage.tsx` | Market Data Explorer | all sections and wording | split into list + tabs + inspector | — |
| `/replay/:id` `ReplayPage.tsx`, `HeatmapView.tsx`, `heatmap.ts`, `DepthLadder.tsx` | Order Book Lab (market only) | renderers, key handling | controls → toolbar | page layout |
| `/runs/:id/replay` `ReplayTerminalPage.tsx`, `TerminalHeatmap.tsx`, `TerminalLadder.tsx`, `EventLog.tsx`, `overlay.ts` | Order Book Lab (with run) | renderers, URL sync, keys | panels → slots/tabs | `terminal.css` page grid |
| `/lab` `LabPage.tsx`, `fields.ts`, `FieldInput.tsx`, `validate.ts`, `ScriptedOrdersEditor.tsx` | Strategy Lab | field model, validation, editor | numbered sections → three panels; submit → job | — |
| `/runs` `RunsPage.tsx` | Simulation Workbench → Runs | data + compare selection | DataTable | `Pager` |
| `/runs/:id` `RunOverviewPage.tsx` | Workbench run detail (manifest, cross-check); Risk & Performance (metrics, charts); Execution Forensics (orders/fills blotter) | each panel component | redistributed | page |
| `/runs/:id/orders/:oid` `OrderForensicsPage.tsx` + `features/forensics/*` | Execution Forensics | all renderers + text | panels → slots/tabs | `forensics.css` page grid |
| `/latency` `LatencyLabPage.tsx` | Experiment Comparison → Set | comparison tables | DataTable | `latency.css` cards |
| `/compare` `ComparePage.tsx`, `EquityOverlay.tsx` | Experiment Comparison → Pair | all | DataTable | `compare.css` page grid |
| `/grid` `GridPage.tsx`, `gridModel.ts` | Workbench → Sweep (definition); Experiment Comparison → Experiment (matrix) | `gridModel.ts`, matrix renderer | submit → sweep job | sync "Running the grid" panel |
| (report download) `GET /runs/:id/report` | Research Reports | endpoint | rendered in-app | — |
| `ServerStatus` (`App.tsx:73-115`) | Status bar + System | health call, contract check | segments | footer |
| `BasisLegend` footer | Help overlay + System → About + per-badge tooltips | `BASIS_MEANING` | — | footer placement |
| `RESEARCH_BANNER` (`App.tsx:23-24`) | Status bar | exact text | — | `.shell-banner` row |
| `StaticViewerBanner` | Status bar mode segment + notice in Overview | text | — | banner row |

---

## 13. Anti-pattern checklist

Every PR that touches `web/` is reviewed against this list. Any "yes" blocks the merge.

- [ ] Giant rounded cards, or any radius above 3 px (except 8 px status-dot circles and chart
  markers).
- [ ] Gradients of any kind (`linear-`/`radial-`/`conic-gradient`) in UI chrome. Sequential and
  diverging **data** colour ramps in heatmaps and matrices are data encodings, not gradients, and
  are allowed.
- [ ] Glow: `box-shadow` or `text-shadow` with a colour, neon outlines, purple/cyan accent glows.
- [ ] Glassmorphism: `backdrop-filter`, translucent panels, frosted overlays.
- [ ] Hero sections, splash banners, big headline typography (> 18 px), marketing taglines.
- [ ] Fake or decorative stats: any number not returned by the API for the current context,
  placeholder metrics, animated counters, "trusted by" rows.
- [ ] Emojis anywhere in UI text, labels, toasts or reports.
- [ ] Random or ambient animation: pulsing dots, shimmering skeletons, auto-playing charts,
  transform-on-hover, parallax, confetti. Only user-started replay animates.
- [ ] Colour as the only carrier of meaning (side, state, sign, evidence basis).
- [ ] Hidden or dismissible research disclaimer, or paraphrased honesty text.
- [ ] Progress bars or ETAs not backed by server-reported counts.
- [ ] A second styling convention (Tailwind, CSS-in-JS, a component kit's theme) next to the tokens.
- [ ] Float conversion of nanosecond values (§7.3), or numbers formatted with `toLocaleString()`
  (locale-dependent).
- [ ] Secrets in URL, `localStorage`, logs or exports.
- [ ] Raw vendor (Tardis) rows, screenshots of vendor-data pages, or vendor datasets in the public
  demo.

## 14. Accessibility checklist

Target: WCAG 2.2 AA [S20], with the APG patterns named per component [S23].

- [ ] Text contrast ≥ 4.5:1 (all token pairs used are listed in §4.3). fg-2 is never placed on
  bid/ask tints.
- [ ] Non-text contrast ≥ 3:1 for control boundaries (`--border-control`) and focus indicators
  (SC 1.4.11).
- [ ] Not colour alone (SC 1.4.1): asks above bids plus a `Side` column; status pills carry text and
  shape; evidence badges carry text and border style; P&L has a sign character; matrix cells print
  values; missing cells are hatched and labelled.
- [ ] CVD side palette available in Settings and the palette (§4.2).
- [ ] Every action keyboard-operable (SC 2.1.1). There are no traps except modal dialogs, which
  close on Esc.
- [ ] Single-key shortcuts can be turned off in Settings and are inactive in text fields
  (SC 2.1.4).
- [ ] Focus visible (2 px accent outline) and not obscured by sticky headers, the status bar or
  toasts (SC 2.4.7, 2.4.11). Tables scroll the focused cell into view below the sticky header.
- [ ] Targets ≥ 24×24 CSS px (SC 2.5.8). Compact density is opt-in, and row actions remain
  reachable by keyboard and in the inspector.
- [ ] Landmarks: `header` (top bar), `nav` (rail, label "Workspaces"), `main` (workspace area),
  `aside` (inspector), `footer` (status bar). The skip link "Skip to workspace" is kept
  (`App.tsx:121-123`).
- [ ] Grid, tabs, toolbar, dialog, combobox and window splitter follow the APG patterns.
- [ ] Every canvas chart has `role="img"` with a text summary, a keyboard path (§8.3), and a
  **tabular alternative** ("View as table" in the panel menu) with the plotted points or events.
- [ ] Status messages via one polite live region (SC 4.1.3). Job transitions and API state
  changes are announced once per change.
- [ ] Tooltips are dismissible with Esc, hoverable, and persistent while focused (SC 1.4.13).
- [ ] Reduced motion: no transitions, no spinner rotation, no replay auto-advance on load
  (SC 2.3.3). Playback (SC 2.2.2) always has a visible pause control and Space.
- [ ] Reflow at 400 % zoom / 320 px width: panels stack, no two-dimensional page scroll except
  inside data tables, which are exempt as two-dimensional content (SC 1.4.10).
- [ ] Language and titles: `lang="en"`, and `document.title` = `<Workspace> · <context> — BlackBox
  HFT`.
- [ ] Numbers: screen readers get the unformatted sign ("minus 74.7096 USDT") via `aria-label` on
  cells that use U+2212 and thin spaces.

---

## 15. Implementation order and acceptance

1. **Tokens** (§4): replace `:root`, add fonts, and map old variable names. Acceptance: every
   existing page renders with the new tokens. A text search finds no hex colours outside
   `theme.css` except data ramps in `heatmap.ts`/`gridModel.ts`.
2. **Shell** (§5): top bar, rail, workspace toolbar, panel grid with `react-resizable-panels`,
   inspector, status bar (API/worker/mode/banner/clock), and legacy redirects (§9.2). Acceptance:
   all old URLs land on the new workspaces with their query preserved. F6 cycles regions.
   `RESEARCH_BANNER` is visible on every route.
3. **Core components** (§6): DataTable (virtualized), Panel v2, tabs, toolbar, notices, status
   pill, command palette, dialogs, glossary tooltips. Acceptance: APG keyboard behaviour for grid,
   tabs and splitter, verified in Playwright (`web/e2e`).
4. **Formatting** (§7): `parseNs`/`fmtNsUtc`/`fmtNsRel`/`fmtNsDuration`/`cmpNs` with unit tests,
   including values above 2^53 and negative values.
5. **Access + jobs** (§10): key dialog, `aria-disabled` mutations, job table with every state.
   Acceptance: with the API stopped, the UI shows WAKING then UNREACHABLE and keeps STALE data.
6. **Workspaces** (§11) in this order: Order Book Lab, Execution Forensics, Simulation Workbench,
   Strategy Lab, Experiment Comparison, Risk & Performance, Market Data Explorer, Overview, Research
   Reports, System.

New runtime dependencies (licence and version as read from the npm registry on 2026-11-14):
`react-resizable-panels` 4.22.1 (MIT), `@tanstack/react-virtual` 3.14.26 (MIT), `lucide-react`
1.72.0 (ISC), `@fontsource/inter` 5.3.0 (OFL-1.1), `@fontsource/jetbrains-mono` 5.3.0 (OFL-1.1).
No other UI dependencies. Docking libraries such as `dockview` (MIT, 8.9.0) were considered and
rejected in favour of the fixed-slot model (P3).

## 16. Backend dependencies the UI needs

These items are owned by the API/worker design, not by this document. The UI degrades as stated if
a field is absent.

| Need | Used by | If absent |
|---|---|---|
| All ns values as decimal strings (i64) | §7.3 everywhere | Number path with safe-integer assertion |
| Health: `worker.state`, `worker.last_heartbeat_age_ms`, `worker.heartbeat_interval_ms`, `queue.depth`, `server_time_ns` | status bar, System | segment shows `unknown`; no stale-heartbeat warning |
| Job resource: `id`, `state` (8 states of §10.4), `created/queued/started/finished_at` (ns strings), `queue_position`, `progress {done, total}`, `last_heartbeat_at`, `error {code, message}`, `expired_reason`, `run_ids[]` | Workbench, Overview, toasts | omitted fields → `unknown`, never estimated |
| Job list filter by state and by "mine" (key identity) | Workbench | client-side filter over the returned page |
| Optional server event stream for job updates | §10.4 | polling (2 s / 5 s) |
| Auth: header name/scheme for the API key, a cheap authenticated "whoami" read, 401 vs 403 semantics | §10.2 | — (required) |
| CORS + CSP: Vercel frontend calling the Render API cross-origin requires `connect-src` to include the API origin (`web/vercel.json:18` currently `'self'`), or a same-origin rewrite | shell | API unreachable from the hosted UI |
| Dataset manifest: timestamp resolution (source unit) | §7.3 precision | 9 digits shown with the sub-µs group dimmed |

## 17. Sources

All read on 2026-11-14.

- [S1] IBKR Guides — TWS Mosaic Layout. https://www.ibkrguides.com/traderworkstation/mosaic-layout.htm
- [S2] IBKR Guides — Color Grouping Linked Panels. https://www.ibkrguides.com/traderworkstation/color-grouping-linked-panels.htm
- [S3] IBKR Guides — TWS Hot Keys. https://www.ibkrguides.com/traderworkstation/hot-keys.htm
- [S4] LSEG Workspace Quick-Start Guide (PDF, 09/2023). https://www.lseg.com/content/dam/lseg/learning-centre/documents/workspace-quick-start-guide.pdf
- [S5] TradingView Advanced Charts docs — Multiple-chart layout. https://www.tradingview.com/charting-library-docs/latest/trading_terminal/multiple-chart-layout
- [S6] TradingView Help Center — How to sync the charts of my layout? https://www.tradingview.com/support/solutions/43000629992-how-to-sync-the-charts-of-my-layout/
- [S7] TradingView Advanced Charts docs — Keyboard shortcuts (and the linked Accessibility page). https://www.tradingview.com/charting-library-docs/latest/configuration/Shortcuts ; https://www.tradingview.com/charting-library-docs/latest/getting_started/accessibility/
- [S8] IBKR Guides — BookTrader Colors. https://www.ibkrguides.com/traderworkstation/booktrader-colors.htm
- [S9] EMSX API Programmers Guide — EMSX Subscription (order/route status tables). https://emsx-api-doc.readthedocs.io/en/latest/programmable/emsxSubscription.html
- [S10] NautilusTrader docs — Reports. https://nautilustrader.io/docs/latest/concepts/reports/ (source: https://github.com/nautechsystems/nautilus_trader/blob/develop/docs/concepts/reports.md)
- [S11] NautilusTrader docs — Data (timestamps `ts_event`/`ts_init`). https://nautilustrader.io/docs/latest/concepts/data
- [S12] QuantConnect docs — Backtesting Results. https://www.quantconnect.com/docs/v2/cloud-platform/backtesting/results
- [S13] Lovable docs — FAQ. https://docs.lovable.dev/introduction/faq
- [S14] Lovable docs — Sync your Lovable project with GitHub. https://docs.lovable.dev/integrations/github
- [S15] Lovable docs — Import a project. https://docs.lovable.dev/features/projects/import-project
- [S16] Lovable docs — Subscription plans. https://docs.lovable.dev/introduction/subscription-plans
- [S17] v0 docs — Import from GitHub. https://v0.app/docs/git-import
- [S18] v0 docs — GitHub. https://v0.app/docs/github
- [S19] v0 docs — FAQs. https://v0.app/docs/faqs
- [S20] W3C — Web Content Accessibility Guidelines (WCAG) 2.2, incl. relative luminance and contrast-ratio definitions and SC 1.4.1, 1.4.3, 1.4.10, 1.4.11, 1.4.13, 2.1.1, 2.1.4, 2.2.2, 2.3.3, 2.4.7, 2.4.11, 2.5.8, 4.1.3. https://www.w3.org/TR/WCAG22/ ; Understanding 2.1.4: https://www.w3.org/WAI/WCAG22/Understanding/character-key-shortcuts.html
- [S21] v0 docs — Pricing. https://v0.app/docs/pricing
- [S22] v0 — Plans and Pricing page. https://v0.app/pricing
- [S23] W3C WAI-ARIA Authoring Practices — Grid, Tabs, Window Splitter, Dialog (Modal) patterns. https://www.w3.org/WAI/ARIA/apg/patterns/grid/ ; https://www.w3.org/WAI/ARIA/apg/patterns/tabs/ ; https://www.w3.org/WAI/ARIA/apg/patterns/windowsplitter/ ; https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/
- [S24] Render docs — Deploy for Free (spin-down after 15 min idle, ~1 min spin-up, loading page, free compute plan 0.1 CPU / 512 MB, 750 instance hours). https://render.com/docs/free
- `react-resizable-panels` README (separator role and ARIA, keyboard, collapsible panels). https://github.com/bvaughn/react-resizable-panels
- npm registry metadata (versions and licences of the packages in §15), e.g. https://registry.npmjs.org/react-resizable-panels/latest
