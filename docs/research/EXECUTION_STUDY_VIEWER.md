# Execution study viewer

Phase 6, tracker E1–E4. The `blackbox study exec-validation` output
([EXECUTION_MODEL_VALIDATION.md](EXECUTION_MODEL_VALIDATION.md)) is now visible in the product at
`/studies` and `/studies/exec-validation`, read-only, from a published synthetic artefact. There is
no upload, no API route and no database change.

## Contract `blackbox.study-result` version 1 (E2)

`study.json` is a research output. It carries wall-clock fields (`wall_time_ms`, every cell
manifest's `created_unix_ms` and `wall_time_ms`), full run manifests and per-cell detail files, so
it is neither byte-stable nor bounded for publishing. The study therefore writes a second,
versioned file next to it. `study.json`, `study.md` and `cells/*.json` keep every existing field
and value; `study.json` gains only a `public_result` object (path, contract and SHA-256).

`<out>/public/study-result.json` (`crates/bb-app/src/study.rs`, `PUBLIC_SCHEMA`,
`PUBLIC_SCHEMA_VERSION`, `PUBLIC_FILE`), with `<out>/public/study-result.json.sha256` in
`sha256sum` format. Compact JSON without any line break, so a checkout that rewrites line endings
cannot change the hashed bytes.

| Field | Content |
|---|---|
| `schema`, `schema_version` | `"blackbox.study-result"`, `1`. Bump the version on any incompatible change. |
| `study_id`, `title`, `data_class` | `exec-validation` (`exec-validation-quick` with `--quick`), `"synthetic"` |
| `generator` | Command, `blackbox_version`, `simulation_revision`, `run_contract_version`, engine, platform class, `source_commit` (from `BB_GIT_SHA` at generation, 7–40 hex, else `null`; declared, not verified) |
| `design` | As in `study.json` (generator seed, session length, latency seeds, bootstrap seed/replicates/method, matched median, cross-regime note) |
| `baseline` | Preset, regime, queue model, latency, feed offset, run id, `config_sha256`, dataset id + SHA-256, full `RunConfig` |
| `queue_models`, `latency_models` | Arrays in study order (baseline first) of `{name, config}`; latency levels also carry `stochastic` |
| `uncertainty` | `source: "seed_variation"`, method, level 0.95, replicates, seed, the stochastic latency levels it applies to, what it excludes |
| `datasets` | Per regime: id, regime, events SHA-256, bytes, counts, generator settings |
| `cells[]` | Per run: `run_id`, `config_sha256`, `result_sha256`, dataset id + SHA-256 + rows, seed, queue model, latency, feed offset, regime, design, stochastic, scalar `measures`, `accounting` (realized, unrealized, gross = realized + unrealized, fees, net), order counts, `comparison` with the within-regime baseline (config diffs, aligned / like-for-like, first divergence and first outcome divergence as order id + tick + time + class + differences, lifecycle counts), `detail` path of the full local cell file |
| `aggregates`, `sensitivity_surfaces` | As in `study.json` (deterministic single value or seed mean with bootstrap interval) |
| `demo_anchors` | The `execution_validation` demo runs recomputed on the demo's own datasets (below) |
| `caveats` | As in `study.json` |

Not included: tapes, fills, equity, order lists, full run manifests, wall-clock or build-profile
facts. Same binary and platform class, same output bytes: two full runs gave identical files
(`cmp`), SHA-256 `d4afcfb8ab05f3313d164c53a213e99a75f5c41c125c4e42fc1eee24084ce659`, 232 946 bytes,
57 cells, 25 aggregate groups, 3 sensitivity surfaces, 7 anchors. The study's existing outputs are
unchanged: the cells (run ids, config and result hashes, measures, comparisons, deltas),
aggregates, surfaces, design and caveats of `study.json` hash identically (canonical `jq -S`) to
the output `data/studies/execution-validation` generated before this change, and `study.md` is
byte-identical (`cmp`).

**Demo anchors.** The study runs live on study-only datasets (`study-<regime>`, 30 s), so none of
them is a stored public run. To make the run → comparison → forensics chain reachable from real
evidence, the study also regenerates the demo's datasets (`SyntheticConfig::default()` and
`demos::stress_datasets()`, 120 s) and runs `demos::execution_validation` with its curated pairs
(`demos::curated_pairs`), keeping only pair-A runs resident. The anchor run ids equal the ids
`demo seed` stores (`253bbc23b342f373`, `08200b05ca35723f`, `5a063572329e1c80`,
`bcf5c37532e25266`, `88e0971baa6491ab`, `69ca873c448e2dd1`, `4d0de6620234a7fe`; pinned by the unit
test), and `synthetic-demo`'s events SHA-256 is the pinned
`0bc26426a4c945630737db01e82921c28edf45c37f8de1671d45ce921e818f38`.

### Publishing

```sh
cargo build --release -p bb-app --bin blackbox
BB_GIT_SHA=$(git rev-parse HEAD) target/release/blackbox study exec-validation --out data/studies/exec-validation-public
cp data/studies/exec-validation-public/public/study-result.json* web/public/study-results/exec-validation/
```

The published file was generated from `af3a28477540244fe7d89e12474104b83ece5e24` plus the
uncommitted study export code of this change (simulation code unchanged), release binary,
platform class `windows-x86_64-gnu+libm-0352e099`. The artefact lives under
`web/public/study-results/`, not `web/public/studies/`: a `studies` directory in the build output
would shadow the `/studies` route. `blackbox serve --web-dir web/dist` answers a directory path
with a redirect to its trailing-slash form (observed: `GET /study-results` → 307
`/study-results/`), while `GET /studies` and `GET /studies/exec-validation` return the app (200).

Reproduction check at the Phase 6/7 release candidate (`p6/int` `cfd3d0c`, debug build, same platform
class): `blackbox study exec-validation` wrote a `study-result.json` that equals the published one in
every field except `generator.source_commit` (57 cells, 7,145 ms). The published file is therefore kept
unchanged; its `source_commit` still names the commit it was generated from.

## Viewer (E1)

`web/src/features/study/`: `load.ts` (the public loader API, below), `contract.ts` (types, hash
and contract checks), `cellKey.ts` (`?cell=` keys), `explore.ts` (selection, controlled
differences, comparison labels), `views.ts` (validated B2 views and B4 spreads), `links.ts`
(stored-run matching, evidence chain, reproduction command), `model.ts` (comparisons, data
classes, labels), `charts.tsx` (dot/interval, decomposition and seed-strip charts as positioned
marks on a shared scale, numbers printed beside every mark; no chart library), `CellPanel.tsx`,
`StudyViews.tsx`, `parts.tsx`, `StudiesPage.tsx`, `study.css`. Routes in `web/src/App.tsx`:
`studies`, `studies/:studyId` (one lazy chunk).

Loading: the page downloads the JSON and the hash file, hashes the bytes with WebCrypto
(`crypto.subtle.digest('SHA-256')`) and compares before parsing anything, then checks `schema` and
`schema_version`, then the shape of every field it reads. Each failure is a visible error
(`data-testid="study-error"`, `data-kind` = `unavailable`, `hash_mismatch`, `unsupported_contract`,
`invalid`) and nothing from the file is shown.

Sections: identity (study, contract, verified content SHA-256, generator, source commit, platform
class, command); how to read the numbers (E4); design (baseline preset, dataset/regime, queue
model, latency distribution, cancel delay, feed latency, seed ensemble, held-constant assumptions,
what each design block varies, latency levels, datasets); a measure picker (`?metric=`);
sensitivity matrix queue × latency per regime with deltas against the regime's reference cell;
queue-model comparison; latency comparison (including asymmetric, cancel-delay and feed-offset
rows); fill rate across every group; fee/P&L decomposition (gross − fees = net, mean over a
group's seeds); regime comparison; evidence (anchors); every study run with hashes and its
reproduction command.

## Public loader API (Phase 7)

Other features import only `web/src/features/study/load.ts`:

- `loadStudyResult(id: string, signal: AbortSignal): Promise<StudyLoadResult>` downloads
  `/study-results/<id>/study-result.json` and its `.sha256`, verifies the hash, the contract and
  the shape, and resolves `{ ok: true, study, sha256 }` or `{ ok: false, kind, message }` with
  `kind` one of `unavailable`, `hash_mismatch`, `unsupported_contract`, `invalid`. It never throws
  for a missing or bad study; it rejects only when `signal` aborts.
- `PUBLISHED_STUDIES`, `studyHref(id, { cell?, metric? })`, `runCellKey(runId)`,
  `groupCellKey({ regime, queue_model, latency, feed_offset_ns })`, type `StudyResult`.

## Explorer (Phase 7, B1–B4)

**Cell selection (B1).** Every matrix cell, chart row (queue, latency, inventory, fill rate,
decomposition, regime), seed mark and seed value, the cancel/feed delta tables and every run row
selects a cell. The selection is the URL query `?cell=`:

| Key | Selects |
|---|---|
| `run.<run_id>` | One run: a study cell or a demo anchor run (16 lowercase hex) |
| `group.<regime>.<queue_model>.<latency>.<feed_offset_ns>` | One aggregate group (all its latency seeds) |

A selection pushes a history entry, so Back and Forward restore it; `?metric=` and the phone
`?tab=` replace. A key that does not parse or names nothing shows "No such cell". The detail panel
(a sticky column from 1180 px, a drawer below that, the Cell tab on a phone) shows: exact
parameters (dataset, queue model config, latency distribution, cancel delay, feed offset, seeds,
design block); the baseline configuration (levels, run, config hash, full JSON); controlled
differences against the study baseline (only fields that differ, readable names, the raw config
path on hover and to screen readers); key results against the comparison reference with deltas;
run identity (run id, config, result and dataset SHA-256 in full); order-level evidence (the
stored-run chain only when E3 matches the run by config and dataset hash, otherwise "Not stored:
reproduce locally" with the command; plus the artefact's first divergence, first outcome
divergence and order-pair classes); statistical notes; and reproduction commands.

Controlled differences come from the run's artefact comparison. A cell compared with its
regime's baseline run (study baseline levels and seed on that regime's dataset, checked field by
field) gets the dataset row added from the two dataset ids; any other reference is reported as
not derivable. A config field absent on one side and `null` on the other is not a difference. A
group lists its members' shared differences plus the seeds it averages over, and refuses when
members differ in anything but the seed.

**Views (B2).** Each view is built in `views.ts` only when its fields exist and agree, otherwise
the page prints "Omitted: <reason>" in its place:

- *Inventory impact*: mean absolute inventory (with seed intervals) of every baseline-regime
  group outside the factorial block, and the maximum over each group's runs; requires both
  measures on every member, finite and non-negative, mean ≤ max per run.
- *Cancellation delay* and *feed delay*: the latency levels with a separate cancel leg, and the
  feed-offset groups, against the baseline group, measure by measure, plus cancelled orders;
  requires every member compared with the reference run, aligned like-for-like on the same
  dataset hash, config diffs only in the varied factor (feed: the diff carries the group's
  offset), and `filled + canceled ≤ submitted`.
- *Seed variation*: a strip per stochastic group (each seed a mark on the row's own scale, the
  mean as a tick, the bootstrap interval as a band, every seed value printed); requires the
  members, distinct seeds from `design.latency_seeds`, a seed count equal to the bootstrap `n`,
  the bootstrap mean equal to the mean of the seed values (relative 1e-9) and the interval inside
  the seed range.

**Comparison labels (B3).** `classifyComparison` derives the labels from facts, never from a
row's position: equal dataset SHA-256, the comparison's `aligned && like_for_like`, the set of
controlled factors whose config paths differ (dataset, queue model, order latency, feed latency,
seed; the seed is dropped for a multi-seed mean) and the number of runs behind the value.

| Label | When |
|---|---|
| Like-for-like paired | Same dataset and aligned order by order |
| Metrics only | Not aligned (including every cross-dataset pair) |
| Cross-dataset sensitivity | Dataset hashes differ |
| Multi-seed | The value is a mean over more than one seed |
| Difference not causally attributable | Two or more factors differ on one dataset, any factor besides the dataset differs across datasets, or the differences are not in the artefact |

A group is compared with its regime's baseline group; a regime baseline with the study baseline;
a run with its artefact reference (a regime baseline run with the study baseline run). A single
seed of a stochastic model therefore reads "paired, not attributable" (latency model and seed
both change); its group mean reads "multi-seed, paired". Labels appear on every chart row,
matrix cell, run row, anchor row, delta table and in the panel.

**Uncertainty (B4).** One section separates: seed variability (what the intervals are: method,
resamples, bootstrap seed, seeds, which latency models, with the seed strips); parameter
sensitivity (paired min/max/range across queue models, latency levels and feed offsets);
dataset variation (range across the regime baselines; one generated session per regime, generator
seeds not varied); sampling error (not estimated here); model uncertainty (not addressed) and
real-world calibration uncertainty (not addressed; synthetic). Nothing is annualised and nothing
is a profitability claim.

**Phone (B5).** Below 600 px the page keeps its header (synthetic badges, calibration notice)
and stages the rest as Overview (identity, how to read, design, evidence, every run) / Matrix
(measure, matrices, all comparison views) / Cell (the panel) / Uncertainty tabs; selecting a cell
opens the Cell tab. Number tables stack into labelled rows and chart rows put the label above the
marks, so no value is cut off; hashes print in full and wrap.

## Links to stored runs (E3)

A run links only when `GET /demos` lists a run with the same run id, which the run manifest
defines as the first 16 hex characters of `config_sha256` (that hash covers the run contract
version, engine, dataset SHA-256 and the whole configuration), the study's `config_sha256` starts
with it, and `GET /runs/{id}` reports the same dataset events SHA-256. The API exposes no full
config hash, so the prefix is the strongest config check available. Labels, positions and dataset
ids are never used. A stored run whose `result_sha256` differs is still linked (same inputs) and
says "result hash differs (other platform class)".

Chain for a linked run: Run (`/forensics/<run>`), Comparison (`/compare/pair?a=<ref>&b=<run>`,
only when the reference run is linked too), First divergence (`/book/runs/<run>?t=<decision
ns>&order=<id>`) and Order forensics (`/forensics/<run>/orders/<id>`), both only for an aligned
comparison with a divergence, and Report (`/reports/<run>`). Every other run shows why it is not
linked (no stored run with this config hash; dataset hash mismatch; stored runs not checked) and a
"Reproduce locally" block: the study command, where the run's summary lands, and the expected
`config_sha256` and `result_sha256` with the platform class.

## Statistical honesty (E4)

Every value carries one of three data classes, defined on the page:

- **Deterministic outcome**: one run of a fixed configuration on a fixed synthetic dataset; no
  sampling error, no interval.
- **Model sensitivity**: differences between runs that change one modelling assumption; how much
  the simulated outcome depends on it, not which assumption is true.
- **Seed variation**: mean and 95 % percentile-bootstrap interval over the seeds of a stochastic
  latency model on one generated market; Monte Carlo dispersion, not market uncertainty.

Plus, for everything: **not calibrated to real markets** (synthetic data; modelled queue
position, latency and fills). Intervals appear only for stochastic latency groups. P&L values are
session totals in quote currency; nothing is annualised, expressed as a return rate or forecast.
Regime comparisons state that regimes replay different events and are not like-for-like.

## Verification

Commands executed locally (Windows):

```sh
cargo test -p bb-app --lib study::
# 3 passed (matched medians, quick smoke, public result deterministic/hashed/anchored)
cargo fmt --all -- --check; cargo clippy --workspace --all-targets -- -D warnings
# clean
cargo test --workspace   # with BLACKBOX_TEST_DATABASE_URL set to a throwaway local database
# every test binary ok, 0 failed (golden: results_match_the_pinned_hashes ok)
npx tsc -b; npx vitest run
# clean; 23 files, 203 tests passed (study: 3 files, 25 tests)
npm run build
# StudiesPage chunk 42.63 kB (12.64 kB gzip), lazy
npx playwright test e2e/studies.spec.ts   # own database and data dir, release binary
# 3 passed
```

Phase 7 (explorer), Windows, debug `bb-app` binary, own database `blackbox_e2e_explorer`:

```sh
npx tsc -b                     # clean
npx vitest run                 # 27 files, 272 tests passed (study: 6 files, 81 tests)
npm run build                  # StudiesPage chunk 91.99 kB (25.45 kB gzip), lazy
npx playwright test            # 70 passed (studies.spec.ts: 7)
```

Unit tests: `study::tests::public_result_is_deterministic_hashed_and_anchored`;
`web/src/features/study/{contract,links,model,cellKey,explore,views}.test.ts` (WebCrypto hash,
hash-file parsing, tampered bytes, contract name/version mismatch, field errors, run-id/config-hash
prefix, the published artefact against its hash file; matching by config + dataset hash only,
chain links, reproduction command; data classes, comparisons, decomposition; cell-key parsing and
rejection; selection, controlled differences, comparison labels on the fixture and the published
artefact, order independence; seed-strip, inventory, cancel- and feed-delay validation and
omission reasons; uncertainty spreads). Playwright: `web/e2e/studies.spec.ts` (index → study,
verified hash, labels, charts, metric picker, stored-run chain into the comparison, reproduction
for study-only runs, tampered file, contract version 2; Phase 7: matrix cell and chart-row
selection, `?cell=` in the URL, Back/Forward, keyboard selection, a stored anchor run's panel
opening `/forensics/<run>/orders/<id>`, "Not stored: reproduce locally" for a study run, B2 views
and the six uncertainty sources with no omission, 390 px tabs with full hashes and no sideways
overflow).
