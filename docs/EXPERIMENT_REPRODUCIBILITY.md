# Experiment reproducibility

A BlackBox run is an experiment with a content-derived identity. Running the same experiment again
returns the same run; running it in a fresh process produces the same result bytes. This page
defines both hashes, lists the evidence, and explains how to reproduce a run from its report. Why the
simulation is deterministic in the first place is summarised in
[ARCHITECTURE.md](ARCHITECTURE.md#determinism-by-design).

## Run identity

`crates/bb-app/src/pipeline.rs`, `run_identity`:

```text
config_sha256 = SHA-256( canonical JSON of ( CONTRACT_VERSION, SIMULATION_REVISION, "hftbacktest",
                                             "0.9.4", dataset events_sha256, RunConfig ) )
run_id        = first 16 hex characters of config_sha256
```

- **Canonical JSON** is `serde_json::to_vec` of the Rust value: struct fields in declaration order,
  no unordered maps in hashed types, floats in shortest round-trip form (`crates/bb-core/src/hash.rs`).
- **`RunConfig`** is everything a user can choose: dataset id, strategy kind and parameters,
  evaluation interval, exchange model, queue model, order latency, feed offset, fees, risk limits,
  recorder settings and the seed (`crates/bb-core/src/config.rs`). Changing any of them changes the
  identity (integration test `execution_is_deterministic_and_identity_tracks_every_assumption`).
- **The dataset is identified by its bytes**: `events_sha256` is the SHA-256 of `events.npz`, which
  loading re-verifies ([MARKET_DATA.md](MARKET_DATA.md#the-engines-event-format)).
- **`SIMULATION_REVISION`** (`crates/bb-core/src/lib.rs`, currently 2) names the semantics of the
  simulation code: adapter, recorder, ledger, risk rules, strategies. Any change that can alter a
  run's result for the same configuration bumps it, so a stored run is never returned for code that
  would now produce something else ([Keeping identity honest](#keeping-identity-honest)).
- **Not covered:** the run label, platform, build profile and wall-clock time.

## Result hash

```text
result_sha256 = SHA-256( canonical JSON of ( tape, fills, equity ) )        # bb_forensics::result_sha256
```

The tape is the complete ordered event record; fills include their derived mid and markouts; equity
is every sample. Order records and metrics are pure functions of these and are not hashed
separately. The engine cross-check is excluded on purpose: the engine's own account sums fills in a
non-reproducible order, so its floating-point totals can differ in the last bit between runs
([EXECUTION_MODEL.md](EXECUTION_MODEL.md#1-simultaneous-fills-are-emitted-in-a-non-reproducible-order)).

Contract: two runs with equal `config_sha256` on the same platform must have equal `result_sha256`
(`RunManifest` doc comment, `crates/bb-core/src/result.rs`).

## The run manifest

Stored with every run (`RunManifest`) and printed in its report:

| Field | Purpose |
|---|---|
| `contract_version`, `simulation_revision`, `engine {name, version, features}` | what produced it (`features` is `["backtest"]`); runs stored before the revision field existed read back as revision 0 |
| `run_id`, `config_sha256`, `config` | the experiment, in full, including the seed |
| `dataset {id, sha256, rows, origin_ns}` | the exact input bytes |
| `result_sha256` | the outcome |
| `blackbox_version`, `platform` (`<os>-<arch>`), `profile` (`debug`/`release`) | provenance for the caveats below |
| `created_unix_ms`, `wall_time_ms` | informational; in neither hash |

## Idempotent runs

`find_or_execute` computes the identity first. If the store holds a run with that `run_id` **and**
the same full `config_sha256` (the 64-bit id prefix alone is not trusted), the stored run is returned
without simulating. Otherwise the run is simulated and stored. The same applies to `POST /api/runs`,
`blackbox run`, every grid cell and every demo run, so repeating any experiment returns the same run
id and result hash instantly. The CLI says so: "This configuration was already simulated on this
dataset; showing the stored run."

Stored runs are read back with `serde_json`'s `float_roundtrip` feature, so a reloaded run still
hashes to its own `result_sha256` (`crates/bb-app/Cargo.toml`).

## Evidence

| Evidence | What it shows | Source |
|---|---|---|
| 16 orders filled in two crowded engine steps, repeated 40 times per configuration (both exchange models, constant and jittered latency) | identical hash every time, despite the engine's randomized `HashMap` iteration | `crates/bb-sim/tests/determinism.rs::simultaneous_fills_are_recorded_in_a_reproducible_order` |
| Every bundled strategy × exchange model × queue model × latency model, run twice | identical outputs | `crates/bb-sim/tests/invariants.rs` |
| Generated markets and request streams | identical inputs give identical hashes | `crates/bb-sim/tests/properties.rs::identical_inputs_have_identical_hashes` |
| The application pipeline, twice | identical run id, both hashes, tape, orders, fills, equity, metrics | `crates/bb-app/tests/integration.rs::execution_is_deterministic_and_identity_tracks_every_assumption` |
| Seed changes jittered latency and nothing else | different seed → different jitter; same seed → identical | `determinism.rs::seed_changes_jittered_latency_and_nothing_else_is_hidden` |
| Benchmark sessions 1 and 2, separate processes on the same day | the four `no_partial_fill` result hashes identical across processes | [BENCHMARKS.md](BENCHMARKS.md#determinism-under-load) |
| CI "Determinism across processes" step (Linux) | `maker_baseline` on a freshly generated synthetic dataset, simulated in two separate processes, must print the same result hash | `.github/workflows/ci.yml` |
| Golden runs | seven fixed preset runs on a fixed synthetic dataset must reproduce pinned `result_sha256` values | `crates/bb-app/tests/golden.rs` |
| Fresh processes on the Tardis sample (measured for this page on 2026-11-14) | see below | release binary `target/release/blackbox.exe` |

**Fresh-process check.** The dataset `binance-futures-btcusdt-2020-02-01-10m` was copied into an
empty scratch data directory and `blackbox run --preset maker_baseline` was executed three times,
deleting the scratch run store before each so every run simulated from scratch in a new process.
All three printed `run id 6e84d8b06ed6b959` and
`result sha256 afcb3a9eaaa1e5c02dee417a574e279c1242fca4f0027342478a08a116ab027c`, identical to the
run stored earlier in `data/runs` by the long-running server process. A fourth invocation without
deleting the store returned the stored run. The scratch directory was deleted afterwards. (This
binary predates `SIMULATION_REVISION`; the current code gives the same configuration a different
run id because the revision is now part of the identity.)

**Seed only matters for jitter.** In the local run store (runs from before `SIMULATION_REVISION`),
two `avellaneda_stoikov` runs on the Tardis sample whose configurations differ only in `seed` (1 and
999) under constant latency have different run ids (`f7be6da3783eef94`, `835ba545c3da0595`) and the
same `result_sha256` (`93ee659c2386…`); two `maker_baseline` runs with seeds 1 and 11 likewise share
`afcb3a9eaaa1…`.

## Keeping identity honest

Because the identity covers the simulation revision, code and stored results cannot silently
disagree:

1. `crates/bb-app/tests/golden.rs` pins the synthetic dataset's hash and the `result_sha256` of
   seven runs on it: `maker_baseline` under both exchange models, `maker_slow`,
   `maker_conservative_queue`, `maker_rebate`, `imbalance_taker` and `forensics_probe`, plus the
   revision they were pinned at. Any code change that alters those results makes it fail.
2. If the change is intended, the same commit bumps `SIMULATION_REVISION` and updates the pinned
   hash. Every configuration then has a new run id, so `find_or_execute` simulates it afresh instead
   of returning a run produced by the old code.
3. The manifest records `simulation_revision` and `blackbox_version`, so an old stored run states
   which code produced it.

The revision history is kept in the constant's doc comment (1 = first release; 2 = the review fixes:
late cancel rejections recorded, `fill_reset` queue events, hard risk limits, market orders
rejected, drawdown halt on locally known fills). Earlier, before the revision existed, `bb-sim`'s
partial-fill handling changed between two benchmark sessions and the partial-fill hashes changed
with it ([BENCHMARKS.md](BENCHMARKS.md#determinism-under-load)); that is the situation this
mechanism now catches.

## Caveats

- **Platform.** Results are reproducible for the same binary on the same platform. Across platforms
  they are expected to match for exact arithmetic; strategies that call `ln`/`exp`
  (Avellaneda-Stoikov) can differ in the last bit of an intermediate value between math libraries,
  which changes a quote only if it lands exactly on a rounding boundary
  ([BUILDING.md](BUILDING.md#reproducibility)). The PRNG uses only integer and IEEE `+ − × ÷`
  operations (`crates/bb-core/src/rng.rs`). The golden test runs on Windows locally and on Linux in
  CI; the same pinned hashes passing on both is the cross-platform evidence for the
  exact-arithmetic strategies it covers. Avellaneda-Stoikov is deliberately not pinned and has no
  cross-platform evidence.
- **Build profile.** Recorded in the manifest. Debug and release builds have not been compared hash
  for hash. The one known profile-dependent engine behaviour (integer overflow in the partial-fill
  market-order sweep) is unreachable because the harness blocks market orders
  ([EXECUTION_MODEL.md](EXECUTION_MODEL.md#5-market-orders)).
- **The revision is only as good as the golden coverage.** A change that alters results only on
  paths the golden runs do not exercise (Avellaneda-Stoikov, jittered latency, a feed offset, a
  drawdown halt) would not fail it. The determinism, invariants and property suites still run, but
  they check consistency, not equality with earlier results.
- **Dataset bytes.** Re-ingesting the same vendor files with a different BlackBox version can
  produce different `events.npz` bytes, and therefore a different experiment. The dataset manifest
  records each input file's SHA-256, so it is always possible to tell whether the inputs were the
  same.

## Reproducing a run from its report

`GET /api/runs/{id}/report` (Markdown) contains the identity hashes, engine and platform, the full
dataset manifest including input-file hashes, the full configuration, every assumption in plain
language, all metrics, the engine cross-check, and a POSIX-shell recipe
(`crates/bb-app/src/report.rs`). To reproduce:

1. **Obtain the same dataset bytes.** Synthetic: `blackbox synth` with the `generator` configuration
   recorded in the report's dataset manifest. Recorded: download the vendor files yourself, check
   their SHA-256 against `source.inputs[].sha256`, and ingest with the same instrument and window
   parameters. The vendor's terms forbid obtaining them from anyone else
   ([MARKET_DATA.md](MARKET_DATA.md#redistribution-rules)).
2. **Check the dataset hash**: `blackbox quality <id>` prints the `events.npz` SHA-256; it must equal
   the report's "Dataset events SHA-256".
3. **Run the recipe**: it writes the configuration to `blackbox-run-<run_id>-config.json` and calls
   `blackbox run --config <file> --dataset <id>`.
4. **Compare** the printed `run id` and `result sha256` with the report. Equal run ids mean the same
   experiment; equal result hashes mean the same outcome, byte for byte.

`GET /api/runs/{id}/export` returns the complete `RunResult` JSON, and each canonical demo
(`GET /api/demos`) carries one such recipe per run.
