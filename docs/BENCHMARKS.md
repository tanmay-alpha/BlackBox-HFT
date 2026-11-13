# Benchmarks

Wall-clock cost of replaying a dataset through the BlackBox adapter (`bb_sim::simulate`) compared
with the stock `hftbacktest 0.9.4` engine on the same data. Every number below was measured on the
machine described here with the commands shown; nothing is extrapolated from other hardware.

## Reproduce

From the repository root:

```
cargo run --release -p bb-sim --example bench
cargo run --release -p bb-sim --example bench -- --dataset binance-futures-btcusdt-2020-02-01-10m
```

The first command benchmarks an in-process synthetic dataset only. The second adds a stored dataset
from `data/datasets/<id>` (skipped with a message if the directory is absent). Options:
`--reps N` (default 5), `--duration-s S` (synthetic length, default 600), `--interval-ms MS`
(strategy interval, default 100), `--dataset ID` (repeatable), `--no-synthetic`, `--no-sweep`.
The source is `crates/bb-sim/examples/bench.rs`; it uses only the standard library (no criterion).

The tables below are the output of the second command, run once (session 2). Session 1 ran the
same command earlier the same day; it is used only for the session-to-session comparison further
down.

## Machine and build

| | |
|---|---|
| CPU | Intel Core i7-14650HX, 16 cores / 24 logical processors (WMI; `nproc` = 24); WMI `MaxClockSpeed` 2200 MHz |
| Memory | 16,869,285,888 bytes (WMI `TotalPhysicalMemory`) |
| OS | Windows 11 Home Single Language, 10.0.26300, laptop |
| Toolchain | rustc 1.99.1 (58cf8f9c3 2026-10-13), host `x86_64-pc-windows-gnu`, LLVM 23.1.1 |
| Profile | workspace `release`: `opt-level = 3`, `lto = "thin"`, `codegen-units = 4`, `debug = 1` |
| Engine | `hftbacktest =0.9.4`, `backtest` feature only |

Conditions that were not controlled: the power plan could not be read (the query needs
administrator rights), CPU frequency was not pinned, and other cargo builds/tests from parallel
work were running on the machine. Total CPU utilisation sampled three times around session 2 was
0.7 % to 21.8 %. The benchmark itself is single-threaded.

## Method

* Each workload: one untimed warm-up, then 5 timed repetitions. Reported: min / median / max wall
  time, spread = (max − min) / median, and throughput = engine rows / median time.
* A timed repetition covers engine construction (the engine copies the event rows into its own
  buffer), the full replay and, for the adapter, tape finalisation. Dataset generation, loading
  and output hashing are outside the timed region.
* Strategy interval 100 ms, order latency 1 ms each way (`ConstantLatency`), fees 2 bp maker /
  4 bp taker, sizing 10 lots per order, 100 lots maximum position, 50 lots maximum order, at most
  8 open orders: the bb-app preset values. For the BTCUSDT dataset that is 0.01 BTC per order and
  0.1 BTC maximum position.

Workloads:

| Workload | What runs |
|---|---|
| raw engine (no orders) | Stock engine built with `L2AssetBuilder` exactly as in `crates/bb-sim/tests/engine_defects.rs` (`NoPartialFillExchange`, `RiskAdverseQueueModel`, `ConstantLatency`), `elapse(interval)` until end of data. **Baseline.** |
| adapter, idle strategy | `simulate` with the same execution models and a `Strategy` that never emits anything: harness + recording taps with no orders. |
| adapter, TouchMaker / AvellanedaStoikov | `simulate` with the bundled strategies, `PowerProb3 { n: 3 }` queue model, each exchange model. |
| raw engine, same request stream | The stock engine with `ProbQueueModel<PowerProbQueueFunc3(3)>` and `NoPartialFillExchange`, driven open-loop by a minimal loop that issues exactly the requests the adapter issued (read back from its tape: intents that passed risk checks, and cancels) at the same tick, calling `clear_inactive_orders` each tick. The engine's own cost for the identical order flow. |

The same-request-stream baseline is checked: in every case its engine-reported trade count equals
the adapter's fill count (6,273 / 299 / 305 / 70 below). It is only run for the no-partial model
because the stock `PartialFillExchange` aborts on an exact completion (pinned in
`engine_defects.rs`), which the adapter works around.

## Results (session 2)

Micro-benchmarks (best of 5 × 2,000,000 reads):

* Reading the queue estimate from `ProbQueueModel`'s `QueuePos` through its `Debug` string, as
  `taps.rs` does: **268.7 ns** per read. Reading `RiskAdverseQueueModel`'s `f64` by downcast:
  **1.59 ns** per read.
* `Instant::now()`: 22.1 ns.

### Synthetic, 600 s

`SyntheticConfig::default()` with `duration_ns` = 600 s (seed 7): 317,644 engine rows, 600.0 s of
exchange time, tick 0.1, lot 0.001. Generated in 55.2 ms (single measurement).

| workload | min ms | median ms | max ms | spread % | Mrows/s (median) | vs raw | ticks |
|---|---:|---:|---:|---:|---:|---:|---:|
| raw engine (no orders) | 22.6 | 22.8 | 22.9 | 1.5 | 13.95 | +0.0% | 6001 |
| adapter, idle strategy | 26.3 | 26.6 | 27.1 | 2.7 | 11.96 | +16.7% | 6000 |
| adapter, TouchMaker, no-partial | 105.1 | 106.7 | 108.3 | 3.0 | 2.98 | +368.7% | 6000 |
| adapter, TouchMaker, partial | 104.7 | 105.5 | 106.7 | 1.9 | 3.01 | +363.3% | 6000 |
| adapter, AvellanedaStoikov, no-partial | 79.2 | 80.7 | 81.2 | 2.5 | 3.94 | +254.4% | 6000 |
| adapter, AvellanedaStoikov, partial | 78.9 | 79.1 | 80.1 | 1.5 | 4.01 | +247.6% | 6000 |

| loaded workload | intents | orders sent | fills | tape events | tape Mevents/s (median) | queue probe reads | est. probe ms (share of median) |
|---|---:|---:|---:|---:|---:|---:|---:|
| TouchMaker, no-partial | 10358 | 10358 | 6273 | 126567 | 1.19 | 91434 | 24.6 (23.0%) |
| TouchMaker, partial | 10341 | 10341 | 6340 | 126879 | 1.20 | 91826 | 24.7 (23.4%) |
| AvellanedaStoikov, no-partial | 7965 | 7817 | 299 | 87440 | 1.08 | 41917 | 11.3 (14.0%) |
| AvellanedaStoikov, partial | 7941 | 7765 | 298 | 86931 | 1.10 | 41645 | 11.2 (14.1%) |

Same request stream through the raw engine:

| workload | raw min ms | raw median ms | raw max ms | raw trades | adapter fills | adapter median ms | adapter overhead | strategy on_tick ms | est. queue probe ms | rest of adapter ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| TouchMaker, no-partial | 33.9 | 34.0 | 34.3 | 6273 | 6273 | 106.7 | +213.5% | 11.2 | 24.6 | 36.9 |
| AvellanedaStoikov, no-partial | 32.0 | 32.4 | 33.5 | 299 | 299 | 80.7 | +149.4% | 17.6 | 11.3 | 19.4 |

Interval sweep, idle adapter vs raw engine (medians):

| interval ms | raw ticks | raw ms | idle ticks | idle ms | overhead ms | overhead % |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 60001 | 23.8 | 60000 | 32.0 | 8.1 | +34.1% |
| 100 | 6001 | 22.5 | 6000 | 26.6 | 4.1 | +18.4% |
| 1000 | 601 | 22.3 | 600 | 25.7 | 3.4 | +15.3% |

### Binance Futures BTCUSDT 2020-02-01, first 10 minutes (Tardis sample)

Dataset `binance-futures-btcusdt-2020-02-01-10m`: 201,970 engine rows (194,834 depth updates,
2,220 trades, one 2,000-level snapshot), 601.1 s of exchange time, tick 0.01, lot 0.001. Loaded
from the store with hash verification in 16.5 ms (single measurement).

| workload | min ms | median ms | max ms | spread % | Mrows/s (median) | vs raw | ticks |
|---|---:|---:|---:|---:|---:|---:|---:|
| raw engine (no orders) | 20.6 | 21.5 | 22.9 | 10.4 | 9.39 | +0.0% | 6013 |
| adapter, idle strategy | 23.2 | 23.7 | 24.8 | 6.6 | 8.53 | +10.0% | 6012 |
| adapter, TouchMaker, no-partial | 34.5 | 35.0 | 35.3 | 2.3 | 5.77 | +62.6% | 6012 |
| adapter, TouchMaker, partial | 35.1 | 35.9 | 35.9 | 2.4 | 5.63 | +66.8% | 6012 |
| adapter, AvellanedaStoikov, no-partial | 62.6 | 63.4 | 63.9 | 2.0 | 3.19 | +194.7% | 6012 |
| adapter, AvellanedaStoikov, partial | 65.0 | 65.1 | 65.9 | 1.4 | 3.10 | +202.7% | 6012 |

| loaded workload | intents | orders sent | fills | tape events | tape Mevents/s (median) | queue probe reads | est. probe ms (share of median) |
|---|---:|---:|---:|---:|---:|---:|---:|
| TouchMaker, no-partial | 1874 | 1860 | 305 | 18000 | 0.51 | 7869 | 2.1 (6.0%) |
| TouchMaker, partial | 1873 | 1859 | 311 | 18019 | 0.50 | 7888 | 2.1 (5.9%) |
| AvellanedaStoikov, no-partial | 7245 | 7168 | 70 | 64540 | 1.02 | 7522 | 2.0 (3.2%) |
| AvellanedaStoikov, partial | 7245 | 7168 | 70 | 64540 | 0.99 | 7522 | 2.0 (3.1%) |

Same request stream through the raw engine:

| workload | raw min ms | raw median ms | raw max ms | raw trades | adapter fills | adapter median ms | adapter overhead | strategy on_tick ms | est. queue probe ms | rest of adapter ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| TouchMaker, no-partial | 24.3 | 24.3 | 24.5 | 305 | 305 | 35.0 | +44.2% | 2.5 | 2.1 | 6.1 |
| AvellanedaStoikov, no-partial | 26.9 | 27.1 | 27.6 | 70 | 70 | 63.4 | +134.0% | 23.0 | 2.0 | 11.3 |

Interval sweep, idle adapter vs raw engine (medians):

| interval ms | raw ticks | raw ms | idle ticks | idle ms | overhead ms | overhead % |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 60121 | 21.9 | 60120 | 27.9 | 6.0 | +27.6% |
| 100 | 6013 | 21.7 | 6012 | 24.1 | 2.4 | +11.0% |
| 1000 | 602 | 21.7 | 601 | 23.6 | 1.9 | +8.6% |

### Ingestion (measured by the lead)

Building `binance-futures-btcusdt-2020-02-01-10m` from the two Tardis CSVs took **6.4 s** in a
release build, measured once by the lead (not by this benchmark). Both files are streamed; the
251,173,484-byte gzipped `incremental_book_L2` file (27,199,023 rows, as recorded in the dataset
manifest) is read to the end even though only the first 10 minutes are kept, alongside the
3,090,479-byte trades file.

## Adapter overhead and where it comes from

All percentages are median adapter time over median baseline time, minus one, from session 2.

**No orders (harness + taps only).** The idle adapter costs +16.7 % (synthetic, +3.8 ms) and
+10.0 % (BTCUSDT, +2.2 ms) over the raw engine at the 100 ms interval. The interval sweep splits
this into a per-tick and a per-row part:

* Per tick: between 1,000 ms and 10 ms the tick count grows by ~59,400 and the overhead by 4.7 ms
  (synthetic) / 4.1 ms (BTCUSDT): about **70–80 ns per strategy tick**. That is the per-tick view
  building in `lib.rs` (`clear_inactive_orders`, `MarketView` from the depth, walking and sorting
  the engine's order map, converting and clearing the last-trades buffer), the drawdown marking,
  and one `EquitySample` per tick (`sample_every_ticks = 1`).
* Per row: the overhead left at the 1,000 ms interval, 3.4 ms over 317,644 rows (synthetic) and
  1.9 ms over 201,970 rows (BTCUSDT), is about **9–11 ns per engine row**. Measured parts of it:
  `simulate`'s pre-scan for the last local timestamp, 0.43 ms (1.36 ns/row) and 0.22 ms
  (1.07 ns/row). The rest was not isolated; what runs per row is the step bracket in `procs.rs`
  (`RecExch::process` calls `Tape::begin_step`/`end_step` around every exchange-side event: two
  `RefCell` borrows and an empty-`Vec` take-and-sort in `tape.rs`, even when no order is resting)
  and the adapter's 256-entry last-trades buffer (the stock builder default keeps none), which
  copies every trade row the local side sees.

**With orders.** Against the raw engine running the identical request stream, the adapter costs:

| | synthetic TouchMaker | synthetic AvellanedaStoikov | BTCUSDT TouchMaker | BTCUSDT AvellanedaStoikov |
|---|---:|---:|---:|---:|
| adapter vs same-flow raw engine | +213.5 % | +149.4 % | +44.2 % | +134.0 % |
| same, excluding the strategy's own `on_tick` time | +180.9 % | +94.8 % | +33.7 % | +49.1 % |

The strategy's `on_tick` time is code a strategy would run under any harness, so the second row is
the better measure of the adapter itself. The difference over the raw engine breaks down as:

* **Debug-format queue probe (`taps.rs`).** `hftbacktest 0.9.4` keeps `QueuePos` fields private,
  so `QueuePosProbe` formats the struct with `{:?}` and parses the number back. One read costs
  268.7 ns against 1.59 ns for the `f64` downcast. `TapQueue` reads the estimate once per
  `new_order` and twice (before and after) per `trade`/`depth` callback, including callbacks whose
  update is then dropped by the per-order cap, so the read count in the tables is exact. Estimated
  cost (reads × micro-benchmark time): 24.6 ms of the TouchMaker synthetic run (23.0 % of its
  median, 34 % of its 72.7 ms overhead) and 2.0–2.1 ms (3–6 %) on BTCUSDT, where far fewer queue
  callbacks happen.
* **Strategy `on_tick`.** Measured in separate runs with a timing wrapper (not in the timed rows):
  11.2 / 17.6 ms synthetic, 2.5 / 23.0 ms BTCUSDT. AvellanedaStoikov on BTCUSDT spends 3.8 µs per
  tick. Both strategies format a human-readable reason string per intent
  (`avellaneda_stoikov.rs:174`, `touch_maker.rs:66`); that this formatting dominates is an
  inference, not measured. The wrapper reads the clock twice per tick (`Instant::now` then
  `elapsed`; 22.1 ns per read, about 0.3 ms per 6,000 ticks), so this column is slightly
  overstated and the "rest" column understated by that much.
* **Rest of the adapter** (adapter − raw − strategy − estimated probe): 36.9 / 19.4 ms synthetic,
  6.1 / 11.3 ms BTCUSDT. After removing the idle overhead measured above this is roughly 140–260 ns
  per tape event. It covers the per-step buffering in `tape.rs` (every event inside an engine step
  becomes a `Pending`, the step is sorted by order id, then each entry is committed: ledger
  updates, `FlightEvent` construction, a `BTreeMap` lookup per queue update for the cap), the
  per-tick view building in `lib.rs` with resting orders present, cloning each intent's reason
  `String` onto the tape, pre-trade risk checks, the latency and fee taps, and the local-response
  observer in `procs.rs`. These were not timed individually.

Throughput summary: the raw engine replays 13.95 M rows/s (synthetic) and 9.39 M rows/s
(BTCUSDT); the idle adapter 11.96 and 8.53 M rows/s; the loaded adapter 2.98–4.01 and
3.10–5.77 M rows/s while writing 0.50–1.20 M tape events/s.

### Observations on avoidable cost

These are observations with the evidence above; no library code was changed.

1. The Debug-format probe is the largest single identified cost under load: 268.7 ns per read,
   91,434 reads, ~24.6 ms of a 106.7 ms run. Two things inflate it: the "before" read in
   `TapQueue::trade`/`depth` repeats the previous "after" read whenever nothing else touched the
   order in between, and the probe runs twice even for depth updates the per-order cap then
   discards. Removing the formatting entirely requires reading `QueuePos` without `Debug`, which
   0.9.4's public API does not allow.
2. Every exchange-side engine step is bracketed (`begin_step`/`end_step`) even with no order
   resting; together with the last-trades buffer this is part of the ~9–11 ns/row idle cost
   (about +8–15 % of a raw replay at a 1 s interval).
3. With `sample_every_ticks = 1` an equity sample is built every tick; this is part of the
   ~70–80 ns/tick and grows with tick count (+28–34 % at a 10 ms interval with no orders).

## Determinism under load

Every loaded run, warm-up included (6 runs per configuration), was hashed with
`bb_core::hash::sha256_json(&(tape, fills, equity))`; the benchmark asserts that all hashes of a
configuration are identical. Session 2:

| dataset | configuration | sha256 (identical across 6 runs) |
|---|---|---|
| synthetic 600 s | TouchMaker, no-partial | `ba3748f74f0d2a73a19ba96a3c75951460c3befd81fadaa1e933db398f76836d` |
| synthetic 600 s | TouchMaker, partial | `56d8a1bc6cede5037d316ee0436f2261117ea3528a1c76f5f482907487c1e5bf` |
| synthetic 600 s | AvellanedaStoikov, no-partial | `67c053ead89c7209a13f2f8037358217b2a497c84f76eab6527098803030a6e4` |
| synthetic 600 s | AvellanedaStoikov, partial | `5981b1f1bc8bd3cfafd07dae5f3009013923c0984f398aa3cdb287798cd7e3e9` |
| BTCUSDT 10 min | TouchMaker, no-partial | `d2c8edb1d4bffd2f6edc4e3fa27a8369b156d63c7f831c550e34725a890534bd` |
| BTCUSDT 10 min | TouchMaker, partial | `3df73f2691ff9a089870894322f30547cc98619ff4b5f561cf208d701ef3b684` |
| BTCUSDT 10 min | AvellanedaStoikov, no-partial | `c27077b496c8fe41e81d4a10b1754ced8158e56e0d8edb96240976d2a70c6d38` |
| BTCUSDT 10 min | AvellanedaStoikov, partial | `c27077b496c8fe41e81d4a10b1754ced8158e56e0d8edb96240976d2a70c6d38` |

The four no-partial hashes are also identical to those from session 1, a separate process (so with
different `HashMap` seeds inside the engine). The partial-fill hashes differ between the sessions
because `bb-sim`'s partial-fill handling in `taps.rs` was changed between them; the hashes above
are for the code at the time of session 2. On BTCUSDT the AvellanedaStoikov outputs are the same
under both exchange models.

## Variance

* Within a session, spread (max − min over 5 runs, relative to the median) was 1.4–3.0 % for every
  loaded run, 1.5–2.7 % for raw/idle on the synthetic data and 6.6–10.4 % for raw/idle on BTCUSDT,
  where single runs are ~22 ms.
* Between sessions (no-partial and raw/idle rows, whose outputs did not change): raw medians
  22.8 → 22.8 ms (synthetic) and 21.2 → 21.5 ms (BTCUSDT); idle medians 26.1 → 26.6 ms and
  24.6 → 23.7 ms; loaded medians within 0.3–1.8 ms (e.g. BTCUSDT AvellanedaStoikov no-partial
  61.9 → 63.4 ms).
* In session 2 the idle adapter at 100 ms measured 26.6 ms in the main table and 26.6 ms in the
  sweep (synthetic) but 23.7 vs 24.1 ms on BTCUSDT, and the 1,000 ms sweep overhead was 3.4 ms in
  session 2 vs 2.0 ms in session 1 (synthetic). Differences of 1–2 ms between runs of ~25 ms are
  noise at this run length on this machine, so per-row and per-tick costs above are given as
  ranges.
