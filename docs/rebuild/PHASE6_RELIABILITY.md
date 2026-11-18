# Phase 6 reliability: compare-set busy, missing comparisons, snapshot freshness (A1–A3)

Branch `p6/rel`. Every figure below comes from a command run for this work on the development
machine (Windows, 24 logical CPUs, `dev` profile with optimisation, local Postgres 18, file
artefact store); production was not touched. Hosted figures are marked as estimates.

## A1 — cold `503 busy` on `/compare/set?demo=execution_validation`

### What the page did

Replayed with a throwaway script against a freshly seeded local stack (`demo seed --wait`, 12 demo
runs, `BB_WORKER_SLOTS=1`, 4 heavy-read permits with a 2 s wait, cold bundle cache): one page load
issued 16 API requests, among them **seven `GET /runs/{id}` at once**. Each of these verifies the
run's whole bundle under a heavy-read permit (fetch + `verify_file`), although the page only used
the run's label, dataset, metrics and configuration, all of which are database rows.

| Measurement (local) | Value |
|---|---|
| Demo bundle sizes (12 runs) | 690,455 – 1,036,544 bytes |
| One cold `GET /runs/{id}`, sequential | 141–233 ms wall, 125–203 ms server CPU |
| Same run, warm | 1.5–9.6 ms |
| Former cold page fan-out (16 requests) | 506 ms to usable, 1,672 ms server CPU, peak working set 67 → 244 MB |
| Former warm page fan-out | 82 ms |
| Stored comparison summaries (with per-order pairs) | 348–658 kB aligned, ~5 kB metrics-only |

On the hosted instance (0.1 CPU, 512 MB) the same 1.7 s of CPU is about 17 s of wall time
**[estimate: CPU time / 0.1]**, plus the Blob fetches, so any run read waiting more than 2 s for a
permit is refused `503 busy`.

### Root cause

Two defects, both reproduced in `compare_set_overview_reads_no_bundle_and_busy_run_reads_recover`
with a store that holds every read 4 s:

1. **Fan-out of whole-bundle reads for an overview.** Seven run reads compete for four permits.
2. **A whole-bundle read outside the heavy-read limit.** `verified_summary` resolved a
   `summary_sha256` mismatch caused by JSONB number normalisation (2 of the 7 demo runs) by
   opening — fetching and verifying — the whole bundle *without* a permit. Measured before the
   fix: the seven-request fan-out made 6 store reads and 1 of 7 requests was refused busy; with
   all whole-bundle reads under the 4 permits and a 2 s wait, 3 would have been refused, so 2
   reads ran outside the limit **[inference from those two counts; read sizes were not logged
   before the fix]**. This is consistent with the single `503` seen in production
   (PHASE31_RELEASE). After the fix the test counts whole-bundle reads in flight: the fan-out
   peaks at 4 (a one-off log of read sizes showed the mismatch path reading only the ~191-byte
   header and the ~210–240 kB trailer, verified against the trust root), and one request is still
   refused, which is the intended admission behaviour for that fan-out.

### Repair

- `web/src/features/latency/LatencyLabPage.tsx`: the overview reads no run by id. Runs come from
  the verified run list (`GET /runs`, every row checked against its digest; the demo snapshot for
  anonymous visitors) with the configuration the experiment declares for that run id; pairs from
  the stored, digest-checked comparisons. A run outside both (an ad-hoc set) is read by id **two
  at a time**. Measured with the same script: **9 requests, 182 ms, 156 ms server CPU, peak
  working set 67 → 97 MB, zero store reads.**
- `web/src/api/client.ts`: `retryBusy` retries an idempotent read answered `503 busy` at most
  3 attempts in all, after `Retry-After` clamped to 1–5 s plus ≤ 250 ms jitter; abortable. The
  last refusal is shown with Retry and the number of attempts — never as a missing comparison,
  and the panels state that they show only what was read.
- `crates/bb-app/src/v1/runs.rs`: the summary mismatch path uses `verified_trailer` instead of a
  whole-bundle open (no bundle read outside the permit).
- Limits unchanged (4 permits, 2 s wait, in-flight limit). Whole-run decodes and concurrent cold
  fetches are single-flight. Each active/waiting fetch owns a drop guard: success, failure and
  request cancellation all remove the last map entry. Gate references are released while
  holding the map lock, so simultaneous cancellations cannot both leave cleanup to the other.

### Verified

| Case | Test | Result |
|---|---|---|
| Cold overview, no bundle read | `compare_set_overview_reads_no_bundle_and_busy_run_reads_recover` | 25–53 ms, 0 store reads |
| Warm anonymous overview | same | 0 database acquisitions (comparison details now from the snapshot) |
| 8 concurrent page loads | same | identical classification, 0 store reads |
| Former fan-out, slow store | same | ≥ 1 refused `503 busy`, `Retry-After: 2`; ≤ 4 whole-bundle reads in flight |
| Retry after `Retry-After` | same | refused reads succeed |
| Client cancel | same | 4 reads abandoned after 300 ms; 4 new cold reads right after are all admitted (200) |
| Two-at-a-time fallback, cold, 4 s store | same | 7/7 succeed in 32.8 s |
| Bounded retry in the browser | `web/e2e/compare-set.spec.ts` "busy server" | 3 attempts, then "unavailable: server busy" + Retry; Retry completes the page |
| No run read by id in the browser | `compare-set.spec.ts` first test | 0 `GET /runs/{id}` |

## A2 — "N runs have no stored comparison"

### Cause

Reproduced locally: with a server running, the six `execution_validation` comparisons were
deleted and recreated by a second `demo seed` (another process). The anonymous `/comparisons` list
came from the snapshot built before (`MAX_AGE` 1 h), so the page found no pair for five runs (and
the old ids it did find answered 404). In production the same flow (deploy, then seed pass 2 from
the operator machine) left the list without the new comparisons, so the page reported every run
as having "no stored comparison with the baseline" and showed a locked *Create comparisons*
control. Two UI defects made it misleading: a run not found in a possibly stale or capped list
was reported as missing, and anonymous visitors were shown a locked create action as if a curated
demo required it. The demo itself is consistent: 7 runs, 6 curated pairs — 3 aligned with the
baseline, 2 metrics-only across datasets, and the depleted risk-adverse run paired with the
depleted baseline (one change at a time).

### Repair

The page now classifies every run of the set in a "With the baseline" column (`data-pair-state`)
and in the panels: compared with the baseline (aligned / metrics only, not like-for-like),
compared the other way round, compared with another run instead (link, preferring a run of the
set), not among the listed comparisons (list capped), not compared, unavailable because busy
(after bounded retries), could not be read, loading. *Create* is offered only to a key with the
`run` scope and only for a truly missing pair (nothing stored about the run); an anonymous
visitor is never shown a locked create control. A3 removes the staleness itself.

Tests: `compare_set_overview_reads_no_bundle_and_busy_run_reads_recover` (anonymous and keyed:
`aligned ×3, metrics_only ×2, elsewhere ×1` after a clean seed) and `web/e2e/compare-set.spec.ts`
(3 aligned, 2 metrics-only, 1 compared elsewhere with link, no missing state, no create control,
no locked control in the Run set panel).

## A3 — snapshot freshness

### Design

- In-process changes: unchanged (generation → rebuild within about 1 s; reads rebuild first).
- External writes: `Database::demo_data_version` (`crates/bb-db/src/demo_version.rs`) is one
  statement and one round trip. Each table contributes MD5 of the primary-key-ordered JSON array
  of `(primary key, xmin)` pairs (the quality report key has two components). This replaces
  `count + sum(xmin)`: XIDs follow allocation, **not commit order**, so two updates can preserve
  the old sum. Associating each XID with its key prevents this cancellation; inserts/deletes
  change the key set and updates change the row's XID, including with disabled triggers.
  JSON arrays avoid delimiter ambiguity in text/composite keys. Like any finite digest, this
  is not a collision-proof audit log or a transaction-ID-wraparound detector.
  Cost is a scan plus a per-table primary-key sort/aggregate (`O(n log n)`), scoped to demo and
  public catalogue dependencies, not private experiment history. On a freshly seeded local
  Postgres 18 `blackbox_test_backfix` (12 experiments, 12 runs, 3 datasets/versions/quality
  reports, 11 comparisons, 4 reports), `EXPLAIN (ANALYZE, FORMAT JSON)` of the actual statement:
  first execution **1.577 ms**, 20 warm executions **0.8875 ms median**, **0.774–1.141 ms**
  range (server execution only, not client round trip).
- The version is read before every build. An anonymous read at least `RECHECK` (15 min) after the
  last build or check reads it again; one reader at a time (others keep the current snapshot). A
  change bumps the read version, so the snapshot and the 30 s anonymous response cache are rebuilt
  before anything else is served. A failed check keeps the snapshot (retried after `RECHECK`); a
  snapshot unconfirmed for `MAX_AGE` (1 h) is rebuilt; a failed build leaves no snapshot (database
  path for 60 s, then one retry). Comparisons are kept without per-order pairs (348–658 kB → a few
  kB each), which bounds the snapshot and lets anonymous `GET /comparisons/{id}` of demo
  comparisons skip the database (part of SEC-10/B7).
- Worst-case staleness for an external write: `RECHECK` + up to 30 s of anonymous response cache +
  one build. Database cost: reads served from the snapshot query Postgres at most once per 15 min at
  any request rate, so Neon (5 min to scale to zero) sleeps at least 2/3 of the time; at 0.25 CU
  that bounds this path to 60 CU-h/month **[arithmetic: 0.25 × 720 h / 3]**, against 15 CU-h for
  the previous hourly full rebuild (which also ran on every hourly expiry, not only on change).
  The interval is the explicit trade-off; it is `DemoSnapshot::set_recheck` for tests and drills.

Audited snapshot dependencies (`snapshot::build`, its renderers and their database joins):

| Resource | Rows covered by the version |
|---|---|
| Runs, experiments | Experiments, runs, configs, referenced dataset versions, creating API keys, workspaces (slug and public visibility), linked jobs |
| Jobs embedded in experiments | Job rows, their configs and their referenced run/config (including a job link that differs from the experiment's config) |
| Comparisons, saved reports | Every column of the demo workspace's comparison/report rows, including integrity digests; pairs are removed only after reading |
| Sweeps | Demo sweep rows and every cell experiment/job/config/run selected by `sweep_cell_views`, even a cell linked across workspaces |
| Datasets | Global/demo dataset rows, their versions, every quality report/checker version, referenced data sources |
| Presets, demos | Dataset/version manifests plus visible experiment/config/run identities, including other public workspaces used by `visible_identities` |

All columns of each covered row are protected by `xmin`; newly reachable rows enter through
their own key/version or a changed parent/link row. All workspace rows are included because a
previously private workspace can become public. `instruments` is **not read** by these paths:
the displayed instrument comes from the dataset version's manifest and tick/lot/contract-size
columns. Presets/demos have no database table. Artifact pages/trailers, run/dataset analytics,
quality report bodies, audit/usage counters and system status are not snapshotted by these routes.

### Verified

`snapshot_freshness.rs`:

| Case | Result |
|---|---|
| Initially empty database | lists empty, from memory |
| External dataset registration (dataset version) | invisible within the interval, visible after it; presets follow |
| In-process runs and comparison creation | visible at once |
| External comparison creation (second runtime, as `demo seed`) | visible after the interval; anonymous detail with 0 database acquisitions |
| 32 concurrent readers after an external change | every list complete (old or new label), 31 acquisitions = 1 check + 1 build of 30 |
| Changed comparison row, changed run row | dropped at the next check; detail `integrity_failed`; run quarantined |
| Failed rebuild (`bb.workspaces` renamed) | no snapshot, database path answers current rows; rebuilt after recovery, then 0 acquisitions |
| Server restart | current rows at start |
| Standalone worker, restarted between two jobs | completion invisible within the interval, visible after it, both rounds |
| Equal-sum row updates | `row_version_fingerprint_detects_equal_sum_updates`: old sum equal, new fingerprint different (transaction allocation order opposite commit order) |
| Failed version check (`bb.workspaces` renamed) | keeps the last confirmed snapshot; an in-process generation change then forces the failed-build/database-path case above |
| External newer quality checker | `external_quality_check_refreshes_the_anonymous_dataset_summary`: anonymous summary changes checker 1 → 101, passed true → false, errors 0 → 1 after recheck |

Cancellation regression: `cancelling_active_and_waiting_cold_opens_cleans_the_single_flight_map`
aborts a cold `open_run` with its actual store GET blocked: map length **1 → 0**, also when a
second reader is cancelled concurrently. The existing three-reader cold test still requires
**exactly one** whole-bundle download.

## Open

- `verify_bundle` still takes a heavy permit for a cache hit; a public "cached?" check could let
  warm reads skip the queue. Concurrent cold fetch duplication and cancelled map-entry retention
  are fixed (above).
- SEC-10/B7 remains open for run detail reads and for unknown comparison ids (database path).
- The local server's peak working set reached 274 MB after it had served the former page
  fan-out several times (the cold fan-out alone measured 67 → 244 MB above); not investigated
  further here.

## Gate (2026-11-18, `p6/rel`)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo test --workspace` (`blackbox_test_rel`) | 49 test binaries, 499 passed, 0 failed, 3 ignored |
| `npx tsc -b` | pass |
| `npx vitest run` | 19 files, 173 passed |
| `npx playwright test e2e/compare-set.spec.ts` (port 18860, `blackbox_e2e_rel`) | 2 passed |
