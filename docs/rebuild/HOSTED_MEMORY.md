# Hosted full-process memory (audit AT-5 / SEC-6)

Date: 2026-11-18. Branches `p7/mem` (causes 1–2, rebased on `p6/int` `1c6acab`) and `p7/jobmem`
(cause 3, on `p6/int` `42d9237`). Requirement: the hosted `blackbox serve` process (API +
in-process worker + analytics + parsed-run cache + concurrent HTTP heavy reads) peaks below
**300 MB** on Linux. Result: **not met**. The read path was fixed (peak under 32 concurrent
anonymous heavy readers 697 → 246 MiB) and resident memory no longer ratchets from job to job
(all 57 largest admitted jobs now run in a 512 MB container; before, it was OOM-killed at job
55). What remains is the working set of a job itself: a largest admitted job adds up to 204 MiB
(512 MB run) / 246 MiB (2 GB run) to a start of 186–292 MiB, so single jobs peak at up to
460 MiB, and the largest job plus concurrent reads is still OOM-killed at 512 MB. Disposition at
the end.

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

Phases: (1) cache fill — one client touches 154 heavy anonymous reads once (lazy market workbench,
dataset regeneration, parsed-run cache, demo snapshot, response cache); (2) 32 clients issue
random distinct anonymous heavy reads (run detail/timeline/tape/fills/orders/equity/report,
comparison detail/pairs/report, dataset replay windows, market workbench event detail and book),
each from a new forwarded address, until 600 requests were answered by a handler; (3) every
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

Reads-only runs, 2 GB limit, 600 handler-answered requests from 32 clients (phase-2 peak `VmHWM`):

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

## Residual: the working set of one largest job

Every phase that runs a largest admitted job exceeds 300 MB: phase 3 by up to 174 MiB (460.0),
phase 4 by 200 MiB (485.6), phase 5 by 166 MiB (451.6). Phases 1, 2 and 6 stay under it (143.7,
266.5, 205.4 at most). heaptrack of the one-job run (heap peak 439.9 MB, reached in phase 4)
names the job's share:

| allocation site | MB at peak |
|---|---:|
| run analytics, per-order state: `BTreeMap<order_id, OrderState>` (`bb-analytics` `execution.rs:1084`) | 50.7 |
| run analytics, three `Spreads` per order (`split_spreads`, `execution.rs:782`; 70,251 orders) | 73.7 |
| simulation output: equity sampled every tick (`bb-sim` `lib.rs:741`) | 25.2 |
| simulation output: tape, 150,000 events (`bb-sim` `tape.rs:547`) | 24.1 |
| concurrent keyed whole-run reads (`OpenRun::full` decode, `bb-app` `artifacts.rs:779–782`) | 56.3 |

The analytics pass holds about 1.8 KB per order on top of the whole `RunResult`; it alone is
larger than the room left under 300 MB. Not changed here (outside the worker): allocating the
per-order spread accumulators lazily or streaming them, or running the worker as a separate
process (the paid `worker` in `render.yaml`). Admission limits were not raised or lowered.

## Finding outside memory: GLFT presets cannot be submitted (HTTP 500)

`POST /api/v1/experiments` with the `glft_baseline` preset answers 500: the server log shows
`new row for relation "experiment_configs" violates check constraint
"experiment_configs_strategy_kind_check"`. The GLFT strategy kind is missing from the database
constraint; the fixture records these as `submit_http_500`. A repair needs a migration (being
fixed on a separate branch).

## Observation: phase 5 cancels a job that has already finished simulating

The largest job simulates in 1–2.5 s; the cancel request is sent 0.5 s after the job is
`running` and reaches the worker only through the next heartbeat. In every run here the attempt
ended without a worker log line and the job became `cancelled` 60 s later through the reaper
(`reaper moved job … state=cancelled reason=lease_expired`), with nothing stored (`run_id` null).
[INFERENCE] The completion transaction found the job `cancelling`, lost the lease and the worker
abandoned the attempt silently. Memory-wise phase 5 therefore measures one whole largest job.

## Not done / open

- **Bound not met** for any phase that runs a largest admitted job (residual above).
- Phase 4 is OOM-killed at 512 MB; phases 4–6 were measured at 2 GB.
- No CI job: one run needs a release image build (~10–20 min uncached) plus seeding and 57 jobs,
  and the figures depend on thread count and allocator state; it does not meet the "< 10 min,
  deterministic" bar.

## AT-5 / SEC-6 disposition text

> **Open.** Linux full-process fixture `scripts/research/hosted-memory.sh` (hosted image, 512 MB
> container, Postgres + fake Blob, demo seeded; `docs/rebuild/HOSTED_MEMORY.md`). Fixed: metadata
> reads no longer build the bundle analytics report (`BundleTrailerCore`), the image caps glibc
> arenas (`MALLOC_ARENA_MAX=2`), and the worker returns freed heap after every job
> (`malloc_trim(0)`, Linux glibc). 32 concurrent anonymous heavy readers peak at 245.7 MiB (was
> 697.2); resident memory no longer ratchets between jobs (all 57 largest admitted jobs run in
> 512 MB, was OOM-killed at job 55; 205.4 MiB after 40 s idle). Remaining: one largest admitted
> job adds up to 246 MiB (run analytics ≈ 124 MB of per-order state plus the `RunResult`), so job
> phases peak at 452–460 MiB alone and 485.6 MiB with concurrent reads, which is OOM-killed at
> 512 MB. The 300 MB bound is not established; admission and runtime budgets stopped 24
> over-budget jobs with nothing stored.
