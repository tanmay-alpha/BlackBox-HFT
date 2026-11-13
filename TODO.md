# BlackBox HFT — status and plan

Single authoritative plan. Each item states its evidence. "Done" means verified by a command whose
result is quoted here or in the linked document, not merely written.

## Model and orchestration record

- Lead: `anthropic/claude-opus-5-5`. The first wave of subagents ran on `claude-fable-5-2`;
  three of them (data pipeline, forensics, web shell) stopped with HTTP 429 rate-limit errors
  (`retry-after-ms≈15.6M`). The user then switched the default subagent model to Opus 5.5.
- Specialist implementers and reviewers since then: `openai-codex/gpt-6.1-sol:high` (forensics
  comparison, recorder correctness review, explanation audit, performance tuning, property tests,
  backend endpoints, security review, E2E) and `anthropic/claude-opus-5-5` (web features, docs,
  benchmarks, browser verification, static viewer, final review). The final adversarial review
  was first assigned to gpt-6.1-sol, which hit its usage limit; it was rerun on Opus 5.5 with a
  fresh context. No work is attributed to a model that did not produce it.

## Milestones

### M1 Proof of simulation — done
- hftbacktest `=0.9.4` builds (Windows GNU toolchain; docs/BUILDING.md) and runs.
- Real Tardis sample ingested locally: BTCUSDT 2020-02-01, first 10 min, 201,970 engine rows,
  6.4 s; quality report attached to the dataset (never committed).
- Synthetic dataset generated deterministically (seed 7).

### M2 Structured forensic trace — done
- Typed flight tape with clock and provenance per event (`crates/bb-core/src/flight.rs`).
- Determinism: repeated and fresh-process runs give identical hashes; golden hashes pinned and
  checked on Windows and Linux CI (docs/EXPERIMENT_REPRODUCIBILITY.md).
- Reconciliation: ledger vs engine account agree under the no-partial-fill model; the expected
  disagreement under partial fills is explained and pinned. Independent recompute by the final
  reviewer matched to 1e-9 on six runs.
- Five upstream defects pinned against the unwrapped engine (docs/EXECUTION_MODEL.md).

### M3 Functional replay interface — done
- Mission Control, Datasets/quality, Market Replay, Strategy Lab, Runs, Run overview, Replay
  Terminal (book + own orders + fills under one clock), Order forensics, Compare, Latency Lab,
  Sensitivity Grid. Every page verified in a browser with no console errors or failed requests.

### M4 Latency and queue research — done
- Decision-aligned comparison with first divergence and conservative interpretation; three
  canonical controlled experiments (`/api/demos`, `blackbox demos`); bounded grids.

### M5 Original research features — done
- Rule-based explanations, audited on 9,447 real-data orders (97,890 steps, zero remaining errors
  after fixes); markdown research report and full run export; data-quality reports.

### M6 System quality — done
- Rust: unit, integration, property-based (proptest, with a coverage test), differential parity
  with the unwrapped engine, golden hashes (docs/TESTING.md has the inventory).
- Playwright E2E of the research story; runs in CI.
- Two independent reviews (recorder correctness; security) and a final adversarial review; all
  material findings fixed or documented (docs/SECURITY.md).
- Performance and resource measurements with method (docs/PERFORMANCE.md).

### M7 Deployment and presentation — done
- Public repo; CI (rust, web, e2e, RustSec audit, docker image build + smoke test); Dockerfile;
  documentation set; synthetic-only screenshots; vendor images purged from history.
- Static public viewer of precomputed synthetic runs at https://blackbox-hft.vercel.app, checked
  in a browser with no console errors on Mission Control, Latency Lab, run overview, order
  forensics, Replay Terminal, grid, Strategy Lab and Datasets (docs/DEPLOYMENT.md).

## Deliberate deviations from the brief
- Backend is Rust/axum, not Python/FastAPI: the simulator, ledger and forensics are Rust; a
  Python service would add a process boundary and serialisation without improving correctness
  (docs/ARCHITECTURE.md).
- No reactive market / market impact model: replay only, disclosed everywhere.
- Hosted demo shows synthetic data only (Tardis terms, docs/OPEN_SOURCE_RESEARCH.md).
- No cancellation of an in-flight simulation; work is bounded by configuration limits instead
  (docs/SECURITY.md, residual risks).
