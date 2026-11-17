# Release: Phases 3.1, 3.2, 4 and 5 (2026-11-16)

`main` = `55084c0` (merge of PR #2, branch `phase31/release`). Commits: `fd4a290` backend (L3 workbench, stress
regimes, study CLI, `execution_validation` demo), `177fa2c` web, `efceb96` docs, `cdea31c` config-value formatter,
`799c156` CI curated counts.

PR #1 (`phase31/ui`) was closed unmerged: one of its commits added a docs line that the per-commit secret scan
(SEC-19) flags (`BLACKBOX_TEST_DATABASE_URL=…`, no real value), and a later commit cannot remove it from the scanned
range without a force-push. The same content was rebuilt on a new branch from `7c49dae` with that line corrected.

## CI

| Run | Result |
|---|---|
| PR #2, 43958470088 (`799c156`) | 8/8 green: rust, golden-windows, web, e2e, contract, audit, policy, docker |
| `main`, 43960047213 (`55084c0`) | 7/8 first attempt: `web` failed at `npm audit signatures` (`EMISSINGSIGNATUREKEY` for postcss@8.5.32 attestations, the registry-side error recorded in PRODUCTION_ACCEPTANCE for `d85f743`); re-run of the failed job passed |
| publish 43961883356 | image index `sha256:3cc6b03bc84368d74aed953418a2741f06769476771f97dc166aff0bc45bf6f8`; deploy step skipped (no hook), manual digest release followed |

## Production

| Check | Result |
|---|---|
| Render | `render deploys create srv-dbs11fm1egvs73chs7lg --image ghcr.io/tanmay-alpha/blackbox-hft@sha256:3cc6b03b…f6f8`; deploy `dep-dbtnhvsipgvc739jbs40` live; previous `dep-dbsr463l550s7398h2i0` deactivated |
| `/api/v1/status` | `git_sha=55084c0668215e6e515b20b33a007c6d150ee113`, worker idle, `result_platform=linux-x86_64+libm-53154a4e` |
| Smoke (`scripts/deploy/smoke.sh`) | 8/8 PASS |
| Vercel | production deployment built from `main` (created 21:26 UTC) Ready; bundle contains the Market Data Workbench; `/market/l3-demo` 200 |
| Market endpoints (anonymous) | `/market/datasets`, `/l3-demo`, `/events?limit=5`, `/book?event_index=15000`, `/metrics`, `/comparison` all 200 (0.20–0.73 s) |
| Demo seed | pass 1 registered `synthetic-stress-depletion` and `synthetic-stress-wide-spreads`, queued 6 runs; the hosted worker completed them; pass 2 created 6 comparisons (4 aligned, 2 metrics-only across datasets) and the "Execution model validation" report. The 8 existing demo runs kept their ids (`253bbc23b342f373`, `d6a57aed972e18d0`, `bc2d1df47e2b91fe`, `130d67fc371bd27f`, `6bcdf19136fd3059`, `586374188aae19b2`) |
| `/api/v1/demos` | lists `cost_of_latency`, `queue_position`, `gross_vs_net`, `execution_validation` |
| Anonymous acceptance (`npm run test:prod`) | 7 passed, 1 skipped (the keyed journey needs `PROD_RESEARCH_KEY`, not set) |
| New pages in a browser (1440×900) | `/market/l3-demo?tab=book&event=15000`, `/forensics/253bbc23b342f373/orders/256` (evidence ledger), `/book/runs/253bbc23b342f373`: one h1, no error box, no console error. `/compare/set?demo=execution_validation`: the first load after the deploy got one `503 busy` (heavy-read permit wait of 2 s exceeded while 7 run bundles were fetched cold from Blob; the page showed the error with Retry); two reloads 8 s apart loaded all 6 panels with no error. Screenshots `img/p31-prod-*-1440.webp` |

Notes:
- Anonymous list endpoints come from the in-memory demo snapshot (`v1/snapshot.rs`, `MAX_AGE` 1 h), so the new
  curated comparisons appear in `/comparisons` within an hour of the seed; detail reads by id were 200 immediately.
- The idle in-process worker sleeps until its next scheduled wake when jobs are inserted by another process; the 6
  seeded jobs were claimed a few minutes after insertion, as documented in `worker.rs`.

## Incident during the release (no production data changed)

After the seed, a local development server was restarted from the same shell, which still held the production
`BLOB_READ_WRITE_TOKEN` from the seed step. It started with `storage=vercel_blob` against the local database and made
18 read requests (16,077,417 bytes) to the production Blob store before it was stopped; it wrote nothing (no jobs
ran). The shell variables were unset and every later local server is started with `env -u BLOB_READ_WRITE_TOKEN …`.
Lesson: run production operator steps in a subshell so credentials never persist.

## Rollback

```
render deploys create srv-dbs11fm1egvs73chs7lg --image ghcr.io/tanmay-alpha/blackbox-hft@sha256:c41fd646e2ba36d5ed2450b3bd31e23d53b1f1a36c1898781928c5c83d864c6b --wait --confirm
vercel rollback <previous production deployment> --scope tanmay-alphas-projects
```

No migration. The added demo rows (2 datasets, 6 runs, 6 comparisons, 1 report) are additive and are ignored by the
previous release's code paths except that its `/demos` does not list `execution_validation`.
