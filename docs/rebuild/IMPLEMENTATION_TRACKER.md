# Rebuild implementation tracker

Operational summary. Read this first, then the linked documents.

- Base: `main` = `48e277b` (2026-11-14); production API `7840bc6` (docs-only behind).
- Branches: `rebuild/explore` (lead, main checkout `D:/black box`); `rebuild/replay-cursor`
  (worktree `D:/bbwt/replay`).
- Local genuine-data stack: Postgres :55432 DB `blackbox_explore`; `blackbox serve` on
  127.0.0.1:8787 (`--data-dir data/explore`, CORS `http://localhost:5173`); Vite dev on :5173.
  Restart `serve` after `demo seed`, or anonymous lists stay stale (D8).
- Models: GPT-6.1 Sol returned `usage_limit_reached` on its first spawn (replay task); that task
  was reassigned to Claude Opus 5.5 with the same acceptance criteria.

| ID | Pri | MS | Owner | Files | Deps | Acceptance | Tests | Status | Commit | Blocker |
|---|---|---|---|---|---|---|---|---|---|---|
| T0 | P0 | M0 | lead | docs/rebuild/PRODUCT_BASELINE.md | — | Live audit, baseline commands, defects | commands listed in doc | done | — | — |
| T1 | P0 | M1 | OssReuseResearch (Opus) | docs/rebuild/REUSE_MATRIX.md, OSS_LICENSES_AND_PROVENANCE.md | — | Pinned SHAs, quoted licenses, decision per donor | — | done (OpenCharts reference-only: provenance unestablished; OpenBB rejected: archived, AG Grid Enterprise/TV Advanced Charts; Perspective/Glide rejected for now; lightweight-charts v5 plugins adopt) | 24b010f | — |
| T2 | P0 | M4 | ReplayCursorOpus (Opus; reassigned from Sol) | v1/runs.rs, server.rs, respond.rs, tests, openapi, client/normalize, ReplayTerminalPage, timelinePaging.ts | — | D1/D2 fixed: cursor-paged, byte-bounded timeline; 400 `window_too_large` instead of 500; terminal loads complete 10 s windows | 6 Rust paging tests + api_v1 HTTP paging; 8 vitest; cargo 428 pass / 1 ignored; vitest 101 | done (local); not deployed | 4a5fccf, merge 41bac84 | — |
| T3 | P0 | M2 | ProtoChartFirst (Opus) | web/src/features/explore/protoA/ | story.ts | Full latency journey, chart-first | browser walk + screenshots | in progress | — | — |
| T4 | P0 | M2 | ProtoWorkspace (Opus) | web/src/features/explore/protoB/ | story.ts | Full latency journey, workspace-first | browser walk + screenshots | in progress | — | — |
| T5 | P0 | M2 | independent reviewer | docs/rebuild/UI_PROTOTYPE_COMPARISON.md | T3, T4 | Weighted scoring with evidence; one winner | — | pending | — | — |
| T6 | P0 | M3 | lead | explore/, App.tsx, shell nav | T5 | Winner becomes `/` public experience; 5-section nav (Explore, Research Studio, Market Lab, Execution Analysis, System); prototypes removed | Playwright anonymous story | pending | — | — |
| T7 | P1 | M3 | lead | e2e/ | T6 | Anonymous guided-demo E2E + 768/390 layout checks | Playwright | pending | — | — |
| T8 | P1 | M6 | — | — | T6 | Perspective vs TanStack pilot measured | benchmark | not started | — | — |
| T9 | P1 | M7 | ItchReconstruction (Opus) | bb-data itch module, EXPERIMENT_VALIDATION.md | T1 | ITCH 5.0 decoder + L3 book + invariants + L2 normalization + manifest; hand-built fixtures; NASDAQ samples local-only | fixtures + proptest | in progress (worktree D:/bbwt/itch) | — | — |
| T10 | P2 | M8 | — | — | T1 | QuantReplay PoC go/no-go with evidence | — | deferred until M3/M5 acceptance (directive §17); research: Apache-2.0, 3 containers, amd64 image 46 MB, no published CPU/RAM figures | — | — |
| T11 | P1 | M9 | EgressCaps (Opus) | v1/edge.rs, respond.rs, runs.rs analytics | T2 | FINAL_AUDIT item 1: exact egress byte accounting, analytics within caller caps or 4xx | byte-budget tests | in progress (worktree D:/bbwt/egress) | — | — |
| T12 | P0 | M10 | lead | — | T2, T6 | Deploy and verify production | smoke | not started | — | needs approval for deploy |
| T13 | — | M0 | lead | — | — | D8 investigated | — | closed: by design (`v1/snapshot.rs:7-12`, MAX_AGE 1 h) | — | — |
| T14 | P2 | M9 | lead | App.tsx, shell/Shell.tsx | — | D7: route-level code splitting | build sizes: entry 1,068 → 400 kB (328 → 129 kB gzip) | done | c338afc | — |
