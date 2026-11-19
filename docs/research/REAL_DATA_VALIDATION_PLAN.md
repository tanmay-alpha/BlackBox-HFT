# Real-data validation slice (tracker H1–H3)

Status 2026-11-17, local workstation only. In this document H1 is data rights and a local sample,
H2 is decoding and reconstruction on that sample with the existing tools, and H3 is an
independent cross-check plus the plan for execution-model calibration. Every figure below is an
aggregate (a count, rate or percentile). No event-level row derived from NASDAQ data appears here
or anywhere in the repository. Inputs, per-event outputs and manifests stay under
`data/itch/`, which `.gitignore` excludes (`/data/`).

## 1. Data rights (H1)

| Source | Read on | Finding |
|---|---|---|
| <https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/> (IIS directory listing, HTTP 200) | 2026-11-17 | Only a file listing. It has no licence, terms, README or notice text. `/` and `/ITCH/` are also bare listings. `README.txt` and `robots.txt` return 404. |
| <https://www.nasdaqtrader.com/TraderNews.aspx?id=nva2008-091> (Data News #2008-91, 2008-11-24) | 2026-11-17 | The only Nasdaq statement of purpose found for the EMI sample files. It concerns the 2008 ITCH 4.0/3.1 samples, not today's 5.0 files: "NASDAQ OMX has recorded one day worth of data in the new TotalView-ITCH data formats for customers to use for internal testing purposes." and "The sample data files are now available on NASDAQ OMX public FTP server located at ftp://emi.nasdaq.com/ITCH." |
| <https://www.nasdaqtrader.com/Trader.aspx?id=CopyDisclaimMain> | 2026-11-17 | Website terms for nasdaqtrader.com content. It is not clear that they cover the EMI host. They allow "one temporary copy in a single computer's memory and one unaltered permanent copy to be used by the viewer for personal and non-commercial use only", and otherwise forbid copying, distribution, sublicensing or alteration "without the prior written consent of Nasdaq". |
| <https://www.nasdaqtrader.com/content/technicalSupport/specifications/dataproducts/ITCHFTP.pdf> | 2026-11-17 | The *paid* Historical TotalView-ITCH product is delivered over an authenticated SFTP server with `SMMDDYY-v50.txt.gz` names. Several files on EMI use the same naming (`S121225-v50.txt.gz` and others), so their status as "samples" is not stated anywhere. |
| Nasdaq *US Equities and Options Data Policies* v2.6 (already quoted in [../rebuild/OSS_LICENSES_AND_PROVENANCE.md](../rebuild/OSS_LICENSES_AND_PROVENANCE.md)) | 2026-11-14 | Defines exchange Information to include anything from which it "can be identified, recalculated or re-engineered". |

**Verdict.** No licence grants rights in the EMI files. The only stated purpose is internal
testing, and the website terms permit personal, non-commercial copies at most. This slice
therefore treats the files as follows:

- **local, private, non-commercial validation only**;
- no redistribution, hosting, bundling or upload of the file or of anything derived from it
  event by event (books, L2 rows, tapes, per-event comparison files);
- only aggregates go into documentation.

This matches the rule already in force
([../rebuild/EXPERIMENT_VALIDATION.md](../rebuild/EXPERIMENT_VALIDATION.md#licensing-and-provenance)).
It is a conservative reading, not legal advice. Any use beyond it, such as a hosted demo or a
shared dataset, needs written permission from Nasdaq or a licensed historical feed.

## 2. Download facts

- **Listing (2026-11-17).** 2019–2020 `MMDDYYYY.NASDAQ_ITCH50.gz` days of 3.52–5.60 GB;
  `S*-v50.txt.gz` days of 3.95–17.89 GB, the newest named `S061226` (2026-06-12); two `itch50_05_*.gz` files
  of 13.0 and 16.2 GB; `FEB 2022 Files/` (empty); `NOII/`. Several listed `.md5sum` and `.done`
  files return 404, including `07302019.NASDAQ_ITCH50.gz.md5sum`, so no prefix can be checked
  against a published checksum.
- **Range support.** `HEAD` returns `Accept-Ranges: bytes`, `Content-Length: 3662140094` and
  `Last-Modified: Wed, 31 Jul 2019 04:15:40 GMT` for `07302019.NASDAQ_ITCH50.gz`.
  `curl -r 0-1048575` returns `206 Partial Content` with
  `Content-Range: bytes 0-1048575/3662140094`. That megabyte decompresses to a stream starting
  with a 12-byte System Event `O` frame.
- **File choice.** `07302019.NASDAQ_ITCH50.gz` (session 2019-07-30). It is a different day from
  the 2019-12-30 prefix used on 2026-11-14 (see the correction below), and one of the smallest
  files, so a fixed byte budget reaches furthest into the regular session.
- **Correction (2026-11-18): no 2019-12-30 data is in `data/itch/`.** The only 2019-12-30 file
  there is `terms/md5_1230.txt`, which is a saved HTML page, not a checksum and not data. The
  2026-11-14 prefix lives outside the project data directory, at
  `D:/bbwt-research/itch-data/12302019.NASDAQ_ITCH50.gz.prefix-120MiB` (125,829,120 bytes,
  SHA-256 `e1623c7c62cc993c3e264bc78eb7e4603fbf234e9c1d75ba70b27a9f308e34eb`, measured
  2026-11-18). No earlier record of that hash exists, so its provenance rests on that
  measurement alone.
- **Fetch.** One request, `curl -r 0-299999999`, from 11:22:29Z to 11:45:48Z. Result: HTTP 206,
  300,000,000 bytes (8.19% of the file) in 1,399 s at about 214 KB/s.

| Local file (`data/itch/`) | Bytes | SHA-256 |
|---|---:|---|
| `07302019.NASDAQ_ITCH50.gz.prefix-300MB` | 300,000,000 | `0083093fcab8d91b5da94e270032f5c801db4fff84447befdfd54974fdaa9ef1` |
| `07302019.prefix.raw` (`gzip -dc`, exit 1: "unexpected end of file") | 750,516,796 | `b310cfbb905adf81b42518c0584d3de50b95f67784007fffc3d48b0146ba9248` |

**The prefix decodes as a gzip stream prefix.** gzip 1.14 decompresses it to the cut point and
reports only the truncation. The BlackBox decoder consumes 23,483,868 complete frames
(750,516,774 bytes) and stops at the partial final frame (22 of its bytes are present). It
reports stop reason `truncated` (a reconstruction also fails the `itch.framing` check), with no invalid frame. An
independent Python framer (`xcheck/activity.py`) gives the same frame count, the same byte
offset and the same 22 trailing bytes.

## 3. What was validated (H2, H3)

### Build and commands

`blackbox.exe` was older than the last change to `crates/bb-data/src/itch/book.rs`, so it was
rebuilt. The tree was HEAD `af3a284` plus other agents' uncommitted edits in `bb-app`
(`report.rs`, `study.rs`) and `bb-core` (`compare.rs`, `config.rs`); `crates/bb-data` was
unmodified. The binary was copied to `data/itch/blackbox.exe` (SHA-256 `ddae6a8e…1f53`). Toolchain:
Rust 1.99.1 `x86_64-pc-windows-gnu`, MinGW GCC 16.2.0, Python 3.11.9.

```sh
CARGO_PROFILE_RELEASE_DEBUG=0 cargo build --release -p bb-app
cd data/itch
./blackbox.exe itch-reconstruct 07302019.NASDAQ_ITCH50.gz.prefix-300MB --decode-only
./blackbox.exe itch-reconstruct 07302019.NASDAQ_ITCH50.gz.prefix-300MB --max-messages 20000000 \
    --symbol QQQ --symbol AAPL --midnight-epoch-ns 1564459200000000000 --out manifest-20M.json
./blackbox.exe itch-reconstruct 07302019.NASDAQ_ITCH50.gz.prefix-300MB \
    --symbol QQQ --symbol AAPL --midnight-epoch-ns 1564459200000000000 --out manifest-full.json
python xcheck/activity.py 07302019.prefix.raw > activity.json
```

### Decode and reconstruction (whole complete prefix)

- **Window.** System Event `O` at 03:02:50.642 ET, `S` at 04:00:00.000, `Q` (market open) at
  09:30:00.000. The last complete message is at 09:39:23.453, so the prefix covers 9 min 23 s of
  regular trading. Timestamp regressions: 0.
- **Messages (23,483,868).**

  | Type | Count | Type | Count |
  |---|---:|---|---:|
  | A | 9,707,394 | P | 89,749 |
  | D | 8,540,138 | X | 751,287 |
  | U | 1,911,396 | E | 337,250 |
  | I | 1,067,081 | L | 210,669 |
  | F | 825,964 | Y | 8,893 |
  | H | 8,851 | R | 8,849 |
  | Q | 8,849 | C | 7,494 |
  | S | 3 | V | 1 |

  0 invalid frames and 0 unknown types. L, V, Y and I are counted, not interpreted.
- **Symbols.** 8,849 directory entries, of which 8,840 have orders. The most active symbols in
  regular hours, by A/F/E/C/X/D/U count, are QQQ (147,707), TQQQ, SPY, IWM, AMD, UDOW, MU, TVIX,
  URTY and AAPL (65,279).
- **Book.** 10,533,358 adds, 1,911,396 replaces, 8,540,138 deletes and 344,744 executions
  (55,367,722 shares; 1,071 non-printable). 89,749 non-cross trades, and 8,849 cross prints of
  which 5,915 are zero-share. 1,746,202 orders live at the end.
- **Quarantined: 0** in all seven categories.
- **Crossed and locked books.** 4 crossed and 6 locked episodes, none of which began while the
  stock was trading (`T`). 0 L2 timestamps held back. Worst severity: `error`, from
  `itch.framing` only (the intended truncation).
- **Level 2.** QQQ 262,845 rows (259,827 depth, 3,018 trades); AAPL 83,358 rows (78,287 depth,
  5,071 trades).
- **`--max-messages 20000000`.** Stops at `message_limit` at 09:36:36.388 ET with the same
  quality result: 0 quarantined, the same 4 crossed and 6 locked episodes (0 while trading). QQQ
  224,590 rows, AAPL 69,891.
- **Throughput.** Single runs on a shared machine while other agents were building, so these are
  indicative only:

  | Run | Wall time | Messages/s |
  |---|---:|---:|
  | decode-only from gzip | 3.83 s | 6.13 M |
  | full reconstruction from gzip | 8.73 s | 2.69 M |

### Independent comparison (H3)

**Candidates reviewed.**

- **[groovg/itch-book](https://github.com/groovg/itch-book)** at
  `b502b002abf650760b79c010b3ae3a8f82896ae8` (still remote HEAD on 2026-11-17). MIT licence,
  "Copyright (c) 2026 groovg". A header-only C++23 ITCH 5.0 parser and per-symbol book manager,
  plus a Python package that emits BBO, trade, order-by-order and depth tables. Its README
  reports top-of-book agreement with Databento XNAS.ITCH of 99.69–99.88% of nanoseconds on
  2019-12-30. Every disagreement it reports falls on a nanosecond carrying more than one ITCH
  message. It was built locally, at no cost, with the MinGW GCC already on the machine.
- **[databento/dbn](https://github.com/databento/dbn)** at
  `a77693030595b8f29f6b4c58e63b1b1a9d899b3f`. Apache-2.0. Databento Binary Encoding: a
  normalized storage format and codec for Databento's own records (MBO, MBP and others). It does
  not read Nasdaq BinaryFILE ITCH or rebuild books from it. Using it would require Databento's
  metered XNAS.ITCH data, whose licence text could not be retrieved (UNVERIFIED in
  OSS_LICENSES_AND_PROVENANCE §6). **Not used.**

**Method.** Three implementations replay the same 23,483,868 complete messages from an empty book
at message 1:

- `xcheck/itchbook_adapter.cpp` calls itch-book's `parse` and `BookManager` unchanged;
- `xcheck/bbadapter` calls BlackBox's `decode` and `OrderBooks` unchanged;
- `xcheck/py_ref.py` is a one-symbol Python reference written from the specification's field
  offsets. It handles R, A, F, E, C, X, D and U; P is a print and never changes the displayed
  book.

These sources are now committed under `research/itch-xcheck/` and driven end to end by
`scripts/research/itch-validate.sh`; see [ITCH_VALIDATION_PIPELINE.md](ITCH_VALIDATION_PIPELINE.md).

The two adapters write identical text records:

- `E`: BBO price and size plus the top five levels per side (price, shares, order count) after
  every book event of the target symbol between 09:30:00 and 09:39:00 ET;
- `L`: the target's full depth at every millionth message, and every stock's full depth at the
  end;
- `T`: every disseminated trade of every stock;
- `C` and `Z`: type counts, end offset and live orders.

`compare.py` checks equality record by record (exact integers, line endings normalized), plus a
fixed-seed random sample of N = 1,000 event indexes (seed 20261117). The Python reference is
compared on the `E` records.

| Symbol (window 09:30:00–09:39:00 ET) | Book events compared | itch-book = BlackBox | Python = BlackBox | Sampled N = 1,000, BBO + depth 5 |
|---|---:|---:|---:|---:|
| QQQ | 143,760 | 143,760 (100%) | 143,760 (100%) | 1,000 / 1,000 |
| AAPL | 63,964 | 63,964 (100%) | 63,964 (100%) | 1,000 / 1,000 |

In the same runs, every other record type also agreed at 100%:

| Record | Count | Notes |
|---|---:|---|
| trade records (all stocks) | 442,271 | |
| level records, QQQ run | 783,520 | 23 QQQ checkpoints plus all stocks at the end |
| level records, AAPL run | 809,536 | 23 AAPL checkpoints plus all stocks at the end |
| message-type counts | 16 | |
| end state | 1 | 750,516,774 bytes; 1,746,202 live orders |

itch-book itself reported 0 missing references, 0 duplicate references, 0 rejected and 0
clamped messages. It agrees with the BlackBox manifest (0 quarantined, the same live count).

**Match rate: 100%. Mismatches: none.**

**Negative control.** Changing one bid size in one QQQ `E` record of the itch-book output made
`compare.py` report exactly one unequal `E` record (143,759 / 143,760). The harness therefore
detects a single-share difference.

**What this agreement does and does not show.**

- *Shows:* three implementations of displayed-book maintenance agree, one of them by a different
  author in a different language, on a real day.
- *Does not show* agreement with an exchange-certified or vendor book.
- *Shared interpretations are not tested.* All three share some readings of the specification,
  such as "a replace goes to the back at its new price" and "ITCH file order is the sequence
  order". The Python reference was written by the same team that wrote the BlackBox decoder.

**Window aggregates** (BlackBox side, the same nine minutes):

| Measure | QQQ | AAPL |
|---|---:|---:|
| Book events per second | 266 | 118 |
| Quoted spread p10 / p50 / p90 | $0.01 / $0.01 / $0.02 | $0.01 / $0.03 / $0.07 |
| Median combined touch size (shares) | 2,700 | 392 |
| Touch price changes | 8,659 | 5,755 |
| Events sharing their timestamp with the symbol's previous event | 4.2% | 41.2% |
| Shares leaving the best level by delete | 97.04% | 52.09% |
| Shares leaving the best level by execution | 2.26% | 45.79% |
| Shares leaving the best level by replace | 0.66% | 2.10% |
| Shares leaving the best level by partial cancel | 0.05% | 0.02% |
| Executions (E / printable C / non-displayed P) | 1,871 / 84 / 157 | 3,305 / 26 / 848 |

## 4. What the data can and cannot support for execution-model calibration

| Class | What it is here | Available? | Usable for |
|---|---|---|---|
| **Observed messages** | Displayed adds, cancels, deletes, replaces and executions, with order reference, side, price, shares and exchange timestamp (ns since midnight ET). Non-displayed executions (P) and cross prints. | Yes, exactly as disseminated | Event rates, depletion composition (table above), trade sizes, spread and touch statistics |
| **Reconstructed queue** | FIFO displayed queue per price, rebuilt from the messages (`OrderBooks::observed_queue`). Cross-checked against two other implementations above. | Yes, under the stated interpretations: replace loses priority; file order is time order inside a nanosecond | *Where* cancellations occurred relative to any queue position. This is exactly what L2 queue models must guess ([../QUEUE_MODELS.md](../QUEUE_MODELS.md#what-l2-data-cannot-reveal)). |
| **Hypothetical orders** | A shadow order inserted into the reconstructed queue at a chosen time and position. Its "fill" is computed from later real executions at its price once the displayed shares ahead are gone. | Computable, as a model output | Benchmarking the L2 `front` estimators against an L3-informed reference, under an explicit no-impact assumption |
| **Unobservable counterfactuals** | How others would have reacted to our order (cancels, adds, aggressor size); hidden and reserve interest and its priority; our own order-entry and feed latency; queue position of our real orders; venues other than Nasdaq | No | Nothing. These must stay labelled assumptions and be swept, never fitted. |

Consequences:

- The data can calibrate and falsify **the queue-depletion part** of the execution model (how
  `front` should move given L2 changes). It can do this because L3 reveals the ahead/behind
  split of cancellations, which L2 hides.
- It cannot calibrate **fill probability for real orders**. A shadow fill is an upper-bound-style
  quantity: it assumes the aggressor would still have traded the same size against our order,
  and that nobody reacted to our order being there.
- It cannot calibrate **latency**: there is no receipt clock.
- The two test symbols already differ by an order of magnitude in how the touch empties. Any
  fitted parameter is therefore symbol- and regime-specific until shown otherwise.

## 5. First experiment: L3 shadow queue versus the L2 queue models

**Question.** On real Nasdaq data, how far is BlackBox's modeled queue position `front`, and the
fill timing it implies, from the position observed when the order is tracked through the L3
book?

**Hypotheses**, fixed before any out-of-sample data is opened:

- *H-A.* `RiskAdverse` never credits cancellations ahead, so it understates the touch fill
  fraction within a horizon τ relative to the L3 shadow. The gap should be large where deletes
  dominate depletion (QQQ: 97% of shares leaving the touch) and small where executions do
  (AAPL: 46%).
- *H-B.* The probability models (`log_prob`, `log_prob2`, and `power_prob`, `power_prob2` and
  `power_prob3` with n ∈ {2, 3}) differ from the L3 shadow by less than `RiskAdverse`
  does. No single model and n is within the confidence interval on both symbols.
- *H0* for each model: the L2 model's fill fraction at τ equals the L3 shadow's; tested with a
  two-sided Wilson or Newcombe interval on the difference.

**Design.**

1. **Shadow orders.** Every 1.000 s of exchange time inside the window, alternating sides, place
   one 100-share order. It joins the back of the displayed best queue at time `t + L`, where L is
   a latency *assumption* in {0, 50 µs, 500 µs}, swept and never fitted. The order is not added
   to the displayed book (no impact).
2. **L3 reference** (`OrderBooks`). Track the shares ahead in FIFO, removing executions and
   cancellations ahead as they happen. The order fills when executions at its price reach it, or
   when the price trades through. It is censored at τ ∈ {1 s, 5 s, 30 s} or at the window end;
   the existing Kaplan–Meier code handles censoring. Record the share of cancelled volume that
   was ahead of the shadow order.
3. **L2 model.** Use the same shadow orders, the L2 projection of the same messages, and
   BlackBox's queue models with the formulas in QUEUE_MODELS.md. `front` starts at the displayed
   quantity at `t + L`.
4. **Metrics.** Per model, symbol, latency and τ:
   - fill fraction with a 95% interval;
   - Kaplan–Meier median time to fill;
   - mean absolute `front − ahead_L3` at fixed checkpoints.

   Report all cells; do not select the best one.
5. **Prerequisite (code).** `itch-reconstruct` stores no `DatasetManifest`. Running the
   simulator on ITCH L2 needs a dataset whose `local_ts` provenance is `Assumed` and records L.
   That change is not part of this slice. Until it exists, step 3 runs the queue formulas offline
   on the L2 rows.

**In-sample / out-of-sample policy.**

- *In-sample (design and any tuning):* QQQ and AAPL, 2019-07-30, 09:30:00–09:39:00 ET, the
  prefix in this document.
- *Out-of-sample (opened once, after the configuration hash is frozen):*
  - 2019-12-30, 09:30:00–09:31:50. This is **not** in `data/itch/` (see §2). The only copy is
    the 120 MiB prefix outside the project; it must be copied in, hashed and recorded before it
    is opened, or fetched again;
  - the same opening window on at least two further days, each fetched as a ≤ 300 MB prefix,
    for example `10302019` and `01302020`;
  - one held-out liquid symbol chosen by an activity rule (the most active regular-hours symbol
    other than the in-sample ones), not by inspection.
- No parameter, horizon or latency grid may change after out-of-sample results are seen. If one
  must, the change is reported as a new in-sample study.
- *Prefix constraint.* gzip has no random access, so a byte prefix only ever reaches the start of
  the day. This 300 MB prefix ends at 09:39 ET. Mid-day and closing behaviour are untested and
  stay out of scope until a whole file is fetched under an explicit data budget.

**Execution status/provenance correction (2026-11-19):** see [QUEUE_SHADOW_EXPERIMENT.md §2](QUEUE_SHADOW_EXPERIMENT.md#deviations-from-the-pre-registered-plan) and §6.6–§7: October plus precommitted 2019-01-30/2020-01-30 are clean OOS acquisitions; the two new 300 MB prefixes end at 09:38:54.128393/09:34:31.554873308 ET (full-window coverage incomplete), SPY is mechanically held out on both; December remains previously decoded/inspected validation, and the corrected engine predates the January acquisitions.

**Clock handling.**

- ITCH timestamps are nanoseconds since midnight Eastern, stamped by the exchange. They are not
  arrival times. 2019-07-30 is EDT, so `--midnight-epoch-ns 1564459200000000000` (2019-07-30
  04:00:00 UTC). The tool does not compute daylight saving time, so every day's offset is
  computed and recorded per run.
- `local_ts` is absent (`normalization.local_clock = "absent"`). Every latency is the swept
  assumption L, labelled as such.
- Within one nanosecond, file order is the event order. 41% of AAPL book events in the window
  share their timestamp with the previous AAPL event, so a model that batches by timestamp must
  state how it orders events inside the batch. itch-book's Databento comparison found its only
  disagreements on such nanoseconds.
- The window boundaries are exchange time. Shadow placement uses exchange time plus L.

**Outputs.** Per-cell aggregates only, in this document or a successor. Shadow-order paths,
books and L2 rows stay in `data/itch/`.

## 6. Limitations

- **Legal.** No licence text exists for the EMI files. The local-only reading above is
  conservative; it is not a grant.
- **Coverage.** One day and 9 min 23 s of regular trading: the open, which is not representative
  of the rest of the day. No closing cross, mid-day halt or broken trade occurs in this input
  (`trades.broken` count 0).
- **Integrity.** No published checksum for the file (the `.md5sum` returns 404). Integrity rests
  on gzip framing and the three-way agreement, which cannot detect bytes that are wrong at the
  source.
- **Independence.** The cross-check is implementation agreement, not certification. itch-book
  and BlackBox are independent code; the Python reference is not independent of this team's
  reading of the specification. Hidden and reserve interest is invisible to all three.
- **Shadow orders are counterfactual.** Their fills assume no market reaction and that aggressors
  would have consumed our order. Real fill rates are expected to differ, in a direction that is
  unknown.
- **Performance.** Throughput figures are single runs on a shared machine.

## Local file inventory (not committed)

`data/itch/`:

- the prefix and its decompression;
- `blackbox.exe`;
- `manifest-20M.json` and `manifest-full.json`, with run logs;
- `activity.json`;
- `terms/` and `listing.html` (the pages read for §1);
- `xcheck/`: the adapter sources and executables, `compare.py`, `py_ref.py`, `activity.py`, the
  per-event comparison files (local only), and the `summary-*.json` and `pystats-*.json`
  aggregates.
