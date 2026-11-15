# UI audit (phase 2)

Date: 2026-11-15. Base: `main` = `a745039`. Local stack: Vite dev (`web/`, port 5173) proxying a
`blackbox serve` on 127.0.0.1:8787 (`data/explore`, database `blackbox_explore`), signed in with a
research key (`explore-lab` workspace) unless noted. Browser: managed Chromium, device scale 1.

Method: 18 workspace views (Explore `/`, `/experiments`, Strategy Lab guided and advanced, `/sim`,
`/reports`, `/reports/<run>`, `/data`, `/book`, `/book/runs/253bbc23b342f373`, `/forensics`,
`/forensics/<run>`, `/forensics/<run>/orders/0`, `/compare/pair`, `/compare/set`,
`/compare/sweeps`, `/risk/<run>`, `/system`) at 1920×1080, 1440×900, 1280×800, 768×1024 and
390×844 (90 screenshots), 45 Tab presses per view at 1440 and 390, console errors and HTTP ≥ 400
per view, `scrollWidth` per view, and 12 error routes (unknown run, order, job, sweep, dataset,
saved report, 404 route) at 1440 and 390.

Results that need no fix: no console error or uncaught exception on any of the 90 views;
`scrollWidth` equals the viewport on all 90; every error route renders an error state (no blank
page, no crash); the 404 route renders the not-found page.

## Defect matrix (ranked)

Sev: **H** wrong or missing result / unusable; **M** hard to use or misleading wording; **L** polish.

| # | Sev | Page | Viewport | Repro | Screenshot | Status |
|---|---|---|---|---|---|---|
| 1 | H | Explore `/` | all | Sign in with a research key, open `/`. The result strip (fills, fees, net P&L) is missing, "Show the first divergence" and "First divergence" are disabled, the lifecycle says "The stored comparison has no divergence". Cause: `GET /comparisons` lists only the caller's workspace, so a keyed caller never finds the demo's comparison. | `audit-before-explore-keyed-1440.webp` → `audit-after-explore-keyed-1440.webp` | Fixed: the story reads `workspace=demo` |
| 2 | H | Comparisons pair / set | all | Signed in, open `/compare/pair?a=253bbc23b342f373&b=d6a57aed972e18d0` (the Explore "Stored comparison" link): "No stored comparison", offers to create a duplicate; `/compare/set` says NOT COMPARED. Same cause as 1. | `audit-before-compare-keyed-1440.webp`, `audit-compare-set-1440.webp` | Fixed: `api.readableComparisons` (own + demo workspace for keyed callers) |
| 3 | H | Risk & P&L `/risk/<run>` | all | Open the page directly (not after Runs): Equity, Drawdown, Position and Fees charts are ~10 px tall, series key unstyled. Cause: `.chart-host` height lived in `runs.css`, loaded only by the Runs chunk. | `audit-before-risk-1440.webp` → `audit-after-risk-1440.webp` | Fixed: `TimeSeriesChart.css` imported by the component |
| 4 | H | Order Book Replay `/book/runs/<run>` | 390 | Toolbar wraps to ~430 px; the heatmap, ladder, account, event log and selected order are stacked panes of 240 px that each scroll. | `audit-before-replay-390.webp` → `audit-after-replay-390.webp` | Fixed: phone layout (< 600 px) with tabs Book / Events / Order; speed, jump and links behind one disclosure; arrow-only step buttons (names unchanged) |
| 5 | H | Order forensics `/forensics/<run>/orders/0` | 390 | Order context, timeline, queue chart and evidence are stacked short panes; the timeline pane shows only its zoom buttons. | `audit-before-order-390.webp` → `audit-after-order-390.webp` | Fixed: phone tabs Order / Timeline / Evidence |
| 6 | M | System → Queue tab, Overview "My jobs" | all | `JobViews` used `sim.css` classes but did not import it; direct visits to `/system` drew job progress unstyled. | — | Fixed: `JobViews.tsx` imports `sim.css` |
| 7 | M | Order forensics | all | "Verdict `filled_queue_exhausted`" and "gtx limit" in the primary order card. | `audit-order-1440.webp` | Fixed: plain verdict wording (`VERDICT_TEXT`, code in a tooltip); `tifLabel` (post-only, …) |
| 8 | M | Replay selected order, Run blotter | all | Time in force shown as `gtx`; blotter column "TIF". | `audit-replay-1440.webp` | Fixed: "post-only"; column "Time in force" |
| 9 | M | Reports, Risk picker, Book index, All experiments, Compare/Sweep run pickers, Sweep base summary | all | Strategy shown as `touch_maker every 100 ms`. | — | Fixed: `strategyName` everywhere (Runs and Compare already used it) |
| 10 | M | Every page with `ErrorBox` | all | A 404 (`/forensics/0000000000000000`, `/risk/zzz`, `/reports/zzz`, `/sim/jobs/zzz`, `/compare/sweeps/zzz`) shows `ERROR 404 not_found`, `run not found` and a Retry button that cannot help. | `audit-error-404-1440.webp` | Fixed: 404 states what to check (id, workspace key) and hides Retry; the verbatim head stays (contract) |
| 11 | M | Risk analytics table | 1440, 1920 | A long `unavailable: insufficient sample: …` widens the table past the panel; the Value column of every other row is cut off. | `audit-before-risk-1440.webp` | Fixed: the reason wraps in its cell |
| 12 | M | Replay "Depth at cursor" | 1280–1920 | Column "Own orders (order qty)" and the caption are cut at the pane edge. | `audit-replay-1440.webp` | Fixed: header "Own orders", caption wraps |
| 13 | M | Cross-workspace context | all | Run views do not link each other: Replay links only to the run overview; Risk lacks Replay/Compare; Report lacks Replay/Compare; order forensics lacks Risk/Report/Compare; Run overview lacks Risk/Report. Breadcrumbs exist only on run overview, replay and order pages. | — | Fixed: `RunContextLinks` (Run overview · Replay · Risk & P&L · Report · Compare with…, carrying the run id, current view omitted) on Run overview, Replay, Order forensics, Risk, Report; breadcrumbs added for `/risk/<run>`, `/reports/<run>`, `/compare/pair?a&b`, `/sim/jobs/<job>`, `/sim/sweep?base` |
| 14 | L | All docked workspaces | < 960 | "Reset layout" shown although panes are not resizable below 960 px; row density takes a toolbar row on phones. | `audit-before-replay-390.webp` | Fixed: hidden below 960 / 600 px |
| 15 | M | Explore replay chart | 1920 | Last time-axis label clipped at the right edge ("0.6("). Markers are small (size 0.8–1). Replay slider reads `0.1 / 120.0 s` while the chart header shows 6 decimals. Zoom (wheel/drag) has no reset control except loading a new window. | `audit-explore-1920.webp` | Fixed: `rightOffsetPixels: 36` (1920: last label "0.660 s" whole), markers 1.5 / 2.5 selected, slider and header both `0.100 s` (3 decimals, exact ns in `title`; lifecycle keeps ns detail), "Reset zoom" button refits |
| 16 | M | Strategy Lab advanced | 1280, 1440 | Section tab row overflows ("Feed latenc…") with no scroll affordance; dataset select text truncated. | `audit-lab-advanced-1440.webp` | Fixed: `.lab-tabs` wraps onto a second row; all nine sections visible at 1440 |
| 17 | L | Comparisons set | 1440 | Run table "Execution assumptions" column cut; stray dashed rule under the baseline row. | `audit-compare-set-1440.webp` | Fixed: run-set pane 38 % (both rows visible), assumptions column 320 px with ellipsis and full text in `title` |
| 18 | L | Risk | all | Caption "bb-analytics 1.0.0 …", Lightweight Charts logo overlaps the plot's lower-left corner. | `audit-after-risk-1440.webp` | Fixed: "Computed by the analytics engine (version 1.0.0)", raw name in `title`; logo off, text credit in the series key |
| 19 | L | System | 1440 | Worker detail "in_process · busy 0/1"; Datasets list Level column `l2_mbp`; run labels from sweeps carry `order_latency_ns=` (server-generated). | `audit-system-1440.webp` | Fixed (UI labels): "in-process worker · 0 of 1 slots busy", Mode column plain; Level "Price-level book" (raw code in `title`). Sweep run labels are server data, unchanged |
| 20 | L | Top bar | 390 | Access badge wraps "Sign out" under it; workspace switcher label ellipsised ("Order Bo…"). | `audit-after-replay-390.webp` | Fixed: below 600 px the badge shows "DEMO"/"RESEARCH" on one line (detail in `title`), switcher reads "Latency experiment" in full |
| 21 | L | Error routes | all | Toolbar actions (Open in Order Book Replay, Download report, Compare with…) stay enabled for a run that 404s. | `audit-error-404-1440.webp` | Fixed: run overview actions are disabled buttons until the run loads (404: all off except All runs / Reload) |

## Keyboard (Tab order)

At 1440 the order is skip link → top bar → rail → page toolbar → content on every view. Explore:
Research views → "Both scenarios lose money" → scenario buttons → First divergence → Replay tools
→ replay time → chart note → orders table → guided steps. With the comparison found (fix 1) the
lifecycle CTA follows the orders table, matching the visual order of the wide grid (book has no
focusable control); the e2e keyboard path (Tab to "First divergence", Enter, Tab into the
lifecycle) is unchanged. Order forensics: the glossary terms of the order card take focus in
document order, which is the visual order (unscrolled y: Queue ahead 649, Fees 753, latency 1327,
Feed latency 1375, Fees 1403); the earlier report came from coordinates read while the pane
scrolled. Verified, no change needed. No focus trap or invisible focus stop found.

## Files changed

`web/src/api/client.ts` (`DEMO_WORKSPACE`, `readableComparisons`), `api/http.ts` + test (404
action), `components/{PaneTabs.tsx (new), Toolbar.tsx, ui.tsx, describe.ts}`, `shell/{Shell.tsx,
Workspace.tsx}`, `styles/shell.css`, `features/explore/{story.ts, data.ts, LifecyclePanel.tsx,
StepGuide.tsx}`, `features/compare/ComparePage.tsx`, `features/latency/LatencyLabPage.tsx`,
`features/terminal/{ReplayTerminalPage.tsx, TerminalLadder.tsx, terminal.css}`,
`features/replay/replay.css`, `features/forensics/{OrderForensicsPage.tsx, labels.ts}`,
`features/runs/{RunContextLinks.tsx (new), TimeSeriesChart.tsx, TimeSeriesChart.css (new),
runs.css, RunOverviewPage.tsx}`, `features/risk/{RiskPage.tsx, AnalyticsPanels.tsx, risk.css}`,
`features/reports/ReportsPage.tsx`, `features/book/BookIndexPage.tsx`,
`features/mission/MissionControlPage.tsx`, `features/grid/{AxisEditor.tsx, SweepFormPage.tsx}`,
`features/sim/JobViews.tsx`. No e2e selector, test id or heading used by the specs changed.

## Checks

`npx tsc --noEmit -p tsconfig.app.json` pass; `npx vitest run` 11 files, 112 tests pass;
`npm run build` pass. Playwright not run here (lead runs it).
