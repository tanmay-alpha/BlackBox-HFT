# Next phases tracker (3.1, 3.2, 4, 5)

Status of each deliverable on `main` `55084c0` (2026-11-17). Production: **released** 2026-11-16 21:44 UTC
(record: [PHASE31_RELEASE.md](PHASE31_RELEASE.md)); the `execution_validation` demo is seeded in production.

## Phase 3.1: UI polish, responsiveness, accessibility

| Deliverable | Evidence | Status |
|---|---|---|
| Review record with defect matrix, regression tests, independent review (7.90) | `docs/rebuild/PHASE31_UI_REVIEW.md` | Done |
| Before/after screenshots 360–1440 px | `docs/rebuild/img/p3-*.webp` | Done |
| Shell, component and page changes | `web/src/shell/`, `web/src/components/`, `web/src/features/*/` (modified) | Done |
| E2E coverage | `web/e2e/responsive.spec.ts`, `web/e2e/a11y-keyboard.spec.ts`, `web/e2e/measure.ts`, `web/e2e/research.spec.ts` (52/52 locally, CI e2e green) | Done |

## Phase 3.2: performance

| Deliverable | Evidence | Status |
|---|---|---|
| Phase 3.2 performance document, harness, baselines, fixes | `docs/rebuild/PHASE32_PERFORMANCE.md`, `web/perf/` | Done (lab measurements only; no field data) |

## Phase 4: Market Data Workbench (L3)

| Deliverable | Evidence | Status |
|---|---|---|
| Synthetic ITCH L3 generator | `crates/bb-data/src/l3synth.rs` | Done |
| Order-level book replay and quarantine | `crates/bb-data/src/l3workbench.rs` | Done |
| L3 metrics and L2 comparison | `crates/bb-analytics/src/l3.rs` | Done |
| Public synthetic-only API (`/api/v1/market/...`) and `blackbox market` CLI | `crates/bb-app/src/v1/market.rs`, `web/openapi.json` | Done |
| API tests | `crates/bb-app/tests/market_workbench.rs` | Done |
| UI at `/market` | `web/src/features/market/` (`MarketWorkbenchPage.tsx`, tabs, `model.test.ts`) | Done |
| Research docs | `docs/research/MARKET_DATA_WORKBENCH.md`, `docs/research/L3_L2_VALIDATION.md` | Done |
| Contract and limitations | `docs/engineering/API_CONTRACT.md` §8.2a, `docs/LIMITATIONS.md` | Done |
| Calibration to a real L3 feed | none (generator always executes the queue head) | Not done |

## Phase 5: Execution Model Validation

| Deliverable | Evidence | Status |
|---|---|---|
| Synthetic liquidity regimes | `crates/bb-data/src/synthetic.rs` (modified) | Done |
| Offline study CLI `blackbox study exec-validation` | `crates/bb-app/src/study.rs` | Done |
| `execution_validation` demo over three datasets | `crates/bb-app/src/demos.rs`, `seed.rs`, `v1/datasets.rs` (modified) | Done |
| Evidence ledger on the order page | `web/src/features/forensics/EvidenceLedger.tsx`, `ledger.ts`, `ledger.test.ts` | Done |
| Research doc | `docs/research/EXECUTION_MODEL_VALIDATION.md` | Done |
| Validation against real executions | none (sensitivity study only) | Not done |
