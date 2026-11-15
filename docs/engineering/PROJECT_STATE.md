# Project state

Single source of truth for the current phase, decisions, infrastructure and next actions.

## Current phase

**Rebuild released (2026-11-15).** `main` = `d85f743` is live: the API reports `git_sha=d85f743`
from image `ghcr.io/tanmay-alpha/blackbox-hft@sha256:1d35116c…`, Vercel serves the console built
from `main`, smoke passed 8/8 and `npm run test:prod` passed 7/7. Evidence and rollback:
[../rebuild/PRODUCTION_ACCEPTANCE.md](../rebuild/PRODUCTION_ACCEPTANCE.md). Review:
[../rebuild/FINAL_REVIEW.md](../rebuild/FINAL_REVIEW.md). Later UI polish commits on
`rebuild/explore` are released only when `/status` reports their SHA.

Hosted system (the `BB_GIT_SHA` service env override was removed on 2026-11-15; the SHA now
comes from the image):

| Component | Where | Notes |
|---|---|---|
| Console (SPA) | https://blackbox-hft.vercel.app | Vercel project `blackbox-hft`, built from Git (root `web`, Vite), env `VITE_API_BASE_URL=https://blackbox-hft-api.onrender.com/api/v1`; no backend secrets in the project (SEC-20: removal recorded and env-name listing independently verified 2026-11-14, FINAL_AUDIT §3) |
| API + worker | https://blackbox-hft-api.onrender.com | Render free web service `srv-dbs11fm1egvs73chs7lg` (singapore), image `ghcr.io/tanmay-alpha/blackbox-hft:main` (public, built by `publish.yml` after green CI), `BB_PROFILE=hosted`, `BB_WORKER_SLOTS=1`, migrations on start |
| Database | Neon free `blackbox-hft-db` (sin1, PostgreSQL 18.7) | audit observed 7 migrations applied; source now embeds migrations through 0009; `sslmode=verify-full` |
| Artefacts | Vercel Blob private store `blackbox-hft-artifacts` (`store_eod21bxLMlokHq3Z`, sin1) | one verified bundle per run |
| Demo data | synthetic dataset `synthetic-demo` v1 (events sha256 `0bc26426…`), demo experiments computed by the hosted Linux worker (canonical platform `linux-x86_64+libm-53154a4e`) | seeded with `blackbox demo seed --platform linux-x86_64+libm-53154a4e` |

Historical production verification (lead, 2026-11-14; not evidence of deployment of later source fixes): `scripts/deploy/smoke.sh` 8/8 PASS (liveness, status
git SHA, readiness, CORS allowed/refused, foreign Origin 403, anonymous mutation 401, malformed key
401); a research-key submission went queued → running → completed in ~20 s and its run is served
(1,241 fills, 2,060 orders, canonical); the console loads every checked workspace with 0 console
errors and shows API OK, worker heartbeat and the six demo runs.

The independent audit observed production `git_sha = 1873f4d4392c3b222d2c28e9e759c3d9b11e510d`
(FINAL_AUDIT R3). It did not establish the deployed image digest or a release of the newer
source: the cited publish run skipped deploy/smoke because no deploy hook was configured.
Use the manual digest-release procedure in DEPLOYMENT §2 and record new release evidence here.

Secrets (never committed): `data/secrets/{neon.env,vercel-prod.env,render.env,api-keys.txt}`.

## Decisions (user-approved)

| Topic | Decision |
|---|---|
| Compute | One free Render web service running `/api/v1` and an in-process worker (`render.yaml` declares the paid private worker as an optional upgrade) |
| PostgreSQL | Neon free via Vercel Marketplace |
| Object storage | Vercel Blob private store over its HTTP API (`BLOB_READ_WRITE_TOKEN`) |
| Access | Public read-only demo workspace + hashed, scoped, peppered API keys |
| Hosted data | Synthetic only (Tardis terms §9.2 forbid hosting; no openly licensed L2 source found, see MARKET_MICROSTRUCTURE §7) |
| Queue | Postgres `FOR UPDATE SKIP LOCKED` with leases; no Redis |
| Platform policy | Results keyed by `<os>-<arch>[-env]+libm-<fingerprint>`; canonical = glibc `53154a4e`; GitHub's Windows runner (`9a463940`) reproduces every canonical pin |
| Anonymous egress budget (rebuild) | Keep 60 MB/day of uncompressed response bodies; shrink the page instead (public page measured at 236,675 B API data per fresh visit, local) — user, 2026-11-14 (rebuild T15) |
| Public page (rebuild) | The latency experiment is the public page at `/`; the experiment catalogue moves to `/experiments` (b819427) |
| QuantReplay (rebuild) | No-go for product and hosting; reference only after a local Docker PoC (QUANTREPLAY_EVALUATION §6, ef168cf) |
| Analytics grid (rebuild) | Perspective and Glide Data Grid not adopted; TanStack Table + Virtual kept (PERFORMANCE_RESULTS, a2f754e) |
| ITCH data (rebuild) | ITCH 5.0 reconstruction is local-only; no ITCH data is hosted or committed (no hosting grant found; REUSE_MATRIX §3) |

## Infrastructure notes

- Render CLI: official v2.36.0 at `~/.blackbox/bin/render-official.exe` (the `render` on PATH is an
  unrelated npm package). Render free instance hours are shared with 8 other services in the workspace.
- Local Postgres 18.7 (dev/test) at `127.0.0.1:55432` (user `blackbox`, local-only password
  `blackbox-local`); start detached with
  `powershell -NoProfile -Command "Start-Process -WindowStyle Hidden -FilePath C:\Users\TANMAY\.blackbox\pgsql\postgresql-18.7.0-x86_64-pc-windows-msvc\bin\pg_ctl.exe -ArgumentList 'start','-D','C:\Users\TANMAY\.blackbox\pgdata','-l','C:\Users\TANMAY\.blackbox\pg.log'"`.
- The local ISP resolver refuses `*.neon.tech`; local admin commands against Neon go through a TCP
  forwarder to the endpoint IP with `sslmode=require&options=endpoint%3D<endpoint-id>`.
- Incident (2026-11-13): the Vercel project's Git integration built the repo root and served 404;
  fixed by re-promoting the prebuilt deployment, then (2026-11-14) by building from `web/`.

## Model and agent record

Lead: Claude Opus 5.5. Implementation agents ran mostly on Opus 5.5; GPT-6.1 Sol produced the
analytics crate, the persistence crates (bb-db, bb-artifacts) and the final audit; two earlier Sol
assignments (audit, quant research) failed with `usage_limit_reached` and were rerun on Opus.
Opus agents hit one rate-limit window (HTTP 429) and were resumed on their own transcripts.

Rebuild (2026-11-14): lead Claude Opus 5.5. GPT-6.1 Sol returned `usage_limit_reached` on both
rebuild spawns (replay task, prototype review); both were reassigned to Opus 5.5 with unchanged
acceptance criteria. All other rebuild agents ran on Opus 5.5.

## Open items

Rebuild open items: [../rebuild/FINAL_REVIEW.md](../rebuild/FINAL_REVIEW.md) §J (narrow split panes
at 390 px, Explore Tab order on wide screens, raw `net_pnl` in sweep caveats) and the pending
release (T12).

All ten ranked defects of `FINAL_AUDIT.md` §4 have source fixes on `main` (§7, with commits and
tests). Source fixes do not establish deployment: re-check production after release
(`/api/v1/status` `git_sha`, then `demo seed`). Still open: SEC-9/11/12/17/18/25 partially tested;
AT-5 full hosted-worker peak-memory coverage (the pipeline runtime-breach/cancellation fixture already exists in `crates/bb-app/tests/budgets.rs`); `quota.exceeded` denial audit rows; anonymous detail reads and
`/sweeps` still use Postgres on an anonymous-cache miss (lists and catalogues come from the demo
snapshot); the debug `serve` binary on the dev machine reports libm fingerprint `0352e099` (its
results are non-canonical; harmless).
