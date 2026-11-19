# Release: Phases 6 and 7 (2026-11-19)

`main` = `a1d128caceacf6c362a25a69593b9b8a09fa371d` (merge of PR #3, branch `phase67/release`: commits `af3d8d3`
backend, `ad2638f` web, `502b975` docs, `9a18bd8` advisory acceptances). The work was integrated on `p6/int`; the
release branch carries the same tree in reviewable commits because two intermediate integration commits contained
lines the per-commit secret scan flags (placeholders, not secrets). No force-push.

Tracker: [../engineering/PHASE6_TRACKER.md](../engineering/PHASE6_TRACKER.md).

## CI and image

| Step | Result |
|---|---|
| PR #3 CI 44287056318 (`502b975`) | 6/8: `audit` and `policy` failed on RustSec advisories issued 2026-11-17 (zip RUSTSEC-2026-0545/0547/0548, chrono RUSTSEC-2026-0543); accepted with reachability evidence in `9a18bd8` ([../SECURITY.md](../SECURITY.md)) |
| PR #3 CI 44289045929 (`9a18bd8`) | 8/8 green |
| `main` CI 44290475971 (`a1d128c`) | green |
| publish 44291893639 | image `ghcr.io/tanmay-alpha/blackbox-hft@sha256:ef148a3b5f4b6a17e54220d041cd323be92d62a701a9eb3b0e3b8f2f8e9cefa0` |

## Production

| Check | Result |
|---|---|
| Render | `render deploys create srv-dbs11fm1egvs73chs7lg --image …@sha256:ef148a3b…cefa0`; deploy `dep-dbuoh75ckfvc73cqrra0` succeeded |
| Migrations | 0010 (`storage_reads_daily.anonymous_reads`) and 0011 (`glft` in the strategy-kind CHECK) applied on start; both additive and compatible with the previous code |
| `/api/v1/status` | `git_sha=a1d128c…`, worker `in_process`, 1 slot, idle |
| Smoke (`scripts/deploy/smoke.sh`) | 8/8 PASS |
| Vercel | production built from `main` (status `success`); `/`, `/research`, `/studies`, `/queue-lab`, `/market/l3-demo` 200; published `queue-shadow` `.sha256` served |
| Production browser suite (`npm run test:prod`, anonymous) | 14 passed, 1 failed, 1 skipped. The failure was the spec, not the product: `studies.prod.spec.ts` expected "mean [lower, upper]" with a space, and the cell explorer renders the mean and interval as separate elements (values identical). Spec fixed to match whitespace-insensitively; rerun on production: 1 passed. Skipped: the keyed journey (`PROD_RESEARCH_KEY` not set) |
| `/research`, `/queue-lab` in a browser | research home rendered all five investigations with live values (e.g. fees row "net −19.5087", workbench row 2,900 → 2,800 shares); queue lab showed "SHA-256 verified" and the nominal-interval caveats |

About 20 production page loads in total, paced; no bot checkpoint was hit.

## Not in the first release (shipped in the follow-ups below)

- AT-5/SEC-6: a largest admitted job peaked at 452–460 MiB in the 512 MB Linux fixture.
- Post-simulation cancellation waited 60 s for the reaper.

## Follow-up 1: `5fe8923` (PR #4)

Commits `68798af` (worker), `6bd0e6a` (production spec), `91db0de` (docs). PR CI 44345185372 8/8; `main` CI
44347566565; image `sha256:0f5fdba142592d573fe4fc515d828f5078b31a7da07b963e62bc5f339d4e6ae3`; Render deploy
`dep-dbutcf5ckfvc73a778m0`; `/status` `git_sha=5fe8923…`; smoke 8/8; production suite 14 passed, 1 flaky, 1 skipped.

- Cancellation after the simulation is recorded as `cancelled` in 22–28 ms ([CANCELLATION.md](CANCELLATION.md)).
- Job memory: one order's analytics state at a time, the analytics report held once as stored text, the result hash
  streamed; analytics, bundle and result bytes unchanged. 512 MB fixture: largest job 340 MiB, job with concurrent
  reads 435 MiB (was OOM-killed).
- The flaky test was a real defect: the study page read every candidate run by id in parallel, and one read was
  refused `503 busy` on the cold server right after the deploy. Fixed in follow-up 2.

## Follow-up 2: `36536d0` (PR #5)

Commits `c450764` (API), `06e98b3` (study page), `ad1a08b` (docs). PR and `main` CI green; publish 44357312552, image
`sha256:d5d2562bef5620364c95195d92cb1b4effec48a8640ecbd919cbc091e2d25595`; Render deploy `dep-dbutv4u1egvs739lbbf0`;
`/status` `git_sha=36536d0…`, worker idle; smoke 8/8; Vercel built from `main`; production suite **15 passed,
1 skipped** (keyed journey, no `PROD_RESEARCH_KEY`), no retries.

- Read path: `/analytics` projects the stored report from a borrowed `RawValue`; run Markdown uses trailer metadata;
  response bytes unchanged. 512 MB fixture: 32 concurrent readers 207 MiB, job with concurrent reads 394 MiB.
- SEC-10: warm anonymous detail, paged and engine-check reads of demo runs use snapshot artefact trust rows (48 reads,
  0 DB acquisitions); anonymous analytics keeps one report query.
- Study page links stored runs from the verified run list; unlisted runs are read one at a time with the busy retry.

Still open after follow-up 2: AT-5/SEC-6 (largest-job phases 362–394 MiB, above 300 MB; under-covered read set), SEC-10
analytics report query.

## Follow-up 3: `18aed41` (PR #7)

Commits `4adf4df` (hosted), `860f0c6` (docs). PR CI 44382600504 and `main` CI 44385000957 green; publish 44387431947,
image `sha256:bac3807b893d5fb99d9882a039c16ac35dc7aeaa656a7a99d52d093da2e84511`; Render deploy
`dep-dbv0d1d6e07s73ab741g`; `/status` `git_sha=18aed41…`, worker idle; smoke 8/8; Vercel built from `main`;
production suite 15 passed, 1 skipped (keyed), no retries.

- SEC-10 closed for demo bundles that contain analytics: anonymous `/analytics` and `/analytics/series` project the
  authenticated bundle-trailer report text (24 reads, 0 DB acquisitions; bytes identical to keyed responses).
- Hosted memory: one-run/128 MiB parsed-run cache, exact-size cached response bodies (32 MiB, idle expiry),
  `MALLOC_MMAP_THRESHOLD_=131072`, SQLx buffer shrink on connection release; memory fixture read set corrected (1,289
  URIs). 512 MB, three pinned repeats: job + concurrent reads + comparison 273.6–288.7 MiB (was 397.3), largest single
  job 186.6–202.7 (339.7), idle 123.7–126.9 (200.5). AT-5/SEC-6 remains Open: 2.6 MiB over in one repeat.
- The mmap threshold costs about 15–20% more CPU under phase-4 concurrency; it is an image `ENV` that can be overridden
  in the Render service settings without a rebuild.

## Follow-up 4: `3ed53cd` (PR #9)

Commits `8c35d57` (hosted memory), `3b61c35` (docs). PR CI 44410135636 and `main` CI green; image
`sha256:9300ae0696002e0b48b355d871c338675d4c5870e0885a16bda938aeb0b3e4a1`; Render deploy `dep-dbv2jk5g1s2s73e0c8m0`;
`/status` `git_sha=3ed53cd…`, worker idle; smoke 8/8; Vercel built from `main`; production suite 15 passed, 1 skipped
(keyed), no retries.

- A cold keyed timeline first page decides `window_too_large` from fills and equity before decoding orders and tape;
  comparisons read the cached run or only orders and tape. Responses, stored comparison summaries and `summary_sha256`
  unchanged (tests).
- AT-5/SEC-6 fixed in the 512 MB Linux fixture: every phase below 286.1 MiB in three pinned repeats; phase 4 (job +
  concurrent and keyed reads + comparison) 240.0/247.0/252.0 MiB (was 264.0/279.6/300.1), margin 34.1 MiB over a 12.0
  MiB repeat spread. Measured locally without a CPU quota, not on Render.
- Keyed production journey (2026-11-19 05:04–05:05 UTC, backend `3ed53cd`; spec `research.prod.spec.ts` with the
  Phase 11 preset-selector fix, spec only): job `01a1f231-a309-7585-8556-e45a2d92b240` completed on attempt 1 on
  `linux-x86_64+libm-53154a4e`, run `c6d47c551e17c5af` (result sha256 `bb34d8c5…`, the same as the 2026-11-15 keyed
  runs; 2,051 orders, 1,182 fills), comparison 201, report 200, anonymous read 404, re-authentication OK, 0 page
  errors ([prod-research-evidence.json](prod-research-evidence.json), [PRODUCTION_ACCEPTANCE.md](PRODUCTION_ACCEPTANCE.md)).

## Follow-up 5: `a24ccf9` (PRs #13, #14): Phase 11 final audit

Audit fixes ([PHASE11_AUDIT.md](PHASE11_AUDIT.md)): anonymous cache 304s and due snapshot recheck, query error
envelope, cancellation and lease races, report provenance and backtick fences, artefact admission and decode
single-flight, accessibility and copy fixes, mobile CLS, `render.yaml` pepper. PR and `main` CI green (all 8 jobs);
image `sha256:631a8111d8e54f11275eb7a64a64f76d7ea855f8eb6928bce6574bb5a2d8c52b`; Render deploy
`dep-dbvceeu7bikc7387i490`; `/status` `git_sha=a24ccf9…`; smoke all checks passed.

- Production suite (2026-11-19 09:55–10:00 UTC): 16 tests incl. keyed. 15 passed on the first run; `market.prod.spec.ts`
  failed because its ladder assertion predated the cells' screen-reader text ("7,000 shares in 11 orders"); spec
  updated, rerun 3/3 passed.
- Keyed journey: job `01a1f33e-ce13-7966-95b0-7b1839413274` completed on attempt 1, run `299f836aa96bd57c`, result
  sha256 `bb34d8c5…` (unchanged), 2,051 orders, 1,182 fills; comparison 201 (aligned, like-for-like); report 200
  (25,696 B; text changed by the provenance fix); anonymous read 404; re-authentication OK; 0 page errors.

## Rollback

Roll back to the previous follow-up (`3ed53cd`), or to an earlier image of this record:

```
render deploys create srv-dbs11fm1egvs73chs7lg --image ghcr.io/tanmay-alpha/blackbox-hft@sha256:9300ae0696002e0b48b355d871c338675d4c5870e0885a16bda938aeb0b3e4a1 --wait --confirm
vercel rollback <previous production deployment> --scope tanmay-alphas-projects
```

No follow-up adds a migration; every image in this record (`a1d128c`, `5fe8923`, `36536d0`, `18aed41`, `3ed53cd`, `a24ccf9`)
runs against the current schema. The release before Phase 6 (`cdde05e`, `sha256:2c14ea32…4ec4`) also runs against
the migrated schema: 0010 adds a defaulted column it never reads, 0011 only widens a CHECK.
