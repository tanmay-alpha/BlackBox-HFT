# Product baseline (M0)

Date: 2026-11-14. Observer: lead (Claude Opus 5.5), with automated headless Chromium. All values
below were observed in this session; nothing is estimated unless marked **[INFERENCE]**.

## 1. Source and infrastructure state

| Item | Observed |
|---|---|
| `main` = `origin/main` | `48e277b` (after `git fetch`); all 13 feature branches (`p1-*`…`p6-*`) are fully merged (0 commits ahead); their worktrees under `D:/bbwt/*` are clean |
| Production API `git_sha` | `7840bc6` (`/api/v1/status`). `7840bc6..48e277b` changes only docs/README/TODO (18 files), so production runs current source |
| Readiness | `/readyz` → `{"blob":"configured","db":"ok","migrations":"0009","status":"ready"}` |
| Worker | `in_process`, 1 slot, `state: idle`, heartbeat age 6–14 s against a 15 s interval (FINAL_AUDIT's 81–119 s stale-idle warning was **not reproduced** today) |
| Queue | depth 0, running 0 |
| Vercel | `https://blackbox-hft.vercel.app` → 200, built from `web/` |
| CI | `ci` and `publish` succeeded for `48e277b` and `7840bc6` (`gh run list`) |
| Hosted data | Synthetic only (`synthetic-demo` v1). Six demo runs, five curated comparisons stored in the `demo` workspace |

Not checked in M0: Render image digest, Neon/Blob quota usage, Vercel/Render error logs (no
dashboard access was used). These remain open for M10.

## 2. Baseline test results (commands actually run)

| Command | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | pass: **422 passed, 0 failed, 1 ignored** |
| `cd web && npx vitest run` | pass: 7 files, **93 tests** |
| `cd web && npm run build` | pass; one 1,068.43 kB JS chunk (327.90 kB gzip) with Vite's >500 kB warning |
| `npm ci` | not rerun (existing `node_modules`, Vite dev server running) |
| Playwright E2E (`npm run test:e2e`) | not run in M0 |
| DB integration tests | included in `cargo test` (local Postgres 18.7 on :55432 running) |

## 3. Anonymous first-visit test (production, 1440×900, clean profile)

Screenshots: `img/before-home-*.webp`, `img/before-latency-set-1440.webp`,
`img/before-forensics-order-1440.webp`, `img/before-terminal-anon-500-1440.webp`.

| # | Task | Outcome |
|---|---|---|
| 1 | Understand what BlackBox does | Hard. The page opens with a dataset selector, a density selector, and an experiment table whose "Varied" column shows raw config paths (`execution.order_latency.entry_ns`). The inspector shows raw JSON for strategy and risk settings. The one sentence that explains the product is in a SYSTEM panel below the fold. |
| 2 | Select a research question | Possible: "Cost of latency" row → "Open results". |
| 3 | Open a useful experiment | Opens `/compare/set` (Experiment Comparison). It is dense: run hashes, a config-path table, and 30 metrics rows. The headline result (both runs lose; 50 ms loses less) is not stated anywhere. |
| 4 | Watch market activity | **Blocked.** Replay Terminal `/book/runs/253bbc23b342f373` shows `ERROR 500 internal — the response would exceed the size limit for this caller` (D1). |
| 5 | Compare two execution scenarios | Possible on the comparison page, but you have to read a table. |
| 6 | Inspect why an order filled or not | Possible via Forensics → order page (works). |
| 7 | Understand simulated P&L | The metrics are present, but nothing states that net = realized + unrealized − fees, or that the fee difference accounts for most of the P&L gap. |
| 8 | Return to the initial experiment | Browser back works; context (selected order/time) is in some URLs but not all. |
| 9 | Export a report | Per-run Markdown report download works. |
| 10 | Recover from a failed API request | The error box shows a Retry button, but retrying the terminal fails the same way (deterministic). |

## 4. Responsive check (production home)

| Viewport | Observation |
|---|---|
| 1920×1080, 1440×900, 1280×800 | Renders. Navigation is an icon rail with two-letter codes (OV, MD, OB, SL, SW, EF, RP, EC, RR, SA). The inspector is open by default and shows a glossary. |
| 768×1024 | The inspector takes about half the width; tables are clipped. |
| 390×844 | Broken: the inspector overlays the content, the header controls overlap, and `document.documentElement.scrollWidth` = **1405 px** (horizontal page overflow). |

## 5. Defects

Severity: Critical = primary journey blocked; High = wrong/misleading or severe usability;
Medium; Low.

| ID | Sev | Category | Defect | Reproduction | Root cause (evidence) |
|---|---|---|---|---|---|
| D1 | Critical | Backend + frontend correctness | Anonymous Replay Terminal returns 500 for the public demo runs | Open `https://blackbox-hft.vercel.app/book/runs/253bbc23b342f373`; or `GET /api/v1/runs/253bbc23b342f373/timeline?max_events=0` → 500 `reason=response_too_large, limit_bytes=1048576` | The terminal requests 60 s windows (`ReplayTerminalPage.tsx:57`); a 60 s window of this run holds about 13k events plus fills/orders/equity and exceeds the 1 MiB anonymous cap. The timeline handler (`v1/runs.rs:854-891`) has no paging and maps the cap breach to 500 `internal`. |
| D2 | High | Frontend correctness | Silent replay truncation: `MAX_EVENTS = 5000` per window (`ReplayTerminalPage.tsx:62`) while one 60 s window holds about 13k events | Same run, keyed caller (4 MiB cap) | `server::timeline` (`server.rs:525-585`) truncates at `max_events` and has no cursor to continue |
| D3 | High | Usability | The first screen leads with config paths, hashes, raw JSON and two-letter navigation codes; the headline result of each experiment is never stated | §3 tasks 1, 3 | Console-first information architecture (`MissionControlPage.tsx`, `Shell.tsx`) |
| D4 | High | Visual / responsive | 390 px layout overflows to 1405 px; inspector overlays content | §4 | Fixed-width rail + inspector + tables with no mobile layout |
| D5 | Medium | Usability | Inspector open by default, showing a generic glossary | Any page | `shell/inspector.tsx` default state |
| D6 | Medium | Science communication | P&L difference not decomposed; a reader can conclude "slower is better" | Latency set page | No page states net = realized + unrealized − fees (stored metrics: 1 ms fees 74.46 / net −74.71; 50 ms fees 44.70 / net −44.84) |
| D7 | Medium | Performance | Single 1.07 MB JS bundle for every route | `npm run build` | No route-level code splitting |
| D8 | Info | Ops | After an out-of-process `demo seed` creates curated comparisons, a running server serves the old anonymous demo snapshot (`/comparisons` → `[]`) for up to 1 h | Local: seed while `serve` runs, then `GET /api/v1/comparisons` → 0 items; after restart → 5 | Documented design: `v1/snapshot.rs:7-12` (`MAX_AGE` = 3600 s) rebuilds on in-process changes or after an hour. Operator note: restart after seeding. Not a defect. |
| D9 | Info | Market-data limitation | Hosted results are synthetic only; nothing is empirically calibrated | `/datasets` | Licensing (MARKET_MICROSTRUCTURE §7) |

Carried from FINAL_AUDIT §4 and not re-tested here: egress/response caps on analytics (item 1),
the comparison mutation timeout (item 2), integrity exceptions on list reads (item 5), CI security
gates (item 7), and the legacy quick start (item 8).

## 6. Performance observations (production; client: this workstation, headless Chromium / curl)

| Measurement | Value | Method |
|---|---|---|
| `/healthz`, `/api/v1/status`, `/readyz` | 0.34 s, 0.23 s, 0.21 s | curl `time_total`, warm instance |
| Timeline, 10 s window, 5000 max events | 200, 792,673 B, 3.07 s | curl |
| Home `domContentLoaded` | 84 ms (CLS 0, no long tasks) | `tab.vitals()` after load, warm cache |
| Main JS | 1,068 kB (328 kB gzip) | Vite build output |

Lighthouse was not run.

## 7. Authenticated researcher journey

Not executed against production in M0, to avoid spending the free-tier budget. A local stack with
genuine data (same content-addressed run ids as production) is available: DB `blackbox_explore`,
`blackbox serve` on 127.0.0.1:8787, research key minted to `data/explore/key.txt` (git-ignored).
FINAL_AUDIT §1.2 has the last production evidence for submit → complete → persist.

Local keyed journey (API-level, `curl`, research key of workspace `explore-lab`):
`POST /experiments` (`maker_baseline` preset, seed `20261114`) → 202 `queued`, job
`01a1da5b-392e…` → `completed` 0.52 s after queueing (1,201 ticks), run `7626ba23d8835b13`
(1,241 fills, net −74.7096, result sha `57f055c5…`). Anonymous read of that run → 404
`not_found` (ownership holds). `POST /comparisons` against demo run `253bbc23b342f373` →
aligned, like-for-like, the only config diff is `seed`; metrics are identical to the demo run
(**[INFERENCE]** this configuration consumes no random draws; not traced). The browser version of
this journey is pending (T7).
