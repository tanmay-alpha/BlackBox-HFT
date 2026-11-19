# Queue shadow experiment: reconstructed L3 queue versus the L2 queue models

Phase 7, Workstream D (queue model and fill uncertainty lab). This is the pre-registered first
experiment of [REAL_DATA_VALIDATION_PLAN.md §5](REAL_DATA_VALIDATION_PLAN.md#5-first-experiment-l3-shadow-queue-versus-the-l2-queue-models).
Every figure from NASDAQ data below is an aggregate (a count, rate, interval or mean). No
shadow-order path, book, L2 row or other event-level derivative appears in this repository; those
stay under `data/queue-shadow/` and `data/itch/`, which `.gitignore` excludes.

**What the numbers are.** A shadow order is a hypothetical 100-share order that never existed.
Its "fill" is a counterfactual model output computed from later real messages under a no-impact
assumption, never a recorded execution. The L3 reference is a queue *reconstructed* from ITCH
messages, not an exchange-certified position, and an actual order's reconstructed position is not
ground truth for a hypothetical order.

**Current summary (§7; three clean OOS acquisitions).** October 2019 / January 2019 /
January 2020 yield 9 / 36 / 3 nominal H0 rejections out of 243 contrasts per day,
0 / 11 / 3 H-A-supported cells out of 27, and maximum absolute fill-fraction
differences 0.0167 / 0.0243 / 0.0258. The single-October outcomes do not replicate
uniformly. January prefixes cover only 534.128393 s and 271.554873308 s of the
540 s window. December remains reused validation, not an untouched day; October's
engine correction followed outcome review, whereas both January acquisitions use
the already-corrected engine. These are descriptive, nominal/exploratory findings,
not pooled inference or calibration of real orders.

## 1. Question and hypotheses

How far is BlackBox's modeled queue position `front`, and the fill timing it implies, from the
position observed when the same hypothetical order is tracked through the L3 book?

Hypotheses were fixed before the October data acquisition, but not before the December
data had been inspected (§2). The code applies these nominal/exploratory operational
decisions (`hypotheses` in the artefact), not multiplicity- or dependence-calibrated tests:

| | Statement (plan §5) | Nominal/exploratory decision |
|---|---|---|
| H-A | `RiskAdverse` never credits cancellations ahead, so it understates the touch fill fraction within τ relative to the L3 shadow; the gap is large where deletes dominate (QQQ) and small where executions do (AAPL). | Nominal support in a cell when the nominal 95% Newcombe interval of RiskAdverse − L3 lies entirely below 0. The size claim is read from the QQQ and AAPL differences. |
| H-B | The probability models differ from the L3 shadow by less than `RiskAdverse` does. No single model and n is within the interval on both symbols. | Part 1: per cell, a model is closer when \|its Δ\| < \|RiskAdverse Δ\| (point estimates). Part 2: per (L, τ), the list of models whose interval contains 0 on every studied symbol; H-B part 2 holds when that list is empty. |
| H0 | For each model, the L2 fill fraction at τ equals the L3 shadow's. | Nominal rejection in a cell when the nominal 95% Newcombe paired interval of the difference excludes 0. |

## 2. Design (frozen)

| Parameter | Value |
|---|---|
| Window | 09:30:00.000–09:39:00.000 ET, exchange clock (`window_start_ns` 34,200,000,000,000, `window_end_ns` 34,740,000,000,000 since midnight) |
| Schedule | shadow k at window start + k × 1.000 s while before the window end (540 per symbol and latency); even k buys, odd k sells |
| Size | 100 shares; lot size 1 share |
| Latency L (assumption, swept, never fitted) | 0, 50 µs, 500 µs |
| Horizons τ | 1 s, 5 s, 30 s |
| Checkpoints for `front − ahead_L3` | 0.1 s, 1 s, 5 s, 30 s after arrival |
| L2 models | `risk_adverse`, `log_prob`, `log_prob2`, `power_prob`, `power_prob2`, `power_prob3` with n ∈ {2, 3} (9 models) |
| Intervals | fill fraction: nominal 95% Wilson score; model − L3: nominal 95% Newcombe (1998) method 10 for paired proportions; Kaplan–Meier: existing `bb_analytics::execution::kaplan_meier` (nominal 95% Greenwood bands) |

The rules below are copied verbatim into the design (`RULES` in
`crates/bb-analytics/src/shadow_queue.rs`), so any change to them changes the hash:

1. Messages are processed in stream (file) order; messages sharing a timestamp are never reordered.
2. A shadow arriving at T (scheduled time + L) is sequenced after every message stamped before T
   and before the first message stamped at or after T. Arrivals at or after the window end are
   not placed.
3. At arrival it joins the back of the displayed best queue of its side, at that price. It is not
   placed when that side is empty or the book is locked or crossed at that moment.
4. No impact: the shadow is never added to the book, and nothing reacts to it.
5. **L3 reference.** Shares ahead are the displayed shares at the price at arrival (exactly the
   orders live there, from `OrderBooks::level_queue`). Executions, partial cancels, deletes and
   replaces of those orders reduce them. A replace loses priority: its new order is behind.
6. **L3 fill**: an execution (E or C, printable or not) of a displayed order on the shadow's side
   at its price that was *not* ahead of it (queue), or at a worse price (trade-through); or the
   opposite best reaching its price after a message (book cross).
7. **L2 projection** (as `bb_data::itch::l2`): a displayed printable execution emits a trade row,
   aggressor opposite the resting side, before its depth row; each changed level emits a depth row
   with the new total. Non-printable executions, non-cross trades (P) and crosses (Q) emit no
   sided row and are ignored, as hftbacktest's `NoPartialFillExchange` ignores unsided trades.
8. **L2 models** apply the hftbacktest 0.9.4 formulas ([../QUEUE_MODELS.md](../QUEUE_MODELS.md)):
   `front` starts at the displayed quantity at arrival; a trade at the price calls `trade` then
   `is_filled` (fill when `round(−front) > 0`); a trade at a better price fills (trade-through);
   a depth row at the price calls `depth(prev, new)`; the opposite best reaching the price fills
   (book cross).
9. A fill is the first counterfactual execution of any part of the shadow (first touch, the
   `NoPartialFillExchange` convention).
10. Each shadow is followed until every path filled, until its longest horizon or checkpoint
    passed, or until the end of tracking: the window end, or the last message when the input ends
    earlier. The L3 cancellation counters stop at the L3 fill.
11. The fill fraction at τ uses only shadows with arrival + τ ≤ end of tracking, whatever their
    outcome; Kaplan–Meier uses every placed shadow with right censoring.

**Design SHA-256: `0608a9ba4af5b7f03119277570999af47b8212b9285e18a8f1f73f240846f502`**
(SHA-256 of the canonical JSON of `Design::preregistered()`, printed by every run and stored in
`design_sha256`; `--expect-design-sha256` makes a run refuse any other design).

**Freeze record.** The design hash above was recorded in freeze commit `2e091e3` before
the October prefix was fetched. The July in-sample results had already been read, and the
December prefix had already been decoded and analysed: it was not untouched out-of-sample
data. No design parameter, horizon, latency, model, `RULES` string or window changed after
the freeze. The hash does not include an engine implementation version. After outcomes
were reviewed, deadline/checkpoint handling was corrected in implementation revision
`inclusive-deadlines-v2` (§3); these corrected reruns are therefore not an evaluation of an
untouched implementation. The earlier byte-identical speed-up (§11) was not the only
post-freeze implementation change. The October acquisition rule fixed at the freeze was:

- Day: `10302019.NASDAQ_ITCH50.gz` (2019-10-30). Rule: of the two further days the plan names
  (`10302019`, `01302020`), the smaller file (3,872,931,242 versus 5,597,158,940 bytes in the
  2026-11-17 listing), because a fixed 300 MB prefix then reaches further into the session.
- One request, `curl -r 0-299999999`, into `data/itch/10302019.NASDAQ_ITCH50.gz.prefix-300MB`.
- Clock: 2019-10-30 is EDT (daylight saving time ended 2019-11-03), so midnight ET is
  2019-10-30 04:00:00 UTC, `--midnight-epoch-ns 1572408000000000000` (computed with Python
  `zoneinfo` `America/New_York`; the same computation gives the in-sample value
  1564459200000000000).
- Symbols: QQQ, AAPL, plus the held-out symbol given by `--rank-activity` on that file (the most
  active symbol by A/F/E/C/X/D/U count inside the design window, other than QQQ and AAPL),
  applied mechanically.
- Window end: rule 10. If the prefix ends before 09:39:00, tracking ends at its last message.
- Previously decoded December comparison set: the prefix fetched on 2026-11-14,
  `D:/bbwt-research/itch-data/12302019.NASDAQ_ITCH50.gz.prefix-120MiB`
  (outside the repository), 125,829,120 bytes, SHA-256
  `e1623c7c62cc993c3e264bc78eb7e4603fbf234e9c1d75ba70b27a9f308e34eb`.
  Its decoded `12302019.prefix.raw` and full-prefix reconstruction manifest date from
  November 14; QQQ/AAPL were reconstructed and analysed on November 14–15.
  [`docs/rebuild/EXPERIMENT_VALIDATION.md`](../rebuild/EXPERIMENT_VALIDATION.md), commit
  `efd3694` (November 15), documents reconstruction of all 9,766,139 frames and the
  independent comparison. QQQ/AAPL are previously inspected validation data, not OOS.
  AMZN is a held-out symbol on a previously decoded day; all-market processing had
  already included its messages, and absence of prior AMZN-specific analysis cannot
  establish an untouched-day claim. December 30 is EST: midnight ET is 05:00:00 UTC,
  `--midnight-epoch-ns 1577682000000000000`; tracking ends at the prefix's last message.

### Deviations from the pre-registered plan

**Prospective two-day extension pre-commitment (2026-11-19; before downloading).**
Select `01302019.NASDAQ_ITCH50.gz` (2019-01-30) and
`01302020.NASDAQ_ITCH50.gz` (2020-01-30): the first two actual
`MMDDYYYY.NASDAQ_ITCH50.gz` data files in the displayed
[EMI listing](https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/), excluding
2019-07-30, 2019-10-30 and 2019-12-30; ignore checksum entries and other naming
schemes. Selection uses listing order, not inspection of file contents or outcomes.
Fetch exactly one `curl -r 0-299999999` prefix per selected day, at most
300,000,000 bytes each; do not extend a prefix if it falls short of 09:39 ET.
Apply the unchanged activity rule separately to each prefix: QQQ, AAPL and the
most active other symbol by A/F/E/C/X/D/U count inside the frozen window,
selected mechanically with `--rank-activity`. Keep the frozen design hash and
`inclusive-deadlines-v2` implementation unchanged. These two days are new
prospective data holdouts for the already-corrected engine, not retroactive
repairs to December's provenance.

- Plan §5 called for the December prefix **plus at least two further days**. The initial
  experiment fetched only October 30. The prospective extension in §6.6 adds two clean
  days, but neither 300 MB prefix reaches 09:39; full-window further-day coverage
  remains incomplete. No extra bytes were acquired to remedy that constraint.
- December QQQ/AAPL had already been inspected before the queue-design freeze, contrary
  to the plan's unopened-holdout premise. Its 180 cells (162 model-versus-L3 contrasts)
  are reused validation evidence. AMZN adds held-out-symbol evidence, not an untouched day.
- October 30 was initially the only clean OOS data acquisition under the frozen design.
  The `inclusive-deadlines-v2` correction followed review of its outcomes, so corrected
  October reruns retain prospective data provenance but not untouched-engine status.
  The two January acquisitions in §6.6 are prospective data holdouts for the corrected
  engine; their limited coverage does not undo the earlier implementation deviation.
- `REAL_DATA_VALIDATION_PLAN.md` was untracked and absent from the reviewed refs; the
  original plan wording is not independently versioned in those refs. The committed
  freeze record establishes the design hash, not December's alleged lack of inspection.

## 3. Implementation

| File | Role |
|---|---|
| `crates/bb-analytics/src/shadow_queue.rs` | Pure, deterministic engine (`ShadowEngine`), the offline queue formulas (`QueueState`, `ModelSpec`), Wilson and Newcombe intervals, aggregation (`aggregate`) |
| `crates/bb-analytics/src/shadow_queue_tests.rs` | Hand-built L3 streams with known answers, hftbacktest equivalence, determinism, no look-ahead |
| `crates/bb-app/src/queue_study.rs` | `blackbox study queue-shadow`: source handling, artefact, markdown, design hash, activity rank |

The engine owns one `bb_data::itch::book::OrderBooks` for every stock (an ITCH book must start at
the first message) and reacts only to the selected symbols' locates. Before applying an E, C, X,
D or U message it reads the named order's side, price and remaining shares, so it knows whether
the shares that leave were ahead of each live shadow. After applying, it reads the book's own
`changes()` (print before level row, one level row per changed price) as the L2 projection. No
message is reordered and no later message is read before a decision.

**Corrected boundary semantics (`inclusive-deadlines-v2`).** A fill at elapsed time
`<=` the longest horizon is included. Checkpoints and cancellation-horizon counters
reflect state after **all** messages with timestamp `<=` the boundary have been applied
in file order, including all messages tied at that timestamp. Checkpoint pairs exclude
paths already filled by that boundary; cancellation counters stop at the L3 fill and
do not continue counting a filled path. The study window remains exclusive at its end:
messages stamped at or after `window_end_ns` are not applied, even if that time is also
a horizon/checkpoint boundary. This corrects the former left-limit snapshots and
premature longest-horizon retirement without changing the frozen `RULES` text or hash.

The offline formulas are the hftbacktest 0.9.4 source (`src/backtest/models/queue.rs`)
transcribed: `RiskAdverseQueueModel` and `ProbQueueModel` with `LogProbQueueFunc`,
`LogProbQueueFunc2`, `PowerProbQueueFunc{,2,3}`. The test
`offline_formulas_equal_hftbacktest_queue_models` drives hftbacktest's own `QueueModel`
implementations (`new_order`, `trade`, `depth`, `is_filled` on a `HashMapMarketDepth`) and the
offline `QueueState` with the same 96,000 pseudo-random calls (12 models including n = 1, 200
orders, 40 calls each) and requires identical `front` bits and identical executable quantity
after every call. hftbacktest is a dev-dependency of `bb-analytics` only.

### Artefact contract `blackbox.queue-shadow` version 1

`<out>/study-result.json`, compact JSON without line breaks (byte-stable), with
`study-result.json.sha256` in `sha256sum` format, the same convention as
[EXECUTION_STUDY_VIEWER.md](EXECUTION_STUDY_VIEWER.md). Two runs of the same binary on the same
input give identical bytes. The run also writes `study-result.md` (every table), `design.json`
and `run.json` (wall time and throughput, not hashed).

| Field | Content |
|---|---|
| `schema`, `schema_version` | `"blackbox.queue-shadow"`, `1`. Bump the version on any incompatible change. |
| `study_id`, `title` | `"queue-shadow"`, title |
| `data_class` | `"synthetic"` (l3-demo generator) or `"real_local"` (NASDAQ sample data; never published) |
| `labels[]` | Human-readable data-class statements, always including `"L3 reference is reconstructed, shadow fills are counterfactual"`, the no-impact statement and `"latency L is a swept assumption, not calibrated; there is no receipt clock"` |
| `generator` | `command` (input file name only), `blackbox_version`, `source_commit` (`BB_GIT_SHA` at generation, 7–40 hex, else `null`) |
| `engine` | Additive v1 metadata: implementation revision `inclusive-deadlines-v2`, deadline and checkpoint semantics; not an input to `design_sha256` |
| `source` | Synthetic: `kind`, `generator`, `config` (seed, symbol, events), `bytes`, `sha256` of the generated stream. ITCH: `kind`, `name`, `compression`, `bytes`, `sha256` of the stored file, `decompressed_bytes_read` |
| `input` | `frames_read`, `invalid_frames`, `stop` (`reason`, `detail`; `message_limit` means the engine stopped at the window end), `messages_applied_before_window_end` |
| `clock` | `basis`, `local_clock` (`absent`), `midnight_epoch_ns` (`null` for synthetic), `window_start_utc`, `window_end_utc` |
| `design`, `design_sha256` | The frozen design (§2) and its hash |
| `interval_methods` | Text of every nominal 95% interval or estimator method, keyed `fill_fraction_95`, `difference_95`, `km`, `checkpoints`, `cancelled_volume` |
| `interval_note` | Additive v1 string: nominal intervals do not account for serial dependence or unadjusted multiplicity |
| `keys` | Key columns of each result array, `reference_model` (`"l3_shadow"`), units |
| `models[]` | `{id, family, n, description}`; `l3_shadow` first, then the 9 L2 models with their `p` formula |
| `results.tracking_end_ns`, `results.stream_reached_window_end`, `results.messages` | End of tracking (exchange clock) and why |
| `results.symbols[]` | `symbol`, `locate`, `scheduled`, `placed`, `skipped_empty_side`, `skipped_locked_or_crossed`, `not_arrived` |
| `results.cells[]` | One per symbol × `latency_ns` × `horizon_ns` × `model` (10 paths): `family`, `n`, `placed`, `eligible`, `filled`, `fill_fraction` and `fill_fraction_95` (Wilson), `fill_causes` {`queue`, `trade_through`, `book_cross`}, `vs_l3` (`null` for `l3_shadow`; else `both`, `model_only`, `l3_only`, `neither`, `difference`, `difference_95` (Newcombe), `excludes_zero`), `km_median_ns`, `km_fill_probability`, `km_fill_probability_95`, `km_censored` |
| `results.cancelled_volume[]` | One per symbol × `latency_ns` × `horizon_ns`: `placed`, `ahead_shares`, `behind_shares`, `share_ahead` |
| `results.checkpoints[]` | One per symbol × `latency_ns` × `checkpoint_ns` × L2 model: `pairs`, `mean_abs_error_shares`, `mean_signed_error_shares` (front − ahead_L3), `mean_l3_ahead_shares` |
| `hypotheses` | Nominal/exploratory `definitions`, `cells[]` (per symbol × L × τ: RiskAdverse Δ and interval, `h_a_supported`, `closer_than_risk_adverse`, `h0_not_rejected`), `models_within_interval_on_every_symbol[]` |
| `caveats[]` | Limitations that apply to every number, extended with serial dependence and unadjusted multiplicity; the direction and magnitude of coverage distortion are unknown |

Every scalar is the analytics `Value` object: `{"status": "available", "value", "unit", "n",
"excluded"}` or `{"status": "unavailable", "reason", "unit", "n", "excluded"}`; an interval is
`{"lower": Value, "upper": Value, "method"}`. Times are integer nanoseconds.

The published synthetic artefact is `web/public/study-results/queue-shadow/study-result.json`
with its `.sha256`. Real-data artefacts exist only under `data/queue-shadow/`; the command refuses
to write ITCH outputs under `web/public`.

## 4. Data provenance and rights

The data rules of [REAL_DATA_VALIDATION_PLAN.md §1](REAL_DATA_VALIDATION_PLAN.md#1-data-rights-h1)
apply unchanged: no licence grants rights in the EMI files; use is local, private and
non-commercial; nothing derived event by event is redistributed; only aggregates are documented.

| Use | File (`data/itch/`) | Bytes | SHA-256 |
|---|---|---:|---|
| In-sample | `07302019.NASDAQ_ITCH50.gz.prefix-300MB` (fetched 2026-11-17, plan §2) | 300,000,000 | `0083093fcab8d91b5da94e270032f5c801db4fff84447befdfd54974fdaa9ef1` |
| Out-of-sample A | `10302019.NASDAQ_ITCH50.gz.prefix-300MB` (fetched 2026-11-17 20:16:31–20:17:35 UTC after the freeze commit `2e091e3`; one `curl -r 0-299999999`, HTTP 206, `Content-Range: bytes 0-299999999/3872931242`, `Last-Modified: Thu, 31 Oct 2019 04:14:57 GMT`) | 300,000,000 | `58403c2a36df6decb3d1ff02f7a42dcad05a42d1c3a575306874f61e67e65a89` |
| Reused validation / held-out symbol, not OOS-B | `12302019.NASDAQ_ITCH50.gz.prefix-120MiB` (fetched and decoded 2026-11-14; QQQ/AAPL inspected November 14–15; historical source outside the repository under `D:/bbwt-research/itch-data/`, local copy under `D:/black box/data/itch/`) | 125,829,120 | `e1623c7c62cc993c3e264bc78eb7e4603fbf234e9c1d75ba70b27a9f308e34eb` |

The synthetic input is the `l3-demo` generator (`bb_data::l3synth`, `L3SynthConfig::default()`:
seed 7, 30,000 order-flow messages, symbol `SYNTH`), wholly invented and redistributable. Its
generated stream has SHA-256
`bfaa37069283d47d4d4527336802d726c954c862c8d077b43065a0462c1d30f4`.

## 5. Clock handling

- ITCH timestamps are nanoseconds since midnight US Eastern, stamped by the exchange; they are not
  arrival times. The window, the schedule, the latency L and every horizon are on that clock.
- `--midnight-epoch-ns` is recorded with derived UTC window bounds; it is not used for any
  computation. The tool does not compute daylight saving time; each day's value is computed and
  recorded above (in-sample 2019-07-30 EDT: 1564459200000000000, window 13:30:00–13:39:00 UTC).
- There is no receipt clock (`local_clock: absent`). L is an assumption, not a measured latency.
- Inside one nanosecond, file order is the event order and a shadow arriving at T precedes every
  message stamped T (rule 2). The test
  `same_timestamp_messages_keep_file_order_and_arrivals_precede_them` pins both.
- The synthetic session's clock starts at 09:30:00 of an undated synthetic day.

## 6. Results

The original four cohorts in §6.1–§6.5 were rerun with `inclusive-deadlines-v2`, release
binary built from implementation commit `ccdd493f0812376fe0b341fccdc1a8a44fb8002e`.
Their 297 numeric table rows match the previous rows exactly; §6.5 accounts for every
original result cell. Section 6.6 adds two prospective acquisitions using the same engine.

### 6.1 Synthetic (published)

`blackbox study queue-shadow --source synthetic --seed 7 --events 30000`. The generator's session
lasts 9.61 s of exchange time, so only shadows k = 0…9 arrive (29 placed, one skipped on an empty
side); no shadow is eligible for τ = 30 s. These numbers demonstrate the pipeline and the contract;
they say nothing about real markets. The published file is described in §6.4.

Placement: SYNTH 29 placed of 1620 scheduled (1 empty side, 0 locked/crossed, 1590 not arrived). Tracking ended at 34209610400033 ns (end of input).

**Fill fraction within τ.** L3 column: fraction [nominal 95% Wilson] over eligible shadows (n) and its fill causes queue/trade-through/book-cross. Model columns: Δ = model − L3 [nominal 95% Newcombe paired]; **bold** = nominal/exploratory H0 rejection.

| Symbol | L | τ | n | L3 fill [nominal 95%] | L3 causes | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| SYNTH | 0 ms | 1,000 ms | 8 | 0.875 [0.529, 0.978] | 4/2/1 | 0.000 [−0.316, 0.316] | 0.000 [−0.316, 0.316] | 0.000 [−0.316, 0.316] | 0.000 [−0.316, 0.316] | 0.000 [−0.316, 0.316] | 0.000 [−0.316, 0.316] | 0.000 [−0.316, 0.316] | 0.000 [−0.316, 0.316] | 0.000 [−0.316, 0.316] |
| SYNTH | 0 ms | 5,000 ms | 4 | 0.750 [0.301, 0.954] | 2/1/0 | 0.000 [−0.427, 0.427] | 0.000 [−0.427, 0.427] | 0.000 [−0.427, 0.427] | 0.000 [−0.427, 0.427] | 0.000 [−0.427, 0.427] | 0.000 [−0.427, 0.427] | 0.000 [−0.427, 0.427] | 0.000 [−0.427, 0.427] | 0.000 [−0.427, 0.427] |
| SYNTH | 0 ms | 30,000 ms | 0 | n/a n/a | 0/0/0 | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a |
| SYNTH | 50 µs | 1,000 ms | 9 | 1.000 [0.701, 1.000] | 5/3/1 | 0.000 [−0.299, 0.299] | 0.000 [−0.299, 0.299] | 0.000 [−0.299, 0.299] | 0.000 [−0.299, 0.299] | 0.000 [−0.299, 0.299] | 0.000 [−0.299, 0.299] | 0.000 [−0.299, 0.299] | 0.000 [−0.299, 0.299] | 0.000 [−0.299, 0.299] |
| SYNTH | 50 µs | 5,000 ms | 5 | 1.000 [0.566, 1.000] | 3/2/0 | 0.000 [−0.434, 0.434] | 0.000 [−0.434, 0.434] | 0.000 [−0.434, 0.434] | 0.000 [−0.434, 0.434] | 0.000 [−0.434, 0.434] | 0.000 [−0.434, 0.434] | 0.000 [−0.434, 0.434] | 0.000 [−0.434, 0.434] | 0.000 [−0.434, 0.434] |
| SYNTH | 50 µs | 30,000 ms | 0 | n/a n/a | 0/0/0 | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a |
| SYNTH | 500 µs | 1,000 ms | 9 | 0.889 [0.565, 0.980] | 4/3/1 | 0.000 [−0.296, 0.296] | 0.000 [−0.296, 0.296] | 0.000 [−0.296, 0.296] | 0.000 [−0.296, 0.296] | 0.000 [−0.296, 0.296] | 0.000 [−0.296, 0.296] | 0.000 [−0.296, 0.296] | 0.000 [−0.296, 0.296] | 0.000 [−0.296, 0.296] |
| SYNTH | 500 µs | 5,000 ms | 5 | 0.800 [0.376, 0.964] | 2/2/0 | 0.000 [−0.394, 0.394] | 0.000 [−0.394, 0.394] | 0.000 [−0.394, 0.394] | 0.000 [−0.394, 0.394] | 0.000 [−0.394, 0.394] | 0.000 [−0.394, 0.394] | 0.000 [−0.394, 0.394] | 0.000 [−0.394, 0.394] | 0.000 [−0.394, 0.394] |
| SYNTH | 500 µs | 30,000 ms | 0 | n/a n/a | 0/0/0 | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a | n/a n/a |

**Kaplan–Meier median time to fill (ms)** over every placed shadow, censored at τ or the end of tracking ("—": survival stayed above 0.5 within τ).

| Symbol | L | τ | l3_shadow | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| SYNTH | 0 ms | 1,000 ms | 42.6 | 55.3 | 42.6 | 55.3 | 42.6 | 42.6 | 42.6 | 41.5 | 42.6 | 44.8 |
| SYNTH | 0 ms | 5,000 ms | 42.6 | 55.3 | 42.6 | 55.3 | 42.6 | 42.6 | 42.6 | 41.5 | 42.6 | 44.8 |
| SYNTH | 0 ms | 30,000 ms | 42.6 | 55.3 | 42.6 | 55.3 | 42.6 | 42.6 | 42.6 | 41.5 | 42.6 | 44.8 |
| SYNTH | 50 µs | 1,000 ms | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 |
| SYNTH | 50 µs | 5,000 ms | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 |
| SYNTH | 50 µs | 30,000 ms | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 | 19.4 |
| SYNTH | 500 µs | 1,000 ms | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 |
| SYNTH | 500 µs | 5,000 ms | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 |
| SYNTH | 500 µs | 30,000 ms | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 | 18.9 |

**Share of cancelled volume ahead of the shadow** (L3; shares cancelled, deleted or replaced away at the shadow's price until min(L3 fill, τ, end of tracking); ahead / (ahead + behind), pooled).

| Symbol | L | τ 1,000 ms | τ 5,000 ms | τ 30,000 ms |
|---|---|---|---|---|
| SYNTH | 0 ms | 0.510 (18,100 / 35,500) | 0.382 (20,500 / 53,600) | 0.268 (21,700 / 81,000) |
| SYNTH | 50 µs | 0.466 (17,700 / 38,000) | 0.466 (17,700 / 38,000) | 0.466 (17,700 / 38,000) |
| SYNTH | 500 µs | 0.495 (18,300 / 37,000) | 0.376 (20,700 / 55,100) | 0.265 (21,900 / 82,500) |

**front − ahead_L3 at checkpoints** (shares): mean absolute (mean signed) over shadows unfilled under both paths; pairs = shadows compared for RiskAdverse; mean L3 ahead for scale.

| Symbol | L | Checkpoint | Pairs | Mean L3 ahead | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| SYNTH | 0 ms | 100 ms | 2 | 3750.0 | 1150.0 (1150.0) | 397.0 (361.8) | 900.4 (900.4) | 97.7 (97.7) | 72.1 (72.1) | 116.6 (−61.7) | 158.2 (−147.9) | 410.3 (410.3) | 556.3 (556.3) |
| SYNTH | 0 ms | 1,000 ms | 1 | 6700.0 | 1800.0 (1800.0) | 806.9 (806.9) | 1395.4 (1395.4) | 107.6 (107.6) | 42.2 (42.2) | 70.8 (70.8) | 16.9 (16.9) | 559.5 (559.5) | 736.6 (736.6) |
| SYNTH | 0 ms | 5,000 ms | 1 | 4300.0 | 4200.0 (4200.0) | 4299.4 (−4299.4) | 3596.2 (3596.2) | 2255.7 (2255.7) | 2389.3 (2389.3) | 163.1 (−163.1) | 1007.9 (−1007.9) | 2742.2 (2742.2) | 3100.0 (3100.0) |
| SYNTH | 0 ms | 30,000 ms | 0 | n/a | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) |
| SYNTH | 50 µs | 100 ms | 1 | 700.0 | 600.0 (600.0) | 35.2 (−35.2) | 492.1 (492.1) | 118.0 (118.0) | 125.2 (125.2) | 178.3 (−178.3) | 306.1 (−306.1) | 320.2 (320.2) | 447.8 (447.8) |
| SYNTH | 50 µs | 1,000 ms | 0 | n/a | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) |
| SYNTH | 50 µs | 5,000 ms | 0 | n/a | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) |
| SYNTH | 50 µs | 30,000 ms | 0 | n/a | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) |
| SYNTH | 500 µs | 100 ms | 2 | 3750.0 | 1150.0 (1150.0) | 397.0 (361.8) | 900.4 (900.4) | 97.7 (97.7) | 72.1 (72.1) | 116.6 (−61.7) | 158.2 (−147.9) | 410.3 (410.3) | 556.3 (556.3) |
| SYNTH | 500 µs | 1,000 ms | 1 | 6700.0 | 1800.0 (1800.0) | 806.9 (806.9) | 1395.4 (1395.4) | 107.6 (107.6) | 42.2 (42.2) | 70.8 (70.8) | 16.9 (16.9) | 559.5 (559.5) | 736.6 (736.6) |
| SYNTH | 500 µs | 5,000 ms | 1 | 4300.0 | 4200.0 (4200.0) | 4299.4 (−4299.4) | 3596.2 (3596.2) | 2255.7 (2255.7) | 2389.3 (2389.3) | 163.1 (−163.1) | 1007.9 (−1007.9) | 2742.2 (2742.2) | 3100.0 (3100.0) |
| SYNTH | 500 µs | 30,000 ms | 0 | n/a | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) | n/a (n/a) |

### 6.2 Real data, in-sample: QQQ and AAPL, 2019-07-30, 09:30:00–09:39:00 ET

`--source 07302019.NASDAQ_ITCH50.gz.prefix-300MB --symbol QQQ --symbol AAPL --midnight-epoch-ns
1564459200000000000 --expect-design-sha256 0608a9ba…f502`, corrected implementation
`ccdd493f0812376fe0b341fccdc1a8a44fb8002e`. Input: 23,022,785 frames read, 0 invalid;
the engine stopped at the first message stamped 09:39:00 (tracking ended at the window end).
Result SHA-256 `750af975c8cb1d9ea6a93124c4a1e6244c155c7aadbe7ff7279feb24db56b9b2`
(local only). All numerical results equal the former `insample-final` run; interval-method
labels and provenance/caveat metadata changed (§6.5).

Placement: QQQ 1620 placed of 1620 scheduled; AAPL 1620 placed of 1620 scheduled (0 empty side, 0 locked/crossed, 0 not arrived on either). Tracking ended at 34740000000000 ns (window end).

**Fill fraction within τ.** L3 column: fraction [nominal 95% Wilson] over eligible shadows (n) and its fill causes queue/trade-through/book-cross. Model columns: Δ = model − L3 [nominal 95% Newcombe paired]; **bold** = nominal/exploratory H0 rejection.

| Symbol | L | τ | n | L3 fill [nominal 95%] | L3 causes | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | 540 | 0.531 [0.489, 0.573] | 58/0/229 | **−0.009 [−0.018, −0.000]** | −0.002 [−0.013, 0.010] | −0.007 [−0.017, 0.002] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.011 [−0.002, 0.024] | 0.013 [−0.001, 0.026] | −0.004 [−0.015, 0.007] | **−0.009 [−0.018, −0.000]** |
| QQQ | 0 ms | 5,000 ms | 536 | 0.789 [0.753, 0.822] | 102/1/320 | −0.004 [−0.011, 0.004] | 0.000 [−0.005, 0.005] | −0.002 [−0.010, 0.006] | 0.000 [−0.008, 0.008] | 0.000 [−0.008, 0.008] | 0.004 [−0.004, 0.011] | 0.004 [−0.004, 0.011] | −0.002 [−0.010, 0.006] | −0.004 [−0.011, 0.004] |
| QQQ | 0 ms | 30,000 ms | 511 | 0.910 [0.882, 0.932] | 126/1/338 | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] |
| QQQ | 50 µs | 1,000 ms | 539 | 0.531 [0.488, 0.572] | 57/0/229 | **−0.009 [−0.018, −0.000]** | −0.002 [−0.013, 0.010] | −0.007 [−0.017, 0.002] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.011 [−0.002, 0.024] | 0.013 [−0.001, 0.026] | −0.004 [−0.015, 0.007] | **−0.009 [−0.018, −0.000]** |
| QQQ | 50 µs | 5,000 ms | 535 | 0.789 [0.752, 0.821] | 102/1/319 | −0.004 [−0.011, 0.004] | 0.000 [−0.006, 0.006] | −0.002 [−0.010, 0.006] | 0.000 [−0.008, 0.008] | 0.000 [−0.008, 0.008] | 0.004 [−0.004, 0.011] | 0.004 [−0.004, 0.011] | −0.002 [−0.010, 0.006] | −0.004 [−0.011, 0.004] |
| QQQ | 50 µs | 30,000 ms | 510 | 0.910 [0.882, 0.932] | 126/1/337 | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] |
| QQQ | 500 µs | 1,000 ms | 539 | 0.532 [0.490, 0.574] | 56/0/231 | **−0.009 [−0.018, −0.000]** | −0.002 [−0.013, 0.010] | −0.007 [−0.017, 0.002] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.011 [−0.002, 0.024] | 0.013 [−0.001, 0.027] | −0.004 [−0.015, 0.007] | **−0.009 [−0.018, −0.000]** |
| QQQ | 500 µs | 5,000 ms | 535 | 0.789 [0.752, 0.821] | 101/1/320 | −0.004 [−0.011, 0.004] | 0.000 [−0.006, 0.006] | −0.002 [−0.010, 0.006] | 0.000 [−0.008, 0.008] | 0.000 [−0.008, 0.008] | 0.004 [−0.004, 0.011] | 0.004 [−0.004, 0.011] | −0.002 [−0.010, 0.006] | −0.004 [−0.011, 0.004] |
| QQQ | 500 µs | 30,000 ms | 510 | 0.910 [0.882, 0.932] | 125/1/338 | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] |
| AAPL | 0 ms | 1,000 ms | 540 | 0.348 [0.309, 0.389] | 76/52/60 | 0.000 [−0.004, 0.004] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.004 [−0.003, 0.010] |
| AAPL | 0 ms | 5,000 ms | 536 | 0.664 [0.623, 0.703] | 173/88/95 | −0.002 [−0.008, 0.004] | 0.000 [−0.008, 0.008] | 0.004 [−0.005, 0.012] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] |
| AAPL | 0 ms | 30,000 ms | 511 | 0.892 [0.863, 0.916] | 226/105/125 | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] |
| AAPL | 50 µs | 1,000 ms | 539 | 0.347 [0.308, 0.388] | 75/52/60 | 0.000 [−0.004, 0.004] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.004 [−0.003, 0.010] |
| AAPL | 50 µs | 5,000 ms | 535 | 0.664 [0.622, 0.702] | 172/88/95 | −0.002 [−0.008, 0.004] | 0.000 [−0.008, 0.008] | 0.004 [−0.005, 0.012] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] |
| AAPL | 50 µs | 30,000 ms | 510 | 0.892 [0.862, 0.916] | 226/105/124 | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] |
| AAPL | 500 µs | 1,000 ms | 539 | 0.347 [0.308, 0.388] | 74/53/60 | 0.000 [−0.004, 0.004] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] | 0.004 [−0.003, 0.010] |
| AAPL | 500 µs | 5,000 ms | 535 | 0.664 [0.622, 0.702] | 169/90/96 | −0.002 [−0.008, 0.004] | 0.000 [−0.008, 0.008] | 0.004 [−0.005, 0.012] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.006 [−0.002, 0.013] | 0.006 [−0.002, 0.013] |
| AAPL | 500 µs | 30,000 ms | 510 | 0.892 [0.862, 0.916] | 221/107/127 | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] |

**Kaplan–Meier median time to fill (ms)** over every placed shadow, censored at τ or the end of tracking ("—": survival stayed above 0.5 within τ).

| Symbol | L | τ | l3_shadow | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | 885.4 | 913.9 | 901.9 | 911.1 | 885.4 | 885.4 | 849.4 | 820.9 | 901.9 | 913.9 |
| QQQ | 0 ms | 5,000 ms | 885.4 | 913.9 | 901.9 | 911.1 | 885.4 | 885.4 | 849.4 | 820.9 | 901.9 | 913.9 |
| QQQ | 0 ms | 30,000 ms | 885.4 | 913.9 | 901.9 | 911.1 | 885.4 | 885.4 | 849.4 | 820.9 | 901.9 | 913.9 |
| QQQ | 50 µs | 1,000 ms | 885.3 | 913.8 | 901.9 | 911.1 | 885.3 | 885.3 | 849.4 | 820.8 | 901.9 | 913.8 |
| QQQ | 50 µs | 5,000 ms | 885.3 | 913.8 | 901.9 | 911.1 | 885.3 | 885.3 | 849.4 | 820.8 | 901.9 | 913.8 |
| QQQ | 50 µs | 30,000 ms | 885.3 | 913.8 | 901.9 | 911.1 | 885.3 | 885.3 | 849.4 | 820.8 | 901.9 | 913.8 |
| QQQ | 500 µs | 1,000 ms | 871.9 | 910.6 | 901.3 | 903.9 | 871.9 | 871.9 | 826.1 | 813.4 | 901.3 | 910.6 |
| QQQ | 500 µs | 5,000 ms | 871.9 | 910.6 | 901.3 | 903.9 | 871.9 | 871.9 | 826.1 | 813.4 | 901.3 | 910.6 |
| QQQ | 500 µs | 30,000 ms | 871.9 | 910.6 | 901.3 | 903.9 | 871.9 | 871.9 | 826.1 | 813.4 | 901.3 | 910.6 |
| AAPL | 0 ms | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 0 ms | 5,000 ms | 1877.3 | 1889.2 | 1889.2 | 1858.8 | 1858.1 | 1858.8 | 1858.1 | 1858.1 | 1858.1 | 1858.8 |
| AAPL | 0 ms | 30,000 ms | 1877.3 | 1889.2 | 1889.2 | 1858.8 | 1858.1 | 1858.8 | 1858.1 | 1858.1 | 1858.1 | 1858.8 |
| AAPL | 50 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 50 µs | 5,000 ms | 1877.2 | 1889.2 | 1889.2 | 1858.8 | 1858.1 | 1858.8 | 1858.1 | 1858.1 | 1858.1 | 1858.8 |
| AAPL | 50 µs | 30,000 ms | 1877.2 | 1889.2 | 1889.2 | 1858.8 | 1858.1 | 1858.8 | 1858.1 | 1858.1 | 1858.1 | 1858.8 |
| AAPL | 500 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 500 µs | 5,000 ms | 1888.7 | 1900.8 | 1888.7 | 1858.3 | 1857.6 | 1858.3 | 1857.6 | 1857.6 | 1857.6 | 1858.3 |
| AAPL | 500 µs | 30,000 ms | 1888.7 | 1900.8 | 1888.7 | 1858.3 | 1857.6 | 1858.3 | 1857.6 | 1857.6 | 1857.6 | 1858.3 |

**Share of cancelled volume ahead of the shadow** (L3; shares cancelled, deleted or replaced away at the shadow's price until min(L3 fill, τ, end of tracking); ahead / (ahead + behind), pooled).

| Symbol | L | τ 1,000 ms | τ 5,000 ms | τ 30,000 ms |
|---|---|---|---|---|
| QQQ | 0 ms | 0.238 (634,372 / 2,668,240) | 0.171 (737,679 / 4,306,751) | 0.145 (759,179 / 5,246,674) |
| QQQ | 50 µs | 0.238 (635,572 / 2,669,040) | 0.172 (738,879 / 4,306,751) | 0.145 (760,379 / 5,246,674) |
| QQQ | 500 µs | 0.236 (628,504 / 2,661,165) | 0.170 (731,111 / 4,305,376) | 0.143 (752,611 / 5,245,299) |
| AAPL | 0 ms | 0.543 (37,850 / 69,740) | 0.373 (49,205 / 131,858) | 0.261 (56,144 / 215,180) |
| AAPL | 50 µs | 0.543 (37,850 / 69,740) | 0.373 (49,205 / 131,858) | 0.261 (56,144 / 215,180) |
| AAPL | 500 µs | 0.546 (38,100 / 69,740) | 0.376 (49,640 / 131,858) | 0.262 (56,579 / 216,100) |

**front − ahead_L3 at checkpoints** (shares): mean absolute (mean signed) over shadows unfilled under both paths; pairs = shadows compared for RiskAdverse; mean L3 ahead for scale.

| Symbol | L | Checkpoint | Pairs | Mean L3 ahead | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 100 ms | 469 | 1041.1 | 46.7 (46.7) | 194.5 (−89.1) | 67.3 (7.8) | 153.8 (−139.0) | 146.3 (−132.1) | 234.8 (−229.0) | 261.0 (−258.0) | 101.9 (−60.3) | 76.5 (−17.6) |
| QQQ | 0 ms | 1,000 ms | 253 | 583.6 | 198.1 (198.1) | 592.8 (336.0) | 171.9 (79.4) | 228.6 (−140.4) | 208.7 (−98.0) | 330.8 (−318.0) | 358.0 (−351.2) | 188.5 (−36.3) | 172.9 (62.4) |
| QQQ | 0 ms | 5,000 ms | 113 | 292.3 | 348.1 (348.1) | 756.8 (608.5) | 229.9 (189.3) | 204.5 (−24.2) | 221.8 (30.6) | 224.2 (−210.7) | 237.1 (−233.8) | 216.5 (78.6) | 262.4 (194.3) |
| QQQ | 0 ms | 30,000 ms | 46 | 169.4 | 176.9 (176.9) | 413.7 (253.0) | 104.2 (66.9) | 118.9 (−47.4) | 119.0 (2.8) | 164.9 (−164.4) | 167.7 (−167.4) | 106.9 (12.9) | 127.3 (99.9) |
| QQQ | 50 µs | 100 ms | 469 | 1041.1 | 46.7 (46.7) | 194.8 (−89.4) | 67.3 (7.8) | 153.7 (−138.9) | 146.1 (−131.9) | 234.2 (−228.5) | 260.1 (−257.1) | 101.9 (−60.3) | 76.5 (−17.6) |
| QQQ | 50 µs | 1,000 ms | 253 | 583.6 | 198.1 (198.1) | 592.8 (336.0) | 171.9 (79.4) | 228.6 (−140.3) | 208.6 (−98.0) | 330.8 (−318.0) | 358.0 (−351.2) | 188.5 (−36.3) | 172.9 (62.4) |
| QQQ | 50 µs | 5,000 ms | 113 | 292.3 | 348.1 (348.1) | 756.8 (608.5) | 229.9 (189.3) | 204.5 (−24.2) | 221.8 (30.6) | 224.2 (−210.7) | 237.1 (−233.8) | 216.5 (78.6) | 262.4 (194.3) |
| QQQ | 50 µs | 30,000 ms | 46 | 169.4 | 176.9 (176.9) | 413.7 (253.0) | 104.2 (66.9) | 118.9 (−47.4) | 119.0 (2.8) | 164.9 (−164.4) | 167.7 (−167.4) | 106.9 (12.9) | 127.3 (99.9) |
| QQQ | 500 µs | 100 ms | 467 | 1032.4 | 45.1 (45.1) | 193.2 (−90.9) | 65.3 (6.0) | 154.2 (−140.5) | 146.3 (−133.3) | 235.5 (−230.2) | 261.6 (−258.7) | 100.9 (−62.2) | 75.0 (−19.5) |
| QQQ | 500 µs | 1,000 ms | 252 | 576.9 | 200.4 (200.4) | 581.3 (329.3) | 174.6 (81.5) | 230.1 (−139.5) | 210.3 (−97.4) | 330.9 (−316.7) | 357.1 (−350.3) | 191.1 (−35.0) | 175.4 (63.8) |
| QQQ | 500 µs | 5,000 ms | 113 | 292.3 | 348.1 (348.1) | 756.8 (608.5) | 229.9 (189.3) | 204.5 (−24.2) | 221.8 (30.6) | 223.7 (−210.2) | 236.2 (−232.9) | 216.5 (78.6) | 262.4 (194.3) |
| QQQ | 500 µs | 30,000 ms | 46 | 169.4 | 176.9 (176.9) | 413.7 (253.0) | 104.2 (66.9) | 118.9 (−47.4) | 119.0 (2.8) | 164.9 (−164.4) | 167.7 (−167.4) | 106.9 (12.9) | 127.3 (99.9) |
| AAPL | 0 ms | 100 ms | 517 | 211.0 | 0.9 (0.9) | 1.3 (−0.6) | 0.9 (0.5) | 0.9 (−0.7) | 0.8 (−0.7) | 1.5 (−1.4) | 1.8 (−1.7) | 0.9 (0.1) | 0.9 (0.4) |
| AAPL | 0 ms | 1,000 ms | 352 | 143.9 | 6.2 (6.2) | 16.8 (−2.8) | 8.5 (2.6) | 13.5 (−6.4) | 13.7 (−6.2) | 15.9 (−12.4) | 16.9 (−14.9) | 11.3 (−1.4) | 10.1 (0.9) |
| AAPL | 0 ms | 5,000 ms | 180 | 109.7 | 13.7 (13.7) | 34.1 (−9.8) | 19.4 (3.1) | 37.6 (−24.0) | 38.7 (−23.2) | 41.7 (−38.0) | 44.9 (−42.7) | 27.0 (−9.4) | 23.3 (−1.7) |
| AAPL | 0 ms | 30,000 ms | 55 | 61.9 | 23.4 (23.4) | 58.1 (22.3) | 31.1 (6.9) | 45.4 (−20.9) | 43.9 (−14.4) | 48.1 (−44.0) | 49.6 (−47.9) | 38.5 (−5.8) | 35.1 (5.4) |
| AAPL | 50 µs | 100 ms | 516 | 211.4 | 0.9 (0.9) | 1.3 (−0.6) | 0.9 (0.5) | 0.9 (−0.7) | 0.8 (−0.7) | 1.5 (−1.4) | 1.8 (−1.7) | 0.9 (0.1) | 0.9 (0.4) |
| AAPL | 50 µs | 1,000 ms | 352 | 143.9 | 6.2 (6.2) | 16.8 (−2.8) | 8.5 (2.6) | 13.5 (−6.4) | 13.7 (−6.2) | 15.9 (−12.4) | 16.9 (−14.9) | 11.3 (−1.4) | 10.1 (0.9) |
| AAPL | 50 µs | 5,000 ms | 180 | 109.7 | 13.7 (13.7) | 34.1 (−9.8) | 19.4 (3.1) | 37.6 (−24.0) | 38.7 (−23.2) | 41.7 (−38.0) | 44.9 (−42.7) | 27.0 (−9.4) | 23.3 (−1.7) |
| AAPL | 50 µs | 30,000 ms | 55 | 61.9 | 23.4 (23.4) | 58.1 (22.3) | 31.1 (6.9) | 45.4 (−20.9) | 43.9 (−14.4) | 48.1 (−44.0) | 49.6 (−47.9) | 38.5 (−5.8) | 35.1 (5.4) |
| AAPL | 500 µs | 100 ms | 516 | 212.6 | 0.9 (0.9) | 1.1 (−0.4) | 0.9 (0.6) | 0.9 (−0.7) | 0.8 (−0.7) | 1.4 (−1.3) | 1.7 (−1.6) | 0.9 (0.1) | 0.9 (0.4) |
| AAPL | 500 µs | 1,000 ms | 352 | 145.5 | 6.2 (6.2) | 16.7 (−2.7) | 8.5 (2.5) | 13.6 (−6.8) | 13.8 (−6.6) | 15.9 (−12.5) | 17.0 (−14.9) | 11.4 (−1.6) | 10.1 (0.7) |
| AAPL | 500 µs | 5,000 ms | 180 | 110.2 | 13.4 (13.4) | 34.0 (−9.9) | 19.0 (2.8) | 37.4 (−24.6) | 38.4 (−23.8) | 42.1 (−38.5) | 45.5 (−43.3) | 26.5 (−9.8) | 22.9 (−2.1) |
| AAPL | 500 µs | 30,000 ms | 55 | 61.9 | 23.4 (23.4) | 58.1 (22.3) | 31.1 (6.9) | 45.4 (−20.9) | 43.9 (−14.4) | 48.1 (−44.0) | 49.6 (−47.9) | 38.5 (−5.8) | 35.1 (5.4) |

### 6.3 Real data: October OOS acquisition and reused December validation data

Both sets use the unchanged design (`--expect-design-sha256 0608a9ba…f502`) and the corrected
`inclusive-deadlines-v2` implementation. Only October was acquired after the freeze without
prior inspection; neither corrected rerun is an untouched-implementation evaluation.
The additional symbol was selected mechanically by `--rank-activity` (A/F/E/C/X/D/U
messages inside the window, QQQ and AAPL excluded): **AMD** on October 30 and **AMZN** on
December 30. AMZN is held out only as a symbol on a previously decoded day (§2).
All result cells are below; each full `study-result.md`, including per-cell nominal/
exploratory hypothesis decisions, stays local.

#### A. 2019-10-30, QQQ, AAPL and AMD (held out), 09:30:00–09:39:00 ET

`--midnight-epoch-ns 1572408000000000000` (EDT; window 13:30:00–13:39:00 UTC). 19,845,594 frames
read (637,764,715 decompressed bytes), 0 invalid; the prefix reaches past 09:39:00, so tracking
ended at the window end. Placement: QQQ, AAPL and AMD 1,620 placed of 1,620 scheduled each.
Result SHA-256 `54bae40bc3c023fc8985c8883e13da8187ee8fd2d0e6f422f6539eebc880f87c` (local only).

| Symbol | L | τ | n | L3 fill [nominal 95%] | L3 causes | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | 540 | 0.443 [0.401, 0.485] | 43/2/194 | −0.004 [−0.010, 0.003] | −0.002 [−0.011, 0.007] | −0.002 [−0.009, 0.005] | 0.000 [−0.008, 0.008] | 0.000 [−0.008, 0.008] | 0.006 [−0.005, 0.016] | 0.006 [−0.005, 0.016] | −0.002 [−0.009, 0.005] | −0.002 [−0.009, 0.005] |
| QQQ | 0 ms | 5,000 ms | 536 | 0.737 [0.698, 0.772] | 101/2/292 | −0.007 [−0.016, 0.001] | **−0.011 [−0.021, −0.001]** | −0.002 [−0.008, 0.004] | 0.002 [−0.006, 0.010] | 0.000 [−0.007, 0.007] | 0.006 [−0.004, 0.015] | 0.006 [−0.004, 0.015] | 0.000 [−0.007, 0.007] | −0.002 [−0.008, 0.004] |
| QQQ | 0 ms | 30,000 ms | 511 | 0.888 [0.858, 0.913] | 133/2/319 | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] |
| QQQ | 50 µs | 1,000 ms | 539 | 0.443 [0.402, 0.486] | 43/2/194 | −0.004 [−0.010, 0.003] | −0.002 [−0.011, 0.007] | −0.002 [−0.009, 0.005] | 0.000 [−0.008, 0.008] | 0.000 [−0.008, 0.008] | 0.006 [−0.005, 0.016] | 0.006 [−0.005, 0.016] | −0.002 [−0.009, 0.005] | −0.002 [−0.009, 0.005] |
| QQQ | 50 µs | 5,000 ms | 535 | 0.738 [0.699, 0.774] | 101/2/292 | −0.007 [−0.016, 0.001] | **−0.011 [−0.022, −0.001]** | −0.002 [−0.008, 0.004] | 0.002 [−0.006, 0.010] | 0.000 [−0.007, 0.007] | 0.006 [−0.004, 0.015] | 0.006 [−0.004, 0.015] | 0.000 [−0.007, 0.007] | −0.002 [−0.008, 0.004] |
| QQQ | 50 µs | 30,000 ms | 510 | 0.888 [0.858, 0.913] | 133/2/318 | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] |
| QQQ | 500 µs | 1,000 ms | 539 | 0.440 [0.398, 0.482] | 41/2/194 | −0.004 [−0.010, 0.003] | −0.002 [−0.011, 0.007] | −0.002 [−0.009, 0.005] | 0.000 [−0.008, 0.008] | 0.000 [−0.008, 0.008] | 0.006 [−0.005, 0.016] | 0.006 [−0.005, 0.016] | −0.002 [−0.009, 0.005] | −0.002 [−0.009, 0.005] |
| QQQ | 500 µs | 5,000 ms | 535 | 0.736 [0.698, 0.772] | 100/2/292 | −0.006 [−0.014, 0.002] | **−0.011 [−0.022, −0.001]** | −0.002 [−0.008, 0.004] | 0.002 [−0.006, 0.010] | 0.000 [−0.007, 0.007] | 0.006 [−0.004, 0.015] | 0.006 [−0.004, 0.015] | 0.000 [−0.007, 0.007] | −0.002 [−0.008, 0.004] |
| QQQ | 500 µs | 30,000 ms | 510 | 0.890 [0.860, 0.914] | 134/2/318 | −0.002 [−0.010, 0.006] | 0.000 [−0.007, 0.007] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | −0.002 [−0.010, 0.006] | −0.002 [−0.010, 0.006] |
| AAPL | 0 ms | 1,000 ms | 540 | 0.243 [0.208, 0.280] | 35/50/46 | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.002 [−0.004, 0.008] | 0.002 [−0.004, 0.008] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] |
| AAPL | 0 ms | 5,000 ms | 536 | 0.593 [0.551, 0.634] | 127/114/77 | −0.002 [−0.007, 0.003] | 0.006 [−0.003, 0.015] | 0.002 [−0.006, 0.009] | 0.007 [−0.001, 0.016] | 0.006 [−0.002, 0.013] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.002 [−0.006, 0.009] | 0.002 [−0.006, 0.009] |
| AAPL | 0 ms | 30,000 ms | 511 | 0.779 [0.741, 0.813] | 173/141/84 | −0.004 [−0.012, 0.004] | 0.004 [−0.006, 0.014] | 0.000 [−0.008, 0.008] | 0.006 [−0.003, 0.015] | 0.004 [−0.004, 0.012] | 0.006 [−0.003, 0.015] | 0.006 [−0.003, 0.015] | 0.000 [−0.008, 0.008] | 0.000 [−0.008, 0.008] |
| AAPL | 50 µs | 1,000 ms | 539 | 0.241 [0.207, 0.279] | 34/50/46 | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.002 [−0.004, 0.008] | 0.002 [−0.004, 0.008] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] |
| AAPL | 50 µs | 5,000 ms | 535 | 0.593 [0.550, 0.633] | 127/114/76 | −0.002 [−0.007, 0.003] | 0.006 [−0.003, 0.015] | 0.002 [−0.006, 0.009] | 0.007 [−0.001, 0.016] | 0.006 [−0.002, 0.013] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.002 [−0.006, 0.009] | 0.002 [−0.006, 0.009] |
| AAPL | 50 µs | 30,000 ms | 510 | 0.778 [0.740, 0.812] | 173/141/83 | −0.004 [−0.012, 0.004] | 0.004 [−0.006, 0.014] | 0.000 [−0.008, 0.008] | 0.006 [−0.003, 0.015] | 0.004 [−0.004, 0.012] | 0.006 [−0.003, 0.015] | 0.006 [−0.003, 0.015] | 0.000 [−0.008, 0.008] | 0.000 [−0.008, 0.008] |
| AAPL | 500 µs | 1,000 ms | 539 | 0.241 [0.207, 0.279] | 34/50/46 | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] | 0.002 [−0.004, 0.008] | 0.002 [−0.004, 0.008] | 0.000 [−0.005, 0.005] | 0.000 [−0.005, 0.005] |
| AAPL | 500 µs | 5,000 ms | 535 | 0.593 [0.550, 0.633] | 127/114/76 | −0.002 [−0.007, 0.003] | 0.006 [−0.003, 0.015] | 0.002 [−0.006, 0.009] | 0.007 [−0.001, 0.016] | 0.006 [−0.002, 0.013] | 0.007 [−0.001, 0.016] | 0.007 [−0.001, 0.016] | 0.002 [−0.006, 0.009] | 0.002 [−0.006, 0.009] |
| AAPL | 500 µs | 30,000 ms | 510 | 0.778 [0.740, 0.812] | 173/141/83 | −0.004 [−0.012, 0.004] | 0.004 [−0.006, 0.014] | 0.000 [−0.008, 0.008] | 0.006 [−0.003, 0.015] | 0.004 [−0.004, 0.012] | 0.006 [−0.003, 0.015] | 0.006 [−0.003, 0.015] | 0.000 [−0.008, 0.008] | 0.000 [−0.008, 0.008] |
| AMD | 0 ms | 1,000 ms | 540 | 0.537 [0.495, 0.579] | 102/27/161 | −0.004 [−0.010, 0.003] | 0.004 [−0.007, 0.015] | 0.002 [−0.007, 0.011] | 0.007 [−0.003, 0.018] | 0.006 [−0.005, 0.016] | **0.017 [0.004, 0.029]** | **0.015 [0.003, 0.027]** | 0.004 [−0.006, 0.013] | 0.004 [−0.006, 0.013] |
| AMD | 0 ms | 5,000 ms | 536 | 0.787 [0.751, 0.820] | 190/37/195 | −0.004 [−0.011, 0.004] | −0.002 [−0.013, 0.009] | 0.000 [−0.009, 0.009] | 0.006 [−0.004, 0.016] | 0.004 [−0.007, 0.014] | 0.007 [−0.003, 0.018] | 0.006 [−0.004, 0.016] | 0.002 [−0.008, 0.012] | 0.000 [−0.009, 0.009] |
| AMD | 0 ms | 30,000 ms | 511 | 0.906 [0.878, 0.928] | 226/37/200 | −0.002 [−0.010, 0.006] | 0.000 [−0.010, 0.010] | 0.000 [−0.009, 0.009] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] |
| AMD | 50 µs | 1,000 ms | 539 | 0.538 [0.496, 0.580] | 102/27/161 | −0.004 [−0.010, 0.003] | 0.004 [−0.007, 0.015] | 0.002 [−0.007, 0.011] | 0.007 [−0.003, 0.018] | 0.006 [−0.005, 0.016] | **0.017 [0.004, 0.029]** | **0.015 [0.003, 0.027]** | 0.004 [−0.006, 0.013] | 0.004 [−0.006, 0.013] |
| AMD | 50 µs | 5,000 ms | 535 | 0.787 [0.750, 0.820] | 190/37/194 | −0.004 [−0.011, 0.004] | −0.002 [−0.013, 0.009] | 0.000 [−0.009, 0.009] | 0.006 [−0.004, 0.016] | 0.004 [−0.007, 0.014] | 0.007 [−0.003, 0.018] | 0.006 [−0.004, 0.016] | 0.002 [−0.008, 0.012] | 0.000 [−0.009, 0.009] |
| AMD | 50 µs | 30,000 ms | 510 | 0.906 [0.877, 0.928] | 225/37/200 | −0.002 [−0.010, 0.006] | 0.000 [−0.010, 0.010] | 0.000 [−0.009, 0.009] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] |
| AMD | 500 µs | 1,000 ms | 539 | 0.538 [0.496, 0.580] | 102/27/161 | −0.004 [−0.010, 0.003] | 0.004 [−0.007, 0.015] | 0.002 [−0.007, 0.011] | 0.007 [−0.003, 0.018] | 0.006 [−0.005, 0.016] | **0.017 [0.004, 0.029]** | **0.015 [0.003, 0.027]** | 0.004 [−0.006, 0.013] | 0.004 [−0.006, 0.013] |
| AMD | 500 µs | 5,000 ms | 535 | 0.787 [0.750, 0.820] | 190/37/194 | −0.004 [−0.011, 0.004] | −0.002 [−0.013, 0.009] | 0.000 [−0.009, 0.009] | 0.006 [−0.004, 0.016] | 0.004 [−0.007, 0.014] | 0.007 [−0.003, 0.018] | 0.006 [−0.004, 0.016] | 0.002 [−0.008, 0.012] | 0.000 [−0.009, 0.009] |
| AMD | 500 µs | 30,000 ms | 510 | 0.906 [0.877, 0.928] | 225/37/200 | −0.002 [−0.010, 0.006] | 0.000 [−0.010, 0.010] | 0.000 [−0.009, 0.009] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] |

Kaplan–Meier median time to fill (ms; "—": not reached within τ):

| Symbol | L | τ | l3_shadow | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| QQQ | 0 ms | 5,000 ms | 1383.2 | 1442.1 | 1442.9 | 1433.3 | 1383.2 | 1383.2 | 1341.3 | 1341.3 | 1433.3 | 1433.3 |
| QQQ | 0 ms | 30,000 ms | 1383.2 | 1442.1 | 1442.9 | 1433.3 | 1383.2 | 1383.2 | 1341.3 | 1341.3 | 1433.3 | 1433.3 |
| QQQ | 50 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| QQQ | 50 µs | 5,000 ms | 1383.1 | 1442.1 | 1442.8 | 1433.2 | 1383.1 | 1383.1 | 1341.3 | 1341.3 | 1433.2 | 1433.2 |
| QQQ | 50 µs | 30,000 ms | 1383.1 | 1442.1 | 1442.8 | 1433.2 | 1383.1 | 1383.1 | 1341.3 | 1341.3 | 1433.2 | 1433.2 |
| QQQ | 500 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| QQQ | 500 µs | 5,000 ms | 1432.8 | 1446.4 | 1448.8 | 1442.4 | 1432.8 | 1432.8 | 1351.3 | 1351.3 | 1442.4 | 1442.4 |
| QQQ | 500 µs | 30,000 ms | 1432.8 | 1446.4 | 1448.8 | 1442.4 | 1432.8 | 1432.8 | 1351.3 | 1351.3 | 1442.4 | 1442.4 |
| AAPL | 0 ms | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 0 ms | 5,000 ms | 3010.6 | 3092.6 | 3009.7 | 3038.3 | 3004.3 | 3004.3 | 2980.6 | 2980.6 | 3038.3 | 3038.3 |
| AAPL | 0 ms | 30,000 ms | 3010.6 | 3092.6 | 3009.7 | 3038.3 | 3004.3 | 3004.3 | 2980.6 | 2980.6 | 3038.3 | 3038.3 |
| AAPL | 50 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 50 µs | 5,000 ms | 3010.5 | 3092.5 | 3009.7 | 3038.3 | 3004.3 | 3004.3 | 2980.5 | 2980.5 | 3038.3 | 3038.3 |
| AAPL | 50 µs | 30,000 ms | 3010.5 | 3092.5 | 3009.7 | 3038.3 | 3004.3 | 3004.3 | 2980.5 | 2980.5 | 3038.3 | 3038.3 |
| AAPL | 500 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 500 µs | 5,000 ms | 3010.1 | 3092.1 | 3009.2 | 3037.8 | 3003.8 | 3003.8 | 2980.1 | 2980.1 | 3037.8 | 3037.8 |
| AAPL | 500 µs | 30,000 ms | 3010.1 | 3092.1 | 3009.2 | 3037.8 | 3003.8 | 3003.8 | 2980.1 | 2980.1 | 3037.8 | 3037.8 |
| AMD | 0 ms | 1,000 ms | 829.3 | 846.0 | 827.4 | 826.0 | 822.2 | 822.2 | 806.7 | 817.8 | 822.2 | 826.0 |
| AMD | 0 ms | 5,000 ms | 829.3 | 846.0 | 827.4 | 826.0 | 822.2 | 822.2 | 806.7 | 817.8 | 822.2 | 826.0 |
| AMD | 0 ms | 30,000 ms | 829.3 | 846.0 | 827.4 | 826.0 | 822.2 | 822.2 | 806.7 | 817.8 | 822.2 | 826.0 |
| AMD | 50 µs | 1,000 ms | 829.2 | 846.0 | 827.3 | 826.0 | 822.1 | 822.1 | 806.7 | 817.7 | 822.1 | 826.0 |
| AMD | 50 µs | 5,000 ms | 829.2 | 846.0 | 827.3 | 826.0 | 822.1 | 822.1 | 806.7 | 817.7 | 822.1 | 826.0 |
| AMD | 50 µs | 30,000 ms | 829.2 | 846.0 | 827.3 | 826.0 | 822.1 | 822.1 | 806.7 | 817.7 | 822.1 | 826.0 |
| AMD | 500 µs | 1,000 ms | 828.8 | 845.5 | 826.9 | 825.5 | 821.7 | 821.7 | 806.2 | 817.3 | 821.7 | 825.5 |
| AMD | 500 µs | 5,000 ms | 828.8 | 845.5 | 826.9 | 825.5 | 821.7 | 821.7 | 806.2 | 817.3 | 821.7 | 825.5 |
| AMD | 500 µs | 30,000 ms | 828.8 | 845.5 | 826.9 | 825.5 | 821.7 | 821.7 | 806.2 | 817.3 | 821.7 | 825.5 |

Share of cancelled volume ahead of the shadow (L3):

| Symbol | L | τ 1,000 ms | τ 5,000 ms | τ 30,000 ms |
|---|---|---|---|---|
| QQQ | 0 ms | 0.240 (484,579 / 2,018,219) | 0.147 (595,769 / 4,063,966) | 0.111 (631,064 / 5,696,296) |
| QQQ | 50 µs | 0.240 (484,379 / 2,018,019) | 0.147 (595,569 / 4,063,966) | 0.111 (630,864 / 5,696,096) |
| QQQ | 500 µs | 0.243 (492,379 / 2,030,019) | 0.148 (603,369 / 4,085,476) | 0.112 (638,564 / 5,716,716) |
| AAPL | 0 ms | 0.592 (25,163 / 42,495) | 0.472 (52,354 / 110,887) | 0.293 (58,251 / 198,996) |
| AAPL | 50 µs | 0.592 (25,163 / 42,495) | 0.472 (52,354 / 110,887) | 0.293 (58,251 / 198,996) |
| AAPL | 500 µs | 0.593 (25,093 / 42,287) | 0.472 (52,284 / 110,679) | 0.293 (58,281 / 198,788) |
| AMD | 0 ms | 0.120 (142,133 / 1,185,158) | 0.073 (167,217 / 2,293,505) | 0.062 (188,526 / 3,056,516) |
| AMD | 50 µs | 0.120 (142,133 / 1,185,158) | 0.073 (167,217 / 2,293,505) | 0.062 (188,526 / 3,056,516) |
| AMD | 500 µs | 0.120 (142,624 / 1,187,838) | 0.073 (167,708 / 2,293,736) | 0.062 (189,017 / 3,056,596) |

front − ahead_L3 at checkpoints (shares; mean absolute (mean signed)):

| Symbol | L | Checkpoint | Pairs | Mean L3 ahead | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 100 ms | 484 | 1032.7 | 29.9 (29.9) | 112.6 (−56.9) | 41.6 (5.8) | 98.9 (−88.9) | 97.5 (−87.8) | 140.6 (−137.7) | 156.9 (−155.7) | 65.2 (−39.0) | 50.9 (−13.9) |
| QQQ | 0 ms | 1,000 ms | 301 | 599.8 | 161.6 (161.6) | 650.6 (241.7) | 162.1 (46.3) | 261.5 (−195.5) | 244.2 (−154.0) | 413.6 (−399.7) | 443.0 (−434.7) | 198.3 (−81.6) | 170.1 (19.8) |
| QQQ | 0 ms | 5,000 ms | 141 | 336.3 | 387.9 (387.9) | 1025.3 (818.4) | 249.2 (180.7) | 203.7 (−51.7) | 214.8 (25.8) | 290.5 (−282.5) | 304.8 (−301.4) | 214.5 (48.0) | 271.8 (194.4) |
| QQQ | 0 ms | 30,000 ms | 57 | 56.1 | 328.8 (328.8) | 489.1 (450.6) | 153.7 (129.3) | 79.6 (51.2) | 129.7 (116.8) | 54.3 (−54.3) | 54.5 (−54.5) | 106.8 (87.5) | 200.6 (196.7) |
| QQQ | 50 µs | 100 ms | 484 | 1032.7 | 29.9 (29.9) | 112.6 (−56.9) | 41.6 (5.8) | 98.9 (−88.9) | 97.5 (−87.8) | 140.6 (−137.7) | 156.9 (−155.7) | 65.2 (−39.0) | 50.9 (−13.9) |
| QQQ | 50 µs | 1,000 ms | 300 | 592.1 | 162.1 (162.1) | 652.8 (242.5) | 162.6 (46.5) | 262.4 (−196.2) | 245.0 (−154.5) | 415.0 (−401.1) | 444.5 (−436.1) | 198.9 (−81.9) | 170.6 (19.8) |
| QQQ | 50 µs | 5,000 ms | 140 | 338.0 | 390.7 (390.7) | 1028.7 (820.4) | 250.5 (182.5) | 204.7 (−51.6) | 215.9 (26.3) | 291.9 (−283.8) | 306.3 (−302.9) | 215.7 (48.7) | 273.7 (195.8) |
| QQQ | 50 µs | 30,000 ms | 57 | 56.1 | 328.8 (328.8) | 489.1 (450.6) | 153.7 (129.3) | 79.6 (51.2) | 129.7 (116.8) | 54.3 (−54.3) | 54.5 (−54.5) | 106.8 (87.5) | 200.6 (196.7) |
| QQQ | 500 µs | 100 ms | 487 | 1030.2 | 29.9 (29.9) | 112.2 (−56.4) | 41.5 (6.0) | 98.3 (−88.3) | 96.9 (−87.2) | 140.0 (−137.1) | 155.6 (−154.4) | 64.9 (−38.5) | 50.7 (−13.5) |
| QQQ | 500 µs | 1,000 ms | 302 | 586.9 | 158.7 (158.7) | 655.9 (248.2) | 159.4 (44.0) | 261.1 (−195.2) | 243.7 (−154.0) | 411.6 (−397.7) | 440.4 (−432.1) | 197.0 (−82.2) | 168.0 (18.1) |
| QQQ | 500 µs | 5,000 ms | 141 | 334.2 | 378.7 (378.7) | 1046.8 (839.2) | 242.2 (174.4) | 202.0 (−52.6) | 212.3 (24.0) | 288.4 (−280.4) | 302.6 (−299.2) | 211.2 (45.4) | 266.8 (189.3) |
| QQQ | 500 µs | 30,000 ms | 56 | 53.6 | 307.9 (307.9) | 494.7 (455.5) | 148.6 (123.7) | 80.0 (51.1) | 127.1 (113.9) | 51.6 (−51.6) | 51.8 (−51.8) | 106.0 (86.4) | 194.8 (190.9) |
| AAPL | 0 ms | 100 ms | 520 | 397.8 | 0.4 (0.4) | 1.2 (−0.6) | 0.8 (−0.0) | 1.6 (−1.3) | 1.6 (−1.3) | 1.6 (−1.4) | 1.6 (−1.5) | 1.5 (−0.8) | 1.4 (−0.7) |
| AAPL | 0 ms | 1,000 ms | 409 | 377.6 | 3.6 (3.6) | 9.9 (−4.5) | 5.6 (0.7) | 10.0 (−7.1) | 10.1 (−7.3) | 11.4 (−10.1) | 12.3 (−11.7) | 8.2 (−2.8) | 7.1 (−1.0) |
| AAPL | 0 ms | 5,000 ms | 218 | 363.0 | 21.2 (21.2) | 34.1 (−6.6) | 23.9 (11.7) | 25.7 (−16.0) | 26.5 (−16.4) | 31.7 (−28.4) | 34.5 (−32.2) | 23.2 (−1.6) | 22.2 (4.9) |
| AAPL | 0 ms | 30,000 ms | 113 | 366.7 | 12.1 (12.1) | 51.8 (−15.6) | 22.2 (−4.2) | 43.0 (−31.7) | 41.9 (−28.6) | 48.0 (−46.1) | 49.9 (−44.7) | 34.4 (−19.6) | 32.0 (−12.5) |
| AAPL | 50 µs | 100 ms | 520 | 397.8 | 0.4 (0.4) | 1.2 (−0.6) | 0.8 (−0.0) | 1.6 (−1.3) | 1.6 (−1.3) | 1.6 (−1.4) | 1.6 (−1.5) | 1.5 (−0.8) | 1.4 (−0.7) |
| AAPL | 50 µs | 1,000 ms | 409 | 377.6 | 3.6 (3.6) | 9.9 (−4.5) | 5.6 (0.7) | 10.0 (−7.1) | 10.1 (−7.3) | 11.4 (−10.1) | 12.3 (−11.7) | 8.2 (−2.8) | 7.1 (−1.0) |
| AAPL | 50 µs | 5,000 ms | 218 | 363.0 | 21.2 (21.2) | 34.1 (−6.6) | 23.9 (11.7) | 25.7 (−16.0) | 26.5 (−16.4) | 31.7 (−28.4) | 34.5 (−32.2) | 23.2 (−1.6) | 22.2 (4.9) |
| AAPL | 50 µs | 30,000 ms | 113 | 366.7 | 12.1 (12.1) | 51.8 (−15.6) | 22.2 (−4.2) | 43.0 (−31.7) | 41.9 (−28.6) | 48.0 (−46.1) | 49.9 (−44.7) | 34.4 (−19.6) | 32.0 (−12.5) |
| AAPL | 500 µs | 100 ms | 520 | 397.6 | 0.4 (0.4) | 1.2 (−0.6) | 0.8 (−0.0) | 1.6 (−1.3) | 1.6 (−1.3) | 1.6 (−1.4) | 1.6 (−1.5) | 1.5 (−0.8) | 1.4 (−0.7) |
| AAPL | 500 µs | 1,000 ms | 409 | 377.8 | 3.6 (3.6) | 9.9 (−4.5) | 5.6 (0.7) | 10.0 (−7.1) | 10.1 (−7.3) | 11.4 (−10.1) | 12.3 (−11.7) | 8.2 (−2.8) | 7.1 (−1.0) |
| AAPL | 500 µs | 5,000 ms | 218 | 363.4 | 21.2 (21.2) | 34.1 (−6.6) | 23.9 (11.7) | 25.7 (−16.0) | 26.5 (−16.4) | 31.7 (−28.4) | 34.5 (−32.2) | 23.2 (−1.6) | 22.2 (4.9) |
| AAPL | 500 µs | 30,000 ms | 113 | 366.7 | 12.1 (12.1) | 51.8 (−15.6) | 22.2 (−4.2) | 43.0 (−31.7) | 41.9 (−28.6) | 48.0 (−46.1) | 49.9 (−44.7) | 34.4 (−19.6) | 32.0 (−12.5) |
| AMD | 0 ms | 100 ms | 492 | 749.7 | 4.9 (4.9) | 85.4 (−16.6) | 11.0 (−2.8) | 23.6 (−19.6) | 21.0 (−16.4) | 50.7 (−49.0) | 58.8 (−56.2) | 16.1 (−10.6) | 11.6 (−3.9) |
| AMD | 0 ms | 1,000 ms | 250 | 745.6 | 52.6 (52.6) | 412.9 (75.7) | 76.4 (5.8) | 150.4 (−89.1) | 140.3 (−69.2) | 231.9 (−207.5) | 251.3 (−229.3) | 116.1 (−43.6) | 91.9 (−4.4) |
| AMD | 0 ms | 5,000 ms | 114 | 531.9 | 114.1 (114.1) | 947.4 (573.2) | 150.1 (17.0) | 249.4 (−138.6) | 240.2 (−102.3) | 325.8 (−306.6) | 348.8 (−321.6) | 202.3 (−65.1) | 175.1 (12.1) |
| AMD | 0 ms | 30,000 ms | 48 | 261.1 | 262.2 (262.2) | 794.0 (556.6) | 236.0 (118.9) | 241.5 (−71.9) | 247.4 (−11.9) | 255.8 (−254.6) | 267.2 (−263.7) | 233.3 (−1.8) | 246.2 (113.1) |
| AMD | 50 µs | 100 ms | 492 | 749.7 | 4.9 (4.9) | 85.4 (−16.6) | 11.0 (−2.8) | 23.6 (−19.6) | 21.0 (−16.4) | 50.7 (−49.0) | 58.8 (−56.2) | 16.1 (−10.6) | 11.6 (−3.9) |
| AMD | 50 µs | 1,000 ms | 249 | 747.7 | 51.8 (51.8) | 413.8 (76.9) | 75.9 (5.0) | 150.9 (−89.3) | 140.9 (−69.5) | 232.1 (−207.7) | 251.4 (−229.4) | 116.2 (−44.1) | 91.7 (−5.0) |
| AMD | 50 µs | 5,000 ms | 114 | 531.9 | 114.1 (114.1) | 947.4 (573.2) | 150.1 (17.0) | 249.4 (−138.6) | 240.2 (−102.3) | 325.8 (−306.6) | 348.8 (−321.6) | 202.3 (−65.1) | 175.1 (12.1) |
| AMD | 50 µs | 30,000 ms | 48 | 261.1 | 262.2 (262.2) | 794.0 (556.6) | 236.0 (118.9) | 241.5 (−71.9) | 247.4 (−11.9) | 255.8 (−254.6) | 267.2 (−263.7) | 233.3 (−1.8) | 246.2 (113.1) |
| AMD | 500 µs | 100 ms | 491 | 749.4 | 4.9 (4.9) | 87.3 (−18.4) | 11.2 (−3.0) | 23.9 (−19.9) | 21.1 (−16.5) | 52.2 (−50.5) | 61.0 (−58.4) | 16.4 (−10.9) | 11.8 (−4.0) |
| AMD | 500 µs | 1,000 ms | 249 | 747.7 | 51.8 (51.8) | 417.7 (72.9) | 75.9 (4.9) | 151.0 (−89.3) | 140.9 (−69.5) | 232.1 (−207.7) | 252.6 (−230.5) | 116.3 (−44.1) | 91.7 (−5.0) |
| AMD | 500 µs | 5,000 ms | 114 | 531.9 | 114.1 (114.1) | 946.6 (572.4) | 150.1 (17.0) | 249.4 (−138.6) | 240.2 (−102.3) | 325.8 (−306.6) | 348.8 (−321.6) | 202.3 (−65.1) | 175.1 (12.1) |
| AMD | 500 µs | 30,000 ms | 48 | 261.1 | 262.2 (262.2) | 794.0 (556.6) | 236.0 (118.9) | 241.5 (−71.9) | 247.4 (−11.9) | 255.8 (−254.6) | 267.2 (−263.7) | 233.3 (−1.8) | 246.2 (113.1) |

#### B. Reused 2019-12-30 validation: QQQ/AAPL previously inspected; AMZN held-out symbol, 09:30:00–09:31:50.3 ET

`--midnight-epoch-ns 1577682000000000000` (EST; window from 14:30:00 UTC). 9,766,139 frames, 0
invalid; the 120 MiB prefix ends inside a frame (stop `truncated`), so tracking ended at the last
message, 09:31:50.305 ET (110.3 s of the window): 333 shadows per symbol placed (111 per
latency; 1,287 never arrived), at most 110 eligible per cell. Result SHA-256
`cc2a5796a4bdd75623d7082165905f71bbb7815a1936437075f6521063d0071d` (local only).

| Symbol | L | τ | n | L3 fill [nominal 95%] | L3 causes | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | 110 | 0.527 [0.435, 0.618] | 11/0/47 | −0.018 [−0.048, 0.012] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] |
| QQQ | 0 ms | 5,000 ms | 106 | 0.811 [0.726, 0.874] | 19/0/67 | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] |
| QQQ | 0 ms | 30,000 ms | 81 | 0.901 [0.817, 0.949] | 19/0/54 | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] |
| QQQ | 50 µs | 1,000 ms | 110 | 0.527 [0.435, 0.618] | 11/0/47 | −0.018 [−0.048, 0.012] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] |
| QQQ | 50 µs | 5,000 ms | 106 | 0.811 [0.726, 0.874] | 19/0/67 | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] |
| QQQ | 50 µs | 30,000 ms | 81 | 0.901 [0.817, 0.949] | 19/0/54 | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] |
| QQQ | 500 µs | 1,000 ms | 110 | 0.527 [0.435, 0.618] | 10/0/48 | −0.018 [−0.048, 0.012] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] | −0.009 [−0.044, 0.026] |
| QQQ | 500 µs | 5,000 ms | 106 | 0.811 [0.726, 0.874] | 18/0/68 | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] | −0.019 [−0.058, 0.018] |
| QQQ | 500 µs | 30,000 ms | 81 | 0.901 [0.817, 0.949] | 18/0/55 | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] | 0.000 [−0.043, 0.043] |
| AAPL | 0 ms | 1,000 ms | 110 | 0.364 [0.280, 0.457] | 14/19/7 | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] |
| AAPL | 0 ms | 5,000 ms | 106 | 0.679 [0.585, 0.760] | 31/30/11 | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] |
| AAPL | 0 ms | 30,000 ms | 81 | 0.790 [0.689, 0.865] | 28/28/8 | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] |
| AAPL | 50 µs | 1,000 ms | 110 | 0.364 [0.280, 0.457] | 14/19/7 | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] |
| AAPL | 50 µs | 5,000 ms | 106 | 0.679 [0.585, 0.760] | 31/30/11 | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] |
| AAPL | 50 µs | 30,000 ms | 81 | 0.790 [0.689, 0.865] | 28/28/8 | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] |
| AAPL | 500 µs | 1,000 ms | 110 | 0.364 [0.280, 0.457] | 14/19/7 | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] | 0.000 [−0.020, 0.020] |
| AAPL | 500 µs | 5,000 ms | 106 | 0.679 [0.585, 0.760] | 31/30/11 | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] | 0.000 [−0.022, 0.022] |
| AAPL | 500 µs | 30,000 ms | 81 | 0.790 [0.689, 0.865] | 27/29/8 | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] | 0.000 [−0.035, 0.035] |
| AMZN | 0 ms | 1,000 ms | 110 | 0.127 [0.077, 0.202] | 2/12/0 | 0.000 [−0.031, 0.031] | 0.009 [−0.025, 0.045] | 0.000 [−0.031, 0.031] | 0.000 [−0.031, 0.031] | 0.000 [−0.031, 0.031] | 0.009 [−0.025, 0.045] | 0.009 [−0.025, 0.045] | 0.000 [−0.031, 0.031] | 0.000 [−0.031, 0.031] |
| AMZN | 0 ms | 5,000 ms | 106 | 0.349 [0.265, 0.444] | 11/26/0 | 0.000 [−0.021, 0.021] | 0.009 [−0.018, 0.037] | 0.000 [−0.021, 0.021] | 0.000 [−0.021, 0.021] | 0.000 [−0.021, 0.021] | 0.009 [−0.018, 0.037] | 0.009 [−0.018, 0.037] | 0.000 [−0.021, 0.021] | 0.000 [−0.021, 0.021] |
| AMZN | 0 ms | 30,000 ms | 81 | 0.556 [0.447, 0.659] | 16/29/0 | 0.000 [−0.024, 0.024] | 0.012 [−0.021, 0.046] | 0.000 [−0.024, 0.024] | 0.000 [−0.024, 0.024] | 0.000 [−0.024, 0.024] | 0.012 [−0.021, 0.046] | 0.012 [−0.021, 0.046] | 0.000 [−0.024, 0.024] | 0.000 [−0.024, 0.024] |
| AMZN | 50 µs | 1,000 ms | 110 | 0.127 [0.077, 0.202] | 2/12/0 | 0.000 [−0.031, 0.031] | 0.009 [−0.025, 0.045] | 0.000 [−0.031, 0.031] | 0.000 [−0.031, 0.031] | 0.000 [−0.031, 0.031] | 0.009 [−0.025, 0.045] | 0.009 [−0.025, 0.045] | 0.000 [−0.031, 0.031] | 0.000 [−0.031, 0.031] |
| AMZN | 50 µs | 5,000 ms | 106 | 0.349 [0.265, 0.444] | 11/26/0 | 0.000 [−0.021, 0.021] | 0.009 [−0.018, 0.037] | 0.000 [−0.021, 0.021] | 0.000 [−0.021, 0.021] | 0.000 [−0.021, 0.021] | 0.009 [−0.018, 0.037] | 0.009 [−0.018, 0.037] | 0.000 [−0.021, 0.021] | 0.000 [−0.021, 0.021] |
| AMZN | 50 µs | 30,000 ms | 81 | 0.556 [0.447, 0.659] | 16/29/0 | 0.000 [−0.024, 0.024] | 0.012 [−0.021, 0.046] | 0.000 [−0.024, 0.024] | 0.000 [−0.024, 0.024] | 0.000 [−0.024, 0.024] | 0.012 [−0.021, 0.046] | 0.012 [−0.021, 0.046] | 0.000 [−0.024, 0.024] | 0.000 [−0.024, 0.024] |
| AMZN | 500 µs | 1,000 ms | 110 | 0.127 [0.077, 0.202] | 2/12/0 | 0.000 [−0.031, 0.031] | 0.009 [−0.025, 0.045] | 0.000 [−0.031, 0.031] | 0.000 [−0.031, 0.031] | 0.000 [−0.031, 0.031] | 0.009 [−0.025, 0.045] | 0.009 [−0.025, 0.045] | 0.000 [−0.031, 0.031] | 0.000 [−0.031, 0.031] |
| AMZN | 500 µs | 5,000 ms | 106 | 0.349 [0.265, 0.444] | 11/26/0 | 0.000 [−0.021, 0.021] | 0.009 [−0.018, 0.037] | 0.000 [−0.021, 0.021] | 0.000 [−0.021, 0.021] | 0.000 [−0.021, 0.021] | 0.009 [−0.018, 0.037] | 0.009 [−0.018, 0.037] | 0.000 [−0.021, 0.021] | 0.000 [−0.021, 0.021] |
| AMZN | 500 µs | 30,000 ms | 81 | 0.556 [0.447, 0.659] | 16/29/0 | 0.000 [−0.024, 0.024] | 0.012 [−0.021, 0.046] | 0.000 [−0.024, 0.024] | 0.000 [−0.024, 0.024] | 0.000 [−0.024, 0.024] | 0.012 [−0.021, 0.046] | 0.012 [−0.021, 0.046] | 0.000 [−0.024, 0.024] | 0.000 [−0.024, 0.024] |

Kaplan–Meier median time to fill (ms; "—": not reached within τ):

| Symbol | L | τ | l3_shadow | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | 969.0 | 994.6 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 |
| QQQ | 0 ms | 5,000 ms | 969.0 | 994.6 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 |
| QQQ | 0 ms | 30,000 ms | 969.0 | 994.6 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 |
| QQQ | 50 µs | 1,000 ms | 969.0 | 994.6 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 |
| QQQ | 50 µs | 5,000 ms | 969.0 | 994.6 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 |
| QQQ | 50 µs | 30,000 ms | 969.0 | 994.6 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 | 975.7 |
| QQQ | 500 µs | 1,000 ms | 968.5 | 994.1 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 |
| QQQ | 500 µs | 5,000 ms | 968.5 | 994.1 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 |
| QQQ | 500 µs | 30,000 ms | 968.5 | 994.1 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 | 975.2 |
| AAPL | 0 ms | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 0 ms | 5,000 ms | 2294.7 | 2317.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 |
| AAPL | 0 ms | 30,000 ms | 2294.7 | 2317.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 | 2136.7 |
| AAPL | 50 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 50 µs | 5,000 ms | 2294.7 | 2317.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 |
| AAPL | 50 µs | 30,000 ms | 2294.7 | 2317.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 | 2136.6 |
| AAPL | 500 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 500 µs | 5,000 ms | 2294.2 | 2317.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 |
| AAPL | 500 µs | 30,000 ms | 2294.2 | 2317.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 | 2136.2 |
| AMZN | 0 ms | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AMZN | 0 ms | 5,000 ms | — | — | — | — | — | — | — | — | — | — |
| AMZN | 0 ms | 30,000 ms | 13210.6 | 13210.6 | 11322.1 | 13210.6 | 13210.6 | 13210.6 | 11322.1 | 11322.1 | 13210.6 | 13210.6 |
| AMZN | 50 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AMZN | 50 µs | 5,000 ms | — | — | — | — | — | — | — | — | — | — |
| AMZN | 50 µs | 30,000 ms | 13210.6 | 13210.6 | 11322.1 | 13210.6 | 13210.6 | 13210.6 | 11322.1 | 11322.1 | 13210.6 | 13210.6 |
| AMZN | 500 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AMZN | 500 µs | 5,000 ms | — | — | — | — | — | — | — | — | — | — |
| AMZN | 500 µs | 30,000 ms | 13210.1 | 13210.1 | 11321.6 | 13210.1 | 13210.1 | 13210.1 | 11321.6 | 11321.6 | 13210.1 | 13210.1 |

Share of cancelled volume ahead of the shadow (L3):

| Symbol | L | τ 1,000 ms | τ 5,000 ms | τ 30,000 ms |
|---|---|---|---|---|
| QQQ | 0 ms | 0.287 (167,839 / 584,917) | 0.249 (234,537 / 940,811) | 0.212 (249,537 / 1,174,493) |
| QQQ | 50 µs | 0.287 (167,839 / 584,917) | 0.249 (234,537 / 940,811) | 0.212 (249,537 / 1,174,493) |
| QQQ | 500 µs | 0.287 (167,939 / 584,917) | 0.249 (234,637 / 940,811) | 0.213 (249,737 / 1,174,493) |
| AAPL | 0 ms | 0.405 (6,078 / 14,995) | 0.290 (9,965 / 34,357) | 0.221 (11,002 / 49,809) |
| AAPL | 50 µs | 0.403 (6,078 / 15,095) | 0.289 (9,965 / 34,457) | 0.220 (11,002 / 49,909) |
| AAPL | 500 µs | 0.404 (6,178 / 15,295) | 0.292 (9,965 / 34,157) | 0.221 (11,002 / 49,709) |
| AMZN | 0 ms | 0.810 (2,083 / 2,573) | 0.593 (3,971 / 6,698) | 0.563 (5,264 / 9,356) |
| AMZN | 50 µs | 0.810 (2,083 / 2,573) | 0.593 (3,971 / 6,698) | 0.563 (5,264 / 9,356) |
| AMZN | 500 µs | 0.810 (2,083 / 2,573) | 0.593 (3,971 / 6,698) | 0.563 (5,264 / 9,356) |

front − ahead_L3 at checkpoints (shares; mean absolute (mean signed)):

| Symbol | L | Checkpoint | Pairs | Mean L3 ahead | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 100 ms | 99 | 2261.5 | 39.5 (39.5) | 151.3 (−59.9) | 59.6 (−2.6) | 199.1 (−179.5) | 201.2 (−177.9) | 250.6 (−249.6) | 274.5 (−274.4) | 123.6 (−99.9) | 89.1 (−57.7) |
| QQQ | 0 ms | 1,000 ms | 52 | 1786.1 | 200.8 (200.8) | 1277.7 (−128.1) | 240.5 (−5.0) | 770.5 (−690.7) | 753.9 (−640.4) | 1048.0 (−1043.7) | 1157.6 (−1143.1) | 512.4 (−404.9) | 367.5 (−207.1) |
| QQQ | 0 ms | 5,000 ms | 20 | 920.0 | 562.1 (562.1) | 901.1 (215.9) | 431.0 (378.5) | 513.6 (−211.5) | 553.0 (−151.7) | 567.1 (−566.3) | 650.8 (−641.8) | 459.1 (76.6) | 498.0 (261.9) |
| QQQ | 0 ms | 30,000 ms | 8 | 150.0 | 208.8 (208.8) | 713.7 (532.0) | 116.0 (41.4) | 169.9 (−21.4) | 186.0 (23.5) | 150.0 (−150.0) | 152.0 (−147.7) | 148.1 (25.8) | 174.6 (121.0) |
| QQQ | 50 µs | 100 ms | 99 | 2261.5 | 39.5 (39.5) | 151.3 (−59.9) | 59.6 (−2.6) | 199.1 (−179.5) | 201.2 (−177.9) | 250.6 (−249.6) | 274.5 (−274.4) | 123.6 (−99.9) | 89.1 (−57.7) |
| QQQ | 50 µs | 1,000 ms | 52 | 1786.1 | 200.8 (200.8) | 1277.7 (−128.1) | 240.5 (−5.0) | 770.5 (−690.7) | 753.9 (−640.4) | 1048.0 (−1043.7) | 1157.6 (−1143.1) | 512.4 (−404.9) | 367.5 (−207.1) |
| QQQ | 50 µs | 5,000 ms | 20 | 920.0 | 562.1 (562.1) | 901.1 (215.9) | 431.0 (378.5) | 513.6 (−211.5) | 553.0 (−151.7) | 567.1 (−566.3) | 650.8 (−641.8) | 459.1 (76.6) | 498.0 (261.9) |
| QQQ | 50 µs | 30,000 ms | 8 | 150.0 | 208.8 (208.8) | 713.7 (532.0) | 116.0 (41.4) | 169.9 (−21.4) | 186.0 (23.5) | 150.0 (−150.0) | 152.0 (−147.7) | 148.1 (25.8) | 174.6 (121.0) |
| QQQ | 500 µs | 100 ms | 99 | 2262.5 | 39.5 (39.5) | 151.3 (−62.0) | 60.2 (−3.2) | 201.0 (−181.4) | 203.1 (−179.8) | 253.1 (−252.1) | 277.2 (−277.2) | 124.9 (−101.2) | 90.1 (−58.6) |
| QQQ | 500 µs | 1,000 ms | 52 | 1788.0 | 200.8 (200.8) | 1277.2 (−128.7) | 240.6 (−5.1) | 770.6 (−690.9) | 753.9 (−640.4) | 1049.0 (−1044.7) | 1159.0 (−1144.5) | 512.5 (−405.0) | 367.5 (−207.1) |
| QQQ | 500 µs | 5,000 ms | 20 | 925.0 | 562.1 (562.1) | 899.5 (214.3) | 430.7 (378.2) | 513.2 (−212.0) | 553.0 (−151.8) | 570.7 (−569.8) | 655.0 (−646.1) | 458.7 (76.2) | 498.0 (261.9) |
| QQQ | 500 µs | 30,000 ms | 8 | 150.0 | 221.2 (221.2) | 713.7 (532.0) | 120.2 (45.6) | 170.8 (−20.6) | 187.9 (25.5) | 150.0 (−150.0) | 152.0 (−147.7) | 150.1 (27.7) | 179.0 (125.4) |
| AAPL | 0 ms | 100 ms | 102 | 262.4 | 1.3 (1.3) | 1.6 (−0.4) | 1.4 (1.1) | 1.8 (0.4) | 1.6 (0.4) | 2.1 (−0.5) | 2.3 (−0.8) | 1.6 (1.0) | 1.4 (1.2) |
| AAPL | 0 ms | 1,000 ms | 70 | 179.1 | 15.0 (15.0) | 28.1 (−15.2) | 15.4 (8.1) | 19.8 (−12.0) | 20.0 (−11.9) | 26.9 (−24.5) | 30.6 (−28.9) | 16.9 (0.2) | 16.1 (6.1) |
| AAPL | 0 ms | 5,000 ms | 34 | 65.8 | 25.0 (25.0) | 67.5 (8.6) | 27.4 (14.1) | 32.5 (−14.7) | 33.6 (−10.8) | 43.7 (−40.7) | 50.2 (−48.4) | 26.5 (0.6) | 27.1 (13.1) |
| AAPL | 0 ms | 30,000 ms | 17 | 29.4 | 8.4 (8.4) | 46.5 (−4.6) | 13.8 (−7.1) | 21.5 (−16.9) | 21.5 (−13.9) | 28.2 (−28.2) | 29.3 (−29.3) | 18.1 (−11.8) | 14.8 (−3.8) |
| AAPL | 50 µs | 100 ms | 102 | 262.6 | 1.3 (1.3) | 1.6 (−0.4) | 1.4 (1.1) | 1.8 (0.4) | 1.6 (0.4) | 2.1 (−0.5) | 2.3 (−0.8) | 1.6 (1.0) | 1.4 (1.2) |
| AAPL | 50 µs | 1,000 ms | 70 | 179.1 | 15.0 (15.0) | 28.1 (−15.2) | 15.4 (8.1) | 19.8 (−12.0) | 20.0 (−11.9) | 26.9 (−24.5) | 30.6 (−28.9) | 16.9 (0.2) | 16.1 (6.1) |
| AAPL | 50 µs | 5,000 ms | 34 | 65.8 | 25.0 (25.0) | 67.5 (8.6) | 27.4 (14.1) | 32.5 (−14.7) | 33.6 (−10.8) | 43.7 (−40.7) | 50.2 (−48.4) | 26.5 (0.6) | 27.1 (13.1) |
| AAPL | 50 µs | 30,000 ms | 17 | 29.4 | 8.4 (8.4) | 46.5 (−4.6) | 13.8 (−7.1) | 21.5 (−16.9) | 21.5 (−13.9) | 28.2 (−28.2) | 29.3 (−29.3) | 18.1 (−11.8) | 14.8 (−3.8) |
| AAPL | 500 µs | 100 ms | 102 | 260.6 | 1.3 (1.3) | 1.6 (−0.4) | 1.4 (1.1) | 1.8 (0.4) | 1.6 (0.4) | 2.1 (−0.5) | 2.3 (−0.8) | 1.6 (1.0) | 1.4 (1.2) |
| AAPL | 500 µs | 1,000 ms | 70 | 176.3 | 15.0 (15.0) | 28.1 (−15.2) | 15.4 (8.1) | 19.8 (−12.0) | 20.0 (−11.9) | 26.9 (−24.5) | 30.6 (−28.9) | 16.9 (0.2) | 16.1 (6.1) |
| AAPL | 500 µs | 5,000 ms | 34 | 62.9 | 22.1 (22.1) | 63.9 (12.2) | 25.2 (11.8) | 31.2 (−15.9) | 31.4 (−13.0) | 40.7 (−37.7) | 45.7 (−43.8) | 24.6 (−1.3) | 24.5 (10.5) |
| AAPL | 500 µs | 30,000 ms | 17 | 29.4 | 8.4 (8.4) | 46.5 (−4.6) | 13.8 (−7.1) | 21.5 (−16.9) | 21.5 (−13.9) | 28.2 (−28.2) | 29.3 (−29.3) | 18.1 (−11.8) | 14.8 (−3.8) |
| AMZN | 0 ms | 100 ms | 109 | 107.5 | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) |
| AMZN | 0 ms | 1,000 ms | 96 | 94.2 | 0.3 (0.3) | 1.9 (−1.3) | 0.8 (−0.1) | 1.4 (−1.2) | 1.3 (−1.2) | 1.8 (−1.7) | 2.0 (−2.0) | 1.1 (−0.5) | 0.9 (−0.3) |
| AMZN | 0 ms | 5,000 ms | 69 | 67.1 | 4.6 (4.6) | 9.1 (−1.9) | 4.9 (2.5) | 4.5 (−3.0) | 4.0 (−2.5) | 7.1 (−6.9) | 8.7 (−7.7) | 4.0 (0.3) | 3.7 (1.8) |
| AMZN | 0 ms | 30,000 ms | 36 | 15.4 | 0.5 (0.5) | 2.8 (−1.9) | 1.7 (−0.7) | 4.2 (−3.6) | 4.1 (−3.6) | 5.5 (−5.3) | 5.8 (−5.6) | 2.3 (−1.3) | 1.6 (−0.6) |
| AMZN | 50 µs | 100 ms | 109 | 107.5 | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) |
| AMZN | 50 µs | 1,000 ms | 96 | 94.2 | 0.3 (0.3) | 1.9 (−1.3) | 0.8 (−0.1) | 1.4 (−1.2) | 1.3 (−1.2) | 1.8 (−1.7) | 2.0 (−2.0) | 1.1 (−0.5) | 0.9 (−0.3) |
| AMZN | 50 µs | 5,000 ms | 69 | 67.1 | 4.6 (4.6) | 9.1 (−1.9) | 4.9 (2.5) | 4.5 (−3.0) | 4.0 (−2.5) | 7.1 (−6.9) | 8.7 (−7.7) | 4.0 (0.3) | 3.7 (1.8) |
| AMZN | 50 µs | 30,000 ms | 36 | 15.4 | 0.5 (0.5) | 2.8 (−1.9) | 1.7 (−0.7) | 4.2 (−3.6) | 4.1 (−3.6) | 5.5 (−5.3) | 5.8 (−5.6) | 2.3 (−1.3) | 1.6 (−0.6) |
| AMZN | 500 µs | 100 ms | 109 | 106.8 | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) | 0.0 (0.0) |
| AMZN | 500 µs | 1,000 ms | 96 | 94.2 | 0.3 (0.3) | 1.9 (−1.3) | 0.8 (−0.1) | 1.4 (−1.2) | 1.3 (−1.2) | 1.8 (−1.7) | 2.0 (−2.0) | 1.1 (−0.5) | 0.9 (−0.3) |
| AMZN | 500 µs | 5,000 ms | 69 | 67.1 | 4.6 (4.6) | 9.1 (−1.9) | 4.9 (2.5) | 4.5 (−3.0) | 4.0 (−2.5) | 7.1 (−6.9) | 8.7 (−7.7) | 4.0 (0.3) | 3.7 (1.8) |
| AMZN | 500 µs | 30,000 ms | 36 | 15.4 | 0.5 (0.5) | 2.8 (−1.9) | 1.7 (−0.7) | 4.2 (−3.6) | 4.1 (−3.6) | 5.5 (−5.3) | 5.8 (−5.6) | 2.3 (−1.3) | 1.6 (−0.6) |

### 6.4 Published synthetic artefact

`web/public/study-results/queue-shadow/study-result.json`, 202,303 bytes, SHA-256
`9c172c712440c557903bf8f9a06ab90e1875eb38ff2e47cd61218d635313e1af` (also in
`study-result.json.sha256`). Generated by the corrected release binary of implementation commit
`ccdd493f0812376fe0b341fccdc1a8a44fb8002e`, recorded in `generator.source_commit`, with
`engine.revision` `inclusive-deadlines-v2`. The generator seed, event count and source stream
hash are unchanged. The deterministic byte-identity test generates two fresh outputs from the
same binary and compares their bytes and hash files. Data class `synthetic`; labels, interval
limitations and caveats travel with the file.

### 6.5 Before/after implementation correction

Compared with the prior local runs `synthetic`, `insample-final`, `oos-1030` and `oos-1230`:

| Cohort | Fill/KM cells | Numerically changed | Checkpoint cells | Numerically changed | Cancellation cells | Numerically changed |
|---|---:|---:|---:|---:|---:|---:|
| Synthetic | 90 | 0 | 108 | 0 | 9 | 0 |
| July in-sample | 180 | 0 | 216 | 0 | 18 | 0 |
| October OOS-A | 270 | 0 | 324 | 0 | 27 | 0 |
| December reused validation / held-out symbol | 270 | 0 | 324 | 0 | 27 | 0 |
| Total | 810 | 0 | 972 | 0 | 81 | 0 |

Every numerical scalar, availability state, cause count, placement count, tracking bound and
hypothesis decision is unchanged: maximum absolute numerical change **0**. Thus no observed
cell depended on the corrected boundary handling; this does not make the original bug valid.
The hand-built regression streams exercise it directly (§12). All 810 serialized fill/KM
cell objects do change textually because their interval `method` labels now say **nominal 95%**.
All four `design.json` files are byte-for-byte unchanged, as are their frozen canonical
`Design` hashes and the source-input hashes. The design hash covers the canonical design
object, not the pretty-printed `design.json` envelope that contains it. Artefact byte hashes differ
because implementation/source-commit metadata, interval wording, hypothesis definitions and
dependence/multiplicity caveats are now explicit.

### 6.6 Prospective two-day extension (pre-commitment `a19a31ae1a`)

Exactly the two prefixes selected in §2 were fetched on 2026-11-19, after
pre-commitment `a19a31ae1a689f75a92d6004db2c7e967efd6e29`. Each request was
`curl -r 0-299999999` from the [EMI directory](https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/),
HTTP 206, `Content-Length: 300000000`, with no retries or prefix extension.
The rights restrictions in §4 apply unchanged; all files and outputs remain local.

| New clean day | File under `D:/black box/data/itch/` | Bytes | SHA-256 |
|---|---|---:|---|
| 2019-01-30 (OOS-C) | `01302019.NASDAQ_ITCH50.gz.prefix-300MB` | 300,000,000 | `845f02172015a166518ac4398700981f52e5e301d68aaeb6b043908feeb73d8c` |
| 2020-01-30 (OOS-D) | `01302020.NASDAQ_ITCH50.gz.prefix-300MB` | 300,000,000 | `6b13a10a763378e5da7b9c83ac1742de7e90fc7206339de27a7d1c7e61af1f34` |

The C/D labels avoid reusing the historical B label for December validation.
January 2019 transfer: 01:51:33–01:52:39 UTC, `Content-Range: bytes
0-299999999/4764426091`, `Last-Modified: Thu, 31 Jan 2019 05:18:15 GMT`.
January 2020 transfer: 01:52:40–01:56:26 UTC, `Content-Range: bytes
0-299999999/5597158940`, `Last-Modified: Fri, 31 Jan 2020 05:18:45 GMT`.
Local `dl-start-*.txt` and `range-headers-*.txt` retain the acquisition records.

| Day | Midnight ET epoch ns | UTC design window | Last complete exchange timestamp / tracking end | Reaches 09:39? | Frames / invalid | Held-out symbol (activity count) |
|---|---:|---|---|---|---:|---|
| 2019-01-30 | 1548824400000000000 | 14:30–14:39 | 09:38:54.128393000 ET / 34734128393000 ns | No: 534.128393 s after 09:30 | 23,729,118 / 0 | SPY (139,816; QQQ 170,749, AAPL 130,309) |
| 2020-01-30 | 1580360400000000000 | 14:30–14:39 | 09:34:31.554873308 ET / 34471554873308 ns | No: 271.554873308 s after 09:30 | 23,421,204 / 0 | SPY (102,855; QQQ 144,918, AAPL 67,147) |

Both January days are EST (UTC−05:00); midnight epochs were computed with Python
`datetime` and `zoneinfo.ZoneInfo("America/New_York")`, not a fixed summer offset.
Both inputs stop with `truncated`, at decompressed byte 751,420,758 (4 bytes of
an incomplete frame) and 745,079,996 (20 bytes), respectively. Neither covers
the full frozen window: rule 10 ends tracking at the last complete message;
rule 11 excludes arrivals without a full horizon from fill fractions and
retains right censoring for Kaplan–Meier. The ranking similarly ends at the
prefix cut, as specified by the unchanged activity rule. No design parameter
was shortened and no later data were fetched to repair coverage.

Release binary built on branch `p10/oos` at `a19a31ae1a` with
`CARGO_PROFILE_RELEASE_DEBUG=0 cargo build --release -p bb-app`, copied to
`D:/black box/data/queue-shadow/p10-oos/blackbox.exe`, SHA-256
`9d786eb9caa26101efbcc2bc7230ce7273aae7ad74545f9fb0fd47e17b19d12b`.
The engine is unchanged `inclusive-deadlines-v2`; both runs accepted the frozen
`--expect-design-sha256 0608a9ba4af5b7f03119277570999af47b8212b9285e18a8f1f73f240846f502`.
Unlike October, these data were acquired after the corrected engine existed.

Reproduction from that commit (fresh output directories required):

```sh
CARGO_PROFILE_RELEASE_DEBUG=0 cargo build --release -p bb-app
mkdir -p "D:/black box/data/queue-shadow/p10-oos"
cp target/release/blackbox.exe "D:/black box/data/queue-shadow/p10-oos/"
cd "D:/black box/data/queue-shadow/p10-oos"
export BB_GIT_SHA=a19a31ae1a689f75a92d6004db2c7e967efd6e29
export DESIGN=0608a9ba4af5b7f03119277570999af47b8212b9285e18a8f1f73f240846f502
for spec in '01302019 1548824400000000000' '01302020 1580360400000000000'; do
    set -- $spec
    ./blackbox.exe study queue-shadow --source "../../itch/$1.NASDAQ_ITCH50.gz.prefix-300MB" \
        --symbol X --midnight-epoch-ns "$2" --rank-activity \
        --expect-design-sha256 "$DESIGN" --out "activity-$1"
    # SPY is the highest-ranked symbol other than QQQ/AAPL on both inputs.
    ./blackbox.exe study queue-shadow --source "../../itch/$1.NASDAQ_ITCH50.gz.prefix-300MB" \
        --symbol QQQ --symbol AAPL --symbol SPY --midnight-epoch-ns "$2" \
        --expect-design-sha256 "$DESIGN" --out "oos-$1"
done
```

#### C. Clean 2019-01-30 prospective data holdout (partial window)

Local output `p10-oos/oos-01302019/study-result.json`, SHA-256
`004915f2064d546cfc39c6186e75e19c04d3916493e7790fb3f99855d77655b0`.
All cells follow, including all latencies, horizons, models and checkpoints;
intervals and decisions are nominal/exploratory with the §9 caveats.

Placement: QQQ 1605 placed of 1620 scheduled (0 empty side, 0 locked/crossed, 15 not arrived); AAPL 1605 placed of 1620 scheduled (0 empty side, 0 locked/crossed, 15 not arrived); SPY 1605 placed of 1620 scheduled (0 empty side, 0 locked/crossed, 15 not arrived). Tracking ended at 34734128393000 ns (end of input).

**Fill fraction within τ.** L3 column: fraction [nominal 95% Wilson] over eligible shadows (n) and its fill causes queue/trade-through/book-cross. Model columns: Δ = model − L3 [nominal 95% Newcombe paired]; **bold** = nominal/exploratory H0 rejection.

| Symbol | L | τ | n | L3 fill [nominal 95%] | L3 causes | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | 534 | 0.558 [0.516, 0.600] | 112/4/182 | **−0.017 [−0.028, −0.005]** | −0.007 [−0.022, 0.007] | −0.004 [−0.017, 0.009] | 0.002 [−0.013, 0.017] | 0.002 [−0.013, 0.017] | 0.009 [−0.007, 0.026] | 0.011 [−0.006, 0.028] | 0.002 [−0.013, 0.017] | −0.004 [−0.017, 0.009] |
| QQQ | 0 ms | 5,000 ms | 530 | 0.819 [0.784, 0.849] | 169/4/261 | **−0.011 [−0.022, −0.001]** | −0.004 [−0.016, 0.008] | −0.006 [−0.016, 0.004] | −0.002 [−0.013, 0.009] | −0.002 [−0.013, 0.009] | 0.002 [−0.011, 0.014] | 0.002 [−0.011, 0.014] | −0.002 [−0.013, 0.009] | −0.006 [−0.016, 0.004] |
| QQQ | 0 ms | 30,000 ms | 505 | 0.937 [0.912, 0.955] | 188/4/281 | −0.002 [−0.011, 0.006] | 0.000 [−0.009, 0.009] | −0.002 [−0.011, 0.006] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.000 [−0.009, 0.009] | −0.002 [−0.011, 0.006] |
| QQQ | 50 µs | 1,000 ms | 534 | 0.558 [0.516, 0.600] | 112/4/182 | **−0.017 [−0.028, −0.005]** | −0.007 [−0.022, 0.007] | −0.004 [−0.017, 0.009] | 0.002 [−0.013, 0.017] | 0.002 [−0.013, 0.017] | 0.009 [−0.007, 0.026] | 0.011 [−0.006, 0.028] | 0.002 [−0.013, 0.017] | −0.004 [−0.017, 0.009] |
| QQQ | 50 µs | 5,000 ms | 530 | 0.819 [0.784, 0.849] | 169/4/261 | **−0.011 [−0.022, −0.001]** | −0.004 [−0.016, 0.008] | −0.006 [−0.016, 0.004] | −0.002 [−0.013, 0.009] | −0.002 [−0.013, 0.009] | 0.002 [−0.011, 0.014] | 0.002 [−0.011, 0.014] | −0.002 [−0.013, 0.009] | −0.006 [−0.016, 0.004] |
| QQQ | 50 µs | 30,000 ms | 505 | 0.937 [0.912, 0.955] | 188/4/281 | −0.002 [−0.011, 0.006] | 0.000 [−0.009, 0.009] | −0.002 [−0.011, 0.006] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.000 [−0.009, 0.009] | −0.002 [−0.011, 0.006] |
| QQQ | 500 µs | 1,000 ms | 534 | 0.554 [0.512, 0.596] | 89/4/203 | **−0.015 [−0.026, −0.004]** | −0.006 [−0.019, 0.008] | −0.002 [−0.015, 0.011] | 0.004 [−0.010, 0.018] | 0.004 [−0.010, 0.018] | 0.011 [−0.005, 0.027] | 0.013 [−0.003, 0.029] | 0.004 [−0.010, 0.018] | −0.002 [−0.015, 0.011] |
| QQQ | 500 µs | 5,000 ms | 530 | 0.817 [0.782, 0.848] | 147/4/282 | −0.009 [−0.020, 0.000] | −0.002 [−0.013, 0.009] | −0.004 [−0.013, 0.006] | 0.000 [−0.011, 0.011] | 0.000 [−0.011, 0.011] | 0.004 [−0.008, 0.016] | 0.004 [−0.008, 0.016] | 0.000 [−0.011, 0.011] | −0.004 [−0.013, 0.006] |
| QQQ | 500 µs | 30,000 ms | 505 | 0.937 [0.912, 0.955] | 170/4/299 | −0.002 [−0.011, 0.006] | 0.000 [−0.009, 0.009] | −0.002 [−0.011, 0.006] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.002 [−0.008, 0.012] | 0.002 [−0.008, 0.012] | 0.000 [−0.009, 0.009] | −0.002 [−0.011, 0.006] |
| AAPL | 0 ms | 1,000 ms | 534 | 0.504 [0.461, 0.546] | 106/88/75 | −0.002 [−0.007, 0.003] | **0.013 [0.002, 0.025]** | 0.007 [−0.001, 0.016] | **0.013 [0.003, 0.023]** | **0.013 [0.003, 0.023]** | **0.017 [0.005, 0.028]** | **0.013 [0.002, 0.025]** | **0.009 [0.000, 0.018]** | 0.007 [−0.001, 0.016] |
| AAPL | 0 ms | 5,000 ms | 530 | 0.802 [0.766, 0.834] | 196/138/91 | 0.000 [−0.006, 0.006] | −0.002 [−0.009, 0.005] | 0.002 [−0.005, 0.009] | 0.006 [−0.003, 0.014] | 0.006 [−0.003, 0.014] | 0.006 [−0.003, 0.014] | 0.006 [−0.003, 0.014] | 0.004 [−0.004, 0.012] | 0.002 [−0.005, 0.009] |
| AAPL | 0 ms | 30,000 ms | 505 | 0.911 [0.883, 0.933] | 228/141/91 | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.002 [−0.006, 0.010] | 0.002 [−0.006, 0.010] | 0.002 [−0.006, 0.010] | 0.004 [−0.005, 0.013] | 0.004 [−0.005, 0.013] | 0.002 [−0.006, 0.010] | 0.002 [−0.006, 0.010] |
| AAPL | 50 µs | 1,000 ms | 534 | 0.504 [0.461, 0.546] | 106/88/75 | −0.002 [−0.007, 0.003] | **0.013 [0.002, 0.025]** | 0.007 [−0.001, 0.016] | **0.013 [0.003, 0.023]** | **0.013 [0.003, 0.023]** | **0.017 [0.005, 0.028]** | **0.013 [0.002, 0.025]** | **0.009 [0.000, 0.018]** | 0.007 [−0.001, 0.016] |
| AAPL | 50 µs | 5,000 ms | 530 | 0.802 [0.766, 0.834] | 196/138/91 | 0.000 [−0.006, 0.006] | −0.002 [−0.009, 0.005] | 0.002 [−0.005, 0.009] | 0.006 [−0.003, 0.014] | 0.006 [−0.003, 0.014] | 0.006 [−0.003, 0.014] | 0.006 [−0.003, 0.014] | 0.004 [−0.004, 0.012] | 0.002 [−0.005, 0.009] |
| AAPL | 50 µs | 30,000 ms | 505 | 0.911 [0.883, 0.933] | 228/141/91 | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.002 [−0.006, 0.010] | 0.002 [−0.006, 0.010] | 0.002 [−0.006, 0.010] | 0.004 [−0.005, 0.013] | 0.004 [−0.005, 0.013] | 0.002 [−0.006, 0.010] | 0.002 [−0.006, 0.010] |
| AAPL | 500 µs | 1,000 ms | 534 | 0.504 [0.461, 0.546] | 106/88/75 | −0.002 [−0.007, 0.003] | **0.013 [0.002, 0.025]** | 0.007 [−0.001, 0.016] | **0.013 [0.003, 0.023]** | **0.013 [0.003, 0.023]** | **0.017 [0.005, 0.028]** | **0.013 [0.002, 0.025]** | **0.009 [0.000, 0.018]** | 0.007 [−0.001, 0.016] |
| AAPL | 500 µs | 5,000 ms | 530 | 0.798 [0.762, 0.830] | 195/137/91 | 0.000 [−0.006, 0.006] | −0.002 [−0.009, 0.005] | 0.002 [−0.005, 0.009] | 0.006 [−0.003, 0.014] | 0.006 [−0.003, 0.014] | 0.006 [−0.003, 0.014] | 0.006 [−0.003, 0.014] | 0.004 [−0.004, 0.012] | 0.002 [−0.005, 0.009] |
| AAPL | 500 µs | 30,000 ms | 505 | 0.911 [0.883, 0.933] | 229/140/91 | 0.000 [−0.007, 0.007] | 0.000 [−0.007, 0.007] | 0.002 [−0.006, 0.010] | 0.002 [−0.006, 0.010] | 0.002 [−0.006, 0.010] | 0.004 [−0.005, 0.013] | 0.004 [−0.005, 0.013] | 0.002 [−0.006, 0.010] | 0.002 [−0.006, 0.010] |
| SPY | 0 ms | 1,000 ms | 534 | 0.571 [0.529, 0.612] | 250/2/53 | **−0.022 [−0.036, −0.009]** | −0.011 [−0.024, 0.002] | **−0.015 [−0.028, −0.002]** | −0.006 [−0.018, 0.007] | −0.006 [−0.018, 0.007] | 0.000 [−0.013, 0.013] | 0.004 [−0.009, 0.017] | −0.009 [−0.021, 0.002] | **−0.015 [−0.028, −0.002]** |
| SPY | 0 ms | 5,000 ms | 530 | 0.842 [0.808, 0.870] | 357/3/86 | **−0.015 [−0.027, −0.004]** | −0.011 [−0.025, 0.001] | −0.008 [−0.020, 0.004] | −0.006 [−0.017, 0.006] | −0.008 [−0.020, 0.004] | 0.000 [−0.012, 0.012] | 0.000 [−0.012, 0.012] | −0.006 [−0.017, 0.006] | −0.011 [−0.025, 0.001] |
| SPY | 0 ms | 30,000 ms | 505 | 0.941 [0.916, 0.958] | 380/3/92 | 0.000 [−0.008, 0.008] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] | 0.006 [−0.004, 0.017] | 0.006 [−0.004, 0.017] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] |
| SPY | 50 µs | 1,000 ms | 534 | 0.571 [0.529, 0.612] | 250/2/53 | **−0.022 [−0.036, −0.009]** | −0.011 [−0.024, 0.002] | **−0.015 [−0.028, −0.002]** | −0.006 [−0.018, 0.007] | −0.006 [−0.018, 0.007] | 0.000 [−0.013, 0.013] | 0.004 [−0.009, 0.017] | −0.009 [−0.021, 0.002] | **−0.015 [−0.028, −0.002]** |
| SPY | 50 µs | 5,000 ms | 530 | 0.842 [0.808, 0.870] | 357/3/86 | **−0.015 [−0.027, −0.004]** | −0.011 [−0.025, 0.001] | −0.008 [−0.020, 0.004] | −0.006 [−0.017, 0.006] | −0.008 [−0.020, 0.004] | 0.000 [−0.012, 0.012] | 0.000 [−0.012, 0.012] | −0.006 [−0.017, 0.006] | −0.011 [−0.025, 0.001] |
| SPY | 50 µs | 30,000 ms | 505 | 0.941 [0.916, 0.958] | 380/3/92 | 0.000 [−0.008, 0.008] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] | 0.006 [−0.004, 0.017] | 0.006 [−0.004, 0.017] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] |
| SPY | 500 µs | 1,000 ms | 534 | 0.575 [0.533, 0.616] | 117/4/186 | **−0.024 [−0.038, −0.011]** | −0.013 [−0.029, 0.002] | **−0.017 [−0.030, −0.003]** | −0.007 [−0.021, 0.006] | −0.007 [−0.021, 0.006] | 0.009 [−0.006, 0.025] | 0.013 [−0.002, 0.029] | **−0.015 [−0.028, −0.002]** | **−0.019 [−0.033, −0.005]** |
| SPY | 500 µs | 5,000 ms | 530 | 0.840 [0.806, 0.868] | 207/5/233 | **−0.015 [−0.027, −0.004]** | −0.011 [−0.025, 0.001] | −0.008 [−0.020, 0.004] | −0.006 [−0.017, 0.006] | −0.008 [−0.020, 0.004] | 0.000 [−0.012, 0.012] | 0.000 [−0.012, 0.012] | −0.006 [−0.017, 0.006] | −0.011 [−0.025, 0.001] |
| SPY | 500 µs | 30,000 ms | 505 | 0.941 [0.916, 0.958] | 241/5/229 | 0.000 [−0.008, 0.008] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] | 0.006 [−0.004, 0.017] | 0.006 [−0.004, 0.017] | 0.002 [−0.006, 0.011] | 0.002 [−0.006, 0.011] |

**Kaplan–Meier median time to fill (ms)** over every placed shadow, censored at τ or the end of tracking ("—": survival stayed above 0.5 within τ).

| Symbol | L | τ | l3_shadow | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | 743.9 | 806.6 | 781.4 | 781.4 | 769.6 | 769.6 | 750.9 | 750.9 | 769.6 | 781.4 |
| QQQ | 0 ms | 5,000 ms | 743.9 | 806.6 | 781.4 | 781.4 | 769.6 | 769.6 | 750.9 | 750.9 | 769.6 | 781.4 |
| QQQ | 0 ms | 30,000 ms | 743.9 | 806.6 | 781.4 | 781.4 | 769.6 | 769.6 | 750.9 | 750.9 | 769.6 | 781.4 |
| QQQ | 50 µs | 1,000 ms | 743.9 | 806.5 | 781.4 | 781.4 | 769.5 | 769.5 | 750.8 | 750.8 | 769.5 | 781.4 |
| QQQ | 50 µs | 5,000 ms | 743.9 | 806.5 | 781.4 | 781.4 | 769.5 | 769.5 | 750.8 | 750.8 | 769.5 | 781.4 |
| QQQ | 50 µs | 30,000 ms | 743.9 | 806.5 | 781.4 | 781.4 | 769.5 | 769.5 | 750.8 | 750.8 | 769.5 | 781.4 |
| QQQ | 500 µs | 1,000 ms | 769.1 | 848.9 | 806.1 | 806.1 | 780.9 | 780.9 | 769.9 | 769.9 | 780.9 | 806.1 |
| QQQ | 500 µs | 5,000 ms | 769.1 | 848.9 | 806.1 | 806.1 | 780.9 | 780.9 | 769.9 | 769.9 | 780.9 | 806.1 |
| QQQ | 500 µs | 30,000 ms | 769.1 | 848.9 | 806.1 | 806.1 | 780.9 | 780.9 | 769.9 | 769.9 | 780.9 | 806.1 |
| AAPL | 0 ms | 1,000 ms | 977.4 | 985.6 | 919.8 | 928.2 | 921.9 | 921.9 | 890.2 | 919.8 | 928.1 | 928.2 |
| AAPL | 0 ms | 5,000 ms | 977.4 | 985.6 | 919.8 | 928.2 | 921.9 | 921.9 | 890.2 | 919.8 | 928.1 | 928.2 |
| AAPL | 0 ms | 30,000 ms | 977.4 | 985.6 | 919.8 | 928.2 | 921.9 | 921.9 | 890.2 | 919.8 | 928.1 | 928.2 |
| AAPL | 50 µs | 1,000 ms | 977.3 | 985.6 | 919.8 | 928.1 | 921.8 | 921.8 | 890.2 | 919.8 | 928.0 | 928.1 |
| AAPL | 50 µs | 5,000 ms | 977.3 | 985.6 | 919.8 | 928.1 | 921.8 | 921.8 | 890.2 | 919.8 | 928.0 | 928.1 |
| AAPL | 50 µs | 30,000 ms | 977.3 | 985.6 | 919.8 | 928.1 | 921.8 | 921.8 | 890.2 | 919.8 | 928.0 | 928.1 |
| AAPL | 500 µs | 1,000 ms | 974.0 | 976.9 | 893.4 | 927.6 | 919.3 | 919.3 | 889.2 | 893.4 | 922.3 | 927.6 |
| AAPL | 500 µs | 5,000 ms | 974.0 | 976.9 | 893.4 | 927.6 | 919.3 | 919.3 | 889.2 | 893.4 | 922.3 | 927.6 |
| AAPL | 500 µs | 30,000 ms | 974.0 | 976.9 | 893.4 | 927.6 | 919.3 | 919.3 | 889.2 | 893.4 | 922.3 | 927.6 |
| SPY | 0 ms | 1,000 ms | 702.7 | 784.1 | 751.7 | 749.2 | 693.2 | 693.2 | 690.3 | 672.2 | 734.9 | 749.2 |
| SPY | 0 ms | 5,000 ms | 702.7 | 784.1 | 751.7 | 749.2 | 693.2 | 693.2 | 690.3 | 672.2 | 734.9 | 749.2 |
| SPY | 0 ms | 30,000 ms | 702.7 | 784.1 | 751.7 | 749.2 | 693.2 | 693.2 | 690.3 | 672.2 | 734.9 | 749.2 |
| SPY | 50 µs | 1,000 ms | 702.7 | 784.0 | 751.7 | 749.2 | 693.2 | 693.2 | 690.2 | 672.2 | 734.8 | 749.2 |
| SPY | 50 µs | 5,000 ms | 702.7 | 784.0 | 751.7 | 749.2 | 693.2 | 693.2 | 690.2 | 672.2 | 734.8 | 749.2 |
| SPY | 50 µs | 30,000 ms | 702.7 | 784.0 | 751.7 | 749.2 | 693.2 | 693.2 | 690.2 | 672.2 | 734.8 | 749.2 |
| SPY | 500 µs | 1,000 ms | 715.3 | 795.7 | 757.5 | 751.2 | 702.2 | 703.2 | 690.9 | 680.6 | 744.1 | 751.2 |
| SPY | 500 µs | 5,000 ms | 715.3 | 795.7 | 757.5 | 751.2 | 702.2 | 703.2 | 690.9 | 680.6 | 744.1 | 751.2 |
| SPY | 500 µs | 30,000 ms | 715.3 | 795.7 | 757.5 | 751.2 | 702.2 | 703.2 | 690.9 | 680.6 | 744.1 | 751.2 |

**Share of cancelled volume ahead of the shadow** (L3; shares cancelled, deleted or replaced away at the shadow's price until min(L3 fill, τ, end of tracking); ahead / (ahead + behind), pooled).

| Symbol | L | τ 1,000 ms | τ 5,000 ms | τ 30,000 ms |
|---|---|---|---|---|
| QQQ | 0 ms | 0.173 (378,205 / 2,189,268) | 0.118 (429,082 / 3,623,960) | 0.098 (460,882 / 4,684,019) |
| QQQ | 50 µs | 0.173 (378,205 / 2,189,274) | 0.118 (429,082 / 3,623,960) | 0.098 (460,882 / 4,684,019) |
| QQQ | 500 µs | 0.175 (388,526 / 2,225,231) | 0.119 (438,900 / 3,683,670) | 0.099 (470,800 / 4,751,023) |
| AAPL | 0 ms | 0.304 (74,544 / 245,194) | 0.175 (88,821 / 507,761) | 0.133 (94,536 / 710,364) |
| AAPL | 50 µs | 0.304 (74,544 / 245,194) | 0.175 (88,821 / 507,761) | 0.133 (94,536 / 710,364) |
| AAPL | 500 µs | 0.309 (74,844 / 242,460) | 0.177 (89,121 / 502,127) | 0.134 (94,836 / 709,446) |
| SPY | 0 ms | 0.171 (181,681 / 1,060,293) | 0.107 (212,597 / 1,980,781) | 0.080 (217,661 / 2,712,461) |
| SPY | 50 µs | 0.172 (182,181 / 1,060,293) | 0.108 (213,097 / 1,980,781) | 0.080 (218,161 / 2,712,461) |
| SPY | 500 µs | 0.174 (185,608 / 1,066,410) | 0.109 (216,866 / 1,989,476) | 0.082 (222,031 / 2,721,744) |

**front − ahead_L3 at checkpoints** (shares): mean absolute (mean signed) over shadows unfilled under both paths; pairs = shadows compared for RiskAdverse; mean L3 ahead for scale.

| Symbol | L | Checkpoint | Pairs | Mean L3 ahead | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 100 ms | 455 | 759.9 | 29.3 (29.3) | 162.0 (−74.0) | 50.8 (−2.8) | 130.8 (−112.0) | 125.8 (−104.6) | 192.8 (−186.3) | 213.6 (−209.7) | 87.7 (−54.8) | 64.9 (−22.6) |
| QQQ | 0 ms | 1,000 ms | 236 | 488.3 | 127.8 (127.8) | 578.2 (237.6) | 142.8 (−7.5) | 261.9 (−209.6) | 233.9 (−160.4) | 387.3 (−378.6) | 394.2 (−389.1) | 205.1 (−123.9) | 159.7 (−21.1) |
| QQQ | 0 ms | 5,000 ms | 96 | 506.1 | 200.5 (200.5) | 714.6 (422.7) | 179.5 (−18.1) | 300.7 (−253.6) | 263.4 (−184.6) | 416.9 (−413.6) | 429.8 (−429.8) | 243.7 (−163.1) | 197.7 (−24.2) |
| QQQ | 0 ms | 30,000 ms | 32 | 327.9 | 198.4 (198.4) | 667.0 (447.7) | 130.3 (−1.0) | 231.7 (−163.8) | 233.7 (−110.2) | 278.5 (−278.5) | 295.3 (−295.3) | 195.1 (−101.6) | 208.2 (6.0) |
| QQQ | 50 µs | 100 ms | 455 | 759.9 | 29.3 (29.3) | 161.3 (−73.0) | 50.9 (−2.9) | 131.0 (−112.1) | 126.0 (−104.8) | 192.8 (−186.3) | 213.4 (−209.5) | 87.8 (−54.9) | 64.9 (−22.7) |
| QQQ | 50 µs | 1,000 ms | 236 | 488.3 | 127.8 (127.8) | 578.2 (237.6) | 142.8 (−7.5) | 261.9 (−209.6) | 233.9 (−160.4) | 387.3 (−378.6) | 394.2 (−389.1) | 205.1 (−123.9) | 159.7 (−21.1) |
| QQQ | 50 µs | 5,000 ms | 96 | 506.1 | 200.5 (200.5) | 714.6 (422.7) | 179.5 (−18.1) | 300.7 (−253.6) | 263.4 (−184.6) | 416.9 (−413.6) | 429.8 (−429.8) | 243.7 (−163.1) | 197.7 (−24.2) |
| QQQ | 50 µs | 30,000 ms | 32 | 327.9 | 198.4 (198.4) | 667.0 (447.7) | 130.3 (−1.0) | 231.7 (−163.8) | 233.7 (−110.2) | 278.5 (−278.5) | 295.3 (−295.3) | 195.1 (−101.6) | 208.2 (6.0) |
| QQQ | 500 µs | 100 ms | 457 | 759.2 | 28.9 (28.9) | 160.4 (−73.7) | 49.9 (−3.2) | 130.3 (−111.6) | 125.3 (−104.2) | 192.9 (−186.4) | 213.8 (−209.9) | 86.7 (−54.7) | 63.6 (−22.6) |
| QQQ | 500 µs | 1,000 ms | 238 | 482.6 | 126.7 (126.7) | 587.0 (250.3) | 139.6 (−6.2) | 257.1 (−205.1) | 229.5 (−156.1) | 381.4 (−372.8) | 388.3 (−383.2) | 200.8 (−120.7) | 156.0 (−19.3) |
| QQQ | 500 µs | 5,000 ms | 97 | 501.9 | 197.5 (197.5) | 704.5 (415.6) | 176.7 (−18.8) | 298.6 (−250.5) | 261.2 (−181.5) | 413.0 (−409.8) | 426.0 (−426.0) | 240.6 (−161.7) | 195.0 (−24.7) |
| QQQ | 500 µs | 30,000 ms | 32 | 327.9 | 198.4 (198.4) | 667.0 (447.7) | 130.6 (−0.7) | 232.5 (−162.9) | 235.7 (−108.1) | 278.5 (−278.5) | 295.3 (−295.3) | 195.6 (−101.1) | 208.7 (6.5) |
| AAPL | 0 ms | 100 ms | 473 | 238.9 | 3.9 (3.9) | 8.0 (−3.8) | 4.5 (2.9) | 4.8 (0.5) | 4.2 (1.4) | 7.7 (−4.7) | 8.5 (−6.2) | 4.6 (2.0) | 4.2 (3.2) |
| AAPL | 0 ms | 1,000 ms | 265 | 146.6 | 13.0 (13.0) | 79.5 (−32.4) | 18.9 (1.9) | 32.8 (−21.6) | 27.5 (−15.2) | 61.3 (−56.8) | 68.5 (−64.6) | 24.4 (−6.9) | 17.9 (3.9) |
| AAPL | 0 ms | 5,000 ms | 105 | 108.1 | 31.0 (31.0) | 118.7 (45.8) | 38.9 (6.6) | 48.7 (−25.7) | 42.7 (−17.1) | 65.5 (−58.7) | 71.7 (−62.4) | 42.8 (−6.4) | 36.7 (11.4) |
| AAPL | 0 ms | 30,000 ms | 45 | 52.4 | 22.8 (22.8) | 102.1 (76.8) | 25.7 (−6.5) | 38.1 (−26.9) | 33.4 (−18.8) | 45.8 (−44.7) | 47.7 (−46.8) | 29.0 (−12.6) | 23.6 (4.3) |
| AAPL | 50 µs | 100 ms | 473 | 238.9 | 3.9 (3.9) | 8.0 (−3.8) | 4.5 (2.9) | 4.8 (0.5) | 4.2 (1.4) | 7.7 (−4.7) | 8.5 (−6.2) | 4.6 (2.0) | 4.2 (3.2) |
| AAPL | 50 µs | 1,000 ms | 265 | 146.6 | 13.0 (13.0) | 79.5 (−32.4) | 18.9 (1.9) | 32.8 (−21.6) | 27.5 (−15.2) | 61.3 (−56.8) | 68.5 (−64.6) | 24.4 (−6.9) | 17.9 (3.9) |
| AAPL | 50 µs | 5,000 ms | 105 | 108.1 | 31.0 (31.0) | 118.7 (45.8) | 38.9 (6.6) | 48.7 (−25.7) | 42.7 (−17.1) | 65.5 (−58.7) | 71.7 (−62.4) | 42.8 (−6.4) | 36.7 (11.4) |
| AAPL | 50 µs | 30,000 ms | 45 | 52.4 | 22.8 (22.8) | 102.1 (76.8) | 25.7 (−6.5) | 38.1 (−26.9) | 33.4 (−18.8) | 45.8 (−44.7) | 47.7 (−46.8) | 29.0 (−12.6) | 23.6 (4.3) |
| AAPL | 500 µs | 100 ms | 470 | 240.3 | 4.1 (4.1) | 7.8 (−3.3) | 4.5 (3.3) | 4.8 (0.9) | 4.4 (1.7) | 7.3 (−4.0) | 8.2 (−5.7) | 4.6 (2.5) | 4.4 (3.5) |
| AAPL | 500 µs | 1,000 ms | 265 | 147.5 | 13.0 (13.0) | 80.1 (−33.0) | 18.9 (1.9) | 32.8 (−21.6) | 27.5 (−15.2) | 61.6 (−57.0) | 68.9 (−65.0) | 24.4 (−6.9) | 17.9 (3.9) |
| AAPL | 500 µs | 5,000 ms | 107 | 108.3 | 30.4 (30.4) | 116.4 (45.0) | 38.2 (6.4) | 47.8 (−25.2) | 41.9 (−16.8) | 64.3 (−57.5) | 70.3 (−61.2) | 42.0 (−6.3) | 36.0 (11.2) |
| AAPL | 500 µs | 30,000 ms | 45 | 52.4 | 22.8 (22.8) | 102.1 (76.8) | 25.7 (−6.5) | 38.1 (−26.9) | 33.4 (−18.8) | 45.8 (−44.7) | 47.7 (−46.8) | 29.0 (−12.6) | 23.6 (4.3) |
| SPY | 0 ms | 100 ms | 455 | 491.6 | 19.8 (19.8) | 80.2 (−15.5) | 31.8 (1.1) | 62.8 (−50.2) | 60.0 (−45.4) | 94.7 (−89.1) | 103.0 (−99.5) | 45.6 (−22.1) | 35.7 (−5.4) |
| SPY | 0 ms | 1,000 ms | 229 | 316.9 | 101.9 (101.9) | 316.5 (129.9) | 101.5 (15.1) | 133.9 (−89.9) | 121.6 (−58.6) | 205.0 (−200.9) | 213.8 (−211.9) | 115.6 (−40.6) | 104.5 (20.7) |
| SPY | 0 ms | 5,000 ms | 84 | 292.7 | 148.8 (148.8) | 336.0 (217.4) | 94.2 (17.7) | 94.5 (−51.9) | 91.1 (−10.6) | 139.4 (−138.5) | 140.7 (−140.7) | 94.3 (−18.2) | 109.8 (53.6) |
| SPY | 0 ms | 30,000 ms | 30 | 90.0 | 99.5 (99.5) | 311.6 (258.7) | 78.3 (−41.5) | 82.5 (−61.0) | 74.6 (−24.9) | 101.1 (−101.1) | 100.0 (−100.0) | 77.0 (−46.9) | 80.2 (9.6) |
| SPY | 50 µs | 100 ms | 455 | 491.6 | 19.8 (19.8) | 79.4 (−14.7) | 31.6 (1.3) | 62.8 (−50.2) | 60.0 (−45.4) | 94.6 (−89.0) | 103.0 (−99.5) | 45.4 (−21.8) | 35.5 (−5.1) |
| SPY | 50 µs | 1,000 ms | 229 | 316.9 | 101.9 (101.9) | 316.5 (129.9) | 101.5 (15.1) | 133.9 (−89.9) | 121.6 (−58.6) | 205.0 (−200.9) | 213.8 (−211.9) | 115.6 (−40.6) | 104.5 (20.7) |
| SPY | 50 µs | 5,000 ms | 84 | 292.7 | 148.8 (148.8) | 336.0 (217.4) | 94.2 (17.7) | 94.5 (−51.9) | 91.1 (−10.6) | 139.4 (−138.5) | 140.7 (−140.7) | 94.3 (−18.2) | 109.8 (53.6) |
| SPY | 50 µs | 30,000 ms | 30 | 90.0 | 99.5 (99.5) | 311.6 (258.7) | 78.3 (−41.5) | 82.5 (−61.0) | 74.6 (−24.9) | 101.1 (−101.1) | 100.0 (−100.0) | 77.0 (−46.9) | 80.2 (9.6) |
| SPY | 500 µs | 100 ms | 457 | 511.9 | 14.6 (14.6) | 73.0 (−22.6) | 27.5 (−4.1) | 62.6 (−53.3) | 59.5 (−48.7) | 94.8 (−90.7) | 103.1 (−100.6) | 42.7 (−27.0) | 32.4 (−10.8) |
| SPY | 500 µs | 1,000 ms | 227 | 324.4 | 108.0 (108.0) | 352.8 (204.3) | 104.4 (18.1) | 136.9 (−93.9) | 124.4 (−62.2) | 213.5 (−209.2) | 222.4 (−220.7) | 118.3 (−42.5) | 106.9 (20.9) |
| SPY | 500 µs | 5,000 ms | 85 | 292.8 | 146.1 (146.1) | 338.7 (225.9) | 90.2 (19.5) | 91.4 (−52.9) | 88.3 (−10.9) | 141.4 (−140.6) | 142.9 (−142.9) | 91.1 (−18.4) | 106.2 (53.0) |
| SPY | 500 µs | 30,000 ms | 30 | 93.3 | 97.1 (97.1) | 308.1 (255.2) | 75.8 (−42.1) | 83.2 (−64.3) | 72.4 (−28.1) | 104.8 (−104.8) | 103.7 (−103.7) | 76.6 (−50.0) | 76.7 (6.9) |

#### D. Clean 2020-01-30 prospective data holdout (partial window)

Local output `p10-oos/oos-01302020/study-result.json`, SHA-256
`4cc93ea56eeb63a9e46ca9335ca56b146c3c697720d595888c52eda3fdb1857a`.
All cells follow; the shorter prefix reduces eligible counts without changing
the frozen schedule or horizons. These are not nine-minute estimates.

Placement: QQQ 816 placed of 1620 scheduled (0 empty side, 0 locked/crossed, 804 not arrived); AAPL 816 placed of 1620 scheduled (0 empty side, 0 locked/crossed, 804 not arrived); SPY 816 placed of 1620 scheduled (0 empty side, 0 locked/crossed, 804 not arrived). Tracking ended at 34471554873308 ns (end of input).

**Fill fraction within τ.** L3 column: fraction [nominal 95% Wilson] over eligible shadows (n) and its fill causes queue/trade-through/book-cross. Model columns: Δ = model − L3 [nominal 95% Newcombe paired]; **bold** = nominal/exploratory H0 rejection.

| Symbol | L | τ | n | L3 fill [nominal 95%] | L3 causes | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | 271 | 0.697 [0.640, 0.749] | 23/1/165 | 0.000 [−0.009, 0.009] | 0.000 [−0.014, 0.014] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] |
| QQQ | 0 ms | 5,000 ms | 267 | 0.858 [0.811, 0.895] | 36/1/192 | −0.007 [−0.024, 0.008] | −0.011 [−0.030, 0.006] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] |
| QQQ | 0 ms | 30,000 ms | 242 | 0.917 [0.876, 0.946] | 39/1/182 | −0.004 [−0.022, 0.013] | −0.008 [−0.028, 0.010] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] |
| QQQ | 50 µs | 1,000 ms | 271 | 0.697 [0.640, 0.749] | 23/1/165 | 0.000 [−0.009, 0.009] | 0.000 [−0.014, 0.014] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] |
| QQQ | 50 µs | 5,000 ms | 267 | 0.858 [0.811, 0.895] | 36/1/192 | −0.007 [−0.024, 0.008] | −0.011 [−0.030, 0.006] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] |
| QQQ | 50 µs | 30,000 ms | 242 | 0.917 [0.876, 0.946] | 39/1/182 | −0.004 [−0.022, 0.013] | −0.008 [−0.028, 0.010] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] |
| QQQ | 500 µs | 1,000 ms | 271 | 0.697 [0.640, 0.749] | 23/1/165 | 0.000 [−0.009, 0.009] | 0.000 [−0.014, 0.014] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] |
| QQQ | 500 µs | 5,000 ms | 267 | 0.858 [0.811, 0.895] | 35/2/192 | −0.007 [−0.024, 0.008] | −0.011 [−0.030, 0.006] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] | −0.007 [−0.024, 0.008] |
| QQQ | 500 µs | 30,000 ms | 242 | 0.917 [0.876, 0.946] | 38/2/182 | −0.004 [−0.022, 0.013] | −0.008 [−0.028, 0.010] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] | −0.004 [−0.022, 0.013] |
| AAPL | 0 ms | 1,000 ms | 271 | 0.306 [0.254, 0.364] | 33/38/12 | 0.000 [−0.009, 0.009] | 0.007 [−0.006, 0.021] | 0.004 [−0.008, 0.015] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] |
| AAPL | 0 ms | 5,000 ms | 267 | 0.655 [0.597, 0.710] | 75/75/25 | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] |
| AAPL | 0 ms | 30,000 ms | 242 | 0.855 [0.806, 0.894] | 85/91/31 | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] |
| AAPL | 50 µs | 1,000 ms | 271 | 0.306 [0.254, 0.364] | 33/38/12 | 0.000 [−0.009, 0.009] | 0.007 [−0.006, 0.021] | 0.004 [−0.008, 0.015] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] |
| AAPL | 50 µs | 5,000 ms | 267 | 0.655 [0.597, 0.710] | 75/75/25 | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] |
| AAPL | 50 µs | 30,000 ms | 242 | 0.855 [0.806, 0.894] | 85/91/31 | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] |
| AAPL | 500 µs | 1,000 ms | 271 | 0.306 [0.254, 0.364] | 34/37/12 | 0.000 [−0.009, 0.009] | 0.007 [−0.006, 0.021] | 0.004 [−0.008, 0.015] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.007 [−0.006, 0.021] | 0.004 [−0.008, 0.015] | 0.004 [−0.008, 0.015] |
| AAPL | 500 µs | 5,000 ms | 267 | 0.655 [0.597, 0.710] | 75/75/25 | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] | 0.000 [−0.009, 0.009] |
| AAPL | 500 µs | 30,000 ms | 242 | 0.855 [0.806, 0.894] | 85/91/31 | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] | 0.000 [−0.014, 0.014] |
| SPY | 0 ms | 1,000 ms | 271 | 0.649 [0.591, 0.704] | 75/7/94 | **−0.026 [−0.046, −0.005]** | −0.011 [−0.032, 0.010] | −0.011 [−0.032, 0.010] | 0.000 [−0.019, 0.019] | −0.004 [−0.024, 0.017] | 0.011 [−0.010, 0.032] | 0.015 [−0.007, 0.037] | −0.004 [−0.024, 0.017] | −0.011 [−0.032, 0.010] |
| SPY | 0 ms | 5,000 ms | 267 | 0.831 [0.782, 0.872] | 102/8/112 | −0.015 [−0.034, 0.003] | −0.004 [−0.024, 0.016] | −0.007 [−0.027, 0.011] | 0.004 [−0.016, 0.024] | 0.000 [−0.021, 0.021] | 0.007 [−0.011, 0.027] | 0.007 [−0.011, 0.027] | 0.000 [−0.021, 0.021] | −0.007 [−0.027, 0.011] |
| SPY | 0 ms | 30,000 ms | 242 | 0.913 [0.871, 0.943] | 111/6/104 | −0.012 [−0.034, 0.007] | −0.004 [−0.025, 0.016] | −0.004 [−0.025, 0.016] | 0.000 [−0.019, 0.019] | −0.004 [−0.025, 0.016] | 0.004 [−0.013, 0.022] | 0.004 [−0.013, 0.022] | −0.004 [−0.025, 0.016] | −0.004 [−0.025, 0.016] |
| SPY | 50 µs | 1,000 ms | 271 | 0.649 [0.591, 0.704] | 75/7/94 | **−0.026 [−0.046, −0.005]** | −0.011 [−0.032, 0.010] | −0.011 [−0.032, 0.010] | 0.000 [−0.019, 0.019] | −0.004 [−0.024, 0.017] | 0.011 [−0.010, 0.032] | 0.015 [−0.007, 0.037] | −0.004 [−0.024, 0.017] | −0.011 [−0.032, 0.010] |
| SPY | 50 µs | 5,000 ms | 267 | 0.831 [0.782, 0.872] | 102/8/112 | −0.015 [−0.034, 0.003] | −0.004 [−0.024, 0.016] | −0.007 [−0.027, 0.011] | 0.004 [−0.016, 0.024] | 0.000 [−0.021, 0.021] | 0.007 [−0.011, 0.027] | 0.007 [−0.011, 0.027] | 0.000 [−0.021, 0.021] | −0.007 [−0.027, 0.011] |
| SPY | 50 µs | 30,000 ms | 242 | 0.913 [0.871, 0.943] | 111/6/104 | −0.012 [−0.034, 0.007] | −0.004 [−0.025, 0.016] | −0.004 [−0.025, 0.016] | 0.000 [−0.019, 0.019] | −0.004 [−0.025, 0.016] | 0.004 [−0.013, 0.022] | 0.004 [−0.013, 0.022] | −0.004 [−0.025, 0.016] | −0.004 [−0.025, 0.016] |
| SPY | 500 µs | 1,000 ms | 271 | 0.642 [0.583, 0.697] | 63/8/103 | **−0.022 [−0.041, −0.003]** | −0.007 [−0.029, 0.014] | −0.007 [−0.029, 0.014] | 0.004 [−0.017, 0.024] | 0.004 [−0.017, 0.024] | 0.015 [−0.007, 0.037] | 0.018 [−0.004, 0.042] | 0.000 [−0.022, 0.022] | −0.007 [−0.029, 0.014] |
| SPY | 500 µs | 5,000 ms | 267 | 0.824 [0.774, 0.865] | 90/9/121 | −0.011 [−0.029, 0.005] | 0.000 [−0.021, 0.021] | −0.004 [−0.024, 0.016] | 0.007 [−0.014, 0.029] | 0.004 [−0.016, 0.024] | 0.011 [−0.008, 0.032] | 0.011 [−0.008, 0.032] | 0.004 [−0.019, 0.026] | −0.004 [−0.024, 0.016] |
| SPY | 500 µs | 30,000 ms | 242 | 0.913 [0.871, 0.943] | 101/7/113 | −0.012 [−0.034, 0.007] | −0.004 [−0.025, 0.016] | −0.004 [−0.025, 0.016] | 0.000 [−0.019, 0.019] | 0.000 [−0.019, 0.019] | 0.004 [−0.013, 0.022] | 0.004 [−0.013, 0.022] | −0.004 [−0.025, 0.016] | −0.004 [−0.025, 0.016] |

**Kaplan–Meier median time to fill (ms)** over every placed shadow, censored at τ or the end of tracking ("—": survival stayed above 0.5 within τ).

| Symbol | L | τ | l3_shadow | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 1,000 ms | 358.6 | 358.6 | 345.8 | 358.6 | 356.3 | 356.3 | 344.7 | 344.7 | 358.6 | 358.6 |
| QQQ | 0 ms | 5,000 ms | 358.6 | 358.6 | 345.8 | 358.6 | 356.3 | 356.3 | 344.7 | 344.7 | 358.6 | 358.6 |
| QQQ | 0 ms | 30,000 ms | 358.6 | 358.6 | 345.8 | 358.6 | 356.3 | 356.3 | 344.7 | 344.7 | 358.6 | 358.6 |
| QQQ | 50 µs | 1,000 ms | 358.6 | 358.6 | 345.7 | 358.5 | 356.2 | 356.2 | 344.6 | 344.6 | 358.5 | 358.5 |
| QQQ | 50 µs | 5,000 ms | 358.6 | 358.6 | 345.7 | 358.5 | 356.2 | 356.2 | 344.6 | 344.6 | 358.5 | 358.5 |
| QQQ | 50 µs | 30,000 ms | 358.6 | 358.6 | 345.7 | 358.5 | 356.2 | 356.2 | 344.6 | 344.6 | 358.5 | 358.5 |
| QQQ | 500 µs | 1,000 ms | 358.1 | 358.1 | 345.3 | 358.1 | 355.8 | 355.8 | 344.2 | 344.2 | 358.1 | 358.1 |
| QQQ | 500 µs | 5,000 ms | 358.1 | 358.1 | 345.3 | 358.1 | 355.8 | 355.8 | 344.2 | 344.2 | 358.1 | 358.1 |
| QQQ | 500 µs | 30,000 ms | 358.1 | 358.1 | 345.3 | 358.1 | 355.8 | 355.8 | 344.2 | 344.2 | 358.1 | 358.1 |
| AAPL | 0 ms | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 0 ms | 5,000 ms | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 |
| AAPL | 0 ms | 30,000 ms | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 | 2282.1 |
| AAPL | 50 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 50 µs | 5,000 ms | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 |
| AAPL | 50 µs | 30,000 ms | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 | 2282.0 |
| AAPL | 500 µs | 1,000 ms | — | — | — | — | — | — | — | — | — | — |
| AAPL | 500 µs | 5,000 ms | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 |
| AAPL | 500 µs | 30,000 ms | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 | 2281.6 |
| SPY | 0 ms | 1,000 ms | 493.4 | 553.8 | 517.2 | 537.5 | 513.8 | 519.9 | 493.4 | 492.8 | 519.9 | 528.2 |
| SPY | 0 ms | 5,000 ms | 493.4 | 553.8 | 517.2 | 537.5 | 513.8 | 519.9 | 493.4 | 492.8 | 519.9 | 528.2 |
| SPY | 0 ms | 30,000 ms | 493.4 | 553.8 | 517.2 | 537.5 | 513.8 | 519.9 | 493.4 | 492.8 | 519.9 | 528.2 |
| SPY | 50 µs | 1,000 ms | 493.3 | 553.8 | 517.2 | 537.4 | 513.7 | 519.9 | 493.3 | 492.8 | 519.9 | 528.2 |
| SPY | 50 µs | 5,000 ms | 493.3 | 553.8 | 517.2 | 537.4 | 513.7 | 519.9 | 493.3 | 492.8 | 519.9 | 528.2 |
| SPY | 50 µs | 30,000 ms | 493.3 | 553.8 | 517.2 | 537.4 | 513.7 | 519.9 | 493.3 | 492.8 | 519.9 | 528.2 |
| SPY | 500 µs | 1,000 ms | 513.3 | 564.0 | 520.1 | 553.3 | 516.7 | 519.4 | 502.3 | 492.9 | 527.7 | 537.0 |
| SPY | 500 µs | 5,000 ms | 513.3 | 564.0 | 520.1 | 553.3 | 516.7 | 519.4 | 502.3 | 492.9 | 527.7 | 537.0 |
| SPY | 500 µs | 30,000 ms | 513.3 | 564.0 | 520.1 | 553.3 | 516.7 | 519.4 | 502.3 | 492.9 | 527.7 | 537.0 |

**Share of cancelled volume ahead of the shadow** (L3; shares cancelled, deleted or replaced away at the shadow's price until min(L3 fill, τ, end of tracking); ahead / (ahead + behind), pooled).

| Symbol | L | τ 1,000 ms | τ 5,000 ms | τ 30,000 ms |
|---|---|---|---|---|
| QQQ | 0 ms | 0.132 (177,914 / 1,349,828) | 0.108 (191,083 / 1,761,986) | 0.094 (192,812 / 2,045,336) |
| QQQ | 50 µs | 0.132 (178,114 / 1,349,828) | 0.109 (191,283 / 1,761,986) | 0.094 (193,012 / 2,045,336) |
| QQQ | 500 µs | 0.137 (183,237 / 1,336,317) | 0.113 (197,606 / 1,756,112) | 0.098 (199,335 / 2,039,462) |
| AAPL | 0 ms | 0.352 (20,611 / 58,558) | 0.212 (23,815 / 112,071) | 0.187 (29,361 / 156,943) |
| AAPL | 50 µs | 0.352 (20,611 / 58,558) | 0.212 (23,815 / 112,071) | 0.187 (29,361 / 156,943) |
| AAPL | 500 µs | 0.351 (20,481 / 58,419) | 0.212 (23,685 / 111,920) | 0.186 (29,231 / 156,792) |
| SPY | 0 ms | 0.180 (136,499 / 756,636) | 0.143 (148,200 / 1,036,971) | 0.122 (150,592 / 1,232,216) |
| SPY | 50 µs | 0.180 (136,499 / 756,636) | 0.143 (148,200 / 1,036,971) | 0.122 (150,592 / 1,232,216) |
| SPY | 500 µs | 0.183 (140,669 / 766,620) | 0.145 (152,370 / 1,054,312) | 0.123 (154,762 / 1,255,757) |

**front − ahead_L3 at checkpoints** (shares): mean absolute (mean signed) over shadows unfilled under both paths; pairs = shadows compared for RiskAdverse; mean L3 ahead for scale.

| Symbol | L | Checkpoint | Pairs | Mean L3 ahead | risk_adverse | log_prob | log_prob2 | power_prob_n2 | power_prob_n3 | power_prob2_n2 | power_prob2_n3 | power_prob3_n2 | power_prob3_n3 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| QQQ | 0 ms | 100 ms | 209 | 461.8 | 28.0 (28.0) | 234.4 (−72.0) | 55.7 (−8.9) | 133.7 (−120.8) | 120.5 (−104.1) | 230.2 (−229.3) | 249.3 (−249.1) | 87.3 (−55.9) | 57.6 (−14.0) |
| QQQ | 0 ms | 1,000 ms | 82 | 267.5 | 77.8 (77.8) | 317.9 (67.3) | 112.2 (−16.1) | 162.4 (−131.5) | 143.5 (−88.0) | 255.9 (−255.9) | 267.7 (−266.5) | 136.7 (−75.1) | 109.8 (0.6) |
| QQQ | 0 ms | 5,000 ms | 38 | 113.5 | 45.0 (45.0) | 179.8 (11.2) | 55.4 (−25.2) | 88.3 (−66.3) | 74.3 (−42.0) | 113.4 (−112.7) | 115.3 (−111.9) | 74.2 (−41.1) | 61.3 (0.1) |
| QQQ | 0 ms | 30,000 ms | 20 | 111.8 | 15.0 (15.0) | 100.7 (28.2) | 77.7 (−71.3) | 100.7 (−93.8) | 88.3 (−72.9) | 111.8 (−111.8) | 111.8 (−111.8) | 92.7 (−83.1) | 72.7 (−49.7) |
| QQQ | 50 µs | 100 ms | 209 | 461.8 | 28.0 (28.0) | 234.4 (−72.0) | 55.7 (−8.9) | 133.0 (−120.1) | 119.6 (−103.2) | 229.4 (−228.5) | 248.4 (−248.1) | 87.3 (−55.9) | 57.6 (−14.0) |
| QQQ | 50 µs | 1,000 ms | 82 | 267.5 | 77.8 (77.8) | 317.9 (67.3) | 112.2 (−16.1) | 162.4 (−131.5) | 143.3 (−87.9) | 255.9 (−255.9) | 267.7 (−266.5) | 136.7 (−75.1) | 109.8 (0.6) |
| QQQ | 50 µs | 5,000 ms | 38 | 113.5 | 45.0 (45.0) | 179.8 (11.2) | 55.4 (−25.2) | 88.3 (−66.3) | 74.3 (−42.0) | 113.4 (−112.7) | 115.3 (−111.9) | 74.2 (−41.1) | 61.3 (0.1) |
| QQQ | 50 µs | 30,000 ms | 20 | 111.8 | 15.0 (15.0) | 100.7 (28.2) | 77.7 (−71.3) | 100.7 (−93.8) | 88.3 (−72.9) | 111.8 (−111.8) | 111.8 (−111.8) | 92.7 (−83.1) | 72.7 (−49.7) |
| QQQ | 500 µs | 100 ms | 210 | 467.1 | 27.4 (27.4) | 237.3 (−73.5) | 55.8 (−10.1) | 137.8 (−125.3) | 124.1 (−109.3) | 233.6 (−232.4) | 252.3 (−252.1) | 90.4 (−59.0) | 59.3 (−16.4) |
| QQQ | 500 µs | 1,000 ms | 82 | 273.4 | 75.4 (75.4) | 317.3 (45.2) | 113.6 (−18.0) | 169.4 (−138.8) | 147.1 (−96.8) | 257.5 (−257.5) | 269.6 (−268.5) | 142.1 (−80.6) | 112.9 (−4.4) |
| QQQ | 500 µs | 5,000 ms | 38 | 113.5 | 45.0 (45.0) | 179.8 (11.2) | 55.4 (−25.2) | 88.3 (−66.3) | 74.3 (−42.0) | 113.4 (−112.7) | 115.3 (−111.9) | 74.2 (−41.1) | 61.3 (0.1) |
| QQQ | 500 µs | 30,000 ms | 20 | 111.8 | 15.0 (15.0) | 100.7 (28.2) | 77.7 (−71.3) | 100.7 (−93.8) | 88.3 (−72.9) | 111.8 (−111.8) | 111.8 (−111.8) | 92.7 (−83.1) | 72.7 (−49.7) |
| AAPL | 0 ms | 100 ms | 261 | 204.7 | 0.3 (0.3) | 1.5 (−1.2) | 0.7 (−0.1) | 1.8 (−1.3) | 1.8 (−1.2) | 2.3 (−2.0) | 2.3 (−2.2) | 1.1 (−0.5) | 0.8 (−0.2) |
| AAPL | 0 ms | 1,000 ms | 188 | 130.1 | 5.9 (5.9) | 17.0 (−5.3) | 8.5 (1.6) | 12.9 (−9.1) | 12.4 (−8.9) | 16.7 (−15.0) | 17.7 (−17.0) | 10.9 (−2.8) | 9.6 (−0.1) |
| AAPL | 0 ms | 5,000 ms | 92 | 76.0 | 13.6 (13.6) | 22.1 (−13.6) | 16.3 (4.9) | 23.9 (−14.0) | 23.3 (−13.2) | 25.8 (−22.6) | 27.5 (−25.8) | 21.5 (−4.7) | 20.2 (0.1) |
| AAPL | 0 ms | 30,000 ms | 35 | 3.3 | 14.2 (14.2) | 29.4 (22.7) | 11.3 (9.8) | 7.9 (3.3) | 8.7 (5.1) | 3.7 (−2.5) | 4.4 (−2.3) | 9.7 (7.4) | 12.1 (10.9) |
| AAPL | 50 µs | 100 ms | 261 | 204.7 | 0.3 (0.3) | 1.5 (−1.2) | 0.7 (−0.1) | 1.8 (−1.3) | 1.8 (−1.2) | 2.3 (−2.0) | 2.3 (−2.2) | 1.1 (−0.5) | 0.8 (−0.2) |
| AAPL | 50 µs | 1,000 ms | 188 | 130.1 | 5.9 (5.9) | 17.0 (−5.3) | 8.5 (1.6) | 12.9 (−9.1) | 12.4 (−8.9) | 16.7 (−15.0) | 17.7 (−17.0) | 10.9 (−2.8) | 9.6 (−0.1) |
| AAPL | 50 µs | 5,000 ms | 92 | 76.0 | 13.6 (13.6) | 22.1 (−13.6) | 16.3 (4.9) | 23.9 (−14.0) | 23.3 (−13.2) | 25.8 (−22.6) | 27.5 (−25.8) | 21.5 (−4.7) | 20.2 (0.1) |
| AAPL | 50 µs | 30,000 ms | 35 | 3.3 | 14.2 (14.2) | 29.4 (22.7) | 11.3 (9.8) | 7.9 (3.3) | 8.7 (5.1) | 3.7 (−2.5) | 4.4 (−2.3) | 9.7 (7.4) | 12.1 (10.9) |
| AAPL | 500 µs | 100 ms | 261 | 205.7 | 0.2 (0.2) | 1.7 (−1.4) | 0.7 (−0.3) | 1.9 (−1.6) | 1.9 (−1.5) | 2.6 (−2.4) | 2.7 (−2.5) | 1.1 (−0.7) | 0.7 (−0.3) |
| AAPL | 500 µs | 1,000 ms | 188 | 130.9 | 5.3 (5.3) | 16.2 (−6.4) | 7.9 (1.1) | 12.7 (−9.5) | 12.2 (−9.3) | 16.8 (−15.3) | 17.9 (−17.3) | 10.4 (−3.4) | 9.0 (−0.6) |
| AAPL | 500 µs | 5,000 ms | 92 | 77.3 | 12.7 (12.7) | 22.3 (−14.5) | 15.8 (3.9) | 24.3 (−15.0) | 23.8 (−14.3) | 26.3 (−23.3) | 28.1 (−26.5) | 21.2 (−5.8) | 19.7 (−0.9) |
| AAPL | 500 µs | 30,000 ms | 35 | 6.8 | 10.7 (10.7) | 30.6 (19.3) | 11.0 (6.4) | 8.7 (0.5) | 9.0 (2.8) | 6.7 (−5.7) | 5.8 (−5.3) | 9.1 (4.0) | 10.3 (7.5) |
| SPY | 0 ms | 100 ms | 218 | 322.2 | 20.9 (20.9) | 75.9 (−18.8) | 35.2 (0.5) | 63.8 (−52.8) | 57.8 (−46.8) | 104.3 (−100.7) | 111.9 (−109.0) | 45.9 (−19.4) | 35.5 (−2.9) |
| SPY | 0 ms | 1,000 ms | 95 | 176.3 | 40.7 (40.7) | 141.9 (−10.5) | 54.1 (−9.2) | 102.5 (−83.1) | 91.8 (−66.2) | 156.9 (−156.4) | 166.3 (−165.9) | 75.9 (−43.4) | 54.7 (−7.7) |
| SPY | 0 ms | 5,000 ms | 45 | 75.2 | 87.4 (87.4) | 155.6 (82.8) | 69.9 (18.3) | 71.4 (−30.1) | 71.5 (−6.3) | 79.9 (−79.8) | 80.8 (−80.8) | 73.6 (−6.7) | 74.3 (35.5) |
| SPY | 0 ms | 30,000 ms | 21 | 4.8 | 108.4 (108.4) | 136.3 (121.1) | 29.5 (27.5) | 21.8 (16.5) | 38.8 (38.1) | 5.0 (−5.0) | 5.0 (−5.0) | 28.1 (24.2) | 60.8 (60.8) |
| SPY | 50 µs | 100 ms | 218 | 322.2 | 20.9 (20.9) | 75.9 (−18.8) | 35.2 (0.5) | 63.8 (−52.8) | 57.8 (−46.8) | 104.3 (−100.7) | 111.9 (−109.0) | 45.9 (−19.4) | 35.5 (−2.9) |
| SPY | 50 µs | 1,000 ms | 95 | 176.3 | 40.7 (40.7) | 141.9 (−10.5) | 54.1 (−9.2) | 102.5 (−83.1) | 91.8 (−66.2) | 156.9 (−156.4) | 166.3 (−165.9) | 75.9 (−43.4) | 54.7 (−7.7) |
| SPY | 50 µs | 5,000 ms | 45 | 75.2 | 87.4 (87.4) | 155.6 (82.8) | 69.9 (18.3) | 71.4 (−30.1) | 71.5 (−6.3) | 79.9 (−79.8) | 80.8 (−80.8) | 73.6 (−6.7) | 74.3 (35.5) |
| SPY | 50 µs | 30,000 ms | 21 | 4.8 | 108.4 (108.4) | 136.3 (121.1) | 29.5 (27.5) | 21.8 (16.5) | 38.8 (38.1) | 5.0 (−5.0) | 5.0 (−5.0) | 28.1 (24.2) | 60.8 (60.8) |
| SPY | 500 µs | 100 ms | 219 | 330.5 | 16.3 (16.3) | 72.8 (−25.1) | 31.8 (−3.8) | 64.4 (−55.7) | 58.2 (−50.2) | 104.6 (−102.0) | 110.3 (−108.9) | 44.6 (−23.2) | 32.7 (−6.9) |
| SPY | 500 µs | 1,000 ms | 97 | 173.1 | 41.7 (41.7) | 138.0 (−12.0) | 53.6 (−7.6) | 102.0 (−82.3) | 92.3 (−65.8) | 155.6 (−155.1) | 164.9 (−164.5) | 75.0 (−42.0) | 54.7 (−6.5) |
| SPY | 500 µs | 5,000 ms | 47 | 72.5 | 83.2 (83.2) | 151.6 (80.5) | 67.9 (18.1) | 69.8 (−29.6) | 68.5 (−6.5) | 78.5 (−78.4) | 79.4 (−79.3) | 71.3 (−6.7) | 72.1 (34.4) |
| SPY | 500 µs | 30,000 ms | 21 | 5.7 | 107.5 (107.5) | 135.3 (120.1) | 30.4 (26.5) | 22.3 (15.7) | 38.9 (37.8) | 6.0 (−6.0) | 6.0 (−6.0) | 28.2 (23.2) | 60.0 (59.8) |

## 7. Hypothesis outcomes

### In-sample (2019-07-30, QQQ and AAPL)

- **Most fills do not come from the queue.** For QQQ within τ = 1 s (L = 0), 229 of the 287 L3
  fills came from the opposite best reaching the shadow's price (book cross), 58 from the queue and
  0 from a trade-through; for AAPL, 60 book cross, 76 queue and 52 trade-through of 188. A book
  cross or trade-through does not depend on queue position and fills every path identically, so
  the fill fraction moves little between models even where the modeled `front` is far from the
  L3 shares ahead.
- **H-A (nominal/exploratory).** QQQ, τ = 1 s: RiskAdverse − L3 = −0.009 [−0.018, −0.000] at every L, so H-A is
  supported there, but the gap is under one percentage point of fill fraction, not large. QQQ
  τ = 5 s and 30 s and every AAPL cell: not supported (intervals contain 0; AAPL τ = 1 s
  difference 0.000 [−0.004, 0.004]). The direction of the size claim holds (QQQ gap ≥ AAPL gap in
  every (L, τ)); its magnitude does not. In queue position, RiskAdverse never undercounts: its
  `front` minus the L3 shares ahead is ≥ 0 in every checkpoint cell (mean signed = mean absolute),
  for QQQ 46.7 shares at 0.1 s, 198.1 at 1 s and 348.1 at 5 s (L = 0).
- **H-B part 1** (closer than RiskAdverse): QQQ τ = 1 s: `log_prob`, `log_prob2`,
  `power_prob_n2`, `power_prob_n3`, `power_prob3_n2` (5 of 8); AAPL τ = 1 s: none (RiskAdverse is
  exact at 0.000). Mixed across cells; see the hypotheses table in `study-result.md`.
- **H-B part 2 (nominal/exploratory)** (no single model within the interval on both symbols): **not supported**. At
  τ = 1 s, seven models (`log_prob`, `log_prob2`, `power_prob_n2`, `power_prob_n3`,
  `power_prob2_n2`, `power_prob2_n3`, `power_prob3_n2`) are within the interval on both symbols at
  every L; at τ = 5 s and 30 s all nine are.
- **H0 (nominal/exploratory).** Rejected only for `risk_adverse` and `power_prob3_n3` (the preset default) on QQQ at
  τ = 1 s, at every L (Δ −0.009 [−0.018, −0.000]); not rejected in the other 156 model cells.
- **Queue position itself is where the models differ.** At 1 s after arrival (L = 0) the mean
  absolute error of `front` against the L3 shares ahead ranges, for QQQ, from 171.9
  (`log_prob2`) to 592.8 shares (`log_prob`) with 583.6 shares ahead on average; for AAPL from
  6.2 (`risk_adverse`) to 16.9 (`power_prob2_n3`) with 143.9 ahead. The `power_prob2` models
  overcredit (front below the L3 position by 318–351 shares for QQQ at 1 s), `log_prob` and
  `risk_adverse` undercredit. 23.8% of the QQQ shares cancelled at the shadow's price within
  1 s were ahead of it (54.3% for AAPL), falling to 14.5% and 26.1% within 30 s.

### October OOS acquisition versus reused December validation (corrected implementation)

| | A: 2019-10-30, QQQ / AAPL / AMD (initial clean OOS acquisition) | B: 2019-12-30 (110 s), QQQ / AAPL inspected; AMZN held-out symbol |
|---|---|---|
| Model cells compared with L3 | 243 | 243 |
| H0 nominal/exploratory rejection | 9: `log_prob` on QQQ τ = 5 s (Δ −0.011 [−0.021, −0.001] at L = 0), `power_prob2_n2` (+0.017 [0.004, 0.029]) and `power_prob2_n3` (+0.015 [0.003, 0.027]) on AMD τ = 1 s; three contrasts each repeated at every L | 0 |
| Largest \|Δ\| | 0.016697588126159513 | 0.018867924528301883 |
| H-A (RiskAdverse interval below 0) | not supported in any cell; QQQ τ = 1 s Δ −0.004 [−0.010, 0.003]; the QQQ gap is not ≥ the AAPL gap at τ = 30 s (−0.002 versus −0.004) | not supported in any cell; QQQ τ = 1 s Δ −0.018 [−0.048, 0.012], τ = 5 s −0.019 [−0.058, 0.018] |
| H-B part 1 (closer than RiskAdverse, L = 0) | QQQ τ = 1 s: 6 of 8; AAPL: 0 / 0 / 3 of 8 at τ = 1 / 5 / 30 s; AMD: 4 / 4 / 8 of 8 | QQQ τ = 1 s: 8 of 8, otherwise none (ties); AAPL and AMZN: none |
| H-B part 2 (no model within the interval on every symbol) | not supported: 7–9 models within on all three symbols at every (L, τ), RiskAdverse included | not supported: all nine within on all three symbols everywhere |
| RiskAdverse `front` ≥ L3 shares ahead (mean signed = mean absolute) | every checkpoint cell | every checkpoint cell |
| Fills from the queue path, τ = 1 s, L = 0 (queue / trade-through / book cross) | QQQ 43 / 2 / 194; AAPL 35 / 50 / 46; AMD 102 / 27 / 161 | QQQ 11 / 0 / 47; AAPL 14 / 19 / 7; AMZN 2 / 12 / 0 |
| Share of cancelled volume ahead, τ = 1 s, L = 0 | QQQ 0.240, AAPL 0.592, AMD 0.120 | QQQ 0.287, AAPL 0.405, AMZN 0.810 |

**Verdict restricted to October OOS acquisition.** The fixed-engine rerun reports all 243
model-versus-L3 contrasts, maximum absolute Δ = 0.016697588126159513, zero cells meeting
H-A's nominal support criterion, and 7 / 8 / 9 models at τ = 1 / 5 / 30 s with intervals
containing zero on every studied symbol (each latency). Nine nominal H0 rejections are three
model/symbol/horizon contrasts repeated across the latency grid, not nine independent
discoveries. These are descriptive, nominal/exploratory outcomes: their joint behaviour
under dependence and unadjusted multiplicity is unknown, so rejection counts are not
explained as chance outcomes or compared with an expected 5% count. At L = 0 and τ = 1 s,
trade-through plus book-cross fills exceed queue fills for each October symbol. Reused
December QQQ/AAPL data cannot corroborate an OOS claim; AMZN provides only held-out-symbol
evidence on that previously decoded day. October alone provides no multi-day or real-order
calibration conclusion, and its implementation correction followed outcome review.
The prospective January comparisons below extend the data evidence, not real-order calibration.

### Clean-day replication: October, January 2019 and January 2020

Every row uses the frozen 3 × 3 latency/horizon grid and all nine L2 models.
H-A counts supported symbol/L/τ cells (27 per day). H-B part 1 counts probability
model/symbol/L/τ comparisons with strictly smaller absolute Δ than RiskAdverse
(216 per day; ties are not closer). H-B part 2 reports how many models' nominal
intervals contain zero on all three symbols, at τ = 1 / 5 / 30 s; these counts
are the same at each latency. A zero count supports that part's criterion.
All intervals, support labels and rejections remain **nominal/exploratory**.

| Clean acquisition | Coverage after 09:30 ET | Held-out symbol | H-A support | H-B part 1 closer | H-B part 2 models within on every symbol, τ 1 / 5 / 30 s | H0 nominal rejections / contrasts | Max \|Δ fill fraction\| |
|---|---|---|---:|---:|---|---:|---:|
| OOS-A: 2019-10-30 | full 540 s | AMD | 0 / 27 | 89 / 216 | 7 / 8 / 9; criterion never holds | 9 / 243 | 0.016697588126159513 |
| OOS-C: 2019-01-30 | 534.128393 s, truncated | SPY | 11 / 27 | 114 / 216 | 0 / 8 / 9; criterion holds only at τ = 1 s | 36 / 243 | 0.02434456928838946 |
| OOS-D: 2020-01-30 | 271.554873308 s, truncated | SPY | 3 / 27 | 70 / 216 | 8 / 9 / 9; criterion never holds | 3 / 243 | 0.02583025830258301 |
| All three, descriptive tally of original strata | unequal opening coverage; not a pooled session | AMD / SPY, not the same third symbol | 14 / 81 | 273 / 648 | intersection across all day/symbol strata: 0 / 7 / 9 | 48 / 729 | 0.02583025830258301 |

The last row sums the reported support/rejection/closer counts and takes the maximum
over the unchanged day-specific cells. The H-B intersection is a descriptive conjunction
of those cells' original intervals, **not** a newly pooled interval or a calibrated
cross-day test. The frozen method defines paired Newcombe contrasts within each
symbol/L/τ cell, not a cross-day random-effects estimator. We therefore do not pool
shadows into a new inferential test, average heterogeneous held-out symbols, or
count latency repetitions as independent discoveries. December is excluded from
this clean-day tally. Unequal prefix coverage changes the eligible populations.

**H-A does not replicate uniformly.** October's absence of nominal support does not
persist on the new days. January 2019 supports H-A for QQQ at τ = 1 s (all L)
and 5 s (L = 0 and 50 µs), and for SPY at τ = 1 s and 5 s (all L):
11 cells. At L = 0, QQQ τ = 1 s has Δ −0.016854
[−0.028345, −0.005347], whereas AAPL has −0.001873
[−0.007042, 0.003298]. January 2020 supports it only for SPY τ = 1 s
(all L): three cells; QQQ and AAPL τ = 1 s both have point Δ = 0.
The proposed large QQQ-versus-small-AAPL effect is not a stable cross-day finding.

**H-B remains mixed, not a universal model improvement.** At L = 0, the number of
closer probability models at τ = 1 / 5 / 30 s on January 2019 is QQQ 8 / 8 / 6,
AAPL 0 / 0 / 0, SPY 8 / 8 / 0; on January 2020 it is QQQ 0 / 0 / 0,
AAPL 0 / 0 / 0, SPY 8 / 8 / 8. The part-2 non-support from October replicates
on January 2020, but not at τ = 1 s on January 2019, where no model is within
the nominal interval on every symbol. At τ = 5 s and 30 s that criterion is
not supported on any of the three days. No model-selection or fitting was done.

**H0 rejection counts and identities do not replicate.** January 2019's 36
rejections comprise QQQ RiskAdverse τ = 1 s (all L) and 5 s (0, 50 µs);
AAPL τ = 1 s for `log_prob`, `power_prob_n2`, `power_prob_n3`,
`power_prob2_n2`, `power_prob2_n3`, `power_prob3_n2` (all L);
SPY τ = 1 s for RiskAdverse, `log_prob2`, `power_prob3_n3` (all L),
plus `power_prob3_n2` at 500 µs; and SPY RiskAdverse τ = 5 s (all L).
January 2020's three are SPY RiskAdverse τ = 1 s (all L). These differ from
October's nine. Serial dependence, overlapping horizons, common market events and
unadjusted multiplicity prevent treating the 48/729 descriptive tally as a
calibrated rejection rate, evidence of 48 independent discoveries, or a count
to compare with an expected 5% under H0. Actual joint coverage is unknown.

**The October maximum is not a replicated bound.** The new maxima are 2.434457%
(SPY RiskAdverse, τ = 1 s, L = 500 µs) and 2.583026% (SPY RiskAdverse,
τ = 1 s, L = 0 and 50 µs), exceeding October's 1.669759%.
Differences remain under 2.6 percentage points in these observed cells only;
that is not a population bound. January 2019 SPY at L = 0 and τ = 1 s has
250 queue fills versus 55 nonqueue fills, so October's nonqueue-majority
observation does not extend to every new symbol/day. RiskAdverse mean signed
checkpoint error still equals mean absolute error in every new checkpoint
cell, replicating that aggregate property without calibrating real orders.

## 8. What this shows and what it does not

Shows:

- Across three clean OOS acquisitions (October plus the two truncated January prefixes),
  the maximum observed model-versus-L3 fill-fraction difference is 0.02583025830258301;
  October's 0.016697588126159513 bound does not replicate (§7). At L = 0 and τ = 1 s,
  October has a nonqueue-fill majority for every symbol, but January 2019 SPY does not.
  This is restricted descriptive evidence, not a general explanation across days or horizons.
- Modeled queue position is model-sensitive; the closest model depends on symbol and
  checkpoint. July and previously inspected December are contextual validation evidence,
  not additional untouched days.
- RiskAdverse's mean signed error equals its mean absolute error in the reported
  checkpoint cells; this is a reported aggregate property, not real-order calibration.

Does not show:

- Real fill probabilities. A shadow fill assumes the aggressor would still have traded the same
  size against the shadow and that nobody reacted to it. It is an upper-bound-style quantity whose
  bias for real orders is unknown in size and direction.
- Latency effects. L is a swept assumption without a receipt clock; nothing is calibrated.
- Anything beyond the open: the frozen window is 09:30–09:39 per day, with tracking
  ending earlier when a gzip prefix falls short (both new January days; §6.6).
- That the reconstructed queue is the exchange's true queue: it follows displayed orders only
  (hidden and reserve interest is invisible) and shares two interpretations with every ITCH book
  builder (a replace loses priority; file order is time order inside a nanosecond).

## 9. Limitations

- One in-sample day (two symbols, nine minutes), three clean OOS acquisitions (three
  symbols each: October nine minutes, January 2019 534.128393 s, January 2020
  271.554873308 s), and one previously decoded validation day (December, 110 s;
  QQQ/AAPL previously inspected, AMZN held-out symbol), all at the open.
  Both new prefixes fall short of the full planned window. October's corrected rerun
  uses an implementation changed after its outcome review; the January acquisitions
  are prospective data holdouts for that already-corrected engine.
- Overlapping horizons (one shadow per second, horizons up to 30 s) and common market
  events make neighbouring shadows dependent. Wilson, paired Newcombe and Greenwood
  intervals are nominal 95% intervals without serial-dependence calibration; actual
  coverage and the direction/magnitude of distortion are unknown.
- Fill = first touch of any part of the order; partial fills and the remaining quantity are not
  modeled.
- The book-cross rule fills a shadow whose price the opposite side reaches; under no impact this
  is what the incoming order would have done, but it removes most of the queue-model dependence
  from the fill fraction.
- The L2 models see only sided printable executions, like hftbacktest; non-printable C
  executions at the opening cross move the L3 reference but not the L2 models.
- Checkpoint means use shadows unfilled under both paths at the checkpoint, so the compared
  population shrinks with time and differs slightly between models.
- Intervals are not adjusted for multiplicity (162 model-versus-L3 contrasts per
  two-symbol run, 243 per three-symbol run). Hypothesis support/rejection labels are
  nominal/exploratory; latency repetitions are not independent discoveries.

## 10. Reproduction

```sh
CARGO_PROFILE_RELEASE_DEBUG=0 cargo build --release -p bb-app --bin blackbox
mkdir -p "D:/black box/data/queue-shadow/fixed-v2"
cp target/release/blackbox.exe "D:/black box/data/queue-shadow/fixed-v2/"   # run the copied binary
cd "D:/black box/data/queue-shadow"
export BB_GIT_SHA=ccdd493f0812376fe0b341fccdc1a8a44fb8002e
./fixed-v2/blackbox.exe study queue-shadow --source synthetic --seed 7 --events 30000 \
    --expect-design-sha256 0608a9ba4af5b7f03119277570999af47b8212b9285e18a8f1f73f240846f502 \
    --out fixed-v2/synthetic
./fixed-v2/blackbox.exe study queue-shadow --source ../itch/07302019.NASDAQ_ITCH50.gz.prefix-300MB \
    --symbol QQQ --symbol AAPL --midnight-epoch-ns 1564459200000000000 \
    --expect-design-sha256 0608a9ba4af5b7f03119277570999af47b8212b9285e18a8f1f73f240846f502 \
    --out fixed-v2/insample
# October prefix was fetched once after the freeze; reuse the hash-identified local file.
./fixed-v2/blackbox.exe study queue-shadow --source ../itch/10302019.NASDAQ_ITCH50.gz.prefix-300MB \
    --symbol X --midnight-epoch-ns 1572408000000000000 --rank-activity --out oos-1030-activity
./fixed-v2/blackbox.exe study queue-shadow --source ../itch/10302019.NASDAQ_ITCH50.gz.prefix-300MB \
    --symbol QQQ --symbol AAPL --symbol AMD --midnight-epoch-ns 1572408000000000000 \
    --expect-design-sha256 0608a9ba4af5b7f03119277570999af47b8212b9285e18a8f1f73f240846f502 \
    --out fixed-v2/oos-1030
# reused December validation (QQQ/AAPL inspected; AMZN held-out symbol on decoded day)
./fixed-v2/blackbox.exe study queue-shadow --source D:/bbwt-research/itch-data/12302019.NASDAQ_ITCH50.gz.prefix-120MiB \
    --symbol X --midnight-epoch-ns 1577682000000000000 --rank-activity --out oos-1230-activity
./fixed-v2/blackbox.exe study queue-shadow --source D:/bbwt-research/itch-data/12302019.NASDAQ_ITCH50.gz.prefix-120MiB \
    --symbol QQQ --symbol AAPL --symbol AMZN --midnight-epoch-ns 1577682000000000000 \
    --expect-design-sha256 0608a9ba4af5b7f03119277570999af47b8212b9285e18a8f1f73f240846f502 \
    --out fixed-v2/oos-1230
python tables.py <run>/study-result.json   # the tables in §6 (local helper, not committed)
```

Each `--out` must be fresh; use a new sibling of `fixed-v2` for a repeat. The ranked
activity outputs are the original local selection records; repeat ranking into fresh
directories if reproducing the mechanical symbol selection.

Publishing the synthetic artefact from the repository root (never copy real-local outputs):
`cp "D:/black box/data/queue-shadow/fixed-v2/synthetic/study-result.json"* web/public/study-results/queue-shadow/`.

## 11. Performance

**Historical measurements, before `inclusive-deadlines-v2`; not corrected-run performance.**
Release binary, single runs on a shared workstation (other agents were building), peak working
set sampled every 20 ms from outside the process (local `measure.ps1`).

| Run | Frames | Wall time | Frames/s | Peak working set |
|---|---:|---:|---:|---:|
| In-sample, first run of the optimized engine (gzip prefix, whole book from the first message, QQQ + AAPL, 3,240 shadows) | 23,022,785 | 21.42 s | 1.09 M | 402.6 MB |
| In-sample, release binary of `4d47a22` | 23,022,785 | 14.77 s | 1.58 M | 402.6 MB |
| Out-of-sample A (QQQ + AAPL + AMD, 4,860 shadows) | 19,845,594 | 7.51 s | 2.68 M | 405.6 MB |
| Reused December validation (QQQ + AAPL + AMZN, 999 shadows; 120 MiB prefix) | 9,766,139 | not measured | 1.54 M | not measured |
| Synthetic (29 shadows) | 30,034 | 0.02 s | 1.66 M | not measured |

Wall time is measured around the process; frames/s is printed by the command (frames read
divided by its own elapsed time, which includes gzip decoding and hashing the input file). For
comparison, `itch-reconstruct` over the same 23,022,785 messages with one L2 symbol took 16.6 s on
the same machine within the same hour. A first version that scanned every live shadow on every
message of every stock took 87 s; the earlier optimization skipped a symbol until its next arrival
or milestone was due (byte-identical output for that optimization only). This does not describe
the subsequent deadline/checkpoint correction. Peak memory was dominated by the full-market L3 book.

Corrected-run measurements (`fixed-v2/*/run.json`, one release run each; internal elapsed
time includes decoding and hashing; peak working set was not measured):

| Run | Frames | Internal wall time | Frames/s | Shadows placed |
|---|---:|---:|---:|---:|
| Synthetic | 30,034 | 0.0085572 s | 3,509,793 | 29 |
| July in-sample | 23,022,785 | 9.6718421 s | 2,380,393 | 3,240 |
| October OOS-A | 19,845,594 | 7.9387154 s | 2,499,850 | 4,860 |
| December reused validation | 9,766,139 | 2.9934078 s | 3,262,549 | 999 |


## 12. Tests

Relevant commands (verification results are recorded by the integrating run, not inferred
from this list): `cargo test -p bb-analytics --lib shadow_queue` and
`cargo test -p bb-app --lib queue_study`.

Independent transcription/hash audit (read-only; the optional path supplies only local
aggregate artefacts, never raw ITCH messages):

```sh
node docs/research/audit-artefacts.mjs "D:/black box/data"
```

The audit compares all six cohorts' fill/interval/cause, KM, cancelled-volume and
checkpoint table rows with their JSON, the execution study's sensitivity, baseline
summary, divergence, demo-anchor and comparison tables, and the clean-day H-A/H-B1/H0
tallies and maxima. It also verifies both published `.sha256` files and viewer contract
versions. `web/src/features/research/artefact-audit.test.ts` checks study chart/matrix
metrics, accounting, research-home derivations and every queue-lab selector against
independent raw-field oracles; `queuelab/conclusions.test.ts` guards the three-day quote.

- known answers on hand-built streams: `cancel_ahead_advances_the_l3_queue_and_every_model_agrees_on_a_fifo_fill`,
  `cancel_behind_leaves_l3_unchanged_while_probability_models_advance`,
  `partial_executions_ahead_never_fill_the_shadow`,
  `an_execution_through_the_shadow_price_fills_every_path`,
  `an_opposite_quote_at_the_shadow_price_fills_as_a_book_cross`,
  `a_replace_loses_priority_in_l3_but_not_under_risk_adverse`,
  `same_timestamp_messages_keep_file_order_and_arrivals_precede_them`,
  `censoring_at_the_end_of_the_stream_limits_eligibility`,
  `messages_at_or_after_the_window_end_are_ignored`, `empty_side_and_locked_book_skip_placement`;
- inclusive deadlines and complete tied-timestamp groups:
  `review_exact_longest_horizon_fill_is_included`,
  `review_checkpoint_at_horizon_excludes_already_filled_paths`,
  `same_timestamp_checkpoint_uses_the_whole_group_without_look_ahead`,
  `same_timestamp_longest_deadline_keeps_paths_until_the_group_ends`,
  `a_fill_after_the_longest_deadline_is_excluded`;
- formulas: `offline_formulas_equal_hftbacktest_queue_models`,
  `wilson_and_newcombe_match_published_values` (Newcombe 1998 Table III, method 10);
- determinism and no look-ahead: `synthetic_output_is_deterministic`,
  `outcomes_never_depend_on_later_messages` (a run cut at 09:30:04 gives the same outcome for
  every shadow already resolved by then);
- artefact: `synthetic_result_is_byte_identical_hashed_and_self_describing`,
  `a_changed_design_hash_is_refused_and_real_outputs_stay_out_of_web_public`.

## 13. Viewer: Queue Model & Fill Uncertainty Lab

`/queue-lab` (listed under Execution studies, whose `/studies/queue-shadow` route redirects there
with its query, and in the navigation as "Queue model lab", Research Studio, chord `g q`) renders
the published synthetic artefact of §6.4 and nothing else.
Code: `web/src/features/queuelab/` (`contract.ts` validator, `load.ts`, `model.ts` derivations and
URL state, `QueueLabPage.tsx`, `charts.tsx`).

- **Verification first.** The page downloads `study-result.json` and its `.sha256` with the study
  feature's loader helpers, checks the SHA-256 with WebCrypto, then the contract name and version
  (`blackbox.queue-shadow` v1), every field it reads and that each result grid holds every
  symbol × L × τ (× model or checkpoint) combination exactly once. `data_class` must be
  `synthetic`; a `real_local` file is refused. Failures are shown as `unavailable`,
  `hash_mismatch`, `unsupported_contract` or `invalid` with the exact reason, and nothing from the
  file renders.
- **No recomputation.** Fill fractions, Wilson, Newcombe and Greenwood bounds, Kaplan–Meier
  medians, checkpoint means, cancelled-volume shares and hypothesis flags are the artefact's
  numbers, formatted only (three decimals for probabilities, as in §6); the raw values are on each
  estimate's `data-value` / `data-lower` / `data-upper`. An unavailable value shows its recorded
  reason (for example τ = 30 s: "no eligible shadow orders").
- **Sections.** The question with its restriction (a reconstructed L3 position of real orders is
  not ground truth for a hypothetical order; shadow fills assume no market reaction); fill fraction
  per path with Wilson intervals and the L3 reference, Δ versus L3 with Newcombe intervals, and the
  model in focus at every (L, τ); queue-ahead error \|front − ahead_L3\| per model at a checkpoint
  and for the model in focus at every checkpoint; cancelled volume ahead; Kaplan–Meier median time
  to fill with the fill probability by τ; the pre-registered hypotheses table; limitations; artefact
  identity. Each section names its evidence classes: reconstructed state, estimated queue position,
  modelled decision, counterfactual shadow fill, derived metric.
- **One shadow order.** Contract v1 stores aggregates only, so the page does not show or invent a
  per-shadow trace. It links the Market Data Workbench's curated cancel-ahead event
  (`/market/l3-demo?tab=book&example=cancel-ahead`), labelled "illustration on the same synthetic
  generator, not the same shadow order".
- **Hypotheses.** Outcomes are derived from `hypotheses.cells` and
  `models_within_interval_on_every_symbol`; a cell without eligible shadows is "not testable" (an
  empty model list there means no data, not that H-B part 2 holds). The real-data outcomes of §7 are
  quoted as text with a link to this document, marked "Quoted text, not data"; no real result file
  is published or read.
- **URL state.** `symbol`, `model` (an L2 model id), `side`, `latency`, `tau`, `checkpoint`
  (nanoseconds as decimal strings, design values only) and `tab` (phone stage). Every model, L, τ or
  checkpoint change is a history entry, so Back restores it; a tab change replaces the entry. A
  value outside the design is reported and replaced by the first design value. `side` is always
  `pooled`: contract v1 pools buy and sell shadows, and `side=buy` or `side=sell` is answered with
  that statement instead of a value.
- **Phone.** Below 600 px the page is staged as Overview / Fills / Queue / Hypotheses tabs; tables
  stack into labelled rows and chart rows put the label above the marks, so no number is cut off.

Tests: `npx vitest run src/features/queuelab` (verification failure kinds, displayed intervals
equal to the artefact values, URL state, hypothesis derivations) and `npx playwright test
e2e/queue-lab.spec.ts`.
