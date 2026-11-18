# Phase 6: start-up, loading states and mobile LCP (tracker C1, C2, C3)

These are lab measurements only. There is no field data and INP is not claimed. The method is the one in
[PHASE32_PERFORMANCE.md](PHASE32_PERFORMANCE.md): the `web/perf/` harness, the `mobile` profile (390×844, DPR 3,
4× CPU, 1.6 Mbit/s down, 150 ms RTT) and the `desktop` profile (1440×900, 10 Mbit/s, 40 ms). Every comparison
below alternates builds in a single session. Each run uses the same server binary (`data/e2e-ui/blackbox-perf.exe`),
database (`blackbox_e2e_ui`), data directory and port (18810), with `BB_CORS_ALLOWED_ORIGINS=http://127.0.0.1:18810`
and production credentials unset. Only `--web-dir` changes between runs. Before each run, `bb.egress_usage_daily` is
reset (`delete … where day >= (now() at time zone 'utc')::date - 1`) and the server is restarted. Each repetition
starts from a cold cache.

Builds under test:

- **base**: `git archive af3a284 web` built unchanged (entry `index-CrlwZvnA.js`).
- **final**: the same tree with the files changed here (entry `index-BIOtS95J.js`).
- **noboot**: `final` without the `index.html` boot markup, used only to attribute effects.

All three are built with the same `node_modules`. The tree includes no change by other Phase 6 agents.

Environment: Windows 11 (10.0.26300), i7-14650HX (24 logical CPUs), 15.7 GiB, Node 24.14.1, Playwright 1.65,
Chromium 158.0.8127.18 headless. **Caveat:** other agents were compiling on the same machine. The harness's per-run
machine CPU utilisation was 0.55–0.93 in A/B 1 and 0.75–1.00 in A/B 2. Blocking-time figures are therefore noisy:
the same build varied by up to 2.7× between repetitions, e.g. Risk 830–2,230 ms. Paint-timing differences are many
times larger than that spread. Local responses are uncompressed (see PHASE32 "Local vs production").

Raw data (local, not committed) is in `data/perf/p6ab-{base,final}-{1,2}/mobile.*` (A/B 1), `data/perf/p6ab3-{base,noboot,final}-{1,2}/mobile.*`
(A/B 2) and `data/perf/p6desk-{base,final}/desktop.*`.

## What dominated mobile LCP at `af3a284` (diagnosis)

These are single-repetition waterfalls from the base build (`data/perf/p6-probe-base`). Times are ms from the
document request. New harness fields `lcp_entries` (every LCP candidate) and `net.*.start_ms/end_ms` (waterfall)
provide them.

**Explore `/` (LCP 5,536 ms, element `h1.ex-question`)**

| Time | Event |
|---|---|
| 186 → 2,604 | entry chunk 397.9 KiB (react-dom 654 kB, react-router 89 kB, react-resizable-panels 76 kB of source) |
| 0 → 3,200 | nothing painted: `#root` was empty until the entry chunk ran (FCP 3,200 = the shell's "Loading workspace" spinner) |
| 2,873 → 5,479 | ExplorePage and **14 dependency chunks**, including `lightweight-charts` 171.4 KiB (ends 5,479) and `DataTable` 89.8 KiB (ends 4,709). The page module cannot run until all of them arrive, so the h1 waited for the chart library |
| 5,513 → 6,887 | the story's API calls, fully serial: `/demos` → 2× `/runs/{id}` → `/comparisons` → `/comparisons/{id}` → dataset |
| 7,109 → 8,222 | replay window (73.9 KiB) and two run timelines (47.8 + 39.5 KiB) |

Explore did not need `DataTable` at all. It reached it through `story.ts` → `terminal/timelinePaging.ts` →
`terminal/EventLog.tsx`, whose only shared code was a 9-line binary search. `EventLog` imports the table.

**Replay `/book/runs/{id}` (LCP 11,480 ms, element `h1.term-title`)**

| Time | Event |
|---|---|
| 200 → 2,618 | entry chunk |
| 4,811 → 5,216 | run and dataset manifest, so the run's title is known here |
| 5,493 → 11,215 | first window: timeline 774.1 KiB (2,238 events, 10 s) and book replay 297.7 KiB (25 levels) |
| 11,480 | h1 painted. The heading was rendered only together with the loaded window |

Fonts are not on the critical path: Inter 400/600 and JetBrains Mono 400 finish while the chunks load, and Mono 600
arrives after LCP. Normalisation and chart initialisation are also not critical: long tasks during load are 330–690 ms
in total on the 4× CPU profile, compared with 2.4–5.7 s of transfer before LCP.

## Changes

| Id | Change | Files |
|---|---|---|
| C1 | The top-level `Suspense fallback={null}` is replaced by a status line (`AppLoading`). The shell's own boundary (the one that actually suspends for every lazy workspace) now renders `WorkspaceLoading`, the frame of a workspace: an empty toolbar row of the same height above `.ws-body`, with `aria-busy="true"` and the existing `Spinner` (`role="status"`, elapsed seconds, "API is waking" text). It shows no data and no skeleton | `web/src/App.tsx`, `web/src/shell/Workspace.tsx`, `web/src/shell/Shell.tsx` |
| C1/C2 | `index.html` ships the same status line inside `#root` (`<p class="state-line" role="status">Loading BlackBox HFT…</p>`). It paints with the stylesheet while the 398 KiB entry chunk downloads, and React replaces it | `web/index.html` |
| C2 | `PriceChart` (and with it Lightweight Charts) is loaded with `lazy`. The import starts when `ReplayPanel` is evaluated, so the download overlaps the page's API calls. The chart box keeps its fixed height and shows "Loading the chart…" until the library arrives | `web/src/features/explore/ReplayPanel.tsx` |
| C2 | `lastEventAtOrBefore` moved from `EventLog.tsx` to `timelinePaging.ts`, so Explore no longer downloads `DataTable` (90 KiB) | `web/src/features/terminal/{EventLog.tsx,timelinePaging.ts}` |
| C2 | The replay page renders its true heading ("Order book replay · <run label>") as soon as the run is loaded, with the loading line under it. The window summary still waits for the data. The page now always owns its `h1` | `web/src/features/terminal/ReplayTerminalPage.tsx` |
| C2 | `loadLatencyStory` makes the same requests with the same results. `/comparisons` is requested with `/demos`, and the stored comparison's detail with the two runs. Of these requests only the two `/runs/{id}` reads take the heavy-read permit (`v1/runs.rs` `verify_bundle`), as before. `/demos` and `/comparisons` are snapshot reads, and `GET /comparisons/{id}` takes no permit. Errors surface in the old order (demos, runs, list, detail), and the abort signal is passed to every request | `web/src/features/explore/story.ts` |
| harness | `lcp_entries`, `phases` (what `#main` shows per frame until ready: `blank`, `boot`, `shell:<text>`, plus `aria-busy`), per-request `start_ms`/`end_ms`, and `long_tasks.blocking_ms` (blocking time from navigation start, independent of FCP) | `web/perf/{probe,run,net}.mjs` |

## Results

### A/B 1: base vs final, every scenario (mobile, 2 × 3 repetitions per build, medians of 6)

| Scenario | FCP ms | LCP ms | CLS (max of 6) | ready ms |
|---|---|---|---|---|
| Explore load | 3,222 → 816 | 5,564 → **4,276** | 0.001 → 0.000 | 7,119 → 5,728 |
| Explore first chart (chart drawn, window current) | 3,234 → 808 | 5,566 → 4,220 | 0.001 → 0.000 | 8,252 → **7,036** |
| Explore scenario switch | 3,316 → 804 | 5,616 → 4,126 | 0.001 → 0.000 | 8,353 → 6,983 |
| Replay load | 3,290 → 806 | 11,462 → **5,232** | 0.001 → 0.001 | 11,445 → 11,439 |
| Replay seek | 3,254 → 800 | 11,498 → 5,256 | 0.001 → 0.001 | 11,476 → 11,452 |
| Order select (`/`) | 3,264 → 808 | 5,580 → 4,144 | 0.001 → 0.000 | 8,331 → 7,015 |
| Run overview | 3,230 → 784 | 5,756 → 5,746 | 0.006 → 0.006 | 6,532 → 6,496 |
| Strategy Lab | 3,242 → 814 | 5,402 → 5,364 | 0.001 → 0.000 | 5,156 → 5,106 |
| Risk | 3,194 → 810 | 5,816 → 5,796 | 0.003 → 0.003 | 9,417 → 9,522 |
| Compare pair | 3,318 → 802 | 4,612 → 4,502 | 0.002 → 0.000 | 11,089 → 10,988 |
| Report | 3,242 → 796 | 4,460 → 4,446 | 0.001 → 0.000 | 4,872 → 4,701 |

On Explore, the JavaScript transferred drops from 766.1 to 675.8 KiB and the total from 1,172.8 to 1,082.7 KiB, with
the same 35 requests and 11 API requests. Other pages are within ±0.6 KiB. API errors: 0 in every run.

### A/B 2: attribution, base vs noboot vs final (mobile, 2 × 3 repetitions each, medians of 6)

| Scenario | FCP base / noboot / final | LCP base / noboot / final | blocking from nav start (ms) base / noboot / final |
|---|---|---|---|
| Explore | 3,288 / 3,338 / 816 | 5,614 / 4,248 / 4,146 | 924 / 1,050 / 892 |
| Replay | 3,202 / 3,284 / 814 | 11,372 / 5,274 / 5,236 | 856 / 1,175 / 992 |
| Strategy Lab | 3,218 / 3,294 / 812 | 5,222 / 5,404 / 5,340 | 536 / 694 / 558 |
| Risk | 3,244 / 3,292 / 812 | 5,626 / 5,802 / 5,724 | 1,206 / 1,778 / 1,645 |
| Report | 3,194 / 3,300 / 812 | 4,486 / 4,484 / 4,434 | 494 / 626 / 492 |

The LCP gains come from the chunk and heading changes (noboot). The FCP gain of about 2.4 s comes from the boot
markup, which does not change LCP. Explore's story API chain, measured from the first API request to the last
story response, fell from a median of 1,038 ms (base, 6 reps: 1,011–1,441) to 838 ms (final: 832–919). The replay
window and timelines finished 2,592 → 2,450 ms after the first API request (medians).

### C1: the workspace area is never blank (phases, mobile, A/B 2, 30 loads per build)

| Build | Time with an empty page or an empty workspace area before ready (ms, per load) |
|---|---|
| base | 1,633–2,593 (blank `#root` until the entry chunk ran) |
| noboot | 2,304–2,779 |
| final | **0** in all 30 loads: `boot` from the first frame, then `shell:Loading workspace…` (`aria-busy`), then the workspace |

The CLS of every page is the same or lower (worst 0.006 mobile, Run overview, unchanged). The loading frame occupies
the workspace's own box. The e2e check (`workspace-loading.spec.ts`) asserts that the `.ws` box is identical before and
after the chunk arrives, and that the replay heading row does not move when the window loads.

### Desktop (base vs final, 3 repetitions each, same session)

| Scenario | FCP | LCP | CLS max |
|---|---|---|---|
| Explore | 676 → 228 | 1,060 → 916 | 0.002 → 0.002 |
| Replay | 644 → 216 | 2,296 → 2,380 | 0.001 → 0.001 |
| Run overview | 636 → 220 | 1,144 → 1,132 | 0.002 → 0.002 |
| Strategy Lab | 772 → 232 | 1,180 → 1,128 | 0.002 → 0.002 |
| Risk | 712 → 232 | 1,352 → 1,204 | 0.016 → 0.016 |
| Compare pair | 736 → 220 | 984 → 940 | 0.002 → 0.001 |
| Report | 732 → 228 | 2,636 → 1,804 | 0.004 → 0.001 |

Desktop CLS is unchanged on every page (worst 0.016, Risk, as in PHASE32). Desktop replay LCP is a different element
(an `.empty` pane, 15,059 px² at about 2.3 s, painted with the window), so the earlier heading does not move it. The
spread is 2,252–3,280 base and 2,328–3,416 final.

## Regressions and trade-offs (measured)

1. **TBT is higher (mobile, e.g. Explore 556 → 1,220 ms in A/B 1)** because TBT counts long tasks only after FCP, and FCP
   moved about 2.4 s earlier, into the entry chunk's evaluation and React mount. Blocking time from navigation
   start (`long_tasks.blocking_ms`) shows no consistent change: final is lower than base on Explore and Report and
   higher on Replay and Risk, within a same-build spread of up to 2.7× on this loaded machine. Long-task totals in A/B
   1 were lower for final on 9 of 13 scenarios (e.g. Risk 2,614 → 2,271, Compare 1,639 → 1,305).
2. **Navigating from Explore to a table workspace now downloads `DataTable` then.** Mobile `mobile-nav/go-workspace`
   (tap Strategy Lab in the menu until its cards render) went from 1,391 to 1,930 ms (medians of 6). Explore no longer
   prefetches the 90 KiB chunk on its own critical path, so the next table page pays for it: about 0.45 s at the
   profile's 200 kB/s uncompressed, and about 26 KiB gzip in production. Every other mobile interaction's
   `interaction_ms` median is within ±24 ms of base (8 ms resolution). Idle prefetching after Explore is ready is
   possible but was not measured, so it was not shipped.
3. Replay **ready** time is unchanged (11.4 s): the heading appears 6.2 s earlier, but the book still waits for the
   first window (next section).

## Recommendation (not implemented): smaller first replay window

The terminal's first window is `WINDOW_NS = 10 s` (`ReplayTerminalPage.tsx`). That is a frontend constant, not an
API contract, but it sets how much book the heatmap shows and how often the page loads windows, which is a product
decision. Measured on the local stack (run `253bbc23b342f373`, `t0 = 0.1 s`):

| First window | Timeline bytes (events) | Book replay bytes (25 levels) | Total |
|---|---|---|---|
| 1 s | 77,918 (222) | 214,334 | 292,252 |
| 2 s | 154,435 (445) | 220,560 | 374,995 |
| 5 s | 399,997 (1,138) | 265,504 | 665,501 |
| 10 s (current) | 792,692 (2,238) | 304,821 | 1,097,513 |

On the Slow 4G profile the 10 s window takes 5.7 s to arrive. A 2 s first window, with the existing paging loading
the rest on demand, cuts the critical bytes by 66 %. In production the 10 s window is about 96 KB gzip
(60,438 + 35,618) and about 66 KB brotli.

## C3: static immutable demo package served by Vercel (evaluation; not shipped)

**Problem.** Render's free instance sleeps after 15 minutes of inactivity. Measured wake times are in PHASE32 and
PHASE3_REVIEW: `/status` took 12.7 s and 22.8 s, Explore TTFB was 7.6 s with LCP 9.3 s, and one cold production FCP
was 19.7 s. DEPLOYMENT.md documents "about a minute". During that time the console shows WAKING. Warm production mobile
LCP is 2.2–4.5 s.

**What the package would contain.** These are the exact responses Explore reads, measured from the local stack (same
demo run ids as production: `253bbc23b342f373`, `d6a57aed972e18d0`):

| Part | Raw | gzip -9 | brotli |
|---|---|---|---|
| First view: `/demos`, 2× `/runs/{id}`, `/comparisons?workspace=demo`, `/comparisons/{id}`, dataset manifest, first replay window (6 levels) and 2 timelines | 235,648 | 27,821 | 22,456 |
| Equity (2× `max_points=200`) | 68,388 | 13,552 | 9,246 |
| Order lifecycle of the first divergence (2 orders) | 17,731 | 5,633 | 4,705 |
| Second moment window (market + 2 timelines) | 205,943 | 16,993 | 13,057 |
| **Explore, these parts** | **527,710** | **63,999** | **49,464** |
| Replay terminal first window (10 s) | 1,097,513 | 96,056 | 66,547 |

Other StepGuide steps add one moment window each (about 17 KB gzip). Each file would be stored under a content-hashed
path (`/demo/<sha256>/…`) with the existing `assets/` `immutable` cache header. A small `manifest.json` (no-cache)
would record, per file, its API route and sha256, plus provenance: each run's `bundle.sha256` (e.g. `65137731…bd4e`
for `253bbc23…`), dataset events SHA-256 (`0bc26426…8f38`), `result_platform`, the API `git_sha` that served it,
the export time, and the sha256 of `web/openapi.json` the package was exported against.

**Invalidation and mismatch detection.**

- The package is regenerated on every web deploy and is immutable by path, so a stale file is never served under a
  new manifest.
- Comparison ids are UUIDv7 values minted per environment (local `01a1df56-…` is not the production id). The package
  must therefore key the comparison by run pair, as `loadLatencyStory` already does, and never by id.
- Contract mismatch: the UI would use the package only when the manifest's OpenAPI hash equals the hash the bundle was
  built with. Both come from the same commit, so a mismatch means an old manifest and the package is ignored.
- Data mismatch: once the API answers, the UI fetches `/runs/{id}` (one request per run, already part of the live path)
  and compares `bundle.sha256` with the manifest. On any difference it drops the packaged view and shows the live one.
- Integrity labelling: the packaged view must say "stored copy exported <date>; verifying against the API…" until that
  check passes. The server verifies every bundle on read, and a static copy is not verified at view time.

**Why it is not shipped now.**

1. It needs an export step at build time. Exporting from production in CI couples every deploy to a woken Render
   instance and to the 60 MB/day anonymous egress budget, and needs network access the build does not have. Exporting
   from a local or CI stack depends on demo runs producing the same run ids and bundle hashes on Linux as in
   production. Run ids already match locally, and golden hashes are kept identical across Windows and Linux, but the
   production `bundle.sha256` was not checked here because production was not touched.
2. It adds a second data path to Explore (packaged and then live) with its own failure modes and tests. That is about
   the same amount of code as the page's loader, for a benefit that applies only to the first visitor after
   15 idle minutes.
3. The benefit is an inference, not a measurement. With the package, Explore's answer would paint at the warm
   static-asset LCP (4.1–4.3 s on the local mobile profile after this phase) instead of after a 12.7–60 s wake. The
   price chart, lifecycle and replay would still need the API unless their windows are packaged too (+64 KB gzip for
   Explore, +96 KB for one replay window).

The changes in this phase cover part of the same need without a second data path. The shell, boot line and workspace
frame now paint at about 0.8 s on mobile even while the API sleeps, and the existing WAKING status and single
automatic retry (PHASE32) remain. If a cold first visit becomes a priority, the smallest version is the "First view"
row above (28 KB gzip) with the manifest checks described here, behind the WAKING state only.

## Remaining limitations

- No field data. All runs are local, uncompressed and on a machine shared with parallel builds, so blocking-time
  conclusions are weak (spread above). Paint timings are many times larger than that spread.
- Production was not measured in this phase (no production load).
- Mobile LCP is still above 2.5 s on every page locally (4.1–5.8 s). Most of it is the 398 KiB uncompressed entry chunk
  (2.4 s at 200 kB/s) plus each page's chunk. Explore and Replay now paint their heading once their own chunk arrives.
