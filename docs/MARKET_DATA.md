# Market data

What BlackBox replays, how it gets there, what is checked, and what the data can and cannot say.
Licences and vendor terms are quoted in [OPEN_SOURCE_RESEARCH.md](OPEN_SOURCE_RESEARCH.md); this
page applies them.

## Sources

| Source | `DatasetKind` | How it is produced | Local timestamps | Redistributable |
|---|---|---|---|---|
| BlackBox synthetic market | `synthetic` | `blackbox synth` (`crates/bb-data/src/synthetic.rs`) | generated (`LocalTsProvenance::Synthetic`) | yes |
| Tardis.dev CSV (`trades` + `incremental_book_L2`) | `recorded` | user downloads (`scripts/fetch-tardis-sample.sh`), then `blackbox ingest-tardis` (`crates/bb-data/src/tardis.rs`) | measured at the vendor's capture host (`Measured`) | **no** |

Both pipelines produce the same artifact: an engine event array plus a `DatasetManifest`
(`crates/bb-core/src/dataset.rs`), after the same normalization and quality checks. Binance's public
archive was evaluated and not adopted: it has no incremental L2 depth stream
([OPEN_SOURCE_RESEARCH.md](OPEN_SOURCE_RESEARCH.md#9-binance-public-data)).

The level of every dataset is `DataLevel::L2MbpWithTrades`: quantity per price level plus trades.
Individual orders are never visible, which is why queue position has to be modeled
([QUEUE_MODELS.md](QUEUE_MODELS.md)).

## The engine's event format

hftbacktest replays rows of eight fields: `ev`, `exch_ts`, `local_ts`, `px`, `qty`, `order_id`,
`ival`, `fval`. `ev` holds the event kind (depth, trade, depth clear, snapshot), the side, and two
flags: `EXCH_EVENT` (the exchange side processes the row at `exch_ts`) and `LOCAL_EVENT` (the
strategy host receives it at `local_ts`). Timestamps are absolute epoch nanoseconds. Depth rows carry
the new absolute quantity at a price, not a delta. The engine requires rows flagged `EXCH_EVENT` to be
non-decreasing in `exch_ts` and rows flagged `LOCAL_EVENT` to be non-decreasing in `local_ts`.

On disk a dataset is `data/datasets/<id>/events.npz` (a zip with one `data.npy` in the engine's own
structured-array format, readable by py-hftbacktest) next to `dataset.json`. The SHA-256 of
`events.npz` is the dataset hash every run manifest records; loading refuses a file whose bytes no
longer match it (`crates/bb-data/src/store.rs`).

## Tardis schema and units

Lead-verified from the sample files ([OPEN_SOURCE_RESEARCH.md](OPEN_SOURCE_RESEARCH.md#8-tardisdev)):

| File | Columns |
|---|---|
| `trades` | `exchange,symbol,timestamp,local_timestamp,id,side,price,amount` |
| `incremental_book_L2` | `exchange,symbol,timestamp,local_timestamp,is_snapshot,side,price,amount` |

- `timestamp` is the exchange-provided time and `local_timestamp` the vendor's receipt time, both
  in **microseconds** since the Unix epoch, UTC. Conversion multiplies by 1,000.
- Book `amount` is the level's new absolute quantity; `0` removes the level.
- Trade `side` is the aggressor.
- `is_snapshot = true` rows form a snapshot batch; a snapshot following non-snapshot rows replaces
  the book.

How BlackBox reads them (`crates/bb-data/src/tardis.rs`, `crates/bb-core/src/time.rs`):

- The header must match the schema exactly; a wrong header is rejected by name. Every row is
  parsed; an unreadable row is a hard error naming the file and line. Nothing is skipped or repaired
  silently.
- **Units are never guessed.** A timestamp converted with the declared unit must land between
  2010-01-01 and 2100-01-01 (`TimeUnit::to_plausible_ns`). A value in the wrong unit is off by a
  factor of at least 1,000 and fails this check (test
  `tardis::tests::declaring_the_wrong_time_unit_is_an_error`).
- Prices and sizes are compared with the tick and lot grid as **exact decimals** of the source
  text (`crates/bb-data/src/decimal.rs`), not as floats.
- `.csv.gz` input is hashed as the compressed bytes; each input's name, size, SHA-256 and row count
  are recorded in the manifest.

### Which exchange field is `timestamp`

The Tardis documentation does not say. Tardis's open-source normalizer stamps Binance Futures
trades with the transaction time `T` and depth updates with the message event time `E`; that the
CSV export does the same is an inference ([OPEN_SOURCE_RESEARCH.md](OPEN_SOURCE_RESEARCH.md#which-binance-futures-field-is-the-exchange-timestamp)).
The ingested manifest's `exch_ts_semantics` string describes the column as `E` for Binance Futures;
by the normalizer evidence that holds for depth rows, while trade rows would carry `T`. The
practical consequence is the trade/book misalignment measured below.

## Normalization

`crates/bb-data/src/normalize.rs` is a Rust port of `correct_local_timestamp`,
`correct_event_order` and `validate_event_order` from py-hftbacktest
(`py-hftbacktest/hftbacktest/data/validation.py`, MIT); the Tardis row conversion follows
`py-hftbacktest/hftbacktest/data/utils/tardis.py` with snapshot mode `process`.

1. **Conversion.** Snapshot rows become one depth-clear row per side followed by the snapshot levels;
   updates become depth rows. All trade rows are placed before all depth rows, so a trade precedes
   the depth update it caused when both share an exchange timestamp. This ordering is required, not
   cosmetic: a probability queue model that saw the depth decrease first would count the traded
   quantity twice ([QUEUE_MODELS.md](QUEUE_MODELS.md#trades-and-depth-at-the-same-timestamp)).
2. **Window.** `--max-duration-s` keeps events up to that many seconds after the first book row.
   A window always starts at the beginning of the file, because the book must be built from its
   initial snapshot.
3. **Local-timestamp correction.** With `m = min(local_ts - exch_ts)` over all rows, if `m < 0`
   every `local_ts` is shifted by `base_latency - m` (reference behaviour). BlackBox extends this so
   that the smallest feed latency afterwards is at least 1 ns, because the engine's
   `FeedLatencyAdjustment` rejects `local_ts <= exch_ts`. The shift is recorded as
   `timestamps.local_ts_shift_ns`; it is a single constant, never a per-row repair.
4. **Dual ordering.** Rows are stably sorted once by `exch_ts` and once by `local_ts` and merged. A
   row at the head of both orders is emitted once with both flags; otherwise it is emitted twice,
   once per clock, each at the position its clock requires. Ties keep source order.
5. **Validation.** Exchange-flagged rows must be ordered by `exch_ts` and local-flagged rows by
   `local_ts` (check `events.order_valid`).

## Quality checks

`crates/bb-data/src/quality.rs` runs the same checks for both pipelines. Row-level facts only the
source knows (line numbers, exact decimals, trade ids) are collected during conversion; everything
else is measured in one pass over the normalized array, replaying the book on the exchange clock.
**Nothing is repaired**, except that exact duplicate trades are dropped and counted. A failed check
records its count, a detail sentence and up to five examples. `blackbox quality <dataset>` prints
them; `QualityReport::worst()` is the dataset's headline severity.

| Check id | Severity when it fails | What it detects |
|---|---|---|
| `time.exch_out_of_order.trades` / `.book` | warn | source rows out of exchange-time order |
| `time.local_before_exch` | warn | a row received before its exchange time (clock skew; triggers the constant shift) |
| `time.rows_split_for_ordering` | info | rows whose local order differs from exchange order, stored once per clock |
| `trades.duplicate_id` | warn; **error** if an id repeats with different content | repeated trade ids (exact duplicates are dropped) |
| `trades.id_gap` | warn | non-consecutive trade ids (missing trades are never invented) |
| `trades.missing_side` | info | trades without an aggressor side (no side is inferred) |
| `depth.duplicate_row` | info | a book row identical to the previous one |
| `depth.initial_snapshot` | warn | book data that does not begin with a snapshot |
| `depth.mid_file_snapshot` | warn | snapshots after the start (a vendor re-snapshot after reconnect: a possible capture gap) |
| `depth.crossed_book` | warn; **error** if crossed for more than 1 s in total | best bid >= best ask after a complete exchange-time batch |
| `depth.event_gap` | warn | gaps over 5 s between distinct exchange timestamps (quiet market or missing data; indistinguishable) |
| `precision.price_off_tick` | error | a source price that is not a whole multiple of the tick size |
| `precision.qty_off_lot` | error | a source quantity that is not a whole multiple of the lot size |
| `values.non_positive_price_or_negative_qty` | error | impossible values |
| `trades.outside_book` | warn | trades more than one tick outside the reconstructed best bid and ask at their exchange time |
| `events.order_valid` | error | the final array violates the engine's dual-ordering requirement |

Trade-id checks run only for sources with trade ids (the synthetic generator has none, so it
reports 15 checks to Tardis's 17). The report also carries four distributions: feed latency
(`local_ts - exch_ts`), spread in ticks at every top-of-book change, inter-event gap, and the
fraction of exchange time during which the book was two-sided and uncrossed.

The book is reconstructed only from `EXCH_EVENT` rows, and only the state after a complete
exchange-time batch is ever reported, because states in between are artifacts of row order that
never existed at the exchange (`crates/bb-data/src/book.rs`).

## Synthetic generator

`crates/bb-data/src/synthetic.rs` keeps an integer book (ticks and lots) and emits what a venue
would publish: an initial snapshot, then market trades, cancellations and limit additions. All
randomness comes from named `Rng` streams of the seed (`crates/bb-core/src/rng.rs`: SplitMix64
seeding, xoshiro256**), and all state is integer, so a configuration produces the same bytes on
every platform.

Guarantees at every row on the exchange clock (tested in `synthetic::tests`): never crossed or
locked; at least the minimum number of levels per side; whole lots and positive whole ticks; every
trade followed by the depth row that reduces the level it hit, at the same exchange timestamp; feed
latency strictly positive.

The bundled `synthetic-demo` dataset uses `SyntheticConfig::default()`: seed 7, 120 s, tick 0.1,
lot 0.001, 20 levels, mean event gap 2.5 ms, trade probability 0.18, cancel probability 0.38, feed
latency 2 ms plus uniform jitter in [0, 1.5 ms] (from its `dataset.json` `generator` field, which
records the exact configuration so it can be regenerated). `blackbox quality synthetic-demo`
reports 62,929 rows (8,348 trades, 49,251 depth updates, 40 snapshot levels), all checks passing
except the informational `time.rows_split_for_ordering` (5,289 rows), feed latency 2.000 to
3.500 ms, spread p50 1 tick, book valid fraction 1.0000.

Synthetic data says nothing about any real market. It exists so the platform, its tests and its
public demo run without vendor files.

## Provenance fields

Every `dataset.json` records (`DatasetManifest`, `crates/bb-core/src/dataset.rs`):

| Field | Meaning |
|---|---|
| `kind`, `level` | synthetic or recorded; the data level, never overstated |
| `source.vendor`, `exchange`, `symbol` | where it came from |
| `source.inputs[]` | every input file: name, role, bytes, SHA-256 of the bytes as read, rows |
| `source.license_note` | redistribution note shown with the dataset |
| `source.exch_ts_semantics` | what the exchange timestamp means for this source |
| `timestamps.input_unit` | unit the source declared or was told to use |
| `timestamps.local_ts` | `measured`, `synthetic` or `assumed`: decides whether feed latency may be called measured anywhere in the UI or explanations |
| `timestamps.local_ts_shift_ns`, `note` | the constant shift, if any, and the window that was kept |
| `instrument` | tick size, lot size, contract size |
| `origin_ns`, `start`, `end` | the epoch origin (decimal string) and the exchange-clock span |
| `counts` | rows, trades, depth updates, snapshot levels, snapshots, depth clears |
| `events_sha256`, `events_bytes` | the dataset hash |
| `generator` | the exact synthetic configuration, or `null` |
| `quality` | the full quality report |

## Redistribution rules

- `data/` is git-ignored. Nothing recorded from a vendor, and nothing converted from it, is
  committed, attached to a release or served to anyone else. A converted `.npz` is "merely
  reformatted" data under the Tardis terms and is treated exactly like the raw CSV.
- Run tapes and replay windows embed book levels and trades from their dataset, so runs over
  recorded data stay local too.
- Test fixtures and the bundled demo use synthetic data only.
- A hosted BlackBox shows synthetic runs only (Tardis Terms of Service clause 9.2(2) forbids
  offering others a backtesting or charting service over the data).
- Aggregate statistics such as the measurements below are the kind of derived data the terms
  allow in research reports (clause 9.3); no event-level data is reproduced in this repository.

## Measured findings on the real sample

`binance-futures-btcusdt-2020-02-01-10m`: Binance Futures BTCUSDT, 2020-02-01, the first 600 s
after the first book row, built from the free Tardis sample with
`--tick-size 0.01 --lot-size 0.001 --max-duration-s 600`. All numbers below are from
`blackbox quality binance-futures-btcusdt-2020-02-01-10m` and its `dataset.json`, read on
2026-11-14.

| Fact | Value |
|---|---|
| Engine rows | 201,970 (2,220 trades, 194,834 depth updates, 2,000 snapshot levels in 1 snapshot) |
| Exchange-clock span | 601.085 s |
| Source rows read | trades file 229,564 rows (3,090,479 bytes gzipped); book file 27,199,023 rows (251,173,484 bytes); 227,344 trade rows and 27,002,189 book rows lay outside the window |
| Local-timestamp shift | 0 (no row was received before its exchange time) |
| Rows split for dual ordering | 2,914 (info) |
| Trades outside the reconstructed book | **659 of 2,212** checked trades more than one tick outside best bid/ask at their exchange time (8 more fell in one-sided or crossed moments) — warn |
| Crossed or locked book | 0 batches |
| Long gaps (> 5 s) | 0 |
| Off-grid prices or sizes | 0 |
| Feed latency (`local_ts - exch_ts`) | n = 199,056; min 116.218 ms, p50 128.911 ms, p90 136.607 ms, p99 910.771 ms, max 2,406.020 ms, mean 155.527 ms |
| Spread | n = 10,909; min 1, p50 16, p90 101, p99 225, max 436 ticks; mean 36.858 ticks |
| Inter-event gap | min 1 ms, p50 3 ms, p99 52 ms, max 565 ms |
| Book valid fraction | 0.9982 |
| Worst severity | warn |

How to read them:

- **Trades outside the book.** The first examples are all at `t = +1.227 s`, with trades printing
  up to 10 ticks above a best ask of 936631. Trades stamped earlier than the depth updates they
  belong to (transaction time versus message event time, above) would produce exactly this; missing
  book updates would too. The data cannot tell which. Fills and markouts on this dataset inherit
  the effect.
- **Feed latency** was measured at the vendor's London capture host in early 2020, not where a
  strategy would run ([LATENCY_MODEL.md](LATENCY_MODEL.md#feed-latency)). That the 116 ms floor is
  mostly network distance is an inference; the p99 near 0.9 s is consistent with Tardis's note that
  collection before 2020-05-14 suffered latency spikes.
- **Spread** is often much wider than one tick (median 16 ticks of 0.01). A touch-joining quote
  therefore rests at a price that the opposite side may be far from, and the p99 of 225 ticks
  exceeds the 100-tick sweep window of the engine's market orders
  ([EXECUTION_MODEL.md](EXECUTION_MODEL.md#5-market-orders)).
