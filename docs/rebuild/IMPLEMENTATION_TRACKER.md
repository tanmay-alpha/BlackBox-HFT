# Rebuild implementation tracker

Operational summary. Read this first, then the linked documents.

- Base: `main` = `48e277b` (2026-11-14); production API `7840bc6`, which is behind only in docs.
- Integration branch: `rebuild/explore` (main checkout `D:/black box`). Feature branches are merged
  into it: `rebuild/replay-cursor`, `rebuild/egress`, `rebuild/itch`, `rebuild/quickstart`; still open:
  `rebuild/compare-timeout`, `rebuild/hardening2`.
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
| T7 | P1 | M3 | ExploreUnify, StudioJourney, E2EFix | Playwright: anonymous, explore, research | Full suite green | in progress (first run: 5 failed / 2 passed; being fixed) | — |
| T8 | P1 | M6 | lead | Analytics grid decision | Perspective/Glide not adopted (evidence in REUSE_MATRIX); TanStack kept | done | a2f754e |
| T9 | P1 | M7 | ItchReconstruction | ITCH 5.0 decoder, L3 book, L2 normalization, manifest | 28 fixture tests + proptest; 9.25 M msg/s local | done | 586712f |
| T10 | P2 | M8 | QuantReplayPoC | QUANTREPLAY_EVALUATION.md | Local Docker PoC ran; no-go for hosted/product, reference only | done | ef168cf |
| T11 | P1 | M9 | EgressCaps | FINAL_AUDIT item 1 | 422 instead of 500; analytics fit caps; exact fail-closed counter | done | 249bf66 |
| T12 | P0 | M10 | lead | Release + production verification | smoke, browser | pending | — |
| T13 | — | M0 | lead | D8 investigation | by design (`v1/snapshot.rs:7-12`) | closed | — |
| T14 | P2 | M9 | lead | Route-level code splitting | entry 1,068 → 400 kB | done | c338afc |
| T15 | P1 | M9 | user | Egress budget | keep 60 MB/day; page ≤ 350 KB (measured 237 KB) | done | — |
| T16 | P1 | M9 | TimelineHardening | SECURITY_REVIEW_REBUILD findings 1–5 | tests for each | in progress | — |
| T17 | P1 | M9 | CompareTimeout | FINAL_AUDIT item 2 (comparison timeout) | metadata-first eligibility; bounded path | in progress | — |
| T18 | P1 | M5 | StudioJourney, StudioPolish | Keyed journey; guided/advanced Strategy Lab; plain names | journey walked twice | done | 2a20d5a, 9d823cd |
| T19 | P1 | M9 | ResponsiveShell | D4: 390/768 layouts, worker status wording | scrollWidth == viewport on 12 routes | done | 2842f30 |
| T20 | P1 | M9 | QuickstartFix | FINAL_AUDIT item 8 | legacy console server removed; local-stack scripts | done | 8205050 |
| T21 | P1 | — | ValidationRecompute | EXPERIMENT_VALIDATION.md (latency demo) | All metrics recomputed within 1e-9 | done | f7c70b2 |
| T22 | P2 | — | LatencySensitivity | LATENCY_SENSITIVITY.md | 96 runs; the net-P&L curve tracks fees | done | 06fa93a |
| T23 | P1 | M9 | CiGates | CI gates (AT-14, SEC-19/21/30) | Already present in ci.yml; all pass locally | done (no change) | — |

Known open items:
- Panes built from stacked resizable panels on `/book/runs/…` and the order forensics page are very short at 390 px.
- On wide screens, keyboard Tab order on the Explore page reaches the lifecycle before the book.
- Sweep caveat text uses the raw `net_pnl` name.
