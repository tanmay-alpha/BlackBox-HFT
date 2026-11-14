# Experiment validation

How BlackBox checks its inputs against real market data without publishing that data. Hosted
data stays synthetic; everything below that touches exchange data runs on one machine only.

## Latency experiment (public demo) — independent recomputation

Measured 2026-11-14 on the local stack (Windows, DB `blackbox_explore`). **All data synthetic**
(`synthetic-demo`, generator seed 7, `events_sha256 0bc26426…`); all results simulated. Scripts
live outside the repo in `D:/bbwt-research/validation/`: `bbz.py` (own reader of the `.bbz` bundle:
concatenated gzip members, JSONL pages), `recompute.py` (metrics + comparison from raw orders,
fills, equity, tape), `timeline_check.py` (anonymous `/timeline` paging). Raw rows were read from the
stored bundles in `data/explore/artifacts/runs/` rather than paged over HTTP (anonymous egress budget).

```text
python recompute.py          # A = 253bbc23b342f373 (1 ms), B = d6a57aed972e18d0 (50 ms)
python timeline_check.py     # local API, Origin: http://localhost:5173
```

### Metrics: recomputed vs stored (`trailer.metrics` = `GET /runs/{id}` `.metrics`)

Recomputed with `math.fsum` for sums and nearest-rank quantiles (`sorted[ceil(q·n)−1]`), latencies
from tape `request_sent.entry_latency_ns` (≥ 0) / `exch_response.response_latency_ns`, round trip =
`first_response_ts − first request_sent(new).ts`.

| Metric | A recomputed | A stored | B recomputed | B stored | max abs residual |
|---|---|---|---|---|---|
| fills (maker / taker) | 1241 (1241/0) | 1241 (1241/0) | 745 (745/0) | 745 (745/0) | 0 |
| orders submitted | 2060 | 2060 | 1759 | 1759 | 0 |
| fees = Σ fill fee | 74.4610532 | 74.4610532 | 44.7006824 | 44.7006824 | 1.7e-13 |
| volume | 12.41 | 12.41 | 7.45 | 7.45 | 2.2e-13 |
| turnover | 372305.266 | 372305.266 | 223503.412 | 223503.412 | 1.0e-9 |
| fill ratio orders / qty | 0.602427 / 0.602427 | same | 0.423536 / 0.423536 | same | 0 / 2.3e-14 |
| realized / unrealized | −0.2485 / 0 | same | −0.136778 / 0.001278 | same | 0 |
| net P&L (last sample) | −74.709553 | −74.709553 | −44.836182 | −44.836182 | 0 |
| entry latency n, p50=p90=p99=max | 2835, 1 ms | same | 2891, 50 ms | same | 0 |
| response latency n, p50=p90=p99=max | 4076, 1 ms | same | 3636, 50 ms | same | 0 |
| round trip n, p50=p90=p99=max | 2060, 2 ms | same | 1759, 100 ms | same | 0 |

Residuals are summation-order only (stored sums are sequential f64; `fsum` is exact-rounded).
Tolerance used: `1e-9 × (1 + |x|)` for sums, exact for counts, quantiles and last-sample P&L.

P&L identity (`docs/PNL_ACCOUNTING.md`), every one of 1201 equity samples per run:

| Check | A | B |
|---|---|---|
| `equity − (realized + unrealized − fees)` max abs | 0 | 0 |
| `realized + unrealized − (cash + position × mid_tick × 0.1 × 1.0)` max abs | 5.6e-12 | 3.5e-12 |
| ledger `fees` (last sample) − Σ fill fees | −1.7e-13 | −6.4e-14 |
| `realized + unrealized − Σ fill fees` vs stored net | 1.7e-13 | 7e-14 |

### Comparison `01a1da55-7482-7c1c-a06b-e5104201d0df` (A vs B)

Orders aligned by `order_id` (decision order); timing fields `decision_ts, exch_arrival_ts,
first_response_ts, first_fill_ts, last_fill_ts, terminal_ts`; outcome fields side, price, qty, TIF,
outcome, fill count/qty, average fill tick, maker/taker qty, fees.

| | Recomputed | Stored (`interpretation`) |
|---|---|---|
| First divergence | order 0: same decision (t=0.100 s) and outcome (filled 0.01 @ 299999); arrival 0.101 s vs 0.150 s, first response 0.102 vs 0.200 s, fill 0.108869266 vs 0.165284155 s | tick 0, order 0, "same outcome, different timing", same timestamps |
| First outcome divergence | order 256: present in A only | tick 1, order 256, "decision only in A" |
| Fill / order counts Δ (B−A) | −496 fills, −301 submitted | −496, −301 |

### Replay invariants (`GET /runs/{id}/timeline`, cursor-paged)

| Run | Window (ns) | `max_events` | Pages | Σ page events = `events_total` | Raw tape count in window | seq strictly increasing | Dups across pages | ts in window | Clocks |
|---|---|---|---|---|---|---|---|---|---|
| A | 1e8 – 1.1e9 | 97 | 3 | 222 = 222 | 222 | yes | 0 | yes | local, exchange |
| A | 6e10 – 6.3e10 | 500 | 2 | 681 = 681 | 681 | yes | 0 | yes | local, exchange |
| B | 1e8 – 2.1e9 | 251 | 2 | 360 = 360 | 360 | yes | 0 | yes | local, exchange |
| B | 1.18e11 – 1.201e11 | 400 | 1 | 332 = 332 | 332 | yes | 0 | yes | local, exchange |

Every page also satisfied `next_cursor == null ⇔ truncated_by == null` and a constant `events_total`.

### Determinism

```text
blackbox --data-dir dd synth                          # same events_sha256 0bc26426…
blackbox --data-dir dd run --config cfgA_n.json       # config of A, wire strings → integers
```

| Source | Platform class | `result_sha256` |
|---|---|---|
| Stored A (local) | `windows-x86_64-gnu+libm-0352e099` | `57f055c52f8ad6b0…` |
| Re-run A, `target/release/blackbox.exe` (built before platform classes; records `windows-x86_64`) | windows-gnu, same toolchain | `57f055c52f8ad6b0…` (identical) |
| Hosted A (read-only GET, canonical) | `linux-x86_64+libm-53154a4e` | `e07028862afb5e2f…` |
| Stored B (local) / hosted B | `…0352e099` / `…53154a4e` | `dffbcf7dece22181…` / `8c2ab2e3644f440a…` |

Same run id (config + dataset identity) and identical counts, fees and net P&L on both platforms, but
the result hash differs from the canonical Linux hash. Both runs use the `power_prob3` queue model,
which calls the platform's `powf`; the windows-gnu math library differs from glibc in the last bit
for some inputs, which changes queue-position floats on the tape and therefore the hash
**[INFERENCE: the differing rows were not located; that needs the hosted tape, not downloaded]**.
This is the documented platform-class policy ([EXPERIMENT_REPRODUCIBILITY.md](../EXPERIMENT_REPRODUCIBILITY.md#platform-policy)):
results are bit-reproducible within a class, not across classes.

### What this does NOT validate

- Realism: no empirical calibration of latency, queue position or fill probability; the data is a
  synthetic generator, not a market.
- Market impact: replay only; simulated orders never move prices.
- The engine itself (hftbacktest 0.9.4 matching): only that BlackBox's stored metrics, ledger and
  comparison are consistent with its own raw records.
- Cross-platform bit equality (shown above not to hold across libm classes), the other demo pairs,
  markouts, spread capture, drawdown and order-age metrics (not recomputed here).

## ITCH reconstruction

M7 workflow: decode a NASDAQ TotalView-ITCH 5.0 file, rebuild the order-by-order (L3) book of
every stock, aggregate selected stocks into the BlackBox Level-2 event model and write a
manifest that says exactly what was read, what was applied, what was quarantined and why.
Code: `crates/bb-data/src/itch/` (`decode.rs`, `book.rs`, `l2.rs`, `mod.rs`, `tests.rs`); CLI:
`blackbox itch-reconstruct`.

It is a module of `bb-data`, not a new crate: it reuses that crate's engine event type, `L2Book`,
quality `Finding`/`QualityCheck` vocabulary, file hashing and error type, and it adds no
dependency (only `proptest` as a dev-dependency, already used by `bb-sim` and `bb-analytics`).

### Licensing and provenance

- NASDAQ's sample files (`emi.nasdaq.com/ITCH/Nasdaq ITCH/`) come with no licence or
  redistribution grant ([OSS_LICENSES_AND_PROVENANCE.md](OSS_LICENSES_AND_PROVENANCE.md),
  [REUSE_MATRIX.md](REUSE_MATRIX.md)). They are used **locally only**: never committed, bundled,
  uploaded or served, and neither is anything derived from them event by event (books, rows,
  run tapes). Every manifest carries `license_note: "Local use only. …"`.
- The decoder is written from the public specification *Nasdaq TotalView-ITCH 5.0*
  (nasdaqtrader.com, revision log ending 2023-04-28). Each message length and offset in
  `decode.rs` cites its section (§1.1 to §1.8). itch-rs and itch-book were reference reading
  only; no code was copied.
- Test fixtures are bytes built in `tests.rs` from the specification's layouts. No NASDAQ byte is
  in the repository.
- This is not an exchange certification. Agreement with another reconstruction (for example a
  vendor's L3 product) has not been measured.

### Decoder

- Input framing: each message behind a 2-byte big-endian length (the `*.NASDAQ_ITCH50` file
  format). `FrameReader` returns a frame as a slice of the read buffer when it is fully buffered
  (always, for in-memory input) and assembles it only when it straddles a buffer refill. Gzip
  input is detected from its first two bytes and decompressed on the fly.
- Interpreted: S, R, H, A, F, E, C, X, D, U, P, Q, B. The other types the specification defines
  (Y, L, V, W, K, J, h, I, N, O) are length-checked and counted, not interpreted; type bytes the
  specification does not define are counted separately.
- Zero copy: symbols, attributions and reason codes borrow from the frame; integers are read
  big endian. Prices stay `Price4(u32)` integers in 1/10,000 USD; timestamps are the 48-bit
  nanoseconds since midnight; tracking number and stock locate are kept.
- A frame whose length or a coded field (buy/sell indicator, printable flag) contradicts the
  specification is skipped and counted (`itch.invalid_message`); the length prefix keeps framing
  intact. An input ending inside a frame stops decoding (`itch.framing`, stop reason
  `truncated`), including a gzip stream cut short.

### L3 book invariants

Enforced before any message changes the book (`book.rs`); a violation quarantines the message
(counted per reason, first five examples kept, nothing applied, nothing invented):

| Check | Quarantined when |
|---|---|
| `book.unknown_order` | E, C, X, D or U names an order that is not live (a replace of an unknown order adds no new order: its side would have to be guessed) |
| `book.locate_mismatch` | the order is live on another stock locate |
| `book.exceeds_remaining` | executed or cancelled shares exceed the order's remaining shares |
| `book.zero_shares` | an add, execution, cancel or replace for zero shares |
| `book.duplicate_order_ref` | an add or replace reuses a live reference number |
| `book.invalid_price` | an add or replace priced 0 or above 200,000.0000 (the spec's maximum) |
| `book.stock_mismatch` | an add's symbol differs from the stock directory entry of its locate |

- An order is removed when its shares reach zero (spec §1.4). A replace removes the original and
  adds the new reference on the same side at the back of the queue.
- P and Q are prints, not book changes: P executes non-displayed interest; Q is a cross's bulk
  print, whose resting orders were already executed by (non-printable) C messages. B is counted
  and its match number checked against the prints seen (`trades.broken_unknown_match`).
- Crossed (bid > ask) and locked (bid = ask) books are judged per stock after each batch of
  messages sharing one timestamp, counted per episode with their total duration, and reported
  (`book.crossed`, `book.locked`), never repaired. Each episode names the stock's Trading Action
  state when its batch ended, and the manifest counts the episodes that began while the state was
  `T`: a halted, paused or not-yet-released stock does not match continuously, so a crossed book
  there is expected, while one under `T` is a finding about the data or the reconstruction.
- Timestamps must not decrease from one message to the next in the file (`time.regression`) and
  must stay below 24 h (`time.beyond_day`). The book follows file order regardless.
- `OrderBooks::check_invariants` walks every level and verifies its totals against its linked
  orders; the tests call it after every message.

**Observed versus modeled queue.** The book keeps each price's displayed orders in ITCH arrival
order. `OrderBooks::observed_queue(order_ref)` returns the live orders ahead of an order and their
shares. That is an observation of the displayed queue only (hidden and reserve interest never
appears in ITCH) and is a different quantity from the simulator's modeled `front`
([../QUEUE_MODELS.md](../QUEUE_MODELS.md)), which estimates queue position from Level-2 data. The
two are separate types and nothing feeds one into the other; comparing them is the validation
this enables.

### Normalization to Level 2

`l2.rs` turns the book changes of the requested symbols into engine rows
([../MARKET_DATA.md](../MARKET_DATA.md#the-engines-event-format)). The rules are stored verbatim in
every manifest (`normalization.decisions`):

- the book starts empty at the first message, so no snapshot rows; a file not starting with
  System Event `O` is flagged (`itch.start_of_messages`);
- one depth row per price level a message changed, with the new total displayed shares (0 removes
  the level); a replace at an unchanged price emits one row;
- an execution of a displayed order (E, printable C) emits a trade row before its depth row, with
  the aggressor side opposite to the resting order; non-printable C changes the book without a
  trade row (its shares are in the cross print); P and Q emit trade rows without a side (P's
  indicator is always `B` since 2014-07-14); zero-share crosses emit nothing;
- prices become `px = price / 10,000` (f64) only here; tick size 0.0001, lot size 1 share;
- `exch_ts = midnight_epoch_ns + ITCH timestamp`; ITCH files carry no date, so the caller supplies
  the session's midnight (default 0, raw nanoseconds since midnight Eastern);
- **local clock absent**: rows carry `EXCH_EVENT` only and `local_ts` is a copy of `exch_ts`, not
  an arrival time (`normalization.local_clock = "absent"`);
- a row whose message timestamp is below its stock's previous row is emitted at that previous
  timestamp and counted (`l2.timestamp_held_back`).

The output is not a stored `DatasetManifest`: that would need an explicit, recorded feed-latency
assumption (`LocalTsProvenance::Assumed`) and is left to a later step.

### Manifest

`ItchManifest` (serde JSON, `--out`) records: source file name, compression, size and SHA-256 of
every stored byte (the file is read to the end even when a message limit stops decoding);
the licence note; options; stop reason; frames and decompressed bytes consumed; counts per
message type, invalid frames per type and unknown types; first/last timestamp and regressions;
system events; book counters (adds, replaces, deletes, executions, prints, crosses, broken
trades, crossed/locked episodes, how many began while trading, and their durations, live orders at
the end, quarantined per reason); the per-symbol row counts; the normalization decisions; and 18
checks in the existing `QualityCheck` shape. The manifest has no wall-clock field, so one input
and one set of options always give the same bytes (tested).

### Tests

`cargo test -p bb-data itch` (28 tests in `crates/bb-data/src/itch/tests.rs`):

| Fixture | Test |
|---|---|
| every interpreted type, field by field (48-bit timestamp, 8-byte cross shares, F attribution) | `decoder_reads_every_field_of_each_interpreted_type` |
| wrong length, bad side, bad printable flag, empty frame | `decoder_rejects_wrong_lengths_and_coded_fields` |
| frames straddling buffer refills (capacities 1 to 4096) | `frame_reader_reassembles_frames_split_across_buffer_refills` |
| add | `add_order_rests_on_the_book_and_emits_one_depth_row` |
| partial cancel | `partial_cancel_reduces_shares_and_keeps_priority` |
| delete | `delete_removes_the_order_and_its_empty_level` |
| replace (same price, new price) | `replace_is_delete_plus_add_on_the_same_side_at_the_back_of_the_queue` |
| execute partial and full | `partial_then_full_execution_prints_before_the_depth_change` |
| execute with price, printable and not | `execution_with_price_prints_at_its_own_price_and_non_printable_is_not_a_trade_row` |
| empty book and empty input | `empty_book_answers_every_query_with_nothing` |
| crossed and locked state, one-batch transient cross, trading state at the time | `crossed_and_locked_books_are_reported_not_repaired`, `crossed_state_still_open_at_the_end_is_closed_at_the_last_timestamp`, `crossed_reports_name_the_trading_state_in_force_when_the_batch_ended` |
| unknown reference, wrong locate | `unknown_references_are_quarantined_and_never_invented` |
| oversized/zero quantities, bad prices, duplicate refs, symbol mismatch | `quantities_and_references_breaking_invariants_are_quarantined` |
| invalid and unknown frames | `invalid_and_unknown_frames_are_skipped_counted_and_framing_continues` |
| truncated message, stray byte | `truncated_input_stops_decoding_and_is_reported` |
| truncated gzip stream | `truncated_gzip_stream_is_reported_as_truncated` |
| out-of-order timestamp | `out_of_order_timestamp_is_reported_and_held_back_in_level2_rows` |
| cross trade, zero-share cross, non-cross trade | `cross_trade_is_a_print_not_a_book_change` |
| broken trade | `broken_trades_are_counted_without_touching_the_book` |
| observed queue ahead | `observed_queue_ahead_follows_itch_arrival_order` |
| message limit; file not starting with `O` | `message_limit_stops_early`, `input_not_starting_with_start_of_messages_is_flagged` |
| file hashing (plain and gzip), deterministic manifest | `file_manifest_hashes_the_whole_stored_file_plain_and_gzip` |
| decode-only scan | `decode_only_scan_counts_the_same_frames_as_a_reconstruction` |
| what `OrderBooks::apply` reports per message kind | `apply_reports_what_each_message_did` |

Property test `random_valid_sequences_keep_l2_equal_to_the_live_l3_orders` (256 cases, up to
249 operations each): random valid add, cancel, execute, execute-with-price, delete and replace
sequences, with crossed books allowed. After every message it requires no quarantine, the
structural invariants, L3 level totals equal to an independent model's sum of live orders per
level, and the emitted Level-2 rows, replayed through the existing `book::L2Book`, equal to the
same sums on both sides; at the end the rows must pass `validate_event_order`.

### Local run on a NASDAQ sample

Run on 2026-11-14 on one workstation. Nothing from it is in the repository: the inputs and the
manifests stay under `D:/bbwt-research/itch-data/`, outside the worktree, and only the aggregate
counts below are reported (no symbol, price or time of an individual event).

**Input.** NASDAQ publishes gzip files of whole days, 3.5 to 18 GB each, above the 1 GB download
limit set for this experiment, so no whole file was downloaded. One HTTP range request fetched
the first 125,829,120 bytes (120 MiB, 91 s) of `12302019.NASDAQ_ITCH50.gz` (2019-12-30;
3,524,013,057 bytes in the directory listing). The prefix cannot be checked against NASDAQ's
checksum of the whole file (its `.md5sum` link returned 404).

| Local file | Bytes | SHA-256 |
|---|---|---|
| gzip prefix as fetched | 125,829,120 | `e1623c7c62cc993c3e264bc78eb7e4603fbf234e9c1d75ba70b27a9f308e34eb` |
| `gzip -dc` of the prefix (stops with "unexpected end of file") | 332,473,876 | `e8e9b745b28b713497183453e80ae9626244ad9c57a26aafa0a36213b012a247` |
| its first 5,000,000 messages (`head -c 149652744`, the `decoded_bytes` of a 5,000,000-message run) | 149,652,744 | `2130728066574d2e12152b903ce198764105439991c76b32a1927e1c8315a388` |

**Commands** (`blackbox` built with `cargo build --release -p bb-app`):

```
blackbox itch-reconstruct first5M.raw --decode-only
blackbox itch-reconstruct first5M.raw
blackbox itch-reconstruct first5M.raw --symbol AAPL --symbol QQQ --symbol MSFT --out first5M.manifest.json
blackbox itch-reconstruct 12302019.NASDAQ_ITCH50.gz.prefix-120MiB --max-messages 5000000 \
    --symbol AAPL --symbol QQQ --symbol MSFT
```

**Throughput.** Five runs each after one warm-up read (file in the OS cache); median with min and
max. The CLI times from after argument parsing to the end of the work, which includes reading and
SHA-256 hashing the whole input file; one thread. Machine: Intel Core i7-14650HX (16 cores, 24
threads), 16 GB RAM, Windows 11, Rust 1.99.1 `x86_64-pc-windows-gnu`, `release` profile
(opt-level 3, thin LTO, 4 codegen units). Other agents' builds may have been running on the same
machine.

| Run (5,000,000 messages) | Wall time | Messages/s | Decompressed MB/s |
|---|---|---|---|
| decode only (`--decode-only`) | 0.161 s (0.156–0.162) | 31.1 M (30.9–32.0 M) | 931 |
| decode + L3 books of all 8,884 stocks | 0.516 s (0.510–0.523) | 9.68 M (9.56–9.81 M) | 290 |
| decode + L3 books + Level-2 rows for AAPL, QQQ, MSFT | 0.541 s (0.534–0.552) | 9.25 M (9.06–9.36 M) | 277 |
| the same from the gzip prefix (decompressed on the fly, the rest of the 120 MiB hashed) | 0.749 s (0.738–0.758) | 6.67 M (6.60–6.78 M) | 200 |

**Quality report of the 5,000,000-message run.** The manifest from the gzip prefix with
`--max-messages 5000000` is identical to the one from the cut raw file apart from `source`,
`options` and `stop`.

- Window: System Event `O` at 03:04:32.058 ET, then `S` at 04:00:00.000, last message at
  09:23:35.668 (before the opening cross); 0 timestamp regressions, 0 beyond 24 h.
- Messages: A 1,971,443; D 1,861,525; X 503,012; U 257,236; L 215,087; F 135,231; E 24,571;
  R 8,906; H 8,901; Y 8,897; P 5,185; K 3; S 2; V 1 (K, L, V, Y counted, not interpreted).
  0 invalid frames, 0 unknown types.
- Book: 8,906 directory entries, 8,884 stocks with orders; 2,106,674 adds, 257,236 replaces,
  1,861,525 deletes, 24,571 executions (8,331,642 shares; 13,283 completed their order); 5,185
  non-cross trades (1,668,553 shares); 231,866 orders live at the end.
- **Quarantined: 0** in all seven categories (no unknown reference, no over-execution, no
  duplicate reference, no locate or symbol mismatch, no invalid price or zero quantity).
- `book.crossed` (warn): 2 episodes, both in stocks whose last Trading Action state was `H`
  (halted), 0 while trading; both lasted to the end of the window (7,356 s together).
  `book.locked`: 0.
- Level-2 rows: AAPL 7,159 (6,741 depth, 418 trades), MSFT 4,445 (4,214, 231), QQQ 120,308
  (119,989, 319); no timestamp held back. Worst severity: warn, from `book.crossed` only.

**The whole decompressed prefix** (one run, same machine): 9,766,139 messages up to 09:31:50.305
ET, through the opening (System Event `Q` at 09:30:00.000). 0 quarantined; 8,906 cross trades
(19,896,150 shares; 5,820 crosses with zero shares), 872 non-printable executions, 1,073,085 NOII
messages counted; 6 crossed and 1 locked episodes, every one under trading state `H`, 0 while
trading; 1,627,978 orders live at the end. The input ends inside a frame (12 of 38 bytes), which
is reported as `itch.framing` (error) with stop reason `truncated`, as intended for a cut file.
1.526 s, 6.40 M messages/s.

### Limitations

- Not an exchange certification. Agreement with an independent L3 reconstruction (a vendor's
  XNAS.ITCH product or itch-book run locally) has not been measured; zero quarantines and no
  crossed book while trading are consistency evidence, not proof of correctness.
- Only a prefix of one day was run: 03:04 to 09:31 ET on 2019-12-30. No closing cross, broken
  trade or mid-day halt was in the input, so those paths are covered by fixtures only.
- Throughput is one machine, one thread, warm cache, a shared machine; memory was not measured.
- The Level-2 output is not a simulator dataset: ITCH has no receipt clock, and turning it into a
  `DatasetManifest` needs a recorded feed-latency assumption. ITCH files carry no date, so
  `--midnight-epoch-ns` must come from the caller (daylight saving time is not computed).
- Hidden and reserve interest is invisible in ITCH; the observed queue is the displayed queue.
  MPID attribution is decoded but not kept in the book.
- Crossed and locked books are judged per stock after each same-timestamp batch; a state that
  exists only inside one batch is not reported, by design.
