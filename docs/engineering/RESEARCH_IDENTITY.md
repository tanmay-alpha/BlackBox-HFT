# Research identity and addressing

Phase 7, A1–A3. How every research object is identified, how its URL is built, and which links
between objects are allowed. The rule behind all of it: a link that claims "same data" or "same
run" exists only when identifiers and content hashes match; everything else is labelled a related
question. Every in-app URL is built by `web/src/shell/paths.ts`; no component concatenates a
route string.

## Objects, identities and URLs

| Object | Identity (what makes two of them the same) | URL helper (`paths.*`) |
|---|---|---|
| Dataset (stored, L2) | `id` plus events SHA-256 (`dataset.events_sha256` of a run, `events_sha256` of a manifest). The id alone is a name; the SHA-256 is the version. | `dataset(id)`, `bookDataset(id, {t0, len, levels})` |
| Workbench dataset (L3) | `dataset_id` plus `dataset_version` (SHA-256 of the source bytes) and its `generator` settings | `marketDataset(id, {tab, event, type, from, example})` |
| Study dataset | `datasets[].id` plus `events_sha256` inside one verified study result | via the study, `study(id, {cell})` |
| Source provenance class | Declared `data_class` of the dataset or artefact (`synthetic` today). Shown as a badge next to the dataset everywhere; never inferred from a name. | — |
| Regime | Generator intervention name (`normal_liquidity`, `liquidity_depletion`, …); a regime is a property of a generated dataset, so two regimes are two datasets | in `cell` keys |
| Run | `run_id` = first 16 hex characters of `config_sha256`, which covers the run contract version, engine, dataset SHA-256 and the full configuration; `result_sha256` identifies its output on one platform class | `run(id)`, `bookRun(id, {t, order})`, `riskRun(id)`, `report(id)` |
| Strategy configuration | `config_sha256` (full) of a run; the API exposes only the 16-hex prefix as the run id | `strategy({from: runId})` |
| Event index | Zero-based physical frame number in the source (workbench) or a session time `t` in ns after the dataset origin (L2 replay); never a timestamp sort | `marketDataset(id, {event})`, `bookRun(id, {t})` |
| Order | `(run_id, order_id)`; an order id means nothing without its run | `order(runId, orderId)` |
| Comparison | Stored comparison `id`, or the ordered pair `(a, b)` of run ids; deltas are `b − a` | `comparePair(a, b)`, `compareSet({runs, demo})` |
| Sweep | Sweep `id` | `sweep(id)` |
| Study | `study_id` plus the SHA-256 of the published `study-result.json`, verified in the browser against the published hash file before anything is read | `studies()`, `study(id, {metric, cell})` |
| Study cell | `run.<run_id>` (one run: a study cell or a demo anchor) or `group.<regime>.<queue_model>.<latency>.<feed_offset_ns>` (an aggregate group over latency seeds); built only with `runCellKey` / `groupCellKey` of `features/study/load.ts` | `study(id, {cell})` |
| Study metric | A `measures` key of the study contract (`net_pnl`, `fill_rate`, …) | `study(id, {metric})` |
| Report | Run report: the run id. Saved report: `reportId` | `report(runId)`, `savedReport(id)` |
| Research home | — | `research()` |

Query values are view parameters: dropping them keeps the object, only the view changes.
Nanosecond values travel as decimal strings. A curated workbench example travels as both its id
(`example=cancel-ahead`) and its event index (`event=1370`); the event index is authoritative,
the example id only selects the picker entry.

## Allowed lineage links

A link may claim **same data / same run** only when the following match; otherwise it is a
**related question** and says so in its text.

| From → to | Required match |
|---|---|
| Study cell → stored run (`/forensics/<run>`) | A demo listed by `GET /demos` carries the same `run_id`, the study's `config_sha256` starts with it, and `GET /runs/{id}` reports the same dataset events SHA-256 (`features/study/links.ts`, `matchRun`). A different `result_sha256` is shown as "other platform class", the link stays (same inputs). |
| Study cell → comparison (`/compare/pair?a&b`) | Both runs linked by the rule above, and the cell's `comparison.reference_run_id` is `a` |
| Study cell → first divergence / order forensics | The comparison above is aligned and names a first divergence (order id + decision time) |
| Study anchor pair → stored demo comparison (Research home, liquidity shock) | Both anchors match by the run rule; the pair differs only in dataset regime (queue model, latency and feed offset equal, read from the anchor fields, never from labels or positions) |
| Workbench event → workbench book/timeline/metrics | Same `dataset_id` and event index in the URL; the page re-reads the event from the API |
| Research home → study | The investigation's numbers come from the verified study whose `study_id` the link names, and `cell` names the run or group the numbers came from |

Never "same data":

- The workbench `l3-demo` (synthetic ITCH 5.0 messages, seed 7, 30 000 events) and the execution
  study's datasets (`study-<regime>`, 30 s, L2 synthetic generator) are different datasets with
  different generators. A link between them is a related question.
- The study datasets and the stored demo datasets (`synthetic-demo`,
  `synthetic-stress-depletion`, `synthetic-stress-wide-spreads`, 120 s) differ; only the
  study's `demo_anchors` were recomputed on the demo datasets and can match stored runs.
- Two regimes are two datasets: a regime comparison is metrics-only, never like-for-like.
- Stored demos answering a similar question (`gross_vs_net`, `cost_of_latency`) are linked from
  Research home as "related question, different data", with their dataset ids.

## Evidence classes

Copy keeps these apart everywhere: recorded source event (real data), synthetic generated event,
reconstructed state, estimated queue position, modelled decision, simulated fill, derived metric,
unobservable counterfactual. Simulated fills are never called executions; synthetic data is never
called historical or observed market data.

## Research home (`/research`)

`web/src/features/research/`: `investigations.ts` (pure derivations, each failing with a
`SourceProblem` that is shown verbatim), `ResearchPage.tsx`, `research.css`. Sources are read
when the page loads: the study through `loadStudyResult` (`features/study/load.ts`, hash and
contract verified), the workbench through `GET /api/v1/market/datasets/l3-demo`, `/examples` and
`/events/{index}`, stored demo runs through `GET /demos` and `GET /runs/{id}`. A failed source
shows its exact state (`data-testid="rh-source-error"`, `data-kind` = the study failure kind,
the API error state, or `inconsistent`); no number is cached, defaulted or carried over.

| Question | Source and computation | Evidence link |
|---|---|---|
| Why does L2 hide queue information? | Curated `cancel-ahead` event: level total before/after (L2) against the deleted order's FIFO position and, per listed order behind it, position and shares-ahead change matched by order reference (L3) | `marketDataset('l3-demo', {tab: 'book', event, example: 'cancel-ahead'})` |
| How do fees affect market-making P&L? | Baseline run accounting: gross, fees, net, checked `gross − fees = net`; count of regime baselines with fees > \|gross\| | `study(id, {metric: 'fees', cell: run.<baseline>})` |
| What changes when orders arrive late? | Latency levels and feed offset on the baseline regime and queue model: fill rate and net P&L against constant 1 ms | `study(id, {metric: 'fill_rate', cell: group.…entry_slow.0})` |
| How does a liquidity shock affect fills? | Regime-baseline runs: fill rate and net P&L, depletion and widening spreads against normal; stored anchor pairs linked by the run rule | `study(id, {metric: 'fill_rate', cell: run.<depletion baseline>})`, `comparePair(a, b)` |
| Which execution assumptions change the result most? | Every `one_factor` group on the baseline regime minus the baseline group in net P&L (deterministic value or seed mean), ranked by absolute change, ties in study order; regimes and factorial groups not ranked | `study(id, {metric: 'net_pnl', cell: group.<top>})` |

Navigation: Research home (`g h`) heads the Explore section and is linked from the latency
experiment's "Research views" menu; Execution studies (`g s`) is in Research Studio; both are in
the command palette, and the document titles name them. Breadcrumbs carry the workbench dataset
and event, and the study and cell.

Tests: `web/src/features/research/investigations.test.ts` (derivations on the published artefact,
with the ranking checked against an independent `jq` computation, and every refusal path);
`web/e2e/research-home.spec.ts` (numbers equal the API and the artefact, evidence links land on the
named state, Back/Forward, tampered study, busy workbench API).
