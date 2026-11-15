# Phase 3 UX — independent product review

Date: 2026-11-15 · Reviewer: P3ReviewOpus. I did not build the candidate. Builder claims were not used as evidence.

| | Production | Candidate |
|---|---|---|
| Build | https://blackbox-hft.vercel.app, release `96a3ccb` (Vercel build, hosted API) | `phase3/ux` @ `c6ef6a3`, `http://localhost:5173` (Vite **dev** server, local `blackbox serve`, same demo data) |
| Browser | managed Chromium, DPR 1.25 at desktop sizes, DPR 2 with touch at phone sizes | same |
| Page loads | about 12 | about 28 |

Screenshots: `img/p3-review-{prod,candidate}-{explore,replay,lab,order,risk}-{1440,1024,768,390,360}.webp`. The previous reviewer captured these, and I checked them against the live pages. Four are new from this review: `img/p3-review-candidate-{explore-lifecycle,lab-invalid,lab-submitted,risk-run}-1440.webp`.

## Verdict: **ACCEPT** (fix D1–D3 before promoting to production)

The candidate is materially better on the first-visit question. At 1440×900 the question, a one-sentence answer, an aligned results table and the whole replay chart fit on the first screen. In production the chart begins at y≈581 and is cut off by the fold. Every requested journey finished on the candidate: anonymous Explore, then the keyed Lab → job → results → replay → forensics → risk → compare → report path. I saw no console errors (only navigation aborts), no horizontal overflow at any tested size, and the numbers match the API. The defects below are wording or state glitches. None of them blocks a journey.

## Scores (0–10)

| Criterion (weight) | Prod | Cand | Evidence |
|---|---|---|---|
| First-time usability (25%) | 6.5 | 8.0 | Cand: question, answer sentence, table and chart all above the fold at 1440 (chart top y=364 vs 581) and at 1024 (466 vs 594). "Δ … is not marked good or bad". Hovering the chart shows bid/ask or order details; prod showed no hover readout at the same point. Prod spreads the results over three big number blocks and hides the explanation in a "Why?" disclosure. |
| End-to-end workflow (25%) | 6.5 | 8.0 | Cand Lab: four steps (choose, configure, review, run), preset cards that each state their question, a sticky summary, clickable validation, and links to every next view once the job finishes. Prod Lab (screenshot) is a flat three-column form. Prod's researcher journey was not run (candidate-only assignment), so that score rests on the screenshots plus Explore parity. |
| Visual quality (20%) | 6.0 | 7.5 | Cand header is more compact, the scenario table is aligned, chips are readable, and the replay depth ladder shows a full book (prod squeezes it to one row). Cand regressions: D4 and D5. |
| Mobile (10%) | 5.0 | 7.5 | Interactive elements under 40 px on Explore at 390: **65 → 38**. On Replay at 390/360: **25 → 4**. The cand phone status bar is one line (prod: three). Cand adds Price chart / Order book tabs. Remaining small targets are listed under D6. No overflow on either side. |
| Accessibility (10%) | 7.0 | 7.5 | Same logical Tab order on both, with visible focus. On the cand, the skip link works, Enter on "Inspect order 256" moves focus to "Order 256 in both scenarios", and an aria-live region announces the selection. Neither Replay page has an `h1`. |
| Performance / maintainability (10%) | 7.0 | 7.0 | Both make the same 11 API calls per fresh anonymous Explore visit, about 237 KB decoded each (prod: 236,716 B decoded, 34,231 B brotli on the wire; cand local server: 236,937 B, uncompressed). Warm navigation to visible question: prod 174–265 ms, cand 269–288 ms (dev server). Prod's first cold visit had FCP 19.7 s (Vercel/API cold start, measured once). Cand bundle size could not be compared because it runs on a dev server. The diff is 94 files, +4,369/−1,251. |
| **Weighted** | **6.35** | **7.70** | |

## Numbers vs API (all match)

| Value | UI (both builds) | `GET /api/v1/runs/{id}` `.metrics` |
|---|---|---|
| Fills, 1 ms / 50 ms | 1,241 / 745 | `fill_count` 1241 / 745 |
| Fees, 1 ms / 50 ms | 74.46 / 44.70 | `fees` 74.461 / 44.701 |
| Net P&L, 1 ms / 50 ms | −74.71 / −44.84 | `net_pnl` −74.7096 / −44.8362 |
| New keyed run `f214079c0e4c36a8`: net / fees / fills / orders | −69.52 / 69.30 / 1,155 / 2,011 | −69.5205 / 69.3010 / 1155 / 2011 |

## Journeys walked

**Explore, anonymous, both builds:**

- Question and results were checked.
- Chart hover: the cand tooltip shows "1 ms · order 257 · Reached the exchange: sell 0.010 @ 30000.0".
- Clicking a marker selects its order (`?order=257&t=200000000&step=3`).
- Scenario change (`scenario=fast`) and seeking with arrows and End update the URL on both builds.
- "First divergence" opens order 0 at step 3 on both builds.
- The lifecycle shows the "What differed: Same outcome, different timing" summary (cand only), then the full forensics page.
- Back restores the exact URL state on both builds. Neither restores the scroll position.
- The catalogue lists 3 canonical experiments and recent runs.

**Wording:**

- The cand states "Both scenarios lose money. At 1 ms the strategy lost 74.71; at 50 ms it lost 44.84", so the +29.87 Δ is not read as a profit.
- The compare page says "deltas are not [coloured], because a smaller loss is still a loss".
- The cand forensics page explains the negative queue estimate directly under the value: "Minimum raw queue estimate (modeled) −0.207 — Negative means traded quantity beyond the modeled front, not negative displayed depth." Prod shows "Smallest queue ahead −0.207 modeled" with no explanation; the explanation appears only further down in the reasoning text.
- Fees are labelled "negative is a rebate" on both builds.

**Researcher, candidate only:**

- Access badge → "Research access" dialog → key stored in sessionStorage. The badge shows only the 4-character key-id prefix.
- Lab: chose "Touch maker: baseline". Guided change: entry latency −5 produced an inline error and disabled Run.
- Then set 7 ms / 7 ms and submitted **one** job, `01a1df4a…`, which completed in 0.6 s as run `f214079c0e4c36a8`.
- From there: job page → run blotter → Replay (`/book/runs/f214…`) → order 1 forensics → Risk (`/risk/f214…`) → Compare against `253bbc23b342f373`.
- Clicking "Create comparison" (a server write) gave ALIGNED with first divergence at order 0.
- Report (`/reports/f214…`) has a table of contents and hashes.
- No horizontal overflow on any of these pages at 390 or 1024.

## Must-fix defects in the candidate (ranked)

1. **D1 — The Lab tells the user a new job was "not needed".** Repro:
   1. Enter a key.
   2. In `/strategy`, change a value so the configuration is new (e.g. entry and response latency 7 ms).
   3. Click **Run simulation**.

   Until the first job poll returns, the "Job" row reads "not needed: this configuration was already simulated". Directly above it, the page says "Job 01a1df4a… queued, position #1". Cause: `web/src/features/lab/LabPage.tsx:245-257` treats `job === null` (not fetched yet) as "no job". This is misleading status text about a research action. Screenshot after the poll: `img/p3-review-candidate-lab-submitted-1440.webp`.
2. **D2 — The step bar says "Run · ready" while the configuration is invalid.** Repro: in `/strategy`, set Entry latency to `-5` and press Tab. The step bar shows "Review · 2 to fix" next to "Run · ready", while the Run button is disabled. See `img/p3-review-candidate-lab-invalid-1440.webp`.
3. **D3 — One invalid field is counted as two problems.** Same repro as D2. The page shows "2 problems to fix", listing the client message ("Entry latency: Must be at least 0 ms.") and the server message for the same field in raw nanoseconds ("order latency must be between 0 and 60000000000 ns (60 s)"). The summary repeats "2 to fix".
4. **D4 — The sticky Explore toolbar covers the chart's "Reset view" button.** Repro: 1440×900, `/`, click **First divergence · 0.100 s**. The page scrolls to the lifecycle and the sticky scenario bar hides the top half of "Reset view". See `img/p3-review-candidate-explore-lifecycle-1440.webp`.
5. **D5 — The Replay depth cards overlap the ladder.** Repro: 1440×900, `/book/runs/253bbc23b342f373`. The sticky "Best bid / Best ask" cards sit over the ladder's first visible ask row ("ASK 30000.2 …" is clipped behind them). See `img/p3-review-candidate-replay-1440.webp`.
6. **D6 — Touch targets under 40 px remain on phones (390/360).**
   - Explore "Inspect order N": 25×28.
   - Replay-time slider: 32 px high.
   - Lifecycle "Full forensics for this order": 17 px at 390.
   - Access badge "DEMO" / "RESEARCH": 24 px high (all pages).
   - Run blotter metric/column headers: about 15 px.
   - Risk "Other run": 20 px.
7. **D7 (minor) — Raw field names leak into Compare.** On `/compare/pair?a=f214079c0e4c36a8&b=253bbc23b342f373` the First-divergence text says "Differs in: exch_arrival_ts, first_response_ts, fill_ts, …". Explore already has plain-language wording for the same list.
8. **D8 (minor) — The Lab places a "Default" row-density select at the top right with no visible label**, where it reads like a form field.

Also seen on both builds, so not a regression: the Replay page has no `h1`, and Back restores state but returns to the top of the page.

## Method limits

- The candidate runs on a Vite dev server with a local API, while production is a Vercel build with a hosted API, so load times and transfer sizes are not like for like. That is why the API comparison above uses decoded bytes and call counts.
- The production researcher journey was not run (the assignment keeps job submission on the candidate only).
- One job and one comparison were created on the candidate's `explore-lab` workspace.
