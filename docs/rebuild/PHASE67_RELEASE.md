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

## Not in this release

- AT-5/SEC-6 remains **Open**: a largest admitted job peaks at 452–460 MiB in the 512 MB Linux fixture. A follow-up
  (`p7/amem`: leaner per-order analytics, single serialisation of the analytics report) is measured separately.
- Post-simulation cancellation waited 60 s for the reaper; fixed on `p7/cancel` (`af7543e`, 22–28 ms), not yet
  released ([CANCELLATION.md](CANCELLATION.md) on that branch).

## Rollback

```
render deploys create srv-dbs11fm1egvs73chs7lg --image ghcr.io/tanmay-alpha/blackbox-hft@sha256:2c14ea32555019eeeea234e090a1b7318fd9d33c12d44142e61f13ca7d1b4ec4 --wait --confirm
vercel rollback <previous production deployment> --scope tanmay-alphas-projects
```

The previous release (`cdde05e`) runs against the migrated schema: 0010 adds a defaulted column it never reads, 0011
only widens a CHECK.
