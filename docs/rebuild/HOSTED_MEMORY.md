# Hosted full-process memory (audit AT-5 / SEC-6)

Date: 2026-11-18. Branches `p7/mem` (causes 1–2, rebased on `p6/int` `1c6acab`), `p7/jobmem`
(cause 3, on `p6/int` `42d9237`), `p7/amem` (causes 4–6) and `p8/readmem` (read-path follow-up).
Requirement: the hosted `blackbox serve` process (API + in-process worker + analytics +
parsed-run cache + concurrent HTTP heavy reads) peaks below **300 MB** on Linux. Result: **not
met**, by a 2.6 MiB residual in one of three controlled repeats (phase 4: 288.7 MiB; the other
repeats 277.7 and 273.6). See [Corrected read set](#corrected-read-set-p9baseline) for the current
figures; every other phase stays under 286.1 MiB in all three repeats.

**Every read-phase figure (phases 1, 2 and 4, and summaries quoting them) before the
corrected-read-set section, including those quoted in `SECURITY_DISPOSITIONS.md` and release
records, is marked † = old read set, under-covered: no dataset replay or timeline-window reads.**
The fixture read the dataset span from the wrong DTO fields and the run's dataset from a field
that does not exist, so its 1,073-URI set silently omitted all 120 dataset-replay and 96
timeline-window URIs. The generator now asserts every read family is present.

Historical † summary: the read path was fixed (peak under 32 concurrent anonymous heavy readers
697 → 207 MiB †), resident memory no longer ratchets from job to job, and a largest job's own
working set shrank (run analytics state, the analytics report as `Value` trees, the result-hash
buffer). The `p8/readmem` 512 MB fixture completed every phase: largest jobs up to 362 MiB, the
largest job plus concurrent reads 394 MiB † (was OOM-killed), cancellation 375 MiB, idle 210 MiB.

All figures below come from the runs named with them; MiB = 2^20 bytes (300 MB = 286.1 MiB).

## Fixture

`scripts/research/hosted-memory.sh` (helpers in `scripts/research/hosted_memory_fixture.py`)
runs the `hosted` image exactly as Render does — `BB_PROFILE=hosted`, `BB_WORKER_SLOTS=1`, hosted
admission limits (`ExecutionLimits::hosted()`), migrations on start, TLS `verify-full` to
Postgres, the real Vercel Blob client — in a container limited to `--memory 512m` (no swap), next
to throwaway stand-ins on a private Docker network:

| container | role |
|---|---|
| `bbmem-db` | `postgres:18` with a fixture-only CA/server certificate |
| `bbmem-blob` | fake Vercel Blob API (PUT `?pathname=`, ranged GET, HEAD, delete) answering as `vercel.com` and `memstore.private.blob.vercel-storage.com`, trusted via `SSL_CERT_FILE` |
| `bbmem-app` | `blackbox serve` (hosted image, cold boot after the demo was seeded) |
| `bbmem-driver` | shares the server's PID namespace; samples `/proc/<pid>/status` every 20 ms, resets `VmHWM` per phase (`clear_refs` 5), drives HTTP |

Blob token, API-key pepper and API key are generated per run (Postgres uses the repository's local-only default password on the private network) and
lives only in the containers and a temp file deleted on exit. Nothing external is contacted.
`blackbox migrate`, `demo seed --wait` and `keys mint` run in one-off containers first.

Phases: (1) cache fill — one client touches every heavy anonymous read family once (lazy market
workbench, dataset regeneration, parsed-run cache, demo snapshot, response cache); (2) 32 clients
issue random distinct anonymous heavy reads (run detail/timeline/timeline windows/tape/fills/
orders/equity/report/analytics/engine-check, comparison detail/pairs/report, dataset replay
windows, market workbench event detail, book and metrics; 1,289 URIs, per-family counts in
`result.json` `read_set.families`, any zero count fails the run), each from a new forwarded
address, until 600 requests were answered by a handler; (3) every
candidate job alone; (4) the largest job again with phase-2 load, keyed whole-run reads and a
comparison of the two largest runs; (5) cancellation of a running largest job; (6) 40 s idle.

Candidate jobs (3): the largest admitted shape of every preset on every seeded dataset — minimum
admitted interval, equity sampled every tick, largest admitted feed-latency offset, with the
preset's queue-update cap and without a cap (57 jobs). The list is generated and checked against
the admission code by `crates/bb-app/tests/hosted_full_process_memory.rs`
(`the_fixture_measures_the_largest_admitted_shape_of_every_hosted_preset`).

```sh
scripts/research/hosted-memory.sh                    # builds blackbox-hft:hosted-mem if missing
BBMEM_IMAGE=<tag> BBMEM_OUT=target/hosted-memory/<name> scripts/research/hosted-memory.sh
BBMEM_CANDIDATES=none BBMEM_MEMORY=2g ...            # reads only (phases 1, 2, 6)
BBMEM_APP_ENV=MALLOC_ARENA_MAX=2 ...                 # extra server env (experiments)
BBMEM_HEAPTRACK=1 ...                                # same binary under heaptrack (diagnostic)
```

Machine: Docker Desktop 29.9.2, WSL2 kernel 6.18, 24 CPUs visible to containers (no CPU quota).
Output: `result.json`, `driver.log`, `app.log` under `target/hosted-memory/<name>/` (git-ignored).
Each phase record also carries the server cgroup's `anon` and `file` (page cache: bundles written
to the disk cache count against the container limit but are reclaimable). When the server is
OOM-killed the driver dies with the shared PID namespace and `result.json` stays empty; the
`boot:` and `phase …:` lines of `driver.log` are the record of that run.

## Cause 1: every metadata read built the analytics report as a `serde_json::Value`

heaptrack on the original image (`BBMEM_HEAPTRACK=1`, phases 1–2, 2 GB limit): peak heap
**284.5 MB**, of which the largest group (**183 MB**) was `serde_json::Value` maps allocated by
`BundleReader::check_metadata` → `decode_json::<BundleTrailer>` — the bundle trailer's `analytics`
report deserialised into a `Value` on every page read (`OpenRun::page`), every whole-run read
(`trailer_and_pages`) and every summary/trailer check (`OpenRun::trailer`). Callers of those paths
never use the report.

Fix (`crates/bb-artifacts/src/bundle.rs`, `crates/bb-app/src/artifacts.rs`): `BundleTrailerCore`
— the trailer with the report skipped (`serde::de::IgnoredAny`: still parsed as JSON, still
authenticated by the trailer member digest, `deny_unknown_fields` kept). `check_metadata`,
`verify_file`, `trailer_and_pages` and the new `trailer_core` return it; `trailer()` (complete
trailer) remains for the one caller that needs the report (the analytics fallback in
`v1/runs.rs`). Bundle format, hashes and run identities are unchanged. Test:
`bb-artifacts` `bundle::page_and_whole_run_reads_skip_the_authenticated_analytics_report`.

## Cause 2: glibc malloc arenas keep freed memory resident

Reads-only runs, 2 GB limit, 600 handler-answered requests from 32 clients (phase-2 peak `VmHWM`;
phase-1/2 columns †, old read set, under-covered: no dataset replay or timeline-window reads):

| image | `MALLOC_ARENA_MAX` | phase 1 end | phase 2 peak | after 40 s idle |
|---|---|---:|---:|---:|
| original | default | 246.0 | 697.2 | 697.1 |
| original | 2 | 163.0 | 353.6 | 353.5 |
| first trailer fix (page/whole-run reads only) | default | 233.7 | 620.0 | 620.0 |
| first trailer fix | 2 | 162.8 | 337.7 | 323.1 |

With the default (8 arenas per CPU) each Tokio/blocking thread that decodes a run keeps its own
arena; nothing is returned while idle. Fix: `ENV MALLOC_ARENA_MAX=2` in the Dockerfile `runtime`
stage (both `hosted` and `local`; Render applies image ENV on the next deploy; noted in
`docs/engineering/DEPLOYMENT.md` §3). heaptrack of the first trailer fix with the arena cap still
showed 172 MB under `OpenRun::trailer` (summary checks); the final change routes those through
`trailer_core` too (`p6/int` now uses `verified_trailer` there, which returns the core trailer).

## Results with causes 1–2 fixed (before cause 3)

Phase-1, phase-2 and phase-4 figures in this section are †: old read set, under-covered: no
dataset replay or timeline-window reads.

Run `target/hosted-memory/final2` (image `blackbox-hft:hosted-mem-final`, `p7/mem`, 512 MB limit):

| figure | MiB |
|---|---:|
| boot (idle after start) → phase-1 start (market workbench built, catalogues read) | 90.4 |
| cache fill end (cache contribution over phase-1 start: +28.8) | 119.2 |
| 32 concurrent anonymous heavy readers, peak (overhead +128.6) | **247.8** |
| statuses of phase 2 | 479×200, 31×400, 5×409, 85×503 busy, 9,391×429 |
| single largest job, highest increment over its start | +108 (`latency_heavy_tail`, demo dataset, no cap: 339.1 → 447.0) |
| highest job-phase peak before the kill | **498.6** (job 53 of 57) |
| resident memory carried from job to job (start RSS of job 0 → job 54) | 247.8 → 482.3 |

The container was **OOM-killed at 512 MB** during the 55th of 57 jobs (`driver failed (137)`), so phases
4–6 did not run in this configuration. Of the 54 jobs that finished before the kill, 26
completed, 24 stopped on the hard 150,000 tape-event budget (`budget_exceeded`, nothing stored)
and 4 were refused at submission (below).

## Cause 3: memory a job frees stays inside the glibc arenas

Diagnostic build (not committed; the patch is kept as `target/hosted-memory/malloc-diag.patch`):
after every job the worker logged glibc `mallinfo2()` (summed over all arenas) and the process
`VmRSS`. Run `target/hosted-memory/diag-notrim-2g` (no trim, 2 GB so the sequence completes):

| after | `VmRSS` | arena (heap obtained from the kernel) | in use (live allocations) | free inside the arenas | large `mmap` chunks |
|---|---:|---:|---:|---:|---:|
| 1st job | 266 | 225 | 101 | 123 | 19 |
| 16th job (first `latency_heavy_tail`, demo) | 410 | 380 | 115 | 265 | 19 |
| 27th job | 515 | 472 | 131 | 340 | 19 |
| 51st job (last of phase 3) | 612 | 572 | 184 | 387 | 19 |
| phase 5 (cancelled job) | 613 | 572 | 116 | 455 | 19 |

Live heap stays at 96–184 MiB across the whole sequence; what ratchets is memory that jobs freed
and glibc keeps: free chunks inside the arenas grow 123 → 455 MiB. glibc only gives back the top
of an arena (`M_TRIM_THRESHOLD`), and a job's freed run, analytics and page buffers sit below
later long-lived allocations, so they stay resident. That is fragmentation of freed memory, not
retained heap.

Live caches, from heaptrack of a one-job run (`target/hosted-memory/diag-heaptrack-job`, live
heap at exit 82.3 MB): parsed-run cache 14.7 MB (capped at 2 runs / 64 MiB), market workbench and
L3 demo state ≈ 22.7 MB, one loaded dataset 4.0 MB (dataset registry; one per seeded dataset),
database rows held by run/snapshot reads and the completion path ≈ 11.6 MB. In the trimmed
diagnostic run below, live heap rose 98 → 167 MiB over the 51 jobs of phase 3 (steps at the
largest completed jobs); that growth is not attributed to one cache, and it did not show as
unbounded: after the whole 2 GB run and 40 s idle the process held 205.4 MiB, less than the
266.5 MiB before the first job.
The page cache is outside the process: cgroup `file` grew 11.2 → 80.4 MiB over phase 3 (bundles
written to the disk cache), and it is reclaimed under pressure (the cgroup reached 512.0 MiB
during jobs 47 and 51 of the 512 MB run without an OOM kill).

Fix (`crates/bb-app/src/worker.rs`): after every job (any outcome) the worker loop calls
`malloc_trim(0)` on a blocking thread, Linux glibc only (`cfg(all(target_os = "linux",
target_env = "gnu"))`, `libc` was already in the lockfile; a no-op elsewhere). It returns the free
pages of every arena to the kernel and touches no live allocation, so simulation output, hashes
and admission are unchanged. Effect in the diagnostic run with trim
(`target/hosted-memory/diag-trim-512m`): `VmRSS` 270 → 161 MiB after the 1st job (in use 103),
410 → 199 after the 16th (in use 113), 432 → 253 after the last (in use 167). What a trim
cannot return (RSS − in use − mmap ≈ 39–67 MiB) is the binary, thread stacks and partly used pages.

## Results with all three fixes

Phase-1, phase-2 and phase-4 figures in this section are †: old read set, under-covered: no
dataset replay or timeline-window reads.

Runs `target/hosted-memory/jobmem-512m` and `jobmem-2g` (image `blackbox-hft:hosted-jobmem`,
`p7/jobmem`, identical fixture; phase 4 needs 2 GB to complete):

| figure (MiB) | before (`final2`, 512 MB) | after, 512 MB | after, 2 GB |
|---|---:|---:|---:|
| phase-1 start (boot + market workbench) | 90.4 | 90.5 | 70.4 |
| cache fill end | 119.2 | 140.1 | 109.6 |
| phase 2: 32 concurrent anonymous heavy readers, peak | 247.8 | **245.7** | 266.5 |
| phase 3: jobs that ran (of 57) | 54, then OOM kill | **57** | 57 |
| phase 3: start RSS of jobs 1–56 | 253.1 → 482.3 (ratchet) | 193.2–291.6 | 186.0–248.4 |
| phase 3: highest job peak | 498.6 | **452.0** (job 47) | **460.0** (job 53) |
| phase 3: highest increment of one job over its start | +108 | +203.7 | +246.3 |
| phase 3: jobs that ran and peaked above 286.1 | — | 37 of 51 | 32 of 51 |
| phase 4: largest job + 32 readers + keyed whole-run reads + comparison, peak | not reached | **OOM-killed** (start 291.6) | **485.6** (+237.2) |
| phase 5: cancellation of a running largest job, peak / end | not reached | not reached | 451.6 / 199.8 |
| phase 6: after 40 s idle | not reached | not reached | **205.4** |
| cgroup `memory.peak` (anon + page cache) | — | 512.0 | 563.0 |

Phase 2 statuses (512 MB): 478×200, 27×400, 8×409, 87×503 busy, 9,381×429. Phase 3 (both runs):
27 completed, 24 `budget_exceeded` (150,000 tape events, nothing stored), 6 GLFT submissions
refused with HTTP 500 (below). Job 47 is `synthetic-stress-wide-spreads/avellaneda_stoikov`
without a queue-update cap, job 53 `synthetic-stress-wide-spreads/latency_heavy_tail` with the
cap. Phase 4 (2 GB): the job completed, keyed reads 200 except `/timeline` 400, comparison
created (201) and read (200). The per-job increment is larger than before because a job no longer
reuses free memory left in the arenas by earlier jobs; the peaks themselves are lower and stable.

## Cause 4: run analytics kept every order's execution state until the end of the tape

heaptrack of the `p7/jobmem` image with one largest job (heap peak 439.9 MB, reached in phase 4)
put 124.4 MB of the peak in the execution analytics pass
(`bb-analytics` `execution.rs`): one `OrderState` per order in a `BTreeMap` (50.7 MB) with three
`Spreads` accumulators per order (73.7 MB, 70,251 orders), all alive until the tape ended, about
1.8 KB per order. Fix (`p7/amem` `387a67d`, `compute_streaming_parts`): the first pass walks the
tape once, accumulates the run-wide spread, bootstrap and P&L metrics in tape order (reordering
fills would change the compensated sums and bootstrap bands) and links each order's events by one
index per event; the second pass replays one order at a time, emits its detail if a sink wants it,
and frees its state before the next. Run summaries (`compute_execution_summary`, used by
`run_analytics` and the execution-validation study) build no per-order spread curves or detail
reports. Exact Kaplan-Meier and quantiles still keep one observation per order.

Output identity: the golden test now also pins the SHA-256 of every golden run's analytics JSON
(`ANALYTICS_SHA256` in `crates/bb-app/tests/golden.rs`), computed by the previous code. Both
toolchains compute identical analytics bytes, also for the three entries whose result hashes
differ (the analytics pass calls no platform math function); the new code reproduces all 16 on
WinLibs (Windows) and glibc (`rust:1.100.0-trixie` container). Result hashes are unchanged.

## Cause 5: the bundle write held the analytics report as two `serde_json::Value` trees

heaptrack of the image with cause 4 fixed (`target/hosted-memory/amem-heaptrack-job`, heap peak
362.9 MB in phase 4): the worker parsed the encoded report into a `Value` (`worker.rs:949`,
41.8 MB) and `BundleWriter::write` cloned it into the trailer (`bundle.rs:353`, 38.9 MB), for a
report of about 11 MB of text. Fix (`d4d59be`): `TrailerAnalytics::from_json` converts the
report once to exactly the text the trailer stores (compact serialization of the `Value`, keys
sorted), drops the `Value`, and the writer serializes a borrowed trailer with that text embedded
verbatim; the worker drops the typed report once encoded. The reader is unchanged. Bundle bytes
are unchanged: `bb-artifacts` `trailer_analytics_text_writes_the_same_bundle_bytes` pins the
SHA-256 and size of bundles written by the previous writer, with and without a report, for both
flate2 backends feature unification can select (`miniz_oxide` when `bb-artifacts` builds alone,
`zlib-rs` once `bb-data`'s `zip` is in the build, as in `blackbox`), and all 16 golden runs write
identical bundles before and after (manifest timestamps normalised; checked on Windows).

## Cause 6: the result hash serialized the whole run into one buffer

heaptrack of the image with causes 4–5 fixed (`amem2-heaptrack-job`, heap peak 353.1 MB):
134.2 MB in `bb_core::hash::sha256_json` → `serde_json::to_vec` of `(tape, fills, equity)` for the
run's `result_sha256` (`bb-forensics` `result_sha256`, `pipeline.rs:210`). Fix (`bfa7dfb`): the
encoding streams through a 64 KiB buffer into SHA-256. The hashed bytes are the same (unit test
`streamed_json_digest_equals_the_buffered_encoding` against the buffered digest on nested maps,
special floats and 200,000-row arrays; golden pins unchanged).

## Results with causes 1–6 fixed

Phase-1, phase-2 and phase-4 figures in this section are †: old read set, under-covered: no
dataset replay or timeline-window reads.

Runs at 512 MB, identical fixture, 57 candidate jobs (`target/hosted-memory/<run>`): `jobmem-512m`
(causes 1–3), `amem-512m` (+ cause 4, image `blackbox-hft:hosted-amem`), `amem2-512m` (+ cause 5,
`hosted-amem2`), `amem3-512m` (+ cause 6, `hosted-amem3`, `p7/amem` `bfa7dfb`). `jobmem-512m` was
OOM-killed in phase 4; none of the three `amem` runs was, and each completed every phase. The
cgroup reaching 512 MB is reclaimable page cache (bundles written to the disk cache), not the
process.

| figure (MiB) | causes 1–3 | + 4 | + 5 | + 6 (final) |
|---|---:|---:|---:|---:|
| boot | — | 57.6 | 57.4 | 57.6 |
| phase 1: cache fill, peak | 140.1 | 141.1 | 108.7 | 141.0 |
| phase 2: 32 concurrent anonymous heavy readers, peak | 245.7 | 248.5 | 254.0 | **245.0** |
| phase 3: start RSS of the 57 jobs | 193.2–291.6 | 199.1–265.8 | 188.5–273.9 | 190.3–262.8 |
| phase 3: highest job peak | 452.0 | 444.4 | 402.9 | **339.9** |
| phase 3: highest increment of one job over its start | +203.7 | +209.6 | +153.0 | +114.8 |
| phase 3: highest peak of a budget-stopped job (nothing stored) | — | 361.2 | 344.0 | 321.8 |
| phase 3: jobs that peaked above 286.1 (of 57) | 37 of 51 | 47 | 33 | **19** |
| phase 4: largest job + 32 readers + keyed whole-run reads + comparison, peak | OOM-killed | 462.8 | 475.9 | **435.0** |
| phase 5: cancellation of a running largest job, peak / end | not reached | 473.8 / 233.8 | 431.4 / 215.8 | **375.6** / 214.2 |
| phase 6: after 40 s idle | not reached | 234.4 | 217.4 | **217.3** |
| cgroup `memory.peak` (anon + page cache) | 512.0 | 512.5 | 512.0 | 512.2 |

Final run statuses: phase 1 149×200, 5×400; phase 2 480×200, 29×400, 7×409, 84×503 busy,
9,277×429; phase 3 27 completed, 30 `budget_exceeded` (150,000 tape events, nothing stored;
the six GLFT candidates now submit and stop on the budget); phase 4 job completed, keyed reads
200 except `/timeline` 400, comparison created (201) and read (200); phase 5 `cancelled` (202).
The phase-3 highest peak is `synthetic-stress-wide-spreads/latency_lognormal` without a cap
(235.6 → 339.9). Phase 4 is noisy between runs (concurrent reads dominate it; see residual).

## Residual

Final run (`amem3-512m`): phases 1, 2 and 6 stay under 286.1 MiB (141.0 †, 245.0 †, 217.3). Every
phase that runs a largest admitted job exceeds it: phase 3 by up to 53.8 MiB (339.9; 19 of 57
jobs above, 14 completed and 5 budget-stopped), phase 4 by 148.9 MiB (435.0 †), phase 5 by
89.5 MiB (375.6). († = old read set, under-covered: no dataset replay or timeline-window reads;
the concurrent-read attribution below is from the same under-covered set.)

heaptrack of the final image, one largest job (`amem3-heaptrack-job`, 2 GB, the phase-3 peak job
`synthetic-stress-wide-spreads/latency_lognormal` without a cap): heap peak **322.8 MB**, reached
in phase 4. Live allocations at that peak, grouped:

| component (MB at the heap peak) | MB |
|---|---:|
| **job** (worker), total | **91.7** |
| simulation output: equity sampled every tick (`bb-sim` `lib.rs:741`) | 25.2 |
| simulation output: tape, 150,000 events (`bb-sim` `tape.rs:547`) | 24.2 |
| analytics row insert, the report JSON copied for the query (`bb-db` `api_reads.rs:668`, worker `persist`) | 11.8 |
| run analytics computation (exact distributions and KM observations, `bb-analytics`) | 12.0 |
| orders and fill enrichment (`pipeline.rs:200–205`) | 9.9 |
| trailer analytics text (`TrailerAnalytics`) | 5.9 |
| bundle write buffers | 2.7 |
| **concurrent reads** (phase 4 load, keyed reads, comparison), total | **≈ 196** |
| keyed whole-run reads: `OpenRun::full` decodes the whole `RunResult` (`artifacts.rs:810–816`) | 88.1 |
| response bodies being serialized (`respond.rs:46`; market workbench 38.5) | 50.0 |
| `/analytics`: stored report parsed into a `serde_json::Value` (`v1/runs.rs:1165`) + its DB row | 46.9 |
| comparison and snapshot rows | 7.3 |
| long-lived caches: market workbench, L3 demo state, loaded datasets | 32.0 |
| other | 2.3 |

RSS is above the live heap: the process idles at 217 MiB with about 120 MB of live heap (heaptrack
live heap at exit 122.8 MB); the rest is the binary, thread stacks and partly used arena pages
that `malloc_trim` cannot return. A job starts from 190–263 MiB RSS, so a 91.7 MB job working set
alone crosses 286.1 MiB.

Next concrete steps, by phase:

- **Phase 3 (one job):** run the worker outside the API process (the paid `worker` service in
  `render.yaml`), so the API's 190–263 MiB baseline and a job's working set are not added; or,
  in-process, keep the job under ≈ 30–40 MB: stream equity samples and tape events to the bundle
  instead of holding them in `RunResult` (49.4 MB), and insert the analytics row from the encoded
  text without a second copy (11.8 MB).
- **Phase 4 (job + reads):** serve keyed whole-run reads page by page from the bundle instead of
  decoding the whole `RunResult` (88.1 MB), and serve `/analytics` from the stored JSON text
  (or a typed, streaming projection) instead of a `serde_json::Value` (46.9 MB).
- **Phase 5:** the same as phase 3; the cancelled job runs to completion before the cancel lands
  (observation below).

Admission limits were not raised or lowered.

## GLFT presets now submit

The jobmem runs answered `POST /api/v1/experiments` with HTTP 500 for the GLFT presets (the GLFT
strategy kind was missing from `experiment_configs_strategy_kind_check`). On `p7/amem` all six
GLFT candidates are accepted and stop on the 150,000 tape-event budget with nothing stored.

## Observation: phase 5 cancels a job that has already finished simulating

The largest job simulates in 1–2.5 s; the cancel request is sent 0.5 s after the job is
`running` and reaches the worker only through the next heartbeat. In every run here (again in
`amem3-512m`) the attempt ended without a worker log line and the job became `cancelled` 60 s later
through the reaper
(`reaper moved job … state=cancelled reason=lease_expired`), with nothing stored (`run_id` null).
[INFERENCE] The completion transaction found the job `cancelling`, lost the lease and the worker
abandoned the attempt silently. Memory-wise phase 5 therefore measures one whole largest job.

Confirmed and fixed separately (not in the measured images): a cancellation that reaches the worker after the
simulation finished was refused at completion and dropped as a lost lease; the worker now records `cancelled` in
22–28 ms ([CANCELLATION.md](CANCELLATION.md), `af7543e`).

## Not done / open

- **Bound not met**: with the corrected read set and the final `p9/baseline` policy, phase 4
  peaked at 288.7 MiB in one of three repeats (2.6 MiB over 286.1); every other phase of every
  repeat stays under it ([Corrected read set](#corrected-read-set-p9baseline)).
- No CI job: one run needs a release image build (~10–20 min uncached) plus seeding and 57 jobs,
  and the figures depend on thread count and allocator state; it does not meet the "< 10 min,
  deterministic" bar.

## AT-5 / SEC-6 disposition text (historical, superseded)

Superseded by the proposed row in [Corrected read set](#corrected-read-set-p9baseline). Its
read-phase figures (32 readers 245.0, combined 435.0, concurrent reads ≈ 196 MB) are †: old read
set, under-covered: no dataset replay or timeline-window reads.

> **Open.** Linux full-process fixture `scripts/research/hosted-memory.sh` (hosted image, 512 MB
> container, Postgres + fake Blob, demo seeded; `docs/rebuild/HOSTED_MEMORY.md`). Fixed: metadata
> reads no longer build the bundle analytics report (`BundleTrailerCore`), the image caps glibc
> arenas (`MALLOC_ARENA_MAX=2`), the worker returns freed heap after every job (`malloc_trim(0)`,
> Linux glibc), run analytics keep one order's execution state at a time (was ≈ 124 MB for
> 70,251 orders), the bundle write holds the analytics report as its stored text instead of two
> `serde_json::Value` trees (≈ 81 MB), and the result hash streams instead of buffering the run's
> JSON (134 MB); analytics bytes, bundle bytes, result hashes and run identities are unchanged
> (golden, analytics-byte and bundle-byte pins). At 512 MB every phase now completes without an
> OOM kill: 32 concurrent anonymous heavy readers 245.0 MiB (was 697.2), single largest jobs up to
> 339.9 MiB (was 452–460), the largest job with concurrent reads and a comparison 435.0 MiB (was
> OOM-killed), cancellation 375.6 MiB, 217.3 MiB after 40 s idle. Remaining above 300 MB (286.1
> MiB): phases 3–5. A job's working set is ≈ 92 MB (equity and tape 49 MB, analytics 30 MB) on a
> 190–263 MiB process baseline; concurrent reads add ≈ 196 MB (whole-run decode 88 MB,
> `/analytics` as a `Value` 47 MB, response bodies 50 MB). Next steps: worker out of process or a
> streamed `RunResult`, paged keyed reads, `/analytics` served from the stored text. Admission
> and runtime budgets stopped 30 over-budget jobs with nothing stored.

## Read-path follow-up (`p8/readmem`)

The run analytics endpoint projects the authenticated stored JSON through borrowed `RawValue`
fields and a streaming array visitor. It counts a series without retaining its points, then
decodes only the evenly spaced points the response keeps. Sorted object traversal, the same
downsampling indices and the existing wire conversion preserve body bytes, including the
`downsampled` list and cap-driven halving. The complete stored report is never a `Value` tree;
only the projected response is. Golden runs compare old/new bodies at every halving level and
the seeded demo test compares the actual HTTP body against the previous projection.

Whole-run caller inventory: orders, fills, tape and equity already use the section page index;
export verifies and serves the bundle without decoding a `RunResult`. Markdown only needs the
manifest, metrics, cross-check and section counts, so it now renders from `trailer_core` rather
than `full()`. Its wrapper still accepts a `RunResult` for existing callers; trailer/full report
byte comparisons cover every preset and synthetic/recorded provenance. Timeline genuinely
uses all four sections (including order overlap resolved through `last_seq` tape events), so
it retains the bounded parsed-run cache. Analytics recomputation for legacy bundles without a
stored report also still needs the complete run.

Timeline rows are already wire-encoded; its response now serializes them directly, without
copying the entire page into another `Value` tree. Alphabetical DTO field order preserves the
previous JSON bytes and ETags.

### Hosted remeasurement (512 MB, no swap)

Phase-1, phase-2 and phase-4 figures in this section, both columns, are †: old read set,
under-covered: no dataset replay or timeline-window reads.

Image `blackbox-hft:hosted-readmem`, output
`D:/bbwt/p8-readmem/target/hosted-memory/readmem-512m/`, compared with the previous complete
`D:/bbwt/p7-amem/target/hosted-memory/amem3-512m/`. Same fixture, 32 clients, 600 handler-answered
phase-2 requests and all 57 candidate jobs; no admission or runtime budget changes.

| figure (MiB) | before (`amem3-512m`) | after (`readmem-512m`) |
|---|---:|---:|
| boot | 57.6 | 57.4 |
| phase 1: cache fill peak | 141.0 | 105.3 |
| phase 2: 32 concurrent anonymous heavy readers peak | **245.0** | **207.2** |
| phase 3: highest single-job peak | 339.9 | 361.6 |
| phase 4: largest job + concurrent reads + keyed reads + comparison peak | **435.0** | **394.4** |
| phase 4: increment above its starting RSS | 170.9 | 142.5 |
| phase 5: cancellation peak / end | 375.6 / 214.2 | 374.5 / 192.9 |
| phase 6: after 40 s idle | 217.3 | 210.3 |
| cgroup memory peak (anon + page cache) | 512.2 | 473.1 |

The phase-2 peak fell 37.8 MiB (15.4%) and phase 4 fell 40.6 MiB (9.3%).
These are full-process observations, not isolated allocation estimates: scheduling and allocator
state vary (the unchanged job path peaked higher in this run). Phase 4 answered 415 anonymous
handler requests versus 382 before; successful bytes 0.4 versus 0.3 MiB, most handler answers
being `503 busy`. Phase-2 statuses: 492×200, 31×400, 8×409, 69×503, 8,611×429.

Every phase completed, the server stayed running and `oom_killed=false`. All 57 jobs were
measured: 27 completed, 30 stopped on the tape-event budget; 36 peaked above 286.1 MiB.
Phase 4's job completed, every keyed read returned 200 except the unchanged oversized
`/timeline` window (400), and comparison creation/detail/pairs/report returned 201/200/200/200.
Cancellation returned 202 and reached `cancelled`, with no run stored. The fixture exits 1
because the measured overall RSS peak is still above 300,000,000 bytes, not because a phase or
container failed. **AT-5 / SEC-6 remains open for phases 3–5**; the read changes do not establish
the full-process 300 MB bound.

## Baseline cache experiments (`p9/baseline`)

Every read-phase figure in this section (cache hit rates, phase-2 503s and peaks) is †: old
read set, under-covered: no dataset replay or timeline-window reads. The latency probes and
keyed largest-run timeline probes name their URIs explicitly and are unaffected.

The fresh unmodified `36536d0` image is `blackbox-hft:hosted-baseline-before-ready` (the
`-ready` layer only supplies the valid `BB_GIT_SHA=36536d0` runtime environment). Output is under
`D:/bbwt/p9-baseline/target/hosted-memory/`. These runs use the same 512 MiB/no-swap limit,
32 readers and 600 handler-answered phase-2 requests. As in the historical fixture, there is
no CPU quota; these are not latency predictions for Render's 0.1-CPU service.

### Rejecting undersized caches

Warm-read probes run in a fresh fixture after the RSS sampler stops (`BBMEM_STOP_AFTER=boot
BBMEM_LATENCY=1`), not after the full fixture exhausts the anonymous egress budget. Each path
has one initial read and 11 measured warm reads; p95 is the largest of those 11 observations.
Identical URIs exercise the serialized-response cache. Changing the documented `max_events`
or `max_trades` parameter bypasses only that cache, exercising warm parsed-run/dataset reads.

| warm read, p50 / p95 (ms) | unmodified | 4 MiB parsed / 4 MiB response | one-run 32 MiB parsed / 4 MiB response |
|---|---:|---:|---:|
| identical timeline URI | 0.505 / 0.901 | 0.528 / 0.742 | 0.510 / 0.681 |
| timeline, changing `max_events` | 1.421 / 1.693 | **58.160 / 59.433** | 1.426 / 1.538 |
| identical replay URI | 0.461 / 0.875 | 0.473 / 0.819 | 0.455 / 0.788 |
| replay, changing `max_trades` | 1.270 / 2.062 | 1.234 / 1.743 | 1.146 / 1.293 |

Sources: `baseline-before-latency`, `baseline-4m-latency`, `baseline-32m-latency`.
The measured seeded runs need 7,592,095–9,120,374 decoded section bytes for cache admission.
The 4 MiB parsed cap admits none of them: the unchanged single-flight prevents simultaneous
decodes of the same run, but each subsequent window decodes it again. **The 4 MiB parsed cap
was rejected**; fast repeated identical URIs alone would have hidden the regression.

Controlled response-cache measurements use the same after-image, a one-run 32 MiB parsed cap
and trace-level lookup counters (`baseline-32m-control`, `baseline-32m-response4`):

| response-body budget | hits / lookups (whole reads-only fixture) | phase-2 503s / 600 handler answers | phase-2 peak RSS (MiB) |
|---|---:|---:|---:|
| 32 MiB | 162 / 761 (21.3%) | 90 | 195.9 |
| 4 MiB | 33 / 761 (4.3%) | 74 | 172.0 |

The 4 MiB response cache saved memory but lost most hits. **That cut was rejected too**:
the response cache retains its 32 MiB budget. Its expired/stale bodies are now released by
periodic maintenance even when requests stop, rather than waiting for a subsequent insertion.
The 503 figures vary with concurrent scheduling; lower 503s in one sample do not establish that
discarding cache hits is safe on a CPU-limited service.

### Largest runs, not just seeded demo runs

The 16 MiB/one-run trial also had to be rejected: it kept the small demo runs but not the
largest completed jobs. `baseline-largest-before`, `baseline-largest-16m` and
`baseline-largest-128m` run the same wide-spread lognormal/no-cap and heavy-tail/capped
candidates. The heavy-tail run has 61,906,333 decoded section bytes; the lognormal run and its
new-seed repeat have 70,989,182–71,431,683. The former fit the original 64 MiB cache; the latter
did not. The hosted policy therefore retains **one run / 128 MiB decoded section bytes**,
not two runs / 64 MiB and not an undersized cache that continually re-decodes large runs.

| keyed warm timeline, p50 / p95 (ms), 11 windows | original | 16 MiB / one run | 128 MiB / one run |
|---|---:|---:|---:|
| heavy-tail, capped | 5.652 / 6.411 | **289.431 / 345.894** | 6.892 / 8.125 |
| lognormal, no queue cap | **305.036 / 525.056** | 322.873 / 427.176 | 6.182 / 7.470 |

All 12 requests per probe (initial plus warm reads) returned 200. Keyed reads bypass the
anonymous response cache; the difference is parsed-run retention, not repeated-URI hits.
The 128 MiB trial admitted each of these runs once, not once per window. The local profile's
two-run / 64 MiB defaults are unchanged.

### Retained capacity is not the serialized length

Heaptrack of the original image, replayed to the end of phase 2, attributes **46.6 MiB** to
response-body serialization even though the response cache charges at most 32 MiB of body
lengths. Its `Bytes` retain `serde_json::to_vec`'s geometrically grown `Vec` allocations:
many cached market books hold 262,144-byte allocations. The cache now reclaims the uniquely
owned vector, discards spare capacity with `into_boxed_slice`, and only then shares the bytes
with the outgoing response. Shared/static inputs are copied only when ownership requires it.
No body, ETag, cache key, TTL, hit policy or response-size cap changes. The regression test
`cached_body_discards_spare_capacity_without_changing_bytes_or_etag` checks the retained vector
capacity, exact body bytes and ETag.

The market demo was already lazy. Once its report/examples are built, its source-order
observation and L2 arrays are no longer read by public book/event routes; dropping them reduces
retained market state from **25.708 to 15.557 MiB** in the allocation trace. Book/event-detail
reads across checkpoint boundaries and curated examples are pinned by
`discarded_analytics_inputs_do_not_change_workbench_reads`. The workbench expires after 30 s
without use; a later read rebuilds the same deterministic data rather than retaining it
throughout unrelated jobs.

## Corrected read set (`p9/baseline`)

These are the current figures. The read-set generator now takes each dataset's span from
`current_version.start_ns/end_ns` and a run's dataset from `dataset.id`, and
`read_family_counts` fails the run if any family has zero URIs. Generated set (identical in
every run below, seed `20261118`): 1,289 URIs — run detail/analytics/report/engine-check/equity/
fills/orders 12 each, tape 108, timeline 108 of which **96 time windows**, comparison detail 11,
pairs 44, report 11, **dataset replay 120**, market event detail 400, book 400, metrics 1.

Controlled comparison: the phase-4 job is pinned (`BBMEM_COMBINED_CANDIDATE`) to
`synthetic-stress-depletion/latency_heavy_tail`, interval 1 ms, offset 22.099 s, no queue cap —
the worst-case candidate by standalone peak (the highest phase-3 peak in two of the three final
repeats; in the before run wide-spreads `latency_lognormal`, cap 2000,
peaked 10.2 MiB higher alone) — instead of being chosen by RSS in each run. It completed in
every run. Before = unmodified `36536d0` (`blackbox-hft:hosted-baseline-before-ready`,
`v2-before-pinned`); after = final policy (`blackbox-hft:hosted-baseline-final`,
`v2-final-pinned-{1,2,3}`): one-run 128 MiB parsed-run cache, 32 MiB response cache with
exact-size cached bodies and idle expiry, `MALLOC_MMAP_THRESHOLD_=131072`, hosted pool
`shrink_buffers()` on release, market workbench analytics inputs dropped and idle expiry.
512 MB, no swap, 32 readers, 600 handler-answered phase-2 requests, all 57 jobs. Peak =
max(sampled `VmRSS`, per-phase `VmHWM`), MiB.

| phase (peak MiB) | before | after min | after median | after max |
|---|---:|---:|---:|---:|
| boot | 57.9 | 56.8 | 57.3 | 57.3 |
| 1: cache fill | 149.8 | 90.1 | 94.4 | 114.8 |
| 2: 32 concurrent anonymous heavy readers | 224.6 | 156.9 | 165.6 | 166.3 |
| 3: highest single-job peak (of 57) | 339.7 | 186.6 | 198.9 | 202.7 |
| 3: jobs peaking ≥ 286.1 | 22 | 0 | 0 | 0 |
| 4: pinned job + concurrent reads + keyed reads + comparison | 397.3 | 273.6 | 277.7 | **288.7** |
| 5: cancellation of a running largest job | 357.5 | 227.1 | 229.8 | 230.0 |
| 6: after 40 s idle | 200.5 | 123.7 | 124.2 | 126.9 |
| cgroup `memory.peak` (anon + page cache) | 474.0 | 354.0 | 355.3 | 368.5 |

Phase-3 job start RSS: 185.3–260.2 before, 96.0–144.7 after. Phase-2 statuses: before
478×200, 20×400, 7×409, 95×503; after 487/464/457×200, 17/21/17×400, 10/8/9×409,
86/107/117×503 (the rest 429). Every run: server running, `oom_killed=false`, 27 jobs completed.

CPU and wall time (`/proc/<pid>/stat` deltas per phase, no CPU quota):

| figure | before | after (3 repeats) |
|---|---:|---:|
| phase 3: 57 jobs, total CPU s / wall s | 28.23 / 35.5 | 26.99–27.67 / 34.4–35.2 |
| phase 3: pinned candidate standalone (2 seeds), CPU s / wall s | 1.36, 1.35 / 1.5, 1.4 | 1.15–1.42 / 1.3–1.5 |
| phase 4: wall s | 3.8 | 4.3–4.6 |
| phase 4: user / system CPU s | 7.45 / 3.02 | 8.80–9.11 / 4.76–5.27 |

The fixed mmap threshold does not slow a job alone, but under phase 4's concurrent load the
process spends ≈ 1.5 s more user and ≈ 2 s more system CPU (consistent with mmap/munmap of large
read buffers), +0.5–0.8 s wall (+13–21%). On a 0.1-CPU instance that cost is proportionally
larger; it is the price of the ≈ 90 MiB lower job baseline.

Warm reads (`v2-before-ready-latency2`, `v2-final-latency2`; fresh fixture after boot, one
initial and 11 warm requests per probe, p95 = largest of 11; ms):

| warm read, p50 / p95 (ms) | before | after |
|---|---:|---:|
| run list `?limit=20`, anonymous (response cache) | 0.474 / 0.555 | 0.487 / 0.586 |
| run list `?limit=20`, keyed (database) | 1.653 / 2.315 | 1.587 / 2.728 |
| identical timeline window | 0.553 / 0.925 | 0.503 / 0.872 |
| timeline window, changing `max_events` (parsed-run cache) | 1.278 / 1.462 | 1.495 / 1.635 |
| identical dataset replay window | 0.440 / 0.719 | 0.549 / 0.849 |
| replay window, changing `max_trades` (dataset cache) | 1.127 / 1.359 | 1.206 / 1.938 |
| market summary | 0.521 / 0.799 | 0.475 / 0.771 |
| market book | 0.447 / 0.468 | 0.424 / 0.637 |
| run tape `limit=500` | 0.555 / 0.955 | 0.459 / 0.709 |

All probes returned 200 with identical bodies. Differences are within a few hundred µs.
Keyed largest-run timeline windows (pool shrink + compaction, `baseline-pool-before/after`):
5.96–6.50 / 6.55–7.53 ms before versus 5.94–7.92 / 6.93–10.11 ms after (p50 / p95).

### Verdict

**AT-5 / SEC-6 stays Open.** Phases 1–3, 5 and 6 are under 286.1 MiB in every repeat with
≥ 56 MiB margin; phase 4 is 273.6–288.7 MiB, over the bound in one of three repeats by
2.6 MiB, inside the run-to-run variance (15.1 MiB spread). It is not Fixed until every phase is
below 286.1 MiB in every repeat.

### Proposed AT-5 / SEC-6 row text

> **Open (2.6 MiB residual, phase 4 only).** Linux full-process fixture
> `scripts/research/hosted-memory.sh` (hosted image, 512 MB container, no swap, Postgres + fake
> Blob, demo seeded; read set of 1,289 URIs with every read family asserted non-empty, including
> 120 dataset-replay and 96 timeline-window URIs; `docs/rebuild/HOSTED_MEMORY.md`). Earlier
> read-phase figures used an under-covered read set and are superseded. `p9/baseline` adds a
> hosted one-run/128 MiB parsed-run cache, exact-size cached response bodies (32 MiB budget,
> idle expiry), `MALLOC_MMAP_THRESHOLD_=131072`, SQLx buffer shrink on connection release and a
> leaner, idle-expiring market workbench; bytes, ETags, golden hashes and run identities are
> unchanged. Three controlled repeats (phase-4 job pinned to the worst standalone candidate),
> peak MiB min/median/max versus unmodified `36536d0`: 32 concurrent readers
> 156.9/165.6/166.3 (was 224.6), largest single job 186.6/198.9/202.7 (339.7; 0 of 57 jobs
> above 286.1, was 22), cancellation 227.1/229.8/230.0 (357.5), idle 123.7/124.2/126.9 (200.5),
> largest job + concurrent reads + comparison 273.6/277.7/288.7 (397.3). Remaining: phase 4
> exceeded 286.1 MiB in one repeat. The mmap threshold costs ≈ 15–20% more CPU in phase 4.
> Next step: page keyed whole-run reads or run the worker out of process.
