# Phase 3: product experience (2026-11-15)

Release `b8105d4634c7b09b2f85ba39941671c5556f9e61` (merge of `phase3/ux`). Frontend-only; no Rust, schema or
data-contract change.

## Baseline (production `96a3ccb`, before)

Screenshots: `img/p3-before-{explore,lab,replay,risk,order}-{1440,390}.webp`. No overflow or console errors.
Main problems found:
- On Explore at 1440 the chart started at y≈581 and was cut off at the bottom of the window.
- The Strategy Lab showed three dense columns at once.
- The Risk unit column was clipped.
- On phones the status bar took about 90 px, and 65 controls on Explore and 25 on Replay were under 40 px.
- Tab strips inside height-limited panes collapsed to 8 px.
- Prose was set in monospace.
- Negative "queue ahead" was unexplained.

## Work and owners

| Area | Owner (model) | Commit | Result |
|---|---|---|---|
| Design system | DesignSystem (Opus 5.5) | f26a449 | Unified tokens; button, input, tab and table rules; Inter for prose, mono only for numbers; compact phone status bar. See [DESIGN_SYSTEM.md](DESIGN_SYSTEM.md) |
| Explore | ExploreP3 (Opus 5.5) | d40176b | Compact header; aligned scenario table with units and signed Δ; chart above the fold with the book beside it; tooltips; lifecycle "what differed" |
| Strategy Lab | LabP3 (Opus 5.5) | a137993, 8204579 | Four steps (choose, configure, review, run); preset cards with the question each investigates; validation errors jump to the field; job followed in place with next-step links |
| Replay, forensics | ReplayP3 (Opus 5.5) | 96cc558 | Grouped play/pause toolbar; ladder centred on the spread; zoom controls; visible overlays; Lifecycle tab; phone tabs keep time and order |
| Risk, runs, compare, reports | RiskP3 (Opus 5.5) | 9a05172 | Answer-first risk summary and 2×2 chart grid; run identity with copyable hashes; Held constant → Changed → First divergence → Afterwards → Conclusion; readable report |
| Quant semantics | QuantSol (GPT-6.1 Sol) | 732fa41 | Negative modeled queue estimate is a legitimate signed exhaustion state, not a defect. Wording fixes applied across explore, risk, forensics and terminal. See [QUANT_SEMANTICS_REVIEW.md](QUANT_SEMANTICS_REVIEW.md) |
| Independent review | P3ReviewOpus (Opus 5.5; Sol hit its usage limit mid-review) | d07bc3f | Candidate 7.70 vs production 6.35; ACCEPT conditional on D1–D3. See [PHASE3_REVIEW.md](PHASE3_REVIEW.md) |
| Review defects D1–D8, Replay h1, Back scroll | LabFix, PolishFix (Opus 5.5) | 8204579, b0e763b | All fixed and checked with each review repro |

## Tests

- **Local:** `tsc` clean; vitest 114/114; build entry chunk 406 kB (131 kB gzip; was 405/131); Playwright 7/7.
- **Spec fix:** `research.spec.ts` was corrected to the documented cancel contract. A queued job is cancelled at once; a running job may finish first, so its cancel returns 409.
- **CI:** run 43717865021, all 8 jobs green.

## Production (after)

| Check | Result |
|---|---|
| `/status` | `git_sha=b8105d4`, worker idle; `readyz` migrations 0009 |
| Render | image `sha256:c41fd646…864c6b`, deploy `dep-dbsr463l550s7398h2i0` live |
| Vercel | production `blackbox-drlsnr51e…` Ready |
| Smoke | 8/8 PASS |
| Anonymous acceptance | 7/7 PASS |
| Authenticated journey (new Lab flow) | PASS. Job `01a1df60-f94b-782d-9332-6cf1e88ed3d9`, run `2bfae0fdd474731a`; result sha `bb34d8c5…` identical to the earlier releases, so simulation output is unchanged; comparison 201, report 200, anonymous 404, re-auth OK, 0 page errors |
| Overflow and console | no overflow on 5 pages at 1440 and 390; no console errors. Screenshots `img/p3-after-prod-*.webp` |
| API calls per page (unchanged) | Explore 11, Lab 5, Replay 7, Risk 7 (was 5: adds the comparisons lookup), Order 6 |

Rollback: redeploy image `sha256:565fc4041b13789b069e6a793d2200ad5bced103b33e7991f090cded294c2d86` (release
`96a3ccb`) and promote the previous Vercel deployment. No migration was involved.

## Remaining

- **Phone targets:** still under 40 px for the run blotter's Risk/Report links and Explore's "Show … in the replay" buttons.
- **Replay at 1024×768:** the ladder shows about 2 rows and the toolbar wraps to 3 rows.
- **Desktop toolbars:** secondary toolbar actions are reached with arrow keys inside the toolbar, not with Tab.
- **Lab size estimate:** derived in the browser before submission and labelled so; the server's estimate replaces it after submission.
- **Light theme:** not attempted, by design. A polished dark theme came first.
- **Performance vitals:** LCP/INP were not measured on a controlled throttled profile; only navigation and API timings on this workstation.
