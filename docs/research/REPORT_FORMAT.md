# Research report format

BlackBox renders three Markdown reports. All three use one evidence structure, so a reader finds
the same information in the same place whichever report they open.

| Report | Endpoint | Renderer |
|---|---|---|
| Run | `GET /api/v1/runs/{run_id}/report`, legacy `GET /api/runs/{run_id}/report` | `crates/bb-app/src/report.rs` `markdown` |
| Comparison | `GET /api/v1/comparisons/{id}/report` | `crates/bb-app/src/report.rs` `comparison_markdown` (data loaded by `v1/comparisons.rs`) |
| Sweep | `GET /api/v1/sweeps/{id}/report?format=markdown` | `crates/bb-app/src/v1/sweeps.rs` `report_markdown` |

`POST /api/v1/reports` saves any of them as an immutable snapshot with its body SHA-256.
Snapshots record `generator_version`: **1** is the earlier flat layout, **2** is the evidence
structure below. A saved body is never re-rendered. `blackbox demo seed` creates a demo report
only when none exists, so reports that are already stored stay byte-identical.

## Evidence structure

Every report has these level-2 sections, in this order:

1. **Research question**: the question the report answers, built from its own fields (strategy
   and dataset for a run, changed configuration paths for a comparison, objective or seed count
   for a sweep).
2. **Hypothesis**: what the source declares. None of the three sources declares a hypothesis, so
   the report says so: "Descriptive run, no hypothesis", "Descriptive comparison, no hypothesis",
   "Selection study, no hypothesis" or "Descriptive ensemble, no hypothesis".
3. **Data source**: what was replayed and where it came from: vendor or generator, exchange,
   symbol, data level, the meaning of the exchange timestamp, licence note.
4. **Dataset provenance**: dataset id, events SHA-256, byte and row counts, window, source input
   files with their SHA-256, receipt-timestamp provenance and shift, and the dataset manifest as
   JSON (run report).
5. **Dataset limitations**: worst quality severity, every failed quality check, book-valid
   fraction, measured or generated feed latency, the market-by-price limit on queue visibility.
6. **Model assumptions**: replay, fill, queue and latency models, feed offset, strategy grid,
   seed, pre-trade limits, fees, accounting, recorder.
7. **Experiment configuration**: a setting table (readable value, configuration field, exact
   value) and the full configuration JSON (run), or the differing configuration paths (comparison),
   or design, objective and partition (sweep).
8. **Methods**: how the numbers were produced (engine and version, ticks, records, comparator
   version and eligibility gate, sweep selection rule, Spearman, CSCV).
9. **Results**: every figure with its machine field in backticks, the full metrics JSON and the
   engine cross-check (run), the metric delta table, pair classes, first divergence, timing
   deltas and the stored comparator notes (comparison), the cell table, selection and ensemble
   tables (sweep).
10. **Visual evidence**: console routes for the same subject (`/forensics/<run>`,
    `/book/runs/<run>`, `/risk/<run>`, `/reports/<run>`, `/data/<dataset>`,
    `/compare/pair?a=&b=`, `/compare/sweeps/<sweep>`). Paths are relative to the console origin.
11. **Sensitivity**: only what exists: "not assessed" for a single run, the number of changed
    paths for a comparison, the spread of the objective over cells for a sweep.
12. **Statistical uncertainty**: only what exists. Runs and comparisons are deterministic: the
    report says that no interval exists and whether the latency model draws from the seed.
    Sweeps report Spearman, the selection-bias z score against its noise expectation, PBO (or
    the stated reason each is unavailable) and the seed-ensemble spread.
13. **Reproduction**: every identity hash and version (below) and, for a run, the POSIX-shell
    recipe.
14. **Limitations**: subject-specific limits, each tied to a field (fill model, queue model,
    latency model, timestamp provenance, final position, dataset kind), the stored caveats of a
    sweep, and a pointer to `docs/LIMITATIONS.md`.

## Naming rules

- Prose uses readable names. The exact machine value follows in backticks:
  ``Touch maker (`touch_maker`)``, ``Risk-averse queue (advances only on trades at the order's
  price) (`risk_adverse`)``, ``Whole-order fills (`no_partial_fill`)``,
  ``metrics only, no per-order alignment (`metrics_only`)``.
- Durations are written with `duration_label` (`bb_core::compare::duration_label`, the same
  labels as sweep axes), followed by the field and its exact nanoseconds:
  ``every 100 ms (`strategy.interval_ns` = `100000000`)``. `OrderLatencyConfig::describe` uses the
  same labels.
- Fees are written in basis points with the exact fraction: ``2 bp (fraction `0.0002`)``.
- Numbers in prose are rounded to six decimals; the JSON blocks keep the exact values.
- Free text stored with the data (the manifest's timestamp note, quality-check details) is
  source material, not report prose: it is quoted verbatim as a block quote (`> …`) under a
  label naming its origin, so a raw value it contains (for example `2000000 ns` in a generator
  note) stays exactly as stored.
- Raw identifiers (enum codes, metric keys, configuration paths) appear only inside backticks or
  JSON blocks. The unit tests strip both and fail if a raw code or a raw nanosecond count
  (`100000000 ns`) remains.
- Sweep caveats come from bb-forensics, which names the objective by its key; the sweep report
  writes the readable name followed by the key instead (``Net P&L (`net_pnl`)``).

## Dataset kind

The replay sentence and the disclaimer depend on `DatasetManifest::kind`:

- `synthetic`: "Replay of a synthetic session generated by BlackBox, not recorded exchange data".
  Such a report never says "historical"; its limitations state that the session says nothing
  about any real market.
- `recorded`: "Replay of recorded market data"; the provenance section adds that vendor data must
  not be redistributed.

The comparator (bb-forensics, comparator version 2) stores the note "Both runs are historical
replays without market impact, …". The comparison report restates that one note with the
datasets' kinds; the stored summary, and therefore its SHA-256, keeps the original text.

## Hashes and versions

| Report | Included |
|---|---|
| Run | run id, config SHA-256, result SHA-256, dataset events SHA-256, every source input SHA-256, engine name, version and features, BlackBox version, simulation revision, contract version, platform class, build profile |
| Comparison | comparison id, comparator version, stored summary SHA-256; per run: run id, config SHA-256, result SHA-256, dataset id, version and events SHA-256, engine, BlackBox version, simulation revision, contract version, platform class |
| Sweep | sweep id, spec fingerprint, analysis version, the run id of every cell (each run's report carries its hashes) |

The comparison report reads its run identities from database metadata only (run row, experiment
config, dataset version); it opens no artefact bundle. A run that is no longer visible is stated
as such, never filled in.

## Tests

`crates/bb-app/src/report.rs` tests:

- `synthetic_run_report_follows_the_evidence_structure`: section order, every hash and version,
  JSON blocks, readable names with codes, no "historical", no raw codes in prose.
- `recorded_run_report_names_recorded_data`: recorded replay sentence, measured timestamps,
  redistribution note.
- `replacement_dataset_bytes_are_refused`: a manifest whose events hash differs is rejected.
- `comparison_report_names_assumptions_and_keeps_every_hash`: section order, both runs' hashes,
  readable configuration differences, restated replay note, the unavailable-run case.
- `sweep_report_names_parameters_and_keeps_codes`: section order, spec fingerprint, readable
  parameters with codes, selection and console routes.
