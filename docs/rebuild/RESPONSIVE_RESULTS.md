# Responsive results (D4)

Measured 2026-11-14 in headless Chromium against the Vite dev server (`localhost:5173`) and the local API, anonymous demo access.
`scrollWidth` is `document.documentElement.scrollWidth` after the route settled (2.5 s). Baseline at 390×844 before the fix: `/experiments` 1527, `/forensics` 1210, `/reports` 1104, `/book` 508 (cause: visually hidden table labels positioned against the initial containing block, plus an unclipped top bar and status bar).

Below 960 px: the rail is a **Menu** button opening a labelled section list (modal `<dialog>`: focus trapped, Esc closes, focus returns); the inspector is a full-screen dismissible sheet; the status bar wraps (build, clock and access mode hidden — on System status and the top bar); tables scroll inside their own container with a sticky row-header column.

Screenshots: `docs/rebuild/img/responsive-<route>-<width>.webp`.

| Route | 1440×900 | 768×1024 | 390×844 | Screenshot slug |
|---|---|---|---|---|
| `/experiments` | 1440 | 768 | 390 | `responsive-experiments-*` |
| `/data` | 1440 | 768 | 390 | `responsive-data-*` |
| `/book` | 1440 | 768 | 390 | `responsive-book-*` |
| `/book/runs/253bbc23b342f373` | 1440 | 768 | 390 | `responsive-book-runs-run-*` |
| `/strategy` | 1440 | 768 | 390 | `responsive-strategy-*` |
| `/sim` | 1440 | 768 | 390 | `responsive-sim-*` |
| `/forensics` | 1440 | 768 | 390 | `responsive-forensics-*` |
| `/forensics/253bbc23b342f373/orders/0` | 1440 | 768 | 390 | `responsive-forensics-run-orders-0-*` |
| `/compare/set?runs=253bbc23b342f373,d6a57aed972e18d0&demo=cost_of_latency` | 1440 | 768 | 390 | `responsive-compare-set-*` |
| `/risk/253bbc23b342f373` | 1440 | 768 | 390 | `responsive-risk-run-*` |
| `/reports` | 1440 | 768 | 390 | `responsive-reports-*` |
| `/system` | 1440 | 768 | 390 | `responsive-system-*` |

All 24 narrow measurements equal the viewport width; no element in `#main`, the top bar or the status bar overflows an `overflow: hidden` ancestor at 768 or 390 (one 9 px badge clip in a panel title on `/data` at 390).

Known gaps: workspaces built from stacked resizable split panes (`/book/runs/…`, `/forensics/…/orders/…`) keep their split layout on narrow screens, so some panes are short at 390×844 (each pane scrolls internally).
