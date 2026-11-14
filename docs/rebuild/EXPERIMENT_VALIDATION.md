# Experiment validation

How BlackBox checks its inputs against real market data without publishing that data. Hosted
data stays synthetic; everything below that touches exchange data runs on one machine only.

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
