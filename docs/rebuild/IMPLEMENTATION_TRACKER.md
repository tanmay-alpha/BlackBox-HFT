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
| T1 | P0 | M1 | OssReuseResearch (Opus) | docs/rebuild/REUSE_MATRIX.md, OSS_LICENSES_AND_PROVENANCE.md | — | Pinned SHAs, quoted licenses, decision per donor | — | in progress | — | — |
| T2 | P0 | M4 | ReplayCursorOpus (Opus; reassigned from Sol) | v1/runs.rs, server.rs, tests, openapi, client/normalize, ReplayTerminalPage | — | D1/D2 fixed: cursor-paged, byte-bounded timeline; no 500; terminal loads complete windows | Rust paging tests (>5k events, equal ts, boundaries); vitest paging | in progress | — | — |
| T3 | P0 | M2 | ProtoChartFirst (Opus) | web/src/features/explore/protoA/ | story.ts | Full latency journey, chart-first | browser walk + screenshots | in progress | — | — |
| T4 | P0 | M2 | ProtoWorkspace (Opus) | web/src/features/explore/protoB/ | story.ts | Full latency journey, workspace-first | browser walk + screenshots | in progress | — | — |
| T5 | P0 | M2 | independent reviewer | docs/rebuild/UI_PROTOTYPE_COMPARISON.md | T3, T4 | Weighted scoring with evidence; one winner | — | pending | — | — |
| T6 | P0 | M3 | lead | explore/, App.tsx, shell nav | T5 | Winner becomes `/` public experience; 5-section nav (Explore, Research Studio, Market Lab, Execution Analysis, System); prototypes removed | Playwright anonymous story | pending | — | — |
| T7 | P1 | M3 | lead | e2e/ | T6 | Anonymous guided-demo E2E + 768/390 layout checks | Playwright | pending | — | — |
| T8 | P1 | M6 | — | — | T6 | Perspective vs TanStack pilot measured | benchmark | not started | — | — |
| T9 | P1 | M7 | — | bb-data | — | ITCH/DBN lawful local validation workflow | fixtures | not started | — | licence check (T1) |
| T10 | P2 | M8 | — | — | T1 | QuantReplay PoC go/no-go with evidence | — | not started | — | — |
| T11 | P1 | M9 | — | bb-app | — | FINAL_AUDIT items 1, 2, 5, 7 | — | not started | — | — |
| T12 | P0 | M10 | lead | — | T2, T6 | Deploy and verify production | smoke | not started | — | needs approval for deploy |
| T13 | — | M0 | lead | — | — | D8 investigated | — | closed: by design (`v1/snapshot.rs:7-12`, MAX_AGE 1 h) | — | — |
| T14 | P2 | M9 | — | web/vite.config.ts, App.tsx | T6 | D7: route-level code splitting | build sizes | not started | — | — |
