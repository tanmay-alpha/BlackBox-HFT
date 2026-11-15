# Rebuild final review

Date: 2026-11-14. Scope: the product rebuild from `main` = `48e277b` to the integration branch
`rebuild/explore`. Every statement cites a document in `docs/rebuild/` or a commit in
`git log 48e277b..rebuild/explore`. Sections H and the production rows are filled by the lead after
the release (see **Lead section** at the end).

## A. Repository state

| Item | State | Source |
|---|---|---|
| Base | `main` = `48e277b`; production API then reported `7840bc6` (behind only in docs) | PRODUCT_BASELINE §1 |
| Integration branch | `rebuild/explore`, HEAD `b71a46a` at the time of writing | `git log` |
| Merged feature branches | `rebuild/replay-cursor` (41bac84), `rebuild/egress` (249bf66), `rebuild/itch` (586712f), `rebuild/quickstart` (8205050), `rebuild/compare-timeout` (9146765), `rebuild/hardening2` (b71a46a) | `git log` |
| New infrastructure | None: no service, database or paid resource added | TARGET_PRODUCT_ARCHITECTURE intro |
| Release to production | Pending (T12) | IMPLEMENTATION_TRACKER |

## B. Work completed

| ID | Work | Commit |
|---|---|---|
| T0, T1 | Product baseline (live audit, 9 defects D1–D9); reuse matrix and OSS provenance | 24b010f |
| T2 | Cursor-paged, byte-bounded run timeline; terminal loads complete 10 s windows (fixes D1, D2) | 4a5fccf, 41bac84 |
| T3–T5 | Two functional prototypes of the latency page; independent review, B 6.95 vs A 6.85 | 6e1c781, 025f25d |
| T6 | Public latency experiment at `/` (B's guided structure with A's chart, filter, window orders, stepping, lifecycle); catalogue moved to `/experiments`; prototypes removed | b819427 |
| — | Five labelled navigation sections; rail open, inspector closed by default (D3, D5) | af7bf93 |
| T7 | Playwright specs: anonymous, explore, research; 7/7 after E2EFix (4 test bugs, 1 app bug: scroll restore) | 60a6ff5 |
| T8 | Analytics grid decision: Perspective and Glide not adopted, TanStack kept | a2f754e |
| T9 | ITCH 5.0 decoder, L3 reconstruction with invariants, L2 normalization, `blackbox itch-reconstruct` (local-only data) | ec6b5ee, 74d2767, 1221009, 586712f |
| T10 | QuantReplay local Docker PoC; no-go for product and hosting | ef168cf |
| T11 | Egress: response caps return 422, analytics fit the caller's cap, exact fail-closed egress counter (FINAL_AUDIT item 1) | 6f84791, 249bf66 |
| T14 | Route-level code splitting (D7) | c338afc |
| T16 | Security findings 1–5 fixed: HMAC cursors bound to run and window, minimum page 100, spent budget refuses > 2 KiB, normalized anonymous cache key with LRU eviction | 7e2889b, b71a46a |
| T17 | Comparison eligibility from header + trailer ranged reads; single-pass bundle decode (FINAL_AUDIT item 2) | 50883ab, 9146765 |
| T18 | Keyed research journey; Guided/Advanced Strategy Lab; paste-a-run comparison picker; plain model names; remount on sign-in/out | 2a20d5a, 9d823cd |
| T19 | Responsive shell (D4): menu and inspector sheet below 960 px; plain worker status | 2842f30 |
| T20 | Legacy `blackbox demo` console server removed; one-command `scripts/local-stack.{sh,ps1}` (FINAL_AUDIT item 8) | 3160a2a, 8205050 |
| T21 | Independent recomputation of the latency demo | f7c70b2 |
| T22 | Latency × queue-model sensitivity, 96 runs on 4 synthetic datasets | 06fa93a |
| T23 | CI gates (AT-14, SEC-19/21/30) already present in `ci.yml`; no change | IMPLEMENTATION_TRACKER |

## C. OSS reuse

Decisions per donor are in REUSE_MATRIX; licences and provenance in OSS_LICENSES_AND_PROVENANCE.

| Donor | Decision | Reason |
|---|---|---|
| TradingView Lightweight Charts (Apache-2.0) | Kept (already a dependency); v5 plugin packages are the top reuse candidate | REUSE_MATRIX §1, §5 |
| TanStack Table + Virtual | Kept as the only table stack | PERFORMANCE_RESULTS "Analytics grid decision" |
| Perspective, Glide Data Grid | Not adopted: multi-MB WASM and a second table stack; Glide 6.0.3 peer-depends on React ≤ 18, 106 KB gzip | PERFORMANCE_RESULTS, REUSE_MATRIX |
| QuantReplay (Apache-2.0) | Reference only; local PoC ran, no-go for product/hosting (random wall-clock-paced flow, no fees, needs Docker + Postgres + Liquibase + FIX, does not fit 512 MB) | QUANTREPLAY_EVALUATION §6 |
| OpenCharts, OpenTerminalUI, ordercraft terminal | Reference only (unclear relicensing right; small patterns cheaper to reimplement) | REUSE_MATRIX §1 |
| OpenBB workspace, TT_Samples, quickfix, qstrader | Rejected | REUSE_MATRIX §1–2 |
| NautilusTrader (LGPL-3.0), ABIDES, Lean, magic-trace | Reference only | REUSE_MATRIX §2 |
| itch-rs, itch-book | Reference only; BlackBox's ITCH decoder is its own module of `bb-data` with no new dependency | REUSE_MATRIX §3, EXPERIMENT_VALIDATION "ITCH reconstruction" |
| databento/dbn | Adapt later (local bring-your-own-file importer); nothing copied now | REUSE_MATRIX §3 |
| Data: NASDAQ ITCH samples, Binance Vision, Tardis.dev, cryptohftdata | Local only; no hosting grant | REUSE_MATRIX §3 |

## D. Visual results

Screenshots are in `docs/rebuild/img/`.

| Journey point | Before (production, `48e277b`) | After (rebuild) |
|---|---|---|
| First screen, 1440 | `before-home-1440.webp` (dataset selector, config paths, raw JSON) | `final-1440-landing.webp` (question and answer first) |
| First screen, 1280 | `before-home-1280.webp` | `final-1280-landing.webp` |
| First screen, 768 | `before-home-768.webp` (inspector takes half the width) | `final-768-landing.webp` |
| First screen, 390 | `before-home-390.webp` (scrollWidth 1405 px) | `final-390-landing.webp` |
| Latency result | `before-latency-set-1440.webp` (30-row metric table, no headline) | `final-1440-result-equity.webp`, `final-1280-50ms-orders.webp` |
| Replay | `before-terminal-anon-500-1440.webp` (500 for anonymous visitors) | `after-terminal-local-1440.webp`, `final-1440-first-divergence.webp`, `final-768-first-divergence.webp`, `final-390-first-divergence.webp` |
| Order forensics | `before-forensics-order-1440.webp` | `responsive-forensics-run-orders-0-{1440,768,390}.webp` |
| Assumptions | raw hash and JSON (UI_PROTOTYPE_COMPARISON, journey step 7) | `final-1280-assumptions.webp`, `final-390-guide-step3.webp` |
| Research journey | — | `rj-lab-guided.webp`, `rj-compare.webp`, `rj-sweep.webp`, `rj-signed-out.webp` |
| All 12 workspaces at 1440/768/390 | — | `responsive-<route>-<width>.webp` (RESPONSIVE_RESULTS) |

Prototype review screenshots: `review-A-*`, `review-B-*`, `protoA-*`, `protoB-*` (UI_PROTOTYPE_COMPARISON).

## E. Functional journeys

| Journey | Result | Source |
|---|---|---|
| Anonymous first visit (10 tasks) | Baseline: task 4 (watch market activity) blocked by D1 (500); headline result never stated (D3, D6) | PRODUCT_BASELINE §3 |
| Prototype journeys (8 steps × 4 widths × 2 prototypes) | B passed the first-divergence jump exactly (cursor 0.100 s, book 0.0992 s); A's book did not move; B's must-fix defects listed and closed in T6 | UI_PROTOTYPE_COMPARISON, IMPLEMENTATION_TRACKER T6 |
| Keyed research journey (12 steps) | Walked twice; fixed: config diffs in form units, plain names, Guided/Advanced mode, comparison picker accepts any run id (step 8 had been blocked), sign-out no longer shows a private run (step 12 defect) | RESEARCH_JOURNEY |
| Playwright (anonymous, explore, research) | 7/7 after E2EFix; one app bug fixed (scroll position on return) | IMPLEMENTATION_TRACKER T7, 60a6ff5 |
| Responsive, 12 routes | `scrollWidth` equals the viewport on all 24 narrow measurements (768, 390) | RESPONSIVE_RESULTS |

## F. Scientific correctness

| Check | Result | Source |
|---|---|---|
| Latency demo metrics (A `253bbc23b342f373` 1 ms, B `d6a57aed972e18d0` 50 ms) recomputed from raw orders, fills, equity and tape | All match; max residual 1.0e-9 (turnover, summation order); counts, quantiles and net P&L exact | EXPERIMENT_VALIDATION |
| P&L identity on all 1,201 equity samples per run | `equity − (realized + unrealized − fees)` max abs 0 | EXPERIMENT_VALIDATION |
| Comparison | First divergence order 0 (timing only), first outcome divergence order 256 (A only), Δ fills −496, Δ orders −301: recomputed = stored | EXPERIMENT_VALIDATION |
| Replay completeness | 4 windows, Σ page events = `events_total` = raw tape count; seq strictly increasing; 0 duplicates | EXPERIMENT_VALIDATION |
| Determinism | Same platform class: identical `result_sha256`; across libm classes the hash differs (documented platform policy) | EXPERIMENT_VALIDATION |
| What the page claims | The 50 ms run's smaller loss is fees: 99.62% of the Δ net; "a smaller loss is still a loss" | UI_PROTOTYPE_COMPARISON ground truth, PRODUCT_BASELINE D6 |
| Latency sensitivity, 96 runs | Net is ≥ 99.3% fees in every run; fills fall monotonically with latency in all 12 series; no claim that slower is better is supported; queue model moves net by ≤ 0.843 per cell | LATENCY_SENSITIVITY "Conclusions" |
| ITCH reconstruction | 28 fixture tests + a 256-case property test (L2 equals live L3 after every message); local NASDAQ prefix run with zero quarantines. Not an exchange certification; no independent L3 comparison yet | EXPERIMENT_VALIDATION "ITCH reconstruction" |

Not validated: realism (no calibration of latency, queue position or fill probability), market
impact, the engine's own matching (EXPERIMENT_VALIDATION "What this does NOT validate").

## G. Performance

| Measurement | Before | After | Where | Source |
|---|---|---|---|---|
| Entry JS | 1,068.43 kB (327.90 kB gzip) | 399.78 kB (129.29 kB gzip) | local build | PERFORMANCE_RESULTS |
| Anonymous replay timeline | 500 `response_too_large` | cursor-paged, first page ≤ 1 MiB | production / local | PERFORMANCE_RESULTS |
| Anonymous run analytics | 2,760,222 B | 462,842 B (series halve to fit, else 422) | older production / local | PERFORMANCE_RESULTS |
| Keyed analytics, 128 KiB cap | 500 `internal` | 200, 96–97 kB | local | PERFORMANCE_RESULTS |
| Public page API bytes per fresh visit | A ~880 KB, B ~637 KB | 236,675 B (≈ 253 fresh visits/day within the 60,000,000 B/day budget) | local | IMPLEMENTATION_TRACKER T6, T15 |
| Comparison eligibility, cold | 4.22 s | 0.96 s | local | IMPLEMENTATION_TRACKER T17 |
| ITCH reconstruction | — | 9.68 M msg/s (L3, all 8,884 stocks), 9.25 M msg/s with L2 rows for 3 symbols | local, one thread | PERFORMANCE_RESULTS |

Production performance after the release: see the Lead section.

## H. Infrastructure

See the Lead section. The rebuild adds no service, database or paid resource
(TARGET_PRODUCT_ARCHITECTURE).

## I. Testing

| Suite | Result | Source |
|---|---|---|
| Baseline at `48e277b` | `cargo fmt --check`, clippy `-D warnings` pass; `cargo test` 422 passed, 1 ignored; vitest 93 tests | PRODUCT_BASELINE §2 |
| Web unit tests after T18 | `tsc --noEmit` pass; vitest 10 files, 110 tests pass | RESEARCH_JOURNEY |
| Playwright | 7/7 | IMPLEMENTATION_TRACKER T7 |
| ITCH | 28 fixture tests + property test | EXPERIMENT_VALIDATION |
| Security fixes T16, comparisons T17 | New tests in `crates/bb-app/tests/{integration,demo_workflow,comparisons}.rs` | 7e2889b, 50883ab |
| CI gates | Already present in `ci.yml`; all pass locally | IMPLEMENTATION_TRACKER T23 |
| Full suite on integrated HEAD | See the Lead section | — |

## J. Outstanding issues

- Stacked resizable panes on `/book/runs/…` and the order forensics page are very short at 390 px
  (each pane scrolls internally) (IMPLEMENTATION_TRACKER, RESPONSIVE_RESULTS).
- On wide screens, keyboard Tab order on the Explore page reaches the lifecycle before the book.
- Sweep caveat text from the server uses the raw `net_pnl` name.
- Cross-platform result hashes differ across libm classes (documented policy, not a defect).
- ITCH: no independent L3 comparison; only a prefix of one day run; ITCH L2 is not yet a simulator
  dataset (needs a recorded feed-latency assumption).
- Not reviewed for security: ITCH CLI path handling, `itch/l2.rs`, explore components in detail
  (SECURITY_REVIEW_REBUILD).
- Still open from PROJECT_STATE: SEC-9/11/12/17/18/25 partially tested; AT-5 hosted-worker peak
  memory; `quota.exceeded` audit rows; anonymous detail reads and `/sweeps` hit Postgres on a cache miss.

## K. Next priority

1. T12: release `rebuild/explore` to production and verify it (smoke, browser journey, `git_sha`,
   `demo seed`), then fill the Lead section.
2. Fix the three open UI items in J.
3. ITCH: compare against an independent L3 reconstruction before any claim beyond consistency.

---

## Lead section: release and production (2026-11-15)

Details and rollback: [PRODUCTION_ACCEPTANCE.md](PRODUCTION_ACCEPTANCE.md).

### H. Infrastructure

| Item | Result |
|---|---|
| Released commit / image digest | `d85f743` / `ghcr.io/tanmay-alpha/blackbox-hft@sha256:1d35116c…b237` (publish run 43692117407, attested) |
| Render service deploy | `dep-dbsmfrjl550s738ssrmg` live on the existing free service. Removed a stale manual `BB_GIT_SHA` env override |
| Vercel production deployment | `blackbox-do96bv953…` Ready (built from `main` 07:48 UTC) |
| Migrations / `readyz` | `migrations 0009`, db ok, blob configured (no new migration in this release) |
| `demo seed` on production | not needed: the simulation crates are unchanged and the 5 curated comparisons are served |
| Neon / Blob quota usage | not measured (no dashboard read) |

### Production results

| Check | Result |
|---|---|
| `/api/v1/status` `git_sha` | `d85f743719ae9d46d29c7c9e0df58f476e8bff42`; worker idle, heartbeat 5.6 s / 15 s |
| `scripts/deploy/smoke.sh` | 8/8 PASS |
| Anonymous journey at `/` (1440, 768, 390) | `npm run test:prod`, 7/7 PASS: numbers equal the API, first-divergence jump, forensics and back, no overflow at 768/390 |
| Anonymous replay window loads completely (no 500) | PASS (`/book/runs/253bbc23b342f373`) |
| Keyed submit → complete → compare | verified locally and in CI Playwright `research.spec.ts`; not exercised against production (budget) |
| Public page API bytes per fresh visit | 236,675 B measured locally with the same API data; not remeasured on production |
| Full test suite on the released commit | `cargo test --workspace` 458 passed / 0 failed; vitest 111/111; CI e2e Playwright green (run 43691546229) |

## Phase 2 (2026-11-15)

Earlier sections are the historical phase-1 record; test counts and SHAs there are not current.

- Production verified 2026-11-15: API `git_sha=a665991` from image `sha256:212495fa…c659cc1`
  (Render deploy `dep-dbsmm76k1f9s739ng650` live); Vercel built from `5b1ea92` (docs-only on top
  of `a665991`); migrations through 0009; worker idle; no `BB_GIT_SHA` override.
- Authenticated production journey PASSED (keyed submit exercised on production): job `01a1dea1`,
  run `e2321a7eec3e23e1`, ~7.2 s on the worker — PRODUCTION_ACCEPTANCE.md, `prod-research-evidence.json`.
- ITCH independent comparison vs `itch-book`: 0 differences over 9,766,139 messages
  (EXPERIMENT_VALIDATION.md, `efd3694`).
- Security dispositions: SECURITY_DISPOSITIONS.md (`ed64dd4`); remaining Open items are listed there.
- Replay verification: REPLAY_VERIFICATION.md (`2e78f11`), byte-budgeted encoding fix; 12,000-tick
  run, 60,382 events, 19 pages, p50/p95 55/421 ms, peak WS 86 MiB (local).
- UI audit: UI_AUDIT.md, 21 ranked defects (`e8344ba`, `b31b0b2`).
- Release of phase 2: `96a3ccb` live 2026-11-15 10:47 UTC (image `sha256:565fc404…`). Smoke 8/8, anonymous
  acceptance 7/7, authenticated journey PASS (run `b0f713f6a8cb46af`, same result hash as run
  `e2321a7eec3e23e1` under `a665991`). Workspace tests 472 passed / 0 failed / 3 ignored; vitest 114; local
  Playwright 7/7; CI run 43704539393 green.
