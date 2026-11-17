# Phase 3.2: measured performance (2026-11-16/17)

Lab measurements only. Nothing here is field data (no RUM); INP is not reported because a scripted lab
interaction is not INP. Each table says where it was measured.

## Harness

`web/perf/` (`npm run perf -- --base <url> --profile desktop|mobile --reps N --label <name>`), Playwright 1.65
Chromium + CDP, no new dependency. Per repetition a new browser context (cold HTTP cache, empty storage).

| Profile | Viewport | CPU | Network (CDP `Network.emulateNetworkConditions`) |
|---|---|---|---|
| desktop | 1440×900, DPR 1, mouse | 1× | 10 Mbit/s down and up, 40 ms RTT (Lighthouse desktop) |
| mobile | 390×844, DPR 3, touch, Pixel 7 UA | 4× (`Emulation.setCPUThrottlingRate`) | 1.6 Mbit/s down, 750 kbit/s up, 150 ms RTT ("Slow 4G") |

Metric definitions (probe injected with `addInitScript`, buffered PerformanceObservers):

- **TTFB** navigation `responseStart`; **FCP** `first-contentful-paint`; **LCP** last candidate before the end of the
  window. **ready**: first animation frame where a scenario predicate holds (e.g. the Explore result table has numbers;
  the replay heatmap canvas has size, the ladder has rows and the cursor shows a time).
- **End of window**: after ready, 2 s without a long task or a network event (cap ready + 20 s; reached in every run).
- **CLS**: layout shifts without `hadRecentInput`, session windows (1 s gap / 5 s cap), largest window.
- **TBT**: Σ max(0, longtask − 50 ms) for long tasks starting between FCP and the end of the window.
- **Bytes**: CDP `loadingFinished.encodedDataLength` (transferred) and `dataReceived.dataLength` (decoded).
- **interaction_ms**: the largest Event Timing `duration` (`durationThreshold: 16`, rounded to 8 ms by the browser)
  for the interaction's `interactionId`; null when the browser records none (scrolling records none).
  **result ms**: trusted input `timeStamp` to the first frame showing the result.
- Summaries: median / p75 / max over 5 repetitions (linear interpolation).

Environment: Windows 11 (10.0.26300), Intel i7-14650HX (24 logical CPUs), 15.7 GiB, Node 24.14.1,
Chromium 158.0.8127.18 headless.

**Local vs production.** The local stack (`blackbox serve --web-dir`, loopback, debug build) serves static files and
API bodies **uncompressed**; production (Vercel + Render behind Cloudflare) sends brotli/gzip (entry JS 132 KiB on the
wire vs 397 KiB decoded; replay API 104 KiB vs 1,084 KiB). Local runs are therefore pessimistic on transfer and
optimistic on server latency; they are the like-for-like baseline for frontend A/B, not a production forecast.

## Baseline: `main` 7c49dae (local, 5 reps, cold cache)

Built from `git archive 7c49dae web` (entry `index-DDUSEMtM.js`). Commands:
`node perf/run.mjs --base http://127.0.0.1:18810 --profile <p> --reps 5 --label local-7c49dae-baseline`.

| Scenario | Desktop LCP | Desktop CLS | Mobile FCP | Mobile LCP | Mobile TBT | Mobile CLS |
|---|---|---|---|---|---|---|
| Explore `/` | 1,008 | 0.010 | 2,900 | 5,304 | 148 | 0.001 |
| Replay `/book/runs/…` | 2,032 | 0.033 | 3,224 | 11,168 | 218 | 0.344 |
| Run overview `/forensics/<run>` | 1,012 | 0.470 | 2,956 | 5,312 | 217 | 0.095 |
| Strategy Lab `/strategy` | 888 | 0.225 | 2,872 | 4,768 | 4 | 0.288 |
| Risk `/risk/<run>` | 1,004 | 0.399 | 2,972 | 5,320 | 266 | 0.200 |
| Compare pair | 860 | 0.283 | 2,948 | 4,328 | 128 | 0.001 |
| Report `/reports/<run>` | 1,000 | 0.226 | 2,884 | 4,140 | 0 | 0.131 |

Milliseconds except CLS; medians. Lab interactions on desktop: every interaction ≤ 40 ms with no long task. Mobile
(4× CPU): scenario switch 64 ms, replay step 128 ms, replay scrub 96 ms, order select 80 ms, blotter next page 88 ms
(longest task up to 357 ms), preset switch 64 ms, open menu 80 ms. Full tables:
`data/perf/local-7c49dae-baseline/{desktop,mobile}.md` (local).

**Main bottlenecks found**

1. **Layout shift** was the only Core Web Vital over its threshold (CLS ≤ 0.1) on desktop: five of eight pages at
   0.22–0.47; on mobile replay 0.34 and Lab 0.29.
2. **Mobile LCP** above 2.5 s on every page locally (4–11 s), dominated by uncompressed transfer on Slow 4G (753 KiB of
   JS on Explore) and, on Replay, 1,087 KiB of API data. In production with compression the same mobile profile gave
   LCP 2.2–4.5 s (single repetition, below).
3. **Large API bodies**: comparison pairs `limit=1000` 571 KiB, replay timeline window 774 KiB, run analytics 452 KiB,
   equity 339 KiB (decoded). These are bounded by the API's caps and compress about 10:1.
4. Entry chunk 397 KiB (132 KiB gzip): react-dom 620 KiB, react-router 368 KiB and react-resizable-panels 231 KiB of
   source; the panels library is needed by the shell itself.

## Production smoke: release `b8105d4` (1 repetition per profile, 5 s pauses)

Run once to stay within Vercel's bot checkpoint and the API's 60 MB/day anonymous budget (used ≈ 13.5 MB). No
checkpoint challenge; 0 API errors. The first desktop load met a cold Render instance: `/status` 22.8 s, Explore
TTFB 7.6 s, LCP 9.3 s. After that, desktop LCP 0.8–2.5 s; mobile FCP 1.5–1.6 s, LCP 2.2–4.5 s, TBT 0–468 ms; CLS
reproduced the local values (runs 0.47, risk 0.43, compare 0.28, Lab 0.23). Raw: `data/perf/prod-b8105d4-smoke/`.

## Production API latency (warm, 5 GETs each, from this workstation, 2026-11-16 15:39 UTC)

| Endpoint | TTFB min / median / max (ms) | Body (decoded) |
|---|---|---|
| `/status` | 211 / 258 / 315 | 961 B |
| `/runs?limit=10` | 247 / 264 / 288 | 18,930 B |
| `/runs/253bbc…` | 219 / 248 / 701 | 3,849 B |
| `/comparisons` | 225 / 236 / 265 | 28,549 B |
| `/datasets/synthetic-demo` | 238 / 262 / 281 | 7,743 B |
| `/runs/253bbc…/orders?limit=100` | 225 / 255 / 509 | 86,970 B |
| `/runs/253bbc…/analytics` | 237 / 271 / 914 | 462,839 B (total median 458 ms) |
| `/runs/253bbc…/timeline` 10 s window, 500 events | 247 / 264 / 320 | 343,981 B |

The ≈ 230 ms floor is network distance to the Singapore region from this workstation (India). Cold start: observed
naturally twice (first `/status` of a day 12.7 s; perf smoke 22.8 s). Not forced, to respect the free tier.

**Cold-start behaviour in the console** (verified in source): the status bar moves `connecting → waking (elapsed s) →
ok`; pane spinners say the free host may take about a minute; a request that failed because the API was asleep is
now retried once automatically when the health poll reports the API up (`components/ui.tsx` `useAsync`), with no
other automatic retries, so a cold start cannot cause a request storm.

## Optimisations (evidence first)

Layout-shift attribution (PerformanceObserver `sources`, same throttling) identified each cause:

| Page | Cause | Fix |
|---|---|---|
| Every page (0.011) | status-bar worker/build segments appeared when `/status` answered, moving the mode and banner | segments always rendered with fixed desktop widths (`shell/StatusBar.tsx`, `styles/shell.css`) |
| Strategy Lab (0.20) | step 1 rendered before the preset list; cards then grew 198 → 497 px | the Lab waits for the first preset list (`features/lab/LabPage.tsx`) |
| Report (0.03 desktop, 0.03 mobile) | side column panels painted before Contents; spinner above the summary | side column fills in once the report is parsed; spinner moved below the summary (`features/reports/ReportsPage.tsx`) |
| Risk (0.07–0.09) | chart grid replaced a 20 px spinner | chart area reserved while equity loads (`features/risk/*`) |
| Replay phone (0.34) | a spinner inside the phone toolbar re-wrapped it on every window load | announced status only; the heatmap and body show loading (`features/terminal/ReplayTerminalPage.tsx`) |
| Run overview (0.47 desktop) | toolbar wrapping and KPI strip | fixed with the Phase 3.1 audit items (toolbar disclosure, wrapping KPI grid) |
| Market workbench (0.04 desktop, 0.08 phone) | meta line appeared late and wrapped | one fixed line with ellipsis and title (`features/market/`) |
| Compare pair (0.27 desktop) | the evidence legend's Inter 500 face arrived after the data and re-wrapped it | Inter 500 requested at start (`main.tsx`) |
| Compare set (0.70 desktop, 0.65 phone on the data path) | each panel grew as its request finished | panels render together once the requests that shape them finished (`features/latency/LatencyLabPage.tsx`) |

Also: the layout-reset context moved out of `SplitPane` (entry chunk −1.75 kB); no other bundle change was made
because no measured bottleneck pointed at JS execution on desktop (0 long tasks during every interaction).

Not changed, with reasons: API body sizes (bounded, cached, compressed in production; shrinking them is an API
contract change); the entry chunk (react-dom/router dominate; no cheap split); local static compression (production
already compresses).

## After

**Measurement pitfall.** A first 5-repetition "after" run taken hours after the baseline showed mobile TBT 4–8× higher
and mobile interactions 40–230 % slower on every page, including unchanged ones. Same-session CDP
`Performance.getMetrics` showed the same main-thread work for both builds (e.g. Risk 1,030 vs 1,023 ms TaskDuration,
Explore 779 vs 718 ms; open-menu 93–116 vs 88–141 ms), so the difference was machine state, not code. All
comparisons below therefore alternate builds in one session (base, final, base, final; 3 repetitions each; mobile
profile; same server binary, database and data; egress reset before every run). Raw: `data/perf/ab-mobile.md`,
`data/perf/ab2-mobile.md` (local).

**A/B 1 (release `55084c0` UI vs `7c49dae`), medians of 6 repetitions per build:**

| Scenario | CLS base → final | TBT ms base → final | LCP ms base → final |
|---|---|---|---|
| Replay | 0.344 → 0.001 | 137 → 107 | 10,768 → 11,214 |
| Run overview | 0.095 → 0.006 | 110 → 82 | 5,198 → 5,720 |
| Strategy Lab | 0.288 → 0.001 | 0 → 18 | 4,676 → 5,026 |
| Risk | 0.200 → 0.003 | 207 → 184 | 5,198 → 5,712 |
| Report | 0.131 → 0.001 | 0 → 0 | 4,090 → 4,592 |
| Explore | 0.001 → 0.001 | 90 → 75 | 5,240 → 5,664 |
| Compare pair | 0.001 → 0.001 | 91 → 76 | 4,234 → 4,682 |

Desktop (5 repetitions, separate session): CLS on every page ≤ 0.016 (baseline 0.21–0.47 on five pages); no long
task during any interaction; interactions ≤ 24 ms. Run-to-run TBT drift within a build was ≤ 17 ms.

**Regression found and fixed.** LCP was 350–520 ms (+4–12 %) later on every mobile page. Causes: (1) the shared
label module imported the Strategy Lab's form definitions to name configuration paths, so most pages loaded a 37 kB
chunk; it now reads a 52-entry static table (`components/labFieldLabels.ts`, checked against the Lab definitions by
`labFieldLabels.test.ts`), and the chunk is 6 kB; (2) all five font faces were requested at start to stop one late
face from re-wrapping text; now only Inter 500 is.

**A/B 2 (fixed UI vs `7c49dae`), same protocol.** The run `ab2-base-2` had higher background load (per-repetition CPU
median 31 % vs 17–19 % for the others), which inflates its TBT; TBT below is therefore base-1 vs final-1 only. LCP and
CLS use all six repetitions.

| Scenario | CLS base → final | LCP ms base → final | TBT ms base-1 → final-1 | Longest task ms (6 reps) |
|---|---|---|---|---|
| Replay | 0.344 → 0.001 | 10,852 → 10,942 (+0.8 %) | 151 → 130 | 192 → 164 |
| Run overview | 0.095 → 0.006 | 5,198 → 5,348 (+2.9 %) | 103 → 85 | 172 → 172 |
| Strategy Lab | 0.288 → 0.001 | 4,686 → 4,912 (+4.8 %) | 0 → 19 | 181 → 158 |
| Risk | 0.200 → 0.003 | 5,220 → 5,360 (+2.7 %) | 241 → 216 | 257 → 200 |
| Report | 0.131 → 0.001 | 4,120 → 4,248 (+3.1 %) | 0 → 0 | 184 → 156 |
| Explore | 0.001 → 0.001 | 5,284 → 5,390 (+2.0 %) | 101 → 87 | 190 → 172 |
| Compare pair | 0.001 → 0.001 | 4,250 → 4,406 (+3.7 %) | 92 → 81 | 186 → 156 |

Mobile interactions (6 repetitions each): scenario switch 60 → 56 ms, replay step 60 → 52, scrub 60 → 48, order select
64 → 52, blotter next page 72 → 40, preset 48 → 48, open menu 76 → 72; blotter touch scroll to visible result 864 →
259 ms. FCP unchanged (±30 ms).

**Outcome.** CLS meets the 0.1 target on every measured page and profile (worst 0.016 desktop Risk; was 0.47).
Mobile TBT and the longest task are lower on almost every page. LCP is still 90–226 ms (+1–5 %) later than the
baseline locally: each page now downloads 9–20 KiB more uncompressed JavaScript (the Workbench, evidence ledger, Lab
and label code), which costs roughly 50–100 ms at the profile's 200 kB/s and is compressed about 3:1 in production.
LCP is above the 2.5 s target on the local mobile profile for every page (4.2–10.9 s) both before and after; the
production mobile smoke measured 2.2–4.5 s with compression. INP is not claimed: these are lab interactions.

## Remaining limitations

- No field data; INP not claimed. Lab interaction latency only.
- Local numbers are uncompressed; production numbers are one repetition each (budget and bot-protection limits).
- Cold start measured only when it occurred naturally.
- Mobile LCP on the Slow 4G profile is dominated by transfer (entry chunk 397 KiB uncompressed, replay window 1.1 MB
  of API data); reducing it needs smaller API windows or a smaller entry chunk, both larger changes left for later.
