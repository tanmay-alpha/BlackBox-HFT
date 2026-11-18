# Cancelling a running job

Observed in the hosted memory fixture (`HOSTED_MEMORY.md`, phase 5, `jobmem-2g`): `POST
/jobs/{id}/cancel` on a running job returned 202, but the job only reached `cancelled` 61.1 s
later, through the lease reaper. This note records the cause, the fix and the bounds now tested.

## Evidence

The worker log of that phase has exactly two lines for the job:

```
10:24:36.032 INFO job claimed        job_id=01a1ee2f-daae-…
10:25:37.041 WARN reaper moved job   job_id=01a1ee2f-daae-… state=cancelled reason=lease_expired
```

No `job completed`, no `job released`, no `lease lost` line: the worker ended the attempt without
logging anything. The only silent exit of a job is `Outcome::Abandoned`, and the only way to it
here is `complete()` returning `LeaseLost`: the completion transaction only accepts a `running`
job, and the cancel had moved it to `cancelling`. The worker then stopped heart-beating, and the
reaper found the 60 s lease expired a minute later.

## Root cause

Two ends of an attempt:

- **During the simulation** cancellation already worked. The API flips the in-process
  `JobControl` flag immediately (`WorkerHub::cancel`; a standalone worker learns it from the next
  heartbeat), and the simulation polls it at every checkpoint, every `PROGRESS_EVERY_TICKS` =
  1,000 strategy ticks (`bb-sim` `checkpoint`). The run stops with `Cancelled` and `release`
  records `cancelled`.
- **After the simulation's last checkpoint** nobody looked at the flag again. The worker went on
  to assemble the result, compute analytics, write, upload and read back the bundle, and then
  tried to complete. The completion was refused (`cancelling` ≠ `running`) as `LeaseLost`, which
  the worker treated as a lost lease: `Abandoned`, silently, leaving the job to the reaper after
  `lease_seconds` (60 s in production). Phase 5's job simulates in about a second, so a cancel
  sent ~0.5 s after `running` was observed regularly landed in this window.

## Fix (`crates/bb-app/src/worker.rs`)

A new outcome, `Cancelled`, recorded with `acknowledge_cancel` (`cancelling` → `cancelled`, attempt
closed `cancelled`) and logged `reason=cancelled` ("job cancelled; nothing stored"). It is chosen:

1. when the simulation stops on a cancel this process saw (before: `Released`, which also ends in
   `cancelled`; only the log reason changes from `released` to `cancelled`);
2. right after the simulation returns, if a cancel arrived after its last checkpoint: no
   analytics, bundle or upload;
3. after analytics and the bundle, before the upload intent: no upload;
4. when the completion is refused as `LeaseLost`: the job is `cancelling` (also covers a cancel
   no heartbeat has delivered yet). If the lease really is lost, `acknowledge_cancel` fails too
   and the attempt is logged `lease_lost` instead of being dropped silently.

A cancel that lands after the upload leaves the object behind with its upload intent, which the
blob garbage collector removes; no run or artefact row is committed. The simulation and the
completed path are untouched, so results and hashes of uncancelled runs are unchanged.

## Bounds

- The simulation stops at the first checkpoint after the flag is set: at most 1,000 strategy
  ticks later (`bb-sim` test `cancellation_stops_the_run_within_one_check_interval`).
- The job is `cancelled` as soon as the worker's current step ends — the simulation's next
  checkpoint, analytics/bundle, or the upload in progress — never after a lease expiry.

## Timings (Postgres, in-process worker, 60 s lease, debug test build)

| Cancel lands | Before | After |
| --- | --- | --- |
| mid-simulation (1.8 M-tick run, cancel sent at tick 5,000–15,000) | 21 ms to `cancelled` (log reason `released`) | 22–41 ms over 5 runs; stopped at tick 15,000–30,000 of 1,800,001 |
| after the simulation, before completion | 59.9 s (reaper; `lease_losses` 1, attempt `lease_expired`) | 22–28 ms over 5 runs |

## Tests

`crates/bb-app/tests/cancellation.rs`, real simulation and Postgres, lease 60 s:

- `cancelling_a_running_simulation_stops_it_and_records_cancelled_promptly`: cancels after the
  first checkpoint; asserts the run stopped before its last tick, `cancelled` within 10 s.
- `cancelling_after_the_simulation_finished_records_cancelled_promptly`: a test `FaultInjector`
  holds the worker at the completion step (simulation, bundle and upload done), the cancel is
  sent, the worker resumes; asserts `cancelled` within 10 s.

Both assert `lease_losses = 0`, the attempt closed `cancelled` without an error code, no run or
artefact row, and a `reason=cancelled` worker log line. `worker_faults.rs` now expects the
`cancelled` reason for its held-simulation cancel.
