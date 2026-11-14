# UI prototype comparison: latency experiment (A `/explore/a` vs B `/explore/b`)

Independent review, 2026-11-14. Every result below comes from using the pages in managed Chromium. Each viewport started in a fresh tab. Screenshots are in `img/review-*.webp`. No code was changed.

- **A** is "chart first": a single scrolling page with a large price chart and order book. An inspector panel opens beside them, and the results and assumptions sit below.
- **B** is a "guided workspace": four steps in resizable panels (Guide, Equity, Market replay, Order lifecycle). The answer is in the header.

## Verdict

| Criterion (weight) | A | B | Deciding evidence |
|---|---|---|---|
| Visual quality (20%) | 8 | 6 | A: one large, readable bid/ask step chart (946×402 at 1440). B: replay is 375×213 at 1440 and 288×180 at 1280; bid/ask are drawn as many short `M…H` segments, so they look like broken dashes. B also has the caption/book overlap and the 768 blank band (see must-fix list). |
| First-time usability (25%) | 6.5 | 7.5 | B states the answer under the question and walks the visitor through four steps. A's whole-run answer is below the fold at every width (top 806 px at 1280×800). On mobile, A's first-divergence click gives no visible feedback. B uses nanosecond clocks and quotes raw strategy strings. |
| Complete functional journey (25%) | 6.5 | 7.5 | A's order book does not move to the first divergence: it shows 0.2493 s after the jump to 0.100 s. B's jump is exact (cursor 0.100, book 0.0992) but leaves a blank band at 768. |
| Performance (10%) | 6 | 7 | Load times are effectively equal. API payload per fresh visit: A 880 KB, B 637 KB. |
| Accessibility (10%) | 7 | 6.5 | Both keep a visible focus ring everywhere and have no keyboard traps. A has an unnamed focusable link and no live region. B has a skip link, live region and labelled separators, but loses focus after the jump and has 63–68 text nodes under 14 px. |
| Maintainability / integration (10%) | 7 | 6.5 | A is smaller (2,326 lines vs 2,962) and uses the existing lightweight-charts dependency. B has three bespoke SVG charts and keeps panel layout in localStorage. Neither has tests. |
| **Weighted total** | **6.85** | **6.95** | |

**Winner: B, by 0.10.** The margin is within judgement error.
- B wins on the two biggest criteria: it answers the question immediately, and its first-divergence jump is the only one that moves the whole replay (chart cursor and book) to the right time.
- A has the better replay visual. If visual quality were weighted ≥ 26%, A would win.
- Recommended direction: B's guided, answer-first structure with A's chart grafted into the replay slot (list below).

## Ground truth check (both prototypes vs API `/runs/{id}` and the stored comparison)

| Value | API | A shows | B shows |
|---|---|---|---|
| Orders, 1 ms / 50 ms | 2,060 / 1,759 | 2,060 / 1,759 ✓ | 2,060 / 1,759 ✓ |
| Fills | 1,241 / 745 | ✓ | ✓ |
| Fees | 74.461 / 44.701 | 74.46 / 44.70 ✓ | ✓ |
| Realized P&L | −0.2485 / −0.1368 | "before fees −0.25 / −0.14" ✓ (realized + unrealized) | −0.25 / −0.14 ✓ |
| Unrealized P&L | 0 / 0.0013 | (in breakdown) | 0.00 / 0.00 ✓ (rounded) |
| Net P&L | −74.710 / −44.836 | −74.71 / −44.84, Δ +29.87 ✓ | ✓; "99.62% of that is fees" = 29.760 / 29.873 ✓ |
| Fill ratio | 0.6024 / 0.4235 | 60.24% / 42.35% ✓ | ✓ |
| Round trip, p50 | 2 ms / 100 ms | ✓ | ✓ |
| Cancels sent / rejected | 775 / 4; 1,132 / 406 | — | ✓ |
| Pair classes | matched pairs = 2,060 − 355 = 1,761 − 56 = 1,705 | — | 856 + 308 + 541 = 1,705 ✓ |
| Order 0 | arrival 0.101 / 0.150 s; fill 0.108869266 / 0.165284155 s | ✓ | ✓ (+ "filled 56.4 ms later") |
| Order 256 | 1 ms run only | "not placed / Decided only in the 1 ms scenario" ✓ | "Decided only in the 1 ms scenario" ✓ |

- Neither prototype shows a fabricated number.
- Both state that a smaller loss is not a profit:
  - A: "a smaller loss, not a profit"
  - B: "A smaller loss is still a loss … does not show that slower execution is better in general"
- Both label the data as synthetic and the replay as having no market impact, in the header.

## Journey, click-only (✓ pass · △ passes with friction · ✗ fail)

| Step | A 1440 | A 1280 | A 768 | A 390 | B 1440 | B 1280 | B 768 | B 390 |
|---|---|---|---|---|---|---|---|---|
| 1 Question in one sentence | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 2 Real chart + book visible | ✓ | ✓ | ✓ | ✓ chart at y = 611 | △ small | △ 288×180 | △ replay at y = 2,150 | ✗ replay at y = 3,209 (≈ 3.8 screens down) |
| 3 Choose 1 ms / 50 ms | ✓ `?view=slow` filters chart and table | ✓ | ✓ | ✓ | ✓ `?scenario=slow` | ✓ | ✓ | ✓ |
| 4 Differences in fills, fees, P&L, timing | △ below fold (y ≈ 790) | △ y = 806 | △ y = 1,442 | △ y = 1,627 | ✓ in header + step 1 | ✓ | ✓ | ✓ |
| 5 Click first divergence → replay jumps | △ chart and order OK, book stays at 0.2493 s | △ same | △ same | ✗ same, and the inspector opens off-screen (y = 1,100) | ✓ cursor 0.100, book 0.0992 | ✓ | ✗ jump works, but the bottom 398 px of the viewport goes blank | ✓ scrolls replay into view |
| 6 Lifecycle in both scenarios | ✓ inspector table | ✓ | △ heading at bottom edge (y = 984) | △ off-screen | ✓ timeline + table | ✓ | △ clipped by the blank band | △ y = 890, just below fold |
| 7 Model assumptions | △ raw hash and JSON | △ | △ | △ | ✓ humanized (step 4) | ✓ | ✓ | △ content at y = 1,061 after click |
| 8 Full forensics → return | ✓ URL keeps `t` and `order` | ✓ | ✓ | ✓ | ✓ URL keeps `step`, `order`, `t` | ✓ | △ state kept, scroll lost (lifecycle at y = 2,661) | ✓ |

**Shared (environmental).** Once the anonymous egress budget was exhausted, both pages showed a raw ErrorBox for un-cached windows: `ERROR 503 unavailable … retry after 34,596 s, request id …` (`review-A-1440-egress-503-seek.webp`, `review-B-1440-equity-click.webp`). An anonymous visitor needs a plain-language "demo budget reached, try later" state.

## Checks per prototype

| Check | A | B |
|---|---|---|
| Horizontal overflow (`scrollWidth` vs `clientWidth`) | None at 1440/1280/768/390. Wide tables scroll inside their own wrappers. | None. |
| Text < 14 px (visible text nodes) | 12, all badges (11–13 px). | 63–68: 13 px step hints, radio subtitles, captions, table captions, legend; 12 px SVG axes. |
| Console / page errors | None (dev StrictMode `ERR_ABORTED` excluded). | None. |
| Dead primary actions | None. "Previous order" is correctly disabled at the first order. | None. "Show" stays disabled until an order id is typed. |
| Empty or unexplained charts | Legend tags OBSERVED / SIMULATED; caption explains zoom/click. First x-axis label clipped ("00 s"). | Equity caption explains bucketing. The replay's segmented bid/ask lines are unexplained. |
| Misleading wording | None found. | None found. |
| Raw fields / hashes | Assumptions list `events SHA-256: 0bc264…`, raw JSON strategy params, `"no_partial_fill"`, `"power_prob3"`, `exch_ts + 2000000 ns`, "order 256 (decision only in A)". Not leading, but in the main flow. | Hashes are behind "Technical details". Step 3 quotes the raw reason `"touch_maker: quote bid … round(20 ticks per unit x position 0.000)"` and "strategy tick 0, slot 0". Clock format `+00:00.100 000 000`. Same server interpretation lines ("price tick 299999", "decision only in A"). |

## Keyboard-only

| | A | B |
|---|---|---|
| Visible focus | 2 px outline on every stop. | 2 px outline on every stop. |
| Traps | None; the cycle returns to the start after 30 stops. | None; cycle of 14 stops (radios and steps use a roving tabindex). |
| Journey | Tab to "First divergence" → Enter opens the inspector, focus stays on the button, next Tab reaches it. Slider: arrows step 100 ms. The "Inspect order N" table is the keyboard alternative to chart markers. | ArrowDown / `]` / `[` change step; arrows change scenario; Enter on "Jump…" works. |
| Defects | Unnamed focusable link at stop 10 (TradingView attribution `<a>`, empty name). No `aria-live` region. | Focus falls to `<body>` after "Jump the replay to order 0" (the button unmounts). A polite live region does announce "Order 0 selected…". Skip link and labelled resize separators are present. The equity chart can only be clicked. |

## Performance

**Load time.** Method, identical for both: init script with rAF polling, using `performance.now()` from navigation start.
- "Question visible": the `h1` contains "What changes" and has a box.
- "Chart drawn":
  - A: the largest canvas has ≥ 30 sampled pixels in the `--bid` colour.
  - B: `.pb-bid-line` has a non-trivial `d`.
- Conditions: 1440×900, warm Vite dev server, 3 runs each.

| Median of 3 | A | B |
|---|---|---|
| Navigation → question visible | 440 ms (430 / 446 / 440) | 428 ms (428 / 446 / 428) |
| Navigation → chart drawn | 632 ms (654 / 630 / 632) | 598 ms (588 / 604 / 598) |

**API payload.** Measured with the Performance API on `/api/v1` resources, cache disabled, deduplicated by URL to remove dev StrictMode double fetches; identical in all 3 runs.

| | A | B |
|---|---|---|
| Fresh load, unique API bytes | **880,331 B** (10 URLs) | **636,896 B** (11 URLs) |
| Largest item | `/comparisons/{id}/pairs` 584,795 B: every stored pair, used only for the window order list | `/runs/{id}/equity` ×2 341,360 B: whole-run equity panel |
| Common to both | demos 18 KB, comparisons list 28.5 KB, replay window 115.6 KB, timelines 109.3 KB | same |
| + click first divergence | +17.7 KB | +17.7 KB |
| Raw dev transfer (with StrictMode duplicates), median | 883 KB | 640 KB |
| Visits per 60 MB/day | ≈ 68 | ≈ 94 |

Responses are uncompressed (`encodedBodySize == decodedBodySize`), so server gzip would cut both. A cheaper option for B: equity at 250–500 points.

## Maintainability / integration

| | A (`protoA/`) | B (`protoB/`) |
|---|---|---|
| Size | 1,556 TS/TSX + 770 CSS = 2,326 lines | 1,937 TS/TSX + 1,025 CSS = 2,962 lines |
| Largest file | `ChartFirstPage.tsx` 454 | `StepGuide.tsx` 516, one component per step in one file |
| Reuse | `lightweight-charts` (already used by `runs/TimeSeriesChart.tsx`), `components/fmt`, `ui`, `forensics/eventFormat` | `react-resizable-panels` (already in `SplitPane`), `forensics/axis`, `forensics/labels`, `forensics/useElementSize`, `components/describe` (humanized model info) |
| Duplication | `Ladder.tsx` re-implements a depth ladder (`replay/DepthLadder.tsx` and `terminal/TerminalLadder.tsx` exist). `Assumptions.tsx` dumps raw `controlled` strings instead of using `components/describe`. | Ladder re-implemented inside `ReplayPanel.tsx`. Three bespoke SVG charts (replay, equity, lifecycle). |
| Coupling | Low: page state in the URL (`t`, `order`, `view`); `story.ts` shared. | Higher: imports forensics internals; layout in localStorage plus URL (`step`, `order`, `t`, `scenario`). |
| Test seams | `story.ts` pure helpers, `data.ts` loaders; no tests | `data.ts` `answerSummary` is pure; no tests |

## What B should take from A

1. **`protoA/PriceChart.tsx`** (lightweight-charts) in place of B's `BboChart` SVG in `ReplayPanel.tsx`:
   - continuous best bid/ask step lines and price-scale labels;
   - drag/scroll zoom;
   - per-scenario arrival and fill markers, dimmed except for the selected order;
   - marker click → select order;
   - click elsewhere → pin the book.
2. **A's scenario filter behaviour.** `?view=fast|slow` filters markers, book chips and the order list. B's radio barely changes the replay.
3. **`protoA/WindowOrders.tsx`**, the "Orders in this window" table:
   - decided time, side/price, per-scenario arrival → result, and comparison class;
   - "first divergence" / "first outcome divergence" row tags;
   - "Inspect order N" buttons, the keyboard path to any order.
4. **Prev/next order stepping** ("← Previous order / Next order →") across divergent orders, keeping the window in sync.
5. **The inspector lifecycle table** (`protoA/Inspector.tsx`): decision/arrival/response/fill/cancel/finish rows, each with clock domain and "+Δ since decision", differing rows highlighted, and "Centre the chart on this order".
6. **Plain seconds** (`0.100000 s`, `+8.87 ms`) instead of B's `+00:00.100 000 000` clock.
7. **Results** (`protoA/Results.tsx`): one compact table with a "Difference (50 ms − 1 ms)" row as the default view; B's 28-measure table stays behind "Show all".
8. **A's mobile order:** question → controls → chart within the first screen (A's chart starts at y = 611 at 390; B's at 3,209).
9. **Bytes:** do not copy A's full `/pairs` fetch. Load pairs for the visible window only.

## Must-fix defects in B (repro at the stated viewport, fresh tab)

1. **Blank band after the jump (768×1024).**
   - Repro: open `/explore/b?step=2`, then click "Jump the replay to order 0".
   - Result: `window.scrollY` becomes 398 while `body` is `overflow: hidden` and `.pb-root` is a 1,024 px `overflow: auto` scroller. The whole shell shifts up and the bottom 398 px of the viewport stay empty; scrolling inside the panel does not recover. `elementFromPoint(384, 700…900)` returns `<html>`.
   - Screenshots: `review-B-768-first-divergence.webp`, `review-B-768-jump-blank-band.webp`.
   - Likely cause: `scrollIntoView` on a target inside nested scrollers, with the document taller than the viewport (`scrollHeight` 2,596).
2. **Replay too small and hard to read (1440 / 1280).**
   - Repro: load `/explore/b`. The replay SVG is 375×213 (1440) or 288×180 (1280).
   - The bid/ask paths are hundreds of `M x,y H x'` segments, so they read as dashes (`review-B-1440-landing.webp`). Fixed by item 1 of the take-list plus a larger default replay panel.
3. **Caption overlaps the order book (1440×900).**
   - Repro: load `/explore/b`. The `.pb-replay .pb-caption` rect (y 851–887, x 245–868) intersects `.pb-ladder` (y 577–898, x 632–868); the text runs under the last book rows (`review-B-1440-landing.webp`, bottom).
4. **No chart on the first mobile screens (390×844).**
   - Repro: load `/explore/b`. The scenario cards, steps and full guide come first; the replay starts at y = 3,209 (`review-B-390-top.webp`).
   - Fix: put the replay (with A's chart) right after the answer.
5. **Keyboard focus lost after the jump.**
   - Repro: Tab to "Jump the replay to order 0" on step 2 and press Enter. `document.activeElement` becomes `<body>`.
   - Fix: move focus to the "Order 0 in both scenarios" heading or the step-3 heading.
6. **Back from forensics loses scroll (768×1024).**
   - Repro: jump to order 0, then click "Full forensics for this order (1 ms) →", then Back. `step`, `order` and `t` are restored, but the page is at the top and the lifecycle is at y = 2,661.
   - Fix: restore the `.pb-root` scroll or re-scroll to the selected order.
7. **Raw/technical text in the main flow.**
   - Step 3 quotes the strategy reason verbatim (`"touch_maker: quote bid 29999.9 x 0.010 post-only: … round(20 ticks per unit x position 0.000)"`) and "strategy tick 0, slot 0".
   - Step 4 shows server interpretation strings with "price tick 299999" and "decision only in A".
   - Clocks are shown with nanosecond grouping. Humanize these, or move them behind the technical disclosure.
8. **Body text under 14 px:** `.pb-step-hint`, `.pb-radio-sub`, `.pb-caption`, `.pb-table-cap`, `.pb-kbd-hint` are 13 px; 63–68 text nodes per viewport.
9. **Desktop-only hint on touch:** "Arrow keys move between steps; [ and ] work anywhere." is shown at 390 (`review-B-390-top.webp`).
10. **Raw 503 error for anonymous visitors.** When the egress budget is exhausted, seeking via the equity chart shows `ERROR 503 unavailable … retry after 34,596 s · request id …` (`review-B-1440-equity-click.webp`). Show a plain explanation and keep the last good window.

## A defects (for the record; relevant if A's parts are ported)

- **Book cursor is anchored to the window, not to `t`.**
  - `ChartFirstPage.tsx:362`: `anchor = t0 + BEFORE_NS`. When the window is clamped at 0, the book shows 0.2493 s for the jump to 0.100 s, after both fills of order 0 (`review-A-1440-first-divergence.webp`). It shows the same 0.2470 s for the 0.200 s jump.
  - There is also no time-cursor line on the chart, and the pinned time is not in the URL. The page has two "times" (slider 0.1 s vs book 0.25 / 0.43 s).
- **390 px:** "First divergence" opens the inspector at y = 1,100, with no scroll or other feedback (`review-A-390-first-divergence.webp`).
- **Unnamed focusable link:** the TradingView attribution link has no accessible name (lightweight-charts `attributionLogo`). Keep the attribution but give it a name, or move it to a text credit as the license allows.
- **Other:**
  - Duration label changes with `t` ("0.1 / 120.1 s" vs "54.3 / 120.0 s").
  - The first x-axis label is clipped ("00 s").

## Screenshots

**A:**
- `review-A-1440-landing.webp`
- `review-A-1440-50ms.webp`
- `review-A-1440-first-divergence.webp`
- `review-A-1440-assumptions.webp`
- `review-A-1440-forensics.webp`
- `review-A-1280-top.webp`, `review-A-1280-first-divergence.webp`
- `review-A-768-top.webp`, `review-A-768-first-divergence.webp`
- `review-A-390-top.webp`, `review-A-390-first-divergence.webp`
- `review-A-1440-egress-503-seek.webp`

**B:**
- `review-B-1440-landing.webp`
- `review-B-1440-50ms.webp`
- `review-B-1440-step2.webp`
- `review-B-1440-jump-order0.webp`
- `review-B-1440-assumptions.webp`
- `review-B-1440-top.webp`, `review-B-1440-first-divergence.webp`
- `review-B-1280-top.webp`, `review-B-1280-first-divergence.webp`
- `review-B-768-top.webp`, `review-B-768-first-divergence.webp`, `review-B-768-first-divergence-after-scroll.webp`, `review-B-768-jump-blank-band.webp`
- `review-B-390-top.webp`, `review-B-390-step2.webp`, `review-B-390-first-divergence.webp`, `review-B-390-assumptions.webp`
- `review-B-1440-equity-click.webp`
