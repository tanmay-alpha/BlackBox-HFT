# BlackBox HFT — status and plan

Single authoritative plan. Each item states its evidence. "Done" means verified by a command whose
result is quoted here, not merely written.

## Model and orchestration record

- Lead: `anthropic/claude-opus-5-5`. The first wave of subagents ran on `claude-fable-5-2`;
  three of them (data pipeline, forensics, web shell) stopped with HTTP 429 rate-limit errors
  (`retry-after-ms≈15.6M`). The user then switched the default subagent model to Opus 5.5.
  Specialist reviewers and implementers since then: `openai-codex/gpt-6.1-sol:high` and
  `anthropic/claude-opus-5-5`. No work is attributed to a model that did not produce it.

## Milestones

### M1 Proof of simulation — done
- hftbacktest `=0.9.4` builds (Windows GNU toolchain; docs/BUILDING.md) and runs.
- Real Tardis sample ingested: BTCUSDT 2020-02-01, first 10 min, 201,970 engine rows, 6.4 s,
  quality report attached to the dataset.
- Synthetic dataset generated deterministically (seed 7, 62,929 rows).

### M2 Structured forensic trace — done
- Typed flight tape with clock and provenance per event (`crates/bb-core/src/flight.rs`).
- Determinism: 40 repeated runs per configuration, identical hashes (`tests/determinism.rs`);
  fresh-process re-runs on real data give identical `result_sha256`.
- Reconciliation: ledger vs engine local account agree under the no-partial-fill model on every
  real-data preset; the expected disagreement under partial fills is explained and pinned.
- Four upstream defects pinned against the unwrapped engine (docs/ENGINE_NOTES.md).
- Independent review (gpt-6.1-sol) found 4 recorder defects; all fixed with regression tests.

### M3 Functional replay interface — in progress
- Done: datasets/quality, market replay heatmap, lab, runs, run overview, order forensics,
  compare, grid (web build passes).
- Open: run-level **Replay Terminal** (book + own orders + fills + inventory under one playback
  clock, jump to order), **Mission Control** landing with one-click sample experiment, browser
  verification of every page (UIVerify agent).

### M4 Latency and queue research — mostly done
- Done: decision-aligned comparison, first divergence, conservative interpretation; grids.
- Open: **Latency Lab** screen driving the three canonical demos (latency, queue model,
  gross vs net) with controlled/varied variables listed.

### M5 Original research features — mostly done
- Done: rule-based explanations; strategy lab; data-quality reports.
- Open: research **exports** (run report markdown, full run JSON); explanation audit on real
  data (ExplainAudit agent).

### M6 System quality — in progress
- Done: 196 Rust tests; benchmarks with hardware and method (docs/BENCHMARKS.md).
- Open: property-based tests (proptest) for accounting and order-state invariants; Playwright E2E;
  security review; recorder overhead optimisation with bit-identical output (PerfTune agent);
  peak memory and API latency measurements.

### M7 Deployment and presentation — open
- Done: public repo https://github.com/tanmay-alpha/BlackBox-HFT, CI workflow.
- Open: Dockerfile for the full local simulator; static public viewer of precomputed
  **synthetic** runs (vendor terms forbid hosting Tardis-derived data); documentation set;
  screenshots; independent final review.

## Deliberate deviations from the brief

- Backend is Rust/axum, not Python/FastAPI: the simulator, ledger and forensics are Rust; a
  Python service would add a process boundary and serialisation without improving correctness.
  Reasoning in docs/ARCHITECTURE.md.
- No reactive market / market impact model: replay only, disclosed everywhere.
- Hosted demo shows synthetic data only (docs/RESEARCH_AND_SOURCES.md, Tardis clause 9.2).
