# Phase 6 and Phase 7 tracker

Single current-state tracker for Phase 6 (reliability, security, research integration) and Phase 7 (scientific
validation, unified research studio). Statuses: Open, Investigating, Confirmed, Fixed locally, Tested, Deployed,
Verified, Accepted risk, Blocked. "Verified" means checked on the deployed production version.

Baseline (2026-11-17/18): `main` = `af3a284`, production `cdde05e`.

**Released 2026-11-19:** `a1d128c` (PR #3), then follow-ups `5fe8923` (PR #4), `36536d0` (PR #5), `18aed41` (PR #7),
`3ed53cd` (PR #9) and the Phase 11 audit `a24ccf9` (PRs #13, #14). Production runs `a24ccf9` (image
`sha256:631a8111…2d8c52b`, Render deploy `dep-dbvceeu7bikc7387i490`, migrations through 0011;
[../rebuild/PHASE67_RELEASE.md](../rebuild/PHASE67_RELEASE.md), [../rebuild/PHASE11_AUDIT.md](../rebuild/PHASE11_AUDIT.md)).
Every row below is in that image. **Verified** rows were checked on production: the production browser suite (16
tests, 16 passed on `a24ccf9` incl. the keyed journey: job `01a1f33e-ce13-7966-95b0-7b1839413274`, run
`299f836aa96bd57c`; [../rebuild/PRODUCTION_ACCEPTANCE.md](../rebuild/PRODUCTION_ACCEPTANCE.md)), smoke and direct
page checks of `/research` and `/queue-lab`. **Deployed** rows are in production but were proved by local tests or the
512 MB local fixture, not by a production measurement.

Model availability: GPT-6.1 Sol returned `usage_limit_reached` on 2026-11-17 and early 2026-11-18 and was used from
2026-11-18 15:20 IST for the two independent reviews (backend, science), the enum-CHECK audit, the backend review
fixes and the queue experiment fixes; it hit its limit again before the analytics-memory and cancellation tasks,
which ran on Claude Opus 5.5. Everything else ran on Claude Opus 5.5.

## Phase 6 entry gate (Phase 7 §2)

| Id | Sev | Requirement | Status | Evidence / commit | Remaining |
|---|---|---|---|---|---|
| A1 | P0 | Cold compare-set `503 busy` | Verified | Root cause: 7 parallel `GET /runs/{id}` whole-bundle verifications for an overview, plus a summary-digest fallback reading whole bundles outside the heavy-read permit. Cold page 16 req / 1,672 ms CPU → 9 req / 156 ms, no store reads (`22770ac`, `cfda25c`); bounded busy retry; single-flight cold fetch (`42372ba`). `compare_set` test; `PHASE6_RELIABILITY.md`; `compare-set.prod.spec.ts` on production | |
| A2 | P0 | False "no stored comparison" on curated demo | Verified | Cause: stale anonymous snapshot (≤1 h) + UI treating "not in list" as missing. Nine explicit pair states, no locked create for anonymous (`cfda25c`); Rust + Playwright `compare-set.spec.ts`; `compare-set.prod.spec.ts` on production | |
| A3 | P0 | Snapshot freshness for externally seeded rows | Deployed | `demo_data_version` (`f1a203e`) replaced after the Sol review: per-table sorted (key, xmin) digest over every table the snapshot reads, incl. quality reports and workspace visibility (`9d507c1`); 0.89 ms warm on a seeded demo DB; equal-sum and quality-report regressions; ≤ 15 min recheck | |
| B1 | P0 | SEC-18 secret-safe logs | Deployed | `630ab6f`; `secret_logging` tests | |
| B2 | P0 | SEC-17 worker fault matrix | Deployed | `630ab6f`; `worker_faults` | |
| B3 | P0 | SEC-25 audit coverage | Deployed | `9004462`; `audit_matrix` | |
| B4 | P0 | SEC-11 §6.3 metered store, anonymous half share | Deployed | `9004462` + migration 0010 (applied on production); `artifact_metering` | |
| B5 | P0 | AT-5/SEC-6 Linux full-process peak < 300 MB | Deployed (fixed in the local fixture) | `8c35d57` (in `3ed53cd`): cold timeline first page refuses `window_too_large` from fills+equity before decoding orders/tape; comparisons use the cached run or orders+tape only; byte-identical. 3 pinned repeats: every phase < 286.1 MiB, phase 4 240.0/247.0/252.0 (was 264.0/279.6/300.1), margin 34.1 > spread 12.0; 6 runs ≤ 257.6. Residual: previous image's spread 36.1 MiB | measured in a local container without a CPU quota, not on Render |
| B8 | P0 | GLFT preset submission returns 500 (new) | Deployed | Migration `0011_glft_strategy_kind.sql` widens `experiment_configs_strategy_kind_check` with `glft` (`c0a2cf4`; applied on production); `strategy_kinds` test submits every `StrategyKind` preset through the API to completion. Sol audit of every enum CHECK in 0001–0011 against the writers: GLFT the only mismatch. 539 Rust pass | |
| B6 | P0 | Production credential isolation | Deployed | `630ab6f`, `99a0351`; local profile refuses cloud credentials; `scripts/ops/prod-run.sh`; `docs/OPERATOR_PRODUCTION.md`; DEPLOYMENT §5/§8 | |
| B7 | P1 | SEC-10 anonymous detail reads wake Postgres | Deployed | Snapshot trust rows (`00fcae4`, `23f896b`, released in `36536d0`) + anonymous analytics/series from the authenticated bundle trailer (`c6b407e`, released in `18aed41`): 48 detail/paged + 24 analytics/series anonymous reads, 0 DB acquisitions; byte-identical to keyed responses; legacy bundles fall back to the DB | DB-free reads proved by tests, not observed on Neon |
| C1 | P1 | Blank route fallback | Deployed | `b30c936`: boot line in `index.html`, `WorkspaceLoading` frame; empty-page time 1.6–2.6 s → 0 ms in 30 loads; `workspace-loading.spec.ts` | |
| C2 | P1 | Mobile LCP | Deployed (partly) | Same-session A/B (mobile, 6 reps): Explore 5,564 → 4,276 ms, Replay 11,462 → 5,232 ms; FCP ~3,250 → ~810 ms; CLS not worse; first table page after Explore +0.45 s (`PHASE6_PERFORMANCE.md`). Phase 10 lazy-boundary candidate improved median LCP by only 92 ms (landing/Explore) and 8 ms (Replay), below the 150 ms gate; not shipped (`../rebuild/PHASE10_LCP.md`) | still > 2.5 s locally (entry chunk) |
| C3 | P1 | Static public demo fast path | Accepted risk (not shipped) | Evaluated: full Explore set ~64 KB gzip; not shipped (second data path, export step, benefit inferred only) | revisit with prod cold-start data |
| D1–D4 | P1 | Workbench L3/L2 before/after, curated events, exact semantics | Verified | `f1d38af`, `ecfdc24`; bb-data causal tests; 499 Rust pass; `market.prod.spec.ts` on production | |
| E1–E4 | P1 | Study viewer on verified artefact | Verified | `fab88b8`; nav entry (P7-A), cell explorer (P7-B); `studies.prod.spec.ts` on production (artefact SHA-256 = published `.sha256`, matrix = artefact) | |
| F1 | P1 | Report wording (synthetic vs historical, raw enums, ns) | Deployed | `a2314be`: evidence-structured run/comparison/sweep reports, `generator_version` 2, stored reports unchanged; 517 Rust pass | |
| F2–F4 | P1 | Terminology, visual defects, continuity | Deployed | CopyPolish (`p7/copy`, see P7-F); research home + study explorer give continuity via URL helpers | |
| G1–G3 | P1 | Production acceptance for Workbench, studies, security | Verified | `web/e2e-prod/{market,studies,compare-set,errors}.prod.spec.ts` pass on production (`36536d0`, `18aed41` and `3ed53cd`: 15 passed, no retries) | |
| H1 | P2 | Real-data rights and independent check | Deployed | `REAL_DATA_VALIDATION_PLAN.md`: no licence text; local private use only; QQQ/AAPL 100% 3-way agreement on a 300 MB prefix | local-only by design; nothing hosted |

## Phase 7 workstreams

| Id | Item | Owner / branch | Status |
|---|---|---|---|
| P7-A | Unified research identity, cross-workspace links, research landing | ResearchHome (`p7/research`, merged) | Verified: `/research` five live investigations on production, `RESEARCH_IDENTITY.md`, nav |
| P7-B | Study explorer (cell selection, controlled differences, evidence) | StudyExplorer (`p7/explorer`) | Verified: `studies.prod.spec.ts` on production |
| P7-C | Reproducible ITCH validation pipeline, quality report, metrics | ItchPipeline (`p7/data`), SolBackendFix | Deployed (CLI; ITCH stays local): QQQ/AAPL 100% three-way reproduced; negative control 1 mismatch each; review fixes merged (coverage from window start, numeric window grammar) |
| P7-D | Queue-model shadow experiment + lab page | QueueLab2, QueueLabUI, SolQueueFix (`p7/queuelab`, merged); OosDays (`p10/oos`) | Verified (page): `/queue-lab` on production shows the released artefact ("SHA-256 verified"). Inclusive deadline/checkpoint semantics (`ccdd493`); original full rerun changed 0 of 810 fill cells, 0 of 972 checkpoints, 0 of 81 cancellation cells; 2019-12-30 QQQ/AAPL previously inspected. After `3ed53cd`, docs only (not on `/queue-lab`): three clean OOS acquisitions: October plus precommitted 2019-01-30/2020-01-30 prefixes (534.128393/271.554873308 s, neither reaches 09:39; SPY held out on both). Per-day H-A support 0/11/3 of 27, H0 nominal rejections 9/36/3 of 243, max abs Δ 0.0167/0.0243/0.0258; nominal/dependence/multiplicity caveats retained; full aggregates in `QUEUE_SHADOW_EXPERIMENT.md` §6.6–§7 |
| P7-E | Evidence-structured reports | StudyReport (`p7/report`, merged) | Deployed |
| P7-F | Research UI/UX, terminology | CopyPolish (`p7/copy`, merged), QueueLabUI | Deployed: evidence glossary, "source data" badge, one config formatter |
| P7-G | Performance and resource safety | StartupC, MemFixture, JobMem, AnalyticsMem, SolReadMem, SolBaseline, Baseline2, Phase4Mem | Deployed in `3ed53cd`: 512 MB local fixture, every phase < 286.1 MiB in 3 pinned repeats; not measured on Render |
| P7-H | Production acceptance for research journeys | ProdSpecs (`p7/prodspecs`, merged) | Verified: on `3ed53cd` 15 anonymous tests passed and the keyed journey passed (job `01a1f231-a309-7585-8556-e45a2d92b240`, run `c6d47c551e17c5af`) |

Independent reviews (GPT-6.1 Sol, 2026-11-18): backend REJECT (3 must-fix: snapshot fingerprint soundness, missing
quality-report table, `l3.rs` coverage start; 2 should-fix: single-flight cancellation leak, numeric window parse);
science REJECT (2 must-fix: OOS-B chronology, deadline semantics; 2 should-fix: plan deviation, artefact caveats).
Both confirmed the rest: 540 Rust tests pass at `42d9237`, API contract additive only, migration 0010 safe with old
code, trailer core still authenticates the skipped report, nine L2 formulas equal hftbacktest 0.9.4, Wilson and
Newcombe (method 10) correct, CKS OFI correct, all five research-home numbers reproduced independently.
