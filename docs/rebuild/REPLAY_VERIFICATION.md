# Replay verification

Date: 2026-11-15. Scope: the cursor-paged timeline, parsed-run cache and terminal paging/stepping helpers. All traffic uses throwaway local Postgres databases, filesystem artifacts and loopback HTTP; no production requests or settings changes.

## Findings and fix

A huge first-page window previously cloned all matching fills, equity samples and orders, then converted all of them to wire JSON **before** checking its response cap. The final response was bounded, but the temporary context allocation was not bounded by that cap.

The timeline now consumes shared borrowed context iterators and stops encoding at the first item that would exceed the available response budget. The legacy timeline uses the same selection rules; its behavior is unchanged. Orders are still returned in ascending order id. The request still returns 400 `invalid_request`, with `details.reason = "window_too_large"`, when its context cannot fit: a shorter window is required, not silent context truncation.

The deterministic serialization regression offers one million context items to a 32-byte context budget. Only four items are visited (three retained, one rejected), and retained encoded bytes are 30. This directly exercises early termination rather than merely checking the final response size. An additional huge-window test uses `t0_ns=0,t1_ns=i64::MAX`, first paging all 12,000 tape events under a 1 MiB cap, then refusing 100,000 context samples under that same cap.

## Coverage

- `crates/bb-app/tests/integration.rs`: the existing 12,000-event adversarial fixture has 9,600 in-window events, alternating clocks, out-of-window events interleaved by sequence, and equal timestamps split across page boundaries. Default 5,000-event paging, 102-event paging, minimum page size, first/last sequence, byte-limited progress, context-only first pages, empty windows and count-only requests are exercised. Added byte-idempotent page comparison, malformed cursor cases and huge-window coverage.
- Existing cursor tests reject tampered positions, another run, another window/platform and count-only cursor requests. Added malformed inputs include empty tokens, invalid base64, invalid tags, extra separators and tokens longer than 1,024 bytes.
- `crates/bb-app/tests/replay_verification.rs`: real separate `blackbox serve` processes page the same stored run; killing and restarting the process with the same pepper must reproduce the exact cursor response bytes. Restarting with a different pepper must reject the cursor. This avoids the false confidence of restarting an in-process router while keeping the process-global signing key.
- The process test compares anonymous 1 MiB and keyed 4 MiB cursor-page caps, and tests both caller classes with exhausted daily egress budgets. Bodies above the tiny-response exception return 503 `unavailable` with `details.reason = "busy"` and `details.budget = "egress"`; small error responses remain available.
- `crates/bb-app/src/artifacts.rs`: deterministic cache tests cover more than two runs, LRU refresh, exceeding the 64 MiB decoded-byte budget, rejecting a single over-budget run, byte accounting and releasing the cache's `Arc` on eviction. The budget is explicitly a decoded-section-byte estimate, not a promise that process working set is at most 64 MiB; active requests can retain evicted runs.
- `web/src/features/terminal/timelinePaging.test.ts`: complete page merge, equal-time boundaries, overlap/wrong-window rejection, progress, cancellation, and forward/backward stepping to the first/last timestamp and stopping at either end. The extracted terminal helper preserves timestamp stepping semantics: equal-time events remain visible together; stepping moves to a strictly earlier/later timestamp.

## Reproduction

Use a local Postgres URL in `BLACKBOX_TEST_DATABASE_URL`; tests create and drop their own databases. Keep the build target separate from any running development server (Windows will not replace a running executable).

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cd web
npm test -- src/features/terminal/timelinePaging.test.ts
npx tsc --noEmit -p tsconfig.app.json
```

For the opt-in Windows measurement, build `blackbox` with the normal release profile (`opt-level=3`, thin LTO, four codegen units), set `BLACKBOX_REPLAY_BINARY` to that release executable and run:

```sh
cargo build --release --locked --bin blackbox
cargo test -p bb-app --locked --test replay_verification replay_release_measurement -- --ignored --nocapture
```

The Rust test runner prepares a synthetic 120-second run at 10 ms strategy intervals (12,000 ticks); **the measured HTTP server is the separate release executable**, not the debug test runner. It pages the entire run in twelve non-overlapping inclusive windows, default 5,000 events/page, with the normal keyed 4 MiB cap. It checks each window's event count, then sorts collected sequences and checks the exact complete tape with no duplicates or omissions. The final window uses `i64::MAX` as its upper bound. Latency includes the request, body transfer and JSON parse. The first page includes cold bundle decode; later pages use the parsed-run cache. Reported percentiles use nearest rank, not interpolated quantiles.

A separate PowerShell process samples `Get-Process -Id <server pid>` / `WorkingSet64` with a 10 ms sleep. Its readiness is acknowledged before paging starts. Peak working set is a sampled Windows RSS proxy, not allocation accounting; short-lived peaks between samples can be missed. Test-runner, database and sampler memory are excluded.

The fixture is produced by the test runner. The read-only child server explicitly selects that fixture's recorded `BB_RESULT_PLATFORM`. A standalone debug executable on this workstation reported `windows-x86_64-gnu+libm-0352e099`, while the test runner recorded `windows-x86_64-gnu+libm-e9b26b47`; leaving the default selected a different result namespace and correctly returned 404. These HTTP measurements do not assert simulation hash equality between those math-library classes.

## Local release measurement

Run `4132ff524027a3f4`: 12,000 ticks at 10 ms, 60,382 tape events. One complete pass of twelve windows returned every sequence exactly once.

| Measurement | Result |
|---|---:|
| HTTP pages | 19 |
| Response bodies, total | 25,294,700 B |
| Smallest / largest page | 6,214 / 2,131,476 B |
| Caller cap | 4,194,304 B |
| Page latency p50 / p95 | 55.124 / 421.158 ms |
| Sampled server peak working set | 90,365,952 B (86.18 MiB) |
| Working-set samples | 63 |

Page response-body bytes in request order (including first-page context):

```text
1: 2078677    2: 46365      3: 2108624    4: 56285
5: 2131476    6: 82303      7: 2097149    8: 2126209
9: 24143     10: 2100220   11: 35877     12: 2032878
13: 2060681  14: 2054959   15: 1983773   16: 2106434
17: 50267    18: 2112166   19: 6214
```

This is one local release-server traversal, not a production latency SLO or sustained-throughput benchmark. With 19 pages, nearest-rank p95 is the maximum page latency and includes the cold first decode. Byte counts are uncompressed JSON response bodies, not HTTP headers or TLS framing. The 86.18 MiB measured server peak is compatible with a 64 MiB *decoded-byte cache estimate*: it also includes the runtime, bundle parsing, response values and serialization buffers.

## Verification results

| Command | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | PASS |
| `cargo test --workspace --locked` with local Postgres configured | 465 passed; 0 failed; 3 ignored |
| `npm test -- src/features/terminal/timelinePaging.test.ts` | 10 passed |
| `npx tsc --noEmit -p tsconfig.app.json` | PASS |
| `cargo build --release --locked --bin blackbox` | PASS |
| Explicit ignored `replay_release_measurement` with the release child | PASS; measurements above |

The release measurement is deliberately ignored in the normal suite because it requires an explicit release-binary path and Windows PowerShell; it was run separately here. The normal suite's process-restart/caps/budget test was not skipped. No Render/Vercel configuration, live API or vendor market-data files were touched.
