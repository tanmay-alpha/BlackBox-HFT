# Phase 6 and Phase 7 tracker

Single current-state tracker for Phase 6 (reliability, security, research integration) and Phase 7 (scientific
validation, unified research studio). Statuses: Open, Investigating, Confirmed, Fixed locally, Tested, Deployed,
Verified, Accepted risk, Blocked. "Verified" means checked on the deployed production version.

Baseline (2026-11-17/18): `main` = `af3a284` (docs only after `cdde05e`). Production API `git_sha=cdde05e…`, image
`sha256:2c14ea32…4ec4`, Render deploy `dep-dbtt9crm8hqs73dgc96g`. No Phase 6 commit is on `main` or deployed yet.

Branches: `p6/sec`, `p6/audit`, `p6/mkt` (finished) are merged into the integration branch `p6/int`
(`D:/bbwt/p6-int`). `p6/rel` (reliability) and the uncommitted `phase6` worktree (study viewer, startup/perf) are
still in progress. Phase 7 branches `p7/queue`, `p7/data`, `p7/mem`, `p7/report` start from `p6/int`.

Model availability: GPT-6.1 Sol returned `usage_limit_reached` on every spawn (2026-11-17 and 2026-11-18); all work
ran on the default agent model (Claude Opus 5.5). No review is attributed to Sol.

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
| B5 | P0 | AT-5/SEC-6 Linux full-process peak < 300 MB | Confirmed (partly fixed) | Linux fixture on the real hosted image, 512 MB, fake Blob, TLS Postgres (`scripts/research/hosted-memory.sh`). 32 anonymous heavy readers: 697 → 248 MiB after skipping the trailer analytics parse and `MALLOC_ARENA_MAX=2` (`p7/mem`). Back-to-back jobs still grow RSS 248 → 482 MiB; OOM-killed at job 55/57 (`HOSTED_MEMORY.md`). Production carries the read-side exposure until deploy | JobMem: release memory between jobs, re-measure |
| B8 | P0 | GLFT preset submission returns 500 (new) | Tested | Migration `0011_glft_strategy_kind.sql` widens `experiment_configs_strategy_kind_check` with `glft` (`c0a2cf4`); `strategy_kinds` test submits every `StrategyKind` preset through the API to completion. Sol audit of every enum CHECK in 0001–0011 against the writers: GLFT the only mismatch. 539 Rust pass | deploy (migration runs on start) |
| B6 | P0 | Production credential isolation | Tested | `630ab6f`, `99a0351`; local profile refuses cloud credentials; `scripts/ops/prod-run.sh`; `docs/OPERATOR_PRODUCTION.md`; DEPLOYMENT §5/§8 updated on `p6/int` | deploy |
| B7 | P1 | SEC-10 anonymous detail reads wake Postgres | Accepted risk (partial fix) | Demo comparison details now served from the snapshot without DB (`f1a203e`); run details and unknown ids still read Postgres | artefact snapshot in `artifacts.rs` |
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
