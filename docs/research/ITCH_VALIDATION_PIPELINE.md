# ITCH validation pipeline

One command validates a NASDAQ TotalView-ITCH 5.0 file end to end: framing, decoding, timestamps,
book reconstruction by three implementations, an exact three-way comparison, a data-quality
report, L3 touch metrics, and a reproducibility manifest. It turns the one-off cross-check of
[REAL_DATA_VALIDATION_PLAN.md](REAL_DATA_VALIDATION_PLAN.md) §3 into committed, rerunnable code.

```sh
scripts/research/itch-validate.sh ITCH_FILE SYMBOL WINDOW OUT_DIR
```

- `ITCH_FILE`: raw BinaryFILE (2-byte big-endian length per message) or its gzip. A cut gzip
  prefix is accepted: gzip exits 1 ("unexpected end of file"), which is recorded.
- `SYMBOL`: Stock Directory symbol (`QQQ`).
- `WINDOW`: exchange time `HH:MM:SS-HH:MM:SS`, or `T0_NS-T1_NS` in nanoseconds since midnight.
  Numeric grammar is `\d+-\d+` (Python decimal digits, no signs or whitespace), with **any**
  digit count on either side: `0-1000000000` is accepted. Clock components have exactly two
  digits; either seconds component may end in `.` followed by 1–9 digits. Both forms require
  `T0 < T1`; messages with `T0 <= ts < T1` are compared and measured.
- `OUT_DIR`: receives event-level files derived from NASDAQ data. Inside this repository the
  script refuses any directory outside `data/` (git-ignored).

Environment: `PYTHON`, `CXX` (a C++23 compiler; MinGW-w64 GCC 16.2 here), `ITCH_BOOK_DIR`
(default `data/itch/tools/itch-book`), `CARGO_TARGET_DIR` (default `target/`), `SAMPLE_EVERY`
(level snapshot every N messages, default 1,000,000), `SAMPLE_N` (sampled events, default
1,000), `MIDNIGHT_EPOCH_NS` (recorded and passed to the reconstruction; it does not change any
compared value), `KEEP_EVENT_FILES` (1 keeps `ref.txt`, `bb.txt`, `py.txt` and the decompressed
input; 0 deletes them after hashing). Exit status: 0 when every gating check passed, 1 when one
failed, 2 on usage or tool errors.

On Windows run it with Git Bash (`C:/Git/usr/bin/bash.exe`), not WSL.

## 1. Sources

`research/itch-xcheck/` (sources only; no data, binaries or outputs):

| File | Role |
|---|---|
| `itchbook_adapter.cpp` | Drives [groovg/itch-book](https://github.com/groovg/itch-book)'s `parse` and `BookManager` unchanged and writes the record format below. |
| `bbadapter/src/main.rs` | The same records from BlackBox's `bb_data::itch::decode` and `OrderBooks`, unchanged. |
| `bbadapter/src/bin/bbreport.rs` | One pass: `Reconstructor` manifest, `QualityReporter` report and `TouchMetrics` for the symbol; aggregates only (`report.json`). |
| `py_ref.py` | One-symbol Python book written from the specification's field offsets (R, A, F, E, C, X, D, U). |
| `activity.py` | Independent Python framer: frame count, bytes consumed, trailing bytes, per-type counts. |
| `compare.py` | Record-by-record exact comparison, fixed-seed sample, mismatch report. |
| `perturb.py` | Negative control: one share added to one record. |
| `manifest.py` | Hashes every output and writes `manifest.json`. |
| `NOTICE` | Licences and pinned commits of itch-book and fixed-decimal. |

Third-party code is fetched, never vendored. itch-book is cloned at
`b502b002abf650760b79c010b3ae3a8f82896ae8` (MIT, "Copyright (c) 2026 groovg"); the script stops
if the checkout is at another commit or modified. Its only dependency, fixed-decimal, is the
archive its `CMakeLists.txt` pins (`a25e15801e32977df9abbea6a1819eecc9befaa9`, SHA-256
`aedd636c8ba9874f7fdeda5b7e7d08f4a6a38c9cb0cc330153865bfe7202d5fa`, MIT, same author), checked
before extraction. `bbadapter` is its own Cargo workspace with a committed `Cargo.lock`, built
`--locked`.

## 2. Stages

Each stage's command line (paths shown as `$OUT` and `$REPO`), exit status and wall time go to
`stages.jsonl` and then into the manifest.

| # | Stage | Command | Output |
|---|---|---|---|
| 0 | tools | clone/verify itch-book; fetch/verify fixed-decimal; `$CXX -std=c++23 -O2 -static`; `cargo build --release --locked` | `bin/` |
| 1 | input | `gzip -dc` when the input is gzip (exit 0 or 1 accepted) | `input.raw`, `gzip.stderr` |
| 2 | framing | `python activity.py RAW` | `activity.json` |
| 3 | decode, timestamps, sequence, quality, metrics | `bbreport RAW SYMBOL T0 T1 report.json MIDNIGHT` | `report.json` |
| 4 | reconstruction | `itchbook_adapter RAW ref.txt 0 T0 T1 EVERY SYMBOL`; `bbadapter` (same arguments) `bb.txt`; `python py_ref.py RAW SYMBOL T0 T1 py.txt pystats.json` | `ref.txt`, `bb.txt`, `py.txt` (event-level) |
| 5 | comparison | `compare.py ref.txt bb.txt` (all records); `compare.py py.txt bb.txt --only E` | `compare-*.json`, `mismatches-*.json` |
| 6 | negative control | `perturb.py ref.txt ref-perturbed.txt`; `compare.py ref-perturbed.txt bb.txt` | `negative-control.json` |
| 7 | manifest | `manifest.py OUT` | `manifest.json` |

**Records** (identical text from both adapters):

- `E seq ts locate bid_px bid_sz ask_px ask_sz | b1_px b1_sz b1_n … | a1_px … |`: BBO and top five
  levels per side after every A/F/E/C/X/D/U of the symbol inside the window (`seq` is the
  1-based message number in the file);
- `L seq locate side price shares orders`: the symbol's full depth every `SAMPLE_EVERY` messages,
  and every stock's full depth at the end;
- `T seq ts locate type side price shares match cross`: every disseminated trade of every stock;
- `C type count` and `Z frames bytes live_orders`.

**Gating checks** (`manifest.checks`; all must hold for exit 0):

| Check | Holds when |
|---|---|
| `framing_agrees` | The Python framer, `bbreport`, itch-book and `bbadapter` read the same number of frames and bytes. |
| `no_invalid_or_unknown_frames` | The decoder found no frame contradicting the specification and no undefined type. |
| `itchbook_equals_blackbox` | Every record of `ref.txt` equals the record at the same position of `bb.txt`. |
| `python_equals_blackbox` | Every `E` record of `py.txt` equals the corresponding `E` record of `bb.txt`, and there is at least one. |
| `negative_control_exactly_one_mismatch` | The perturbed comparison reports exactly one unequal record. |
| `episodes_agree_with_book` | The quality report's crossed/locked entries equal `OrderBooks`' own counts. |

**Mismatch report.** When a comparison is not 100%, `compare-*.json` holds
`unequal_records` and `unequal_by_first_field` (`<kind>.<field>` → count, aggregates), and
`mismatches-*.json` lists the first 100 unequal pairs as

```json
{"record": 812345, "kind": "E", "message_seq": "17000123",
 "first_differing_field": "bid_shares", "field_index": 5,
 "reference_value": "301", "blackbox_value": "300",
 "reference_record": "E 17000123 …", "blackbox_record": "E 17000123 …"}
```

(The values above illustrate the format only.) Field names are `kind seq ts_ns locate
bid_price4 bid_shares ask_price4 ask_shares` then `bid1_price4 … ask5_orders` for `E`; `kind seq
ts_ns locate source side price4 shares match cross` for `T`; `kind seq locate side price4 shares
orders` for `L`. `mismatches-*.json` is event-level and stays in `OUT_DIR`.

## 3. Manifest schema (`blackbox.itch-validate.manifest/1`)

| Key | Content |
|---|---|
| `license_note` | The local-use-only note of `bb_data::itch::LICENSE_NOTE`. |
| `input` | `name`, `bytes`, `sha256` of the file as given; `compression`; `gzip_exit_status` and `gzip_message`; `decompressed_bytes` and `decompressed_sha256`. |
| `truncation` | `decompressed_bytes`, `complete_frames`, `bytes_in_complete_frames`, `trailing_partial_frame_bytes`, BlackBox `stop` (`reason`, `detail`), `last_timestamp_ns`. |
| `parameters` | `symbol`, `window`, `window_ns`, `clock`, `sample_every_messages`, `sample_events`, `sample_seed` (20261117), `midnight_epoch_ns`, `markout_horizons_ns`, `ofi_bucket_ns`. |
| `tools` | `blackbox_commit`, `blackbox_uncommitted_paths` (count under `crates/`, `research/`, `scripts/`), `itch_book` and `fixed_decimal` (URL, commit, licence, archive hash), and the first version line of `rustc`, `cargo`, the C++ compiler, Python, gzip and git. |
| `platform` | `uname -srm` and Python's `platform.platform()`. |
| `stages` | Per stage: `stage`, `argv`, `exit`, `seconds`. |
| `outputs` | Every file of `OUT_DIR` but `manifest.json` (binaries included): `bytes`, `sha256`. |
| `results` | `frames` and `bytes_in_complete_frames` per implementation; `messages_by_type`; `timestamps`; `system_events`; `book` (the reconstruction summary); `itchbook_vs_blackbox`, `python_vs_blackbox`, `negative_control` (counts, rates, `unequal_by_first_field`, sample); `itchbook_stats` (its own missing/duplicate/rejected/clamped counters); `bbadapter_stats`; `python_reference_stats`; `window_aggregates`; `quality` (§4); `metrics` (§5). |
| `checks`, `passed` | The gating checks above. |
| `informational` | `timestamp_regressions`, failed quality-report and reconstruction check ids (the reconstruction's `itch.framing` fails by design on a cut prefix). |

## 4. Data-quality report (`bb_data::itch::report`)

`QualityReporter` runs on the same frames as the reconstruction with its own `OrderBooks`. Every
item is a count; each has a unit test on hand-built ITCH bytes in
`crates/bb-data/src/itch/tests.rs` (`report_*`).

| Item | Definition | Test |
|---|---|---|
| `episodes` | Entries into a crossed (bid > ask) or locked (bid = ask) book, judged per stock after each batch of applied messages sharing one timestamp, exactly as `OrderBooks` judges them; keyed `crossed\|locked/<trading state>/<phase>`. Trading state: the stock's last Stock Trading Action when the batch is judged (`none` before any). Phase: `pre_market` before System Event `Q`, `regular` until `M`, `post_market` after. `total_ns` is exchange time in the state; episodes open at the end close at the last timestamp (`open_at_end`). | `report_episodes_carry_trading_state_and_market_phase`, `report_locked_episode_duration_matches_the_book` |
| `episodes_agree_with_book` | The entries above sum to `OrderBooks::crossed()` / `locked()` counts. | same |
| `missing_ref_by_type` | E, C, X, D, U naming a reference that is not live. | `report_counts_missing_references_per_type` |
| `locate_mismatch_by_type` | Modify messages naming a live order of another locate. | (quarantine path shared with the book tests) |
| `duplicate_live_ref_by_type` | A, F, U whose new reference is live. A replace by its own reference is applied by the book and counted in `replace.new_ref_equals_original`. | `report_counts_duplicate_and_non_increasing_references` |
| `new_ref_not_above_previous_max` | Informational: new references not above every earlier one (references are day-unique, not required to increase). | same |
| `zero_shares_by_type` | A, F, E, C, X, U, P for zero shares, read from the message itself (so another quarantine reason cannot hide it). Zero-share crosses (Q) are legitimate and counted in `zero_share_crosses`. | `report_flags_zero_quantities_and_out_of_range_prices` |
| `price_out_of_range_by_type` | Price 0 or above 200,000.0000 on A, F, U, C, P and Q with shares. | same |
| `exceeds_remaining_by_type` | Executions or cancels larger than the order's remaining shares. | same |
| `replace` | Order Replace semantics: applied; same price / improved (higher bid, lower ask) / worsened; shares up / down / unchanged; same-price size decreases (which keep priority on venues that preserve it; ITCH issues a new reference and all three implementations put it at the back). | `report_classifies_replace_semantics_by_side` |
| `timestamps` | Consecutive-message ties and their fraction; ties among one stock's consecutive book messages; regressions. | `report_counts_timestamp_ties_and_regressions` |
| `sequence` | `supported: false`: BinaryFILE has no sequence field (MoldUDP64 sequence numbers are not in the file; the tracking number is not a sequence), so gaps cannot be detected. Checked instead: byte-identical consecutive frames, messages naming a locate before its Stock Directory entry. | `report_states_that_binaryfile_has_no_sequence_field_and_checks_what_it_can` |
| `session` | System events; Stock Trading Action transitions over all stocks (`from->to` counts) and repeats; every transition of the selected symbol with timestamp and reason. | `report_lists_session_and_trading_state_transitions` |
| `checks` | `frames.invalid`, `time.regression`, `sequence.identical_consecutive_frames`, `sequence.locate_without_directory`, `refs.missing`, `refs.locate_mismatch`, `refs.duplicate_live`, `values.zero_shares`, `values.price_out_of_range`, `values.exceeds_remaining`, `book.crossed_or_locked_while_trading`; passed when the count is 0. | `report_runs_as_a_frame_sink_and_honours_the_message_limit` |

## 5. Metrics (`bb_analytics::l3::TouchMetrics`)

Input: for every accepted A, F, E, C, X, D, U of the symbol, in file order, the touch before and
after the message, its kind, the executed resting side (from the print), its shares, and the
shares left at the pre-message best prices (from the book's level changes). Messages before the
window set the book; messages after it resolve markouts; only messages with `T0 <= ts < T1` are
measured. Prices are ITCH Price(4) integers (1/10,000 USD; the $0.01 tick is 100); timestamps are
exchange-clock nanoseconds as `i64`; spreads, mids (as `2 × mid = bid + ask`), OFI and markout
numerators are exact integers, converted to `f64` only for ratios and means. A touch is
*uncrossed* when both sides exist and bid < ask; one-sided, locked and crossed touches make every
price metric undefined, and the report counts them instead of filling a value. Tests are in
`crates/bb-analytics/src/l3.rs` (`touch_tests`), all with hand-computed values.

| Metric | Formula | Units | Edge cases | Interpretation |
|---|---|---|---|---|
| Relative spread | `20000 (a − b)/(a + b)` after each measured message; event mean, nearest-rank p10/p50/p90, and time-weighted mean over `[T0, min(T1, observed_until))` | bps | not uncrossed: counted in `one_sided` / `locked_or_crossed` / `undefined_ns` | Cost of a round trip at the touch relative to price. |
| Quoted spread | `a − b` | Price(4) | as above | Same in ticks (100 = $0.01). |
| Microprice | `(b Qa + a Qb)/(Qa + Qb)`, exact fraction; reported as offset from mid `(a − b)(Qb − Qa)/(2(Qa + Qb))`; counts above / at / below mid | Price(4) | as above | Where the touch sizes "lean": positive when the bid queue is larger. |
| Order-flow imbalance | Cont, Kukanov and Stoikov (2014) per message: `1{Pb′ ≥ Pb} qb′ − 1{Pb′ ≤ Pb} qb − 1{Pa′ ≤ Pa} qa′ + 1{Pa′ ≥ Pa} qa`; summed per 1 s exchange-time bucket; OLS with intercept of the bucket's summed mid change on its OFI | shares; slope Price(4)/share | message skipped unless touch before and after are uncrossed; regression null with < 2 buckets or no OFI variance | Net pressure at the touch; the slope is the contemporaneous price impact per share. |
| Activity rates | messages and shares per kind (add, execute, partial_cancel, delete, replace); messages per second over the span; `(X + D)/E` | count, 1/s | null rate for a non-positive span; null ratio without executions | How busy the book is and how much of it is cancellation. |
| Touch depletion / replenishment | per message and side, shares lost or gained at the pre-message best price, by kind | shares | a better new level is not replenishment of the old one; empty pre-message side contributes nothing | What empties the touch (QUEUE_MODELS: cancellations vs executions). |
| Markouts | per execution: `s (2mid(t+h) − 2mid(t−))/2`, `s = +1` if the resting order was a sell (aggressor bought); `mid(t+h)` is the touch after every message stamped ≤ `t + h`; h = 100 ms, 1 s, 5 s; mean, mean in bps of `mid(t−)`, share-weighted mean, signs | Price(4), bps | undefined mid before the execution or at the horizon: counted; `t + h` beyond the last timestamp of the input: censored | Short-horizon price move in the aggressor's direction: adverse selection of the resting side. |

The OFI regression and markouts describe one window of one day; they are not fitted parameters.
Every number is computed on exchange time; there is no receipt clock.

Coverage includes the initially empty book from `T0` until the first symbol event, clipped to
the observed span: an empty book over `[0,200)` has `undefined_ns=200` (formerly 0). A first event
at 50 makes the initial 50 ns undefined; defined-time spread means still divide only by defined
time. Activity rates already use the whole span, not the time since the first event; microprice,
OFI, flows and markouts are event-based and need no analogous time-coverage change.
`TouchMetrics` is used by `bbreport`, not by the synthetic workbench endpoint or execution study
generator. No `web/public/study-results/**` number, execution golden/hash or workbench metric
changes. Only `relative_spread_bps.undefined_ns` in a newly generated ITCH touch report can
increase, by the initial empty-book interval inside the observed window.

**Not added to the synthetic workbench endpoint.** The workbench `Observation` carries the best
prices but not the touch sizes, so microprice, OFI and touch-level markouts cannot be computed
from it without changing `bb_data::l3workbench` and the API; that is outside this change.

## 6. Reproduced results (2019-07-30 prefix, aggregates only)

Commands (Git Bash, 2026-11-18, run from a worktree of this repository with the data in the main
checkout's `data/itch/`; one run per symbol, each from an empty `OUT_DIR`):

```sh
export CXX=C:/Users/TANMAY/.blackbox/mingw64/bin/g++.exe ITCH_BOOK_DIR=D:/bbwt-research/itch-book \
       MIDNIGHT_EPOCH_NS=1564459200000000000 CARGO_PROFILE_RELEASE_DEBUG=0
for s in QQQ AAPL; do
  scripts/research/itch-validate.sh "D:/black box/data/itch/07302019.NASDAQ_ITCH50.gz.prefix-300MB" \
      $s 09:30:00-09:39:00 "D:/black box/data/itch/validate/$s"
done
```

Both runs exited 0 with all six gating checks passing. Toolchain recorded in the manifests:
rustc 1.99.1, cargo 1.99.1, MinGW-W64 GCC 16.2.0, Python 3.11.9, gzip 1.14.

**Input and truncation** (identical in both manifests):

| Item | Value |
|---|---|
| Input | `07302019.NASDAQ_ITCH50.gz.prefix-300MB`, 300,000,000 bytes, SHA-256 `0083093f…aa9ef1` |
| gzip | exit 1, "unexpected end of file" (the cut) |
| Decompressed | 750,516,796 bytes, SHA-256 `b310cfbb…a9248` |
| Complete frames | 23,483,868 (Python framer, `bbreport`, itch-book, `bbadapter` all equal) |
| Bytes in complete frames | 750,516,774; 22 trailing bytes of a 33-byte frame |
| Last timestamp | 34,763,453,483,808 ns (09:39:23.453 ET) |
| Invalid / unknown frames, timestamp regressions | 0 / 0, 0 |

These equal the values in REAL_DATA_VALIDATION_PLAN §2 and §3.

**Three-way comparison** (window 09:30:00–09:39:00 ET):

| | QQQ | AAPL |
|---|---:|---:|
| `E` records (book events) | 143,760 | 63,964 |
| itch-book = BlackBox, all records | 1,369,568 / 1,369,568 | 1,315,788 / 1,315,788 |
| … of which `T` / `L` / `C` / `Z` | 442,271 / 783,520 / 16 / 1 | 442,271 / 809,536 / 16 / 1 |
| Python = BlackBox, `E` records | 143,760 / 143,760 | 63,964 / 63,964 |
| Sampled events (seed 20261117), BBO + depth 5 equal | 1,000 / 1,000 | 1,000 / 1,000 |
| Negative control: unequal records | 1 (`E.bid_shares`) | 1 (`E.bid_shares`) |
| itch-book's missing ref / duplicate ref / rejected / clamped | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 |
| Live orders at the end | 1,746,202 | 1,746,202 |

The documented 100% figures reproduce exactly, and the harness still detects a one-share
difference.

**Quality report** (whole file, every stock; identical in both runs):

| Item | Value |
|---|---|
| Crossed episodes | 4: 1 `H`/pre-market (79.67 s); 3 `H`/regular, all still open at the end (1,686.41 s together) |
| Locked episodes | 6, all `H`/pre-market (3,127.75 s together) |
| … while trading (`T`) | 0; entries agree with `OrderBooks` |
| Missing refs, locate mismatches, duplicate live refs, exceeds remaining | 0, 0, 0, 0 |
| Zero-share messages, prices out of range | 0, 0 (5,915 zero-share crosses, legitimate) |
| New references not above the previous maximum (informational) | 9,608,330 |
| Replaces | 1,911,396, all applied; price same 25,365 / improved 1,319,195 / worsened 566,836; shares up 30,382 / down 47,638 / unchanged 1,833,376; same-price size decreases 16,761; self-replaces 0 |
| Timestamp ties, consecutive messages | 982,900 of 23,483,867 (4.19%) |
| Timestamp ties, consecutive book messages of one stock | 925,425 of 22,072,083 (4.19%) |
| Sequence | not supported by BinaryFILE; identical consecutive frames 0; locates before their directory entry 0 |
| Trading-state transitions | none→T 8,844, none→H 5, T→H 2; repeats 0. QQQ none→T at 03:07:21.974, AAPL none→T at 03:07:21.215 |

**Metrics** (window 09:30:00–09:39:00, span 540 s, exchange clock):

| Metric | QQQ | AAPL |
|---|---:|---:|
| Measured book messages | 143,760 | 63,964 |
| add / delete / replace / execute / partial cancel, per s | 130.16 / 116.79 / 15.49 / 3.65 / 0.13 | 78.98 / 26.02 / 7.18 / 6.17 / 0.11 |
| Cancel-to-execution ratio, (X + D)/(E + C) | 32.07 | 4.24 |
| Quoted spread p10 / p50 / p90, Price(4) | 100 / 100 / 200 | 100 / 300 / 700 |
| Relative spread p50, event mean, time-weighted mean (bps) | 0.517, 0.660, 0.659 | 1.435, 1.780, 1.603 |
| One-sided / locked or crossed states | 0 / 0 | 0 / 0 |
| Microprice − mid: mean, mean absolute (Price(4)) | −0.65, 27.38 | −6.65, 76.62 |
| OFI: messages used, 1 s buckets, total (shares) | 143,760, 540, −801,572 | 63,964, 540, 31,912 |
| OFI regression slope (Price(4) per share), correlation | 0.0170, 0.925 | 0.2001, 0.815 |
| Touch depletion: delete / execute / replace / partial cancel (shares) | 14,991,840 / 348,519 / 100,045 / 7,500 | 330,336 / 290,364 / 13,022 / 100 |
| Touch replenishment: add / replace (shares) | 15,638,090 / 514,058 | 326,334 / 12,747 |
| Executions (E + C) with markouts | 1,969 | 3,331 |
| Markout mean, 100 ms / 1 s / 5 s (Price(4)) | 63.6 / 76.8 / 96.9 | 107.6 / 156.1 / 141.3 |
| Markout mean (bps) | 0.329 / 0.397 / 0.501 | 0.514 / 0.746 / 0.675 |
| Markout signs +/0/− at 1 s | 1,173 / 272 / 524 | 2,170 / 281 / 880 |
| Undefined or censored markouts | 0 | 0 |

Cross-checks against the independent Python reference (`pystats.json`): the message counts per
kind equal its A + F, E + C, X, D and U counts (QQQ 70,199 + 89, 1,871 + 98, 72, 63,066, 8,365),
and touch depletion by delete, execution and partial cancel equals its
`touch_shares_removed_by_cause` exactly. Depletion by replace is lower (QQQ 100,045 against
101,955; AAPL 13,022 against 13,322) because the Python reference counts the whole replaced order
leaving the touch, while the metric nets a replace that re-adds shares at the same best price in
the same message.

Reading the metrics: QQQ's touch empties almost entirely by deletion (97.0% of depleted
shares), AAPL's about evenly by deletion and execution (52.1% / 45.8%), which matches §3 of the
plan. Markouts are measured from the midprice *before* the execution, so they include the
execution's own mechanical effect when it empties the level (a QQQ half-tick is 50 Price(4));
they are adverse-selection measures for the resting side over one opening window, not fitted
parameters.

## 7. Limitations

- The comparison shows agreement of three implementations, not certification; shared
  interpretations (replace goes to the back, file order is time order within a nanosecond) are
  untested (REAL_DATA_VALIDATION_PLAN §3).
- One day, the first 9 min 23 s of regular trading; metrics are descriptive of that window.
- `py_ref.py` reads the whole decompressed file into memory (about 0.75 GB here).
