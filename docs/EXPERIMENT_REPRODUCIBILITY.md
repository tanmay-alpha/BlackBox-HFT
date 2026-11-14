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

Contract: two runs with equal `config_sha256` on the same platform class must have equal
`result_sha256` (`RunManifest` doc comment, `crates/bb-core/src/result.rs`).

## The run manifest

Stored with every run (`RunManifest`) and printed in its report:

| Field | Purpose |
|---|---|
| `contract_version`, `simulation_revision`, `engine {name, version, features}` | what produced it (`features` is `["backtest"]`); runs stored before the revision field existed read back as revision 0 |
| `run_id`, `config_sha256`, `config` | the experiment, in full, including the seed |
| `dataset {id, sha256, rows, origin_ns}` | the exact input bytes |
| `result_sha256` | the outcome |
| `blackbox_version`, `platform` (platform class, see [Platform policy](#platform-policy)), `profile` (`debug`/`release`) | provenance for the caveats below |
| `created_unix_ms`, `wall_time_ms` | informational; in neither hash |

## Idempotent runs

`find_or_execute` computes the identity first. If the store holds a run with that `run_id` **and**
the same full `config_sha256` (the 64-bit id prefix alone is not trusted) **and** the same platform
class, the stored run is returned without simulating. If the stored run was computed on another
platform class, the request fails with `platform_mismatch` (HTTP 409) and the stored run is left
untouched. Otherwise the run is simulated and stored. The same applies to `POST /api/runs`,
`blackbox run`, every grid cell and every demo run, so repeating any experiment returns the same run
id and result hash instantly. The CLI says so: "This configuration was already simulated on this
dataset; showing the stored run."

Stored runs are read back with `serde_json`'s `float_roundtrip` feature, so a reloaded run still
hashes to its own `result_sha256` (`crates/bb-app/Cargo.toml`).

## Platform policy

The platform class is `<target>+libm-<fingerprint>` (`crates/bb-core/src/platform.rs`). `<target>`
is `<os>-<arch>`, followed by `-<target_env>` unless that is the OS default (glibc on Linux) or
empty: `linux-x86_64`, `windows-x86_64-gnu` (MinGW), `windows-x86_64-msvc`, `macos-aarch64`. The
fingerprint is the first 8 hex digits of the SHA-256 of the bit patterns that this process's math
library returns for a fixed probe (`libm_probe`): 64 inputs each of `powf` (the engine's
power-probability queue models; `0.5^(1/halflife)` in Avellaneda-Stoikov and GLFT), `ln(1 + x)`
(the engine's log-probability queue models), `ln_1p` (Avellaneda-Stoikov, GLFT), `exp` (GLFT) and
`sqrt`, plus `powi(10, d)`, with inputs from BlackBox's integer-only PRNG and in the domains where
each is called. Those are every platform-provided `f64` function on the result path; BlackBox's
latency samplers use their own `exp`/`ln` ([LATENCY_MODEL.md](LATENCY_MODEL.md)).

**Why the math library and not the OS.** CI run 43564035306 ran the golden test on GitHub's
`windows-latest` (x86_64-pc-windows-gnu, rustup's bundled MinGW): `maker_slow` gave
`9a809e78afee37bc…` and `avellaneda_stoikov` `6cafe3ca74bfa4dc…`, identical to Linux (glibc). The
different Windows hashes pinned earlier (`662ad3b6…`, `0731525d…`) come only from the development
machine's WinLibs MinGW-w64 14.0.0 (msvcrt) runtime, whose `pow`/`ln` differ in the last bit. Same
OS, different results; different OS, same results.

- **Canonical:** the math library of `linux-x86_64` (glibc) in CI, the hosted worker's:
  `CANONICAL_LIBM`. It is pinned from a CI log
  (`platform::tests::linux_glibc_has_the_canonical_fingerprint` fails on Linux with the computed
  value until it is); until then no class is
  canonical. A class is canonical when its fingerprint is, whatever the OS. The GitHub Windows
  runner very likely shares it **[INFERENCE from the identical golden hashes]**; one CI run shows
  both fingerprints.
- **Known non-canonical:** WinLibs MinGW-w64 14.0.0 msvcrt, fingerprint `e9b26b47`
  (`WINLIBS_MSVCRT_LIBM`), measured on the development machine on 2026-11-14.
- **Recorded:** every run manifest records the full class. Runs stored before fingerprints existed
  record `linux-x86_64` / `windows-x86_64-gnu`, and before classes `<os>-<arch>`; they are a
  different class and are not reused.
- **Never substituted:** the run store is keyed by `run_id` only, and a stored result whose full
  class differs from the running binary's is refused with `platform_mismatch` rather than returned
  as this platform's result (`pipeline::find_or_execute_with`). Delete the stored run to recompute
  it on this platform. Reading a stored run by id still returns it with its recorded class.
- **Pins:** `golden.rs` keys pins by fingerprint: `canonical` (glibc) and `winlibs`. Entries marked
  portable (risk-adverse queue model, touch maker, any latency model) carry one pin and are checked
  under every fingerprint. An unknown fingerprint, or an unpinned entry, fails with every computed
  hash printed in one block labelled with the class and fingerprint.

## Execution budgets

Every run is admitted against an `ExecutionLimits` profile (`crates/bb-core/src/limits.rs`) before
anything runs, and enforced while it runs:

| Limit | `local()` | `hosted()` | Checked |
|---|---:|---:|---|
| dataset rows | none | 500,000 | admission |
| strategy ticks | 2,000,000 | 150,000 | admission (`estimate_work`) |
| equity samples | none | 150,000 | admission and in the simulation loop |
| tape events | none | 150,000 | admission (per-strategy prediction) and in the simulation loop |
| stored result bytes | none | 128 MiB | admission (prediction) and on the finished result |

A prediction over a limit is refused with `budget_exceeded` (HTTP 422) naming the limit. A run that
crosses a limit while running (or is cancelled through its `RunControl`) is aborted and nothing is
stored. Budget checks and progress reports run every `progress_every_ticks` (1,000) ticks and once at
the end; they only read state, so a completed run is bit-identical under any profile (the golden
test runs through them), and because the tape and equity series only grow, whether a run breaches
its budget does not depend on the cadence (`crates/bb-sim/tests/run_control.rs`). The hosted sizing
is in [PERFORMANCE.md](PERFORMANCE.md#execution-budgets).

**Precision guards.** `seed` must be at most 2^53 − 1 (larger values are refused with
`invalid_config`; a browser would round them). Generation, ingest and saving refuse a dataset any
of whose timestamps lies 2^53 ns (about 104 days) or more from its origin. Absolute nanoseconds in
the dataset manifest (`origin_ns`, `generator.origin_ns`) are decimal strings; older manifests with a
numeric `generator.origin_ns` still load.

## Evidence

| Evidence | What it shows | Source |
|---|---|---|
| 16 orders filled in two crowded engine steps, repeated 40 times per configuration (both exchange models, constant and jittered latency) | identical hash every time, despite the engine's randomized `HashMap` iteration | `crates/bb-sim/tests/determinism.rs::simultaneous_fills_are_recorded_in_a_reproducible_order` |
| Every bundled strategy × exchange model × queue model × latency model, run twice | identical outputs | `crates/bb-sim/tests/invariants.rs` |
| Generated markets and request streams | identical inputs give identical hashes | `crates/bb-sim/tests/properties.rs::identical_inputs_have_identical_hashes` |
| The application pipeline, twice | identical run id, both hashes, tape, orders, fills, equity, metrics | `crates/bb-app/tests/integration.rs::execution_is_deterministic_and_identity_tracks_every_assumption` |
| Seed changes jittered latency and nothing else | different seed → different jitter; same seed → identical | `determinism.rs::seed_changes_jittered_latency_and_nothing_else_is_hidden` |
| Benchmark sessions 1 and 2, separate processes on the same day | the four `no_partial_fill` result hashes identical across processes | [PERFORMANCE.md](PERFORMANCE.md#determinism-under-load) |
| CI "Determinism across processes" step (Linux) | `maker_baseline` on a freshly generated synthetic dataset, simulated in two separate processes, must print the same result hash | `.github/workflows/ci.yml` |
| Golden runs | eleven fixed runs on a fixed synthetic dataset must reproduce pinned `result_sha256` values per platform class | `crates/bb-app/tests/golden.rs` |
| Unwrapped engine, same request stream | the stock engine replaying an adapter run's requests reports as many trades as the adapter recorded fills (touch maker and Avellaneda-Stoikov) | `crates/bb-sim/tests/raw_engine_parity.rs` |
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
   eleven runs on it: `maker_baseline` under both exchange models, `maker_slow`,
   `maker_conservative_queue`, `maker_rebate`, `imbalance_taker`, `forensics_probe`,
   `avellaneda_stoikov`, and the risk-adverse maker with jittered latency, with a 5 ms feed offset
   and with a drawdown halt (each asserted to record a fill or a `risk_halt`), plus the revision
   they were pinned at. Any code change that alters those results makes it fail.
2. If the change is intended, the same commit bumps `SIMULATION_REVISION` and updates the pinned
   hash. Every configuration then has a new run id, so `find_or_execute` simulates it afresh instead
   of returning a run produced by the old code.
3. The manifest records `simulation_revision` and `blackbox_version`, so an old stored run states
   which code produced it.

The revision history is kept in the constant's doc comment (1 = first release; 2 = the review fixes:
late cancel rejections recorded, `fill_reset` queue events, hard risk limits, market orders
rejected, drawdown halt on locally known fills). Earlier, before the revision existed, `bb-sim`'s
partial-fill handling changed between two benchmark sessions and the partial-fill hashes changed
with it ([PERFORMANCE.md](PERFORMANCE.md#determinism-under-load)); that is the situation this
mechanism now catches.

## Caveats

- **Platform.** Results are reproducible for the same binary on the same platform. Across operating
  systems they are identical only for configurations whose result path uses exact arithmetic. The
  engine's probability queue models (`log_prob*`, `power_prob*`) call `f64::ln` / `f64::powf`, and
  Avellaneda-Stoikov calls `ln`; these come from the platform math library (MinGW on the Windows
  development machine, glibc on Linux CI) and may differ in the last bit. That almost never
  matters, but it was observed: in the first Linux CI run (43503383714) the `maker_slow` preset
  (`power_prob3`, 50 ms latency) produced a different result hash than on Windows, while the other
  six pinned configurations, including four that also use `power_prob3`, matched exactly. The
  golden test therefore pins per platform class and requires the libm-free entries (risk-adverse
  queue model) to be identical everywhere. The PRNG uses only integer and IEEE `+ − × ÷`
  operations (`crates/bb-core/src/rng.rs`).
- **Build profile.** Recorded in the manifest. The golden test passes unchanged in both the debug
  and the release profile on Windows (2026-11-14), so the pinned configurations are
  profile-independent there. The one known profile-dependent engine behaviour (integer overflow in
  the partial-fill market-order sweep) is unreachable because the harness blocks market orders
  ([EXECUTION_MODEL.md](EXECUTION_MODEL.md#5-market-orders)).
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
