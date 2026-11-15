# Rebuild implementation tracker

Operational summary. Read this first, then the linked documents.

- Production (2026-11-15): API `a665991` (image `sha256:212495fa…c659cc1`), Vercel from `5b1ea92`;
  authenticated journey passed (PRODUCTION_ACCEPTANCE.md). Earlier base SHAs are historical.
- Rebuild branches merged: `rebuild/replay-cursor`, `rebuild/egress`, `rebuild/itch`,
  `rebuild/quickstart`, `rebuild/compare-timeout`, `rebuild/hardening2` (via `rebuild/explore`).
  Phase 2 integration branch: `phase2/ui` (release pending deploy).
- Local genuine-data stack: Postgres :55432, DB `blackbox_explore`; `blackbox serve` on
  127.0.0.1:8787 (`--data-dir data/explore`, CORS `http://localhost:5173`); Vite dev on :5173.
  Restart `serve` after `demo seed`, otherwise anonymous lists stay stale for up to 1 h (D8, by design).
- Models: GPT-6.1 Sol returned `usage_limit_reached` on both spawns (replay task, prototype review).
  Both tasks were reassigned to Claude Opus 5.5 with unchanged acceptance criteria. All other
  agents ran on Opus 5.5.
- User decisions (2026-11-14): release to production after checks pass; keep the 60 MB/day
  anonymous egress budget and shrink the page payload instead.

| ID | Pri | MS | Owner | Scope | Acceptance | Status | Commit |
|---|---|---|---|---|---|---|---|
| T0 | P0 | M0 | lead | PRODUCT_BASELINE.md | Live audit, baseline commands, defects | done | 24b010f |
| T1 | P0 | M1 | OssReuseResearch | REUSE_MATRIX, OSS_LICENSES_AND_PROVENANCE | Pinned SHAs, quoted licences, decision per donor | done | 24b010f |
| T2 | P0 | M4 | ReplayCursorOpus | Cursor-paged, byte-bounded timeline; terminal loads complete 10 s windows | No 500; pages equal the unpaged window | done | 4a5fccf / 41bac84 |
| T3/T4 | P0 | M2 | ProtoChartFirst, ProtoWorkspace | Two functional prototypes on real data | Full journey | done | 6e1c781 |
| T5 | P0 | M2 | PrototypeReviewOpus | UI_PROTOTYPE_COMPARISON.md | Weighted scoring; B 6.95 vs A 6.85 | done | — |
| T6 | P0 | M3 | ExploreUnify | Public latency page at `/` inside the shell; catalogue at `/experiments` | Must-fix defects closed; 236,675 B API data per fresh visit | done | b819427 |
| T7 | P1 | M3 | ExploreUnify, StudioJourney, E2EFix | Playwright: anonymous, explore, research | Full suite green | done: 7/7 after E2EFix (4 test bugs, 1 app bug: scroll restore) | 60a6ff5 |
| T8 | P1 | M6 | lead | Analytics grid decision | Perspective/Glide not adopted (evidence in REUSE_MATRIX); TanStack kept | done | a2f754e |
| T9 | P1 | M7 | ItchReconstruction | ITCH 5.0 decoder, L3 book, L2 normalization, manifest | 28 fixture tests + proptest; 9.25 M msg/s local | done | 586712f |
| T10 | P2 | M8 | QuantReplayPoC | QUANTREPLAY_EVALUATION.md | Local Docker PoC ran; no-go for hosted/product, reference only | done | ef168cf |
| T11 | P1 | M9 | EgressCaps | FINAL_AUDIT item 1 | 422 instead of 500; analytics fit caps; exact fail-closed counter | done | 249bf66 |
| T12 | P0 | M10 | lead | Release + production verification | smoke, browser | done: a665991 live, journey passed (job 01a1dea1) | a665991 |
| T13 | — | M0 | lead | D8 investigation | by design (`v1/snapshot.rs:7-12`) | closed | — |
| T14 | P2 | M9 | lead | Route-level code splitting | entry 1,068 → 400 kB | done | c338afc |
| T15 | P1 | M9 | user | Egress budget | keep 60 MB/day; page ≤ 350 KB (measured 237 KB) | done | — |
| T16 | P1 | M9 | TimelineHardening | SECURITY_REVIEW_REBUILD findings 1–5 | tests for each | done: HMAC cursors bound to run+window, min page 100, budget-spent refuses > 2 KiB, OpenAPI-normalized cache key with LRU | 7e2889b |
| T17 | P1 | M9 | CompareTimeout | FINAL_AUDIT item 2 (comparison timeout) | trailer-only eligibility; single-pass decode | done: eligible 4.22 s → 0.96 s cold (local) | 50883ab |
| T18 | P1 | M5 | StudioJourney, StudioPolish | Keyed journey; guided/advanced Strategy Lab; plain names | journey walked twice | done | 2a20d5a, 9d823cd |
| T19 | P1 | M9 | ResponsiveShell | D4: 390/768 layouts, worker status wording | scrollWidth == viewport on 12 routes | done | 2842f30 |
| T20 | P1 | M9 | QuickstartFix | FINAL_AUDIT item 8 | legacy console server removed; local-stack scripts | done | 8205050 |
| T21 | P1 | — | ValidationRecompute | EXPERIMENT_VALIDATION.md (latency demo) | All metrics recomputed within 1e-9 | done | f7c70b2 |
| T22 | P2 | — | LatencySensitivity | LATENCY_SENSITIVITY.md | 96 runs; the net-P&L curve tracks fees | done | 06fa93a |
| T23 | P1 | M9 | CiGates | CI gates (AT-14, SEC-19/21/30) | Already present in ci.yml; all pass locally | done (no change) | — |
| P2-M0 | P0 | M0 | lead | Phase 2 baseline: production verification | API a665991, migrations 0009, worker idle | done | a665991 / 5b1ea92 |
| P2-M1 | P1 | M1 | lead | Authenticated production journey | job → run served | done: run e2321a7eec3e23e1, ~7.2 s | — |
| P2-M6 | P1 | M6 | ItchValidate (GPT-6.1 Sol) | ITCH independent comparison (EXPERIMENT_VALIDATION.md) | 0 diffs vs itch-book | done: 9,766,139 msgs, 0 differences | efd3694 |
| P2-M5 | P1 | M5 | SecDisposition (GPT-6.1 Sol) | SECURITY_DISPOSITIONS.md | every finding dispositioned | done; remaining Open items listed there | ed64dd4 |
| P2-M4 | P0 | M4 | ReplayVerify (GPT-6.1 Sol) | REPLAY_VERIFICATION.md | byte-budgeted encoding; long run pages | done: 12,000 ticks, 60,382 events, 19 pages, p50/p95 55/421 ms, peak WS 86 MiB local | 2e78f11 |
| P2-M2 | P1 | M2 | UIAudit (Opus 5.5) | UI_AUDIT.md | ranked defects at 5 viewports | done: 21 defects ranked | e8344ba |
| P2-M3 | P1 | M3 | UIAudit, UIRemaining (Opus 5.5) | UI_AUDIT defect fixes | high-impact defects closed | done: all High/Medium rows Fixed | e8344ba, b31b0b2 |
| P2-M7 | P2 | M7 | — | Research extensions (latency/queue/scenario depth) | — | not started (directive: only after P0 gates) | — |
| P2-M9a | P1 | M9 | DocsCurrent (Opus 5.5) | Current-state docs | stale SHAs/claims replaced | done | — |
| P2-M8 | P0 | M8 | lead | Merge phase 2 to main + deploy | `/status` reports new SHA | done | 96a3ccb |
| P2-M9 | P0 | M9 | lead | Post-deploy production verification | smoke + journey | done: smoke 8/8, prod acceptance 7/7, keyed journey PASS | 96a3ccb |

Known open items:
- Panes built from stacked resizable panels on `/book/runs/…` and the order forensics page are very short at 390 px.
- On wide screens, keyboard Tab order on the Explore page reaches the lifecycle before the book.
- Sweep caveat text uses the raw `net_pnl` name.
