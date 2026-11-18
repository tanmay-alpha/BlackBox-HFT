# Phase 6 and Phase 7 tracker

Single current-state tracker for Phase 6 (reliability, security, research integration) and Phase 7 (scientific
validation, unified research studio). Statuses: Open, Investigating, Confirmed, Fixed locally, Tested, Deployed,
Verified, Accepted risk, Blocked. "Verified" means checked on the deployed production version.

Baseline (2026-11-17/18): `main` = `af3a284`, production `cdde05e`.

**Released 2026-11-19:** `main` = `a1d128c` (PR #3), image `sha256:ef148a3b…cefa0`, Render deploy
`dep-dbuoh75ckfvc73cqrra0`, migrations 0010–0011 applied ([../rebuild/PHASE67_RELEASE.md](../rebuild/PHASE67_RELEASE.md)).
Every row marked Tested below that is in that release is **Deployed**; rows checked by the production browser suite
(compare set A1/A2, workbench D1–D4, studies E1–E4, G1–G3) and by direct production page checks (`/research`,
`/queue-lab`) are **Verified**. Not in the release: `p7/amem` (analytics memory) and `p7/cancel` (post-simulation
cancellation, `af7543e`).

Model availability: GPT-6.1 Sol returned `usage_limit_reached` on 2026-11-17 and early 2026-11-18 and was used from
2026-11-18 15:20 IST for the two independent reviews (backend, science), the enum-CHECK audit, the backend review
fixes and the queue experiment fixes; it hit its limit again before the analytics-memory and cancellation tasks,
which ran on Claude Opus 5.5. Everything else ran on Claude Opus 5.5.

## Phase 6 entry gate (Phase 7 §2)

| Id | Sev | Requirement | Status | Evidence / commit | Remaining |
|---|---|---|---|---|---|
| A1 | P0 | Cold compare-set `503 busy` | Tested | Root cause: 7 parallel `GET /runs/{id}` whole-bundle verifications for an overview, plus a summary-digest fallback reading whole bundles outside the heavy-read permit. Cold page 16 req / 1,672 ms CPU → 9 req / 156 ms, no store reads (`22770ac`, `cfda25c`); bounded busy retry; single-flight cold fetch (`42372ba`). `compare_set` test; `PHASE6_RELIABILITY.md` | deploy, prod check |
| A2 | P0 | False "no stored comparison" on curated demo | Tested | Cause: stale anonymous snapshot (≤1 h) + UI treating "not in list" as missing. Nine explicit pair states, no locked create for anonymous (`cfda25c`); Rust + Playwright `compare-set.spec.ts` | deploy, prod check |
| A3 | P0 | Snapshot freshness for externally seeded rows | Tested | `demo_data_version` (`f1a203e`) replaced after the Sol review: per-table sorted (key, xmin) digest over every table the snapshot reads, incl. quality reports and workspace visibility (`9d507c1`); 0.89 ms warm on a seeded demo DB; equal-sum and quality-report regressions; ≤ 15 min recheck | deploy |
| B1 | P0 | SEC-18 secret-safe logs | Tested | `630ab6f`; `secret_logging` tests | deploy |
| B2 | P0 | SEC-17 worker fault matrix | Tested | `630ab6f`; `worker_faults` | deploy |
| B3 | P0 | SEC-25 audit coverage | Tested | `9004462`; `audit_matrix` | deploy |
| B4 | P0 | SEC-11 §6.3 metered store, anonymous half share | Tested | `9004462` + migration 0010; `artifact_metering` | deploy (migration runs on start) |
| B5 | P0 | AT-5/SEC-6 Linux full-process peak < 300 MB | Open (2.6 MiB residual, 1 of 3 repeats, phase 4) | Corrected read set (1,289 URIs; earlier read-phase numbers under-covered). Final policy (`p9/baseline` `c4773bb`), 3 pinned repeats, min/median/max MiB vs unmodified `36536d0`: readers 156.9/165.6/166.3 (224.6), largest job 186.6/198.9/202.7 (339.7), job + reads + comparison 273.6/277.7/288.7 (397.3), cancellation 227.1/229.8/230.0 (357.5), idle 123.7/124.2/126.9 (200.5); no OOM; mmap threshold ≈ 15–20% more CPU in phase 4 | page keyed whole-run reads or worker out of process |
| B8 | P0 | GLFT preset submission returns 500 (new) | Tested | Migration `0011_glft_strategy_kind.sql` widens `experiment_configs_strategy_kind_check` with `glft` (`c0a2cf4`); `strategy_kinds` test submits every `StrategyKind` preset through the API to completion. Sol audit of every enum CHECK in 0001–0011 against the writers: GLFT the only mismatch. 539 Rust pass | deploy (migration runs on start) |
| B6 | P0 | Production credential isolation | Tested | `630ab6f`, `99a0351`; local profile refuses cloud credentials; `scripts/ops/prod-run.sh`; `docs/OPERATOR_PRODUCTION.md`; DEPLOYMENT §5/§8 updated on `p6/int` | deploy |
| B7 | P1 | SEC-10 anonymous detail reads wake Postgres | Tested | Snapshot trust rows (`00fcae4`, `23f896b`, deployed in `36536d0`) + anonymous analytics/series from the authenticated bundle trailer (`p9/anamem` `c6b407e`): 48 detail/paged + 24 analytics/series anonymous reads, 0 DB acquisitions; byte-identical to keyed responses; legacy bundles fall back to the DB | release |
| C1 | P1 | Blank route fallback | Tested | `b30c936`: boot line in `index.html`, `WorkspaceLoading` frame; empty-page time 1.6–2.6 s → 0 ms in 30 loads; `workspace-loading.spec.ts` | deploy |
| C2 | P1 | Mobile LCP | Tested | Same-session A/B (mobile, 6 reps): Explore 5,564 → 4,276 ms, Replay 11,462 → 5,232 ms; FCP ~3,250 → ~810 ms; CLS not worse; first table page after Explore +0.45 s (`PHASE6_PERFORMANCE.md`) | still > 2.5 s locally (entry chunk) |
| C3 | P1 | Static public demo fast path | Accepted risk (not shipped) | Evaluated: full Explore set ~64 KB gzip; not shipped (second data path, export step, benefit inferred only) | revisit with prod cold-start data |
| D1–D4 | P1 | Workbench L3/L2 before/after, curated events, exact semantics | Tested | `f1d38af`, `ecfdc24`; bb-data causal tests; 499 Rust pass; e2e 6/6 on agent stack | standard e2e, deploy |
| E1–E4 | P1 | Study viewer on verified artefact | Tested | `fab88b8`; 203 vitest, e2e 3/3 | nav entry (ResearchHome), cell explorer (StudyExplorer), regenerate artefact at release commit |
| F1 | P1 | Report wording (synthetic vs historical, raw enums, ns) | Tested | `a2314be`: evidence-structured run/comparison/sweep reports, `generator_version` 2, stored reports unchanged; 517 Rust pass | deploy |
| F2–F4 | P1 | Terminology, visual defects, continuity | Investigating | CopyPolish (`p7/copy`); research home + study explorer give continuity via URL helpers | |
| G1–G3 | P1 | Production acceptance for Workbench, studies, security | Fixed locally | `web/e2e-prod/{market,studies,compare-set,errors}.prod.spec.ts`; 15 pass / 1 skip on a local server; ~15 prod loads per run | run against production after deploy |
| H1 | P2 | Real-data rights and independent check | Tested | `REAL_DATA_VALIDATION_PLAN.md`: no licence text; local private use only; QQQ/AAPL 100% 3-way agreement on a 300 MB prefix | pipeline (Phase 7 C) |

## Phase 7 workstreams

| Id | Item | Owner / branch | Status |
|---|---|---|---|
| P7-A | Unified research identity, cross-workspace links, research landing | ResearchHome (`p7/research`, merged) | Tested: `/research` five live investigations, `RESEARCH_IDENTITY.md`, nav; 231 vitest, 70 e2e |
| P7-B | Study explorer (cell selection, controlled differences, evidence) | StudyExplorer (`p7/explorer`) | Tested: 272 vitest, 70 e2e; merged into `p7/queuelab` |
| P7-C | Reproducible ITCH validation pipeline, quality report, metrics | ItchPipeline (`p7/data`), SolBackendFix | Tested: QQQ/AAPL 100% three-way reproduced; negative control 1 mismatch each; review fixes merged (coverage from window start, numeric window grammar) |
| P7-D | Queue-model shadow experiment + lab page | QueueLab2, QueueLabUI, SolQueueFix (`p7/queuelab`, merged) | Tested: inclusive deadline/checkpoint semantics (`ccdd493`); full rerun changed 0 of 810 fill cells, 0 of 972 checkpoints, 0 of 81 cancellation cells; 2019-12-30 QQQ/AAPL relabelled previously inspected, 2019-10-30 the only clean OOS (243 comparisons, max abs Δ 0.0167, H-A supported in 0 cells); nominal/dependence/multiplicity caveats in the artefact and `/queue-lab` |
| P7-E | Evidence-structured reports | StudyReport (`p7/report`, merged) | Tested |
| P7-F | Research UI/UX, terminology | CopyPolish (`p7/copy`, merged), QueueLabUI | Tested: evidence glossary, "source data" badge, one config formatter; 79 e2e on queuelab branch |
| P7-G | Performance and resource safety | StartupC, MemFixture → JobMem | Reads fixed (697 → 248 MiB); jobs still grow to 482 MiB → JobMem |
| P7-H | Production acceptance for research journeys | ProdSpecs (`p7/prodspecs`, merged) | 15 pass / 1 skip locally; run on production after deploy |

Independent reviews (GPT-6.1 Sol, 2026-11-18): backend REJECT (3 must-fix: snapshot fingerprint soundness, missing
quality-report table, `l3.rs` coverage start; 2 should-fix: single-flight cancellation leak, numeric window parse);
science REJECT (2 must-fix: OOS-B chronology, deadline semantics; 2 should-fix: plan deviation, artefact caveats).
Both confirmed the rest: 540 Rust tests pass at `42d9237`, API contract additive only, migration 0010 safe with old
code, trailer core still authenticates the skipped report, nine L2 formulas equal hftbacktest 0.9.4, Wilson and
Newcombe (method 10) correct, CKS OFI correct, all five research-home numbers reproduced independently.
