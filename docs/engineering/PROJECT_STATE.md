# Project state

Single source of truth for the current phase, decisions, infrastructure and next actions. Updated by
the lead at every phase boundary. Agents read this first instead of re-auditing the repository.

## Current phase

**Phase 0: audit and architecture** (started 2026-11-14 on `main` at `5fce27d`, CI run 43509125259
green: rust, web, e2e, audit, docker).

## Decisions (user-approved 2026-11-14)

| Topic | Decision | Consequence |
|---|---|---|
| Compute | One **free** Render web service runs the axum API **and** an in-process worker pool that consumes the Postgres job queue. The worker is also a separate `blackbox worker` command; `render.yaml` declares the paid private worker as an optional upgrade. | 0.1 CPU / 512 MB; spins down after 15 min idle (~1 min cold start). Jobs interrupted by spin-down must be recovered by lease expiry. No paid resources. |
| PostgreSQL | **Neon free** (`free_v3`) via Vercel Marketplace, resource `blackbox-hft-db`, region `sin1` (AWS ap-southeast-1), PostgreSQL 18.7. | Small storage quota; autosuspend. Connection strings in `data/secrets/neon.env` (git-ignored). |
| Object storage | **Vercel Blob private store** `blackbox-hft-artifacts` (`store_eod21bxLMlokHq3Z`, `sin1`), accessed from Rust with the long-lived `BLOB_READ_WRITE_TOKEN`. | Hobby hard limits: 1 GB stored, 2,000 advanced ops (uploads/lists) and 10,000 simple ops (uncached reads) per month; exceeding blocks access for 30 days (never billed). Design: one artefact bundle per run, local read cache. Token in `data/secrets/vercel-prod.env` (git-ignored). |
| Access | Public **read-only** demo workspace + authenticated research via hashed, scoped **API keys** minted by an admin CLI. | No OAuth, no guest submissions. |
| Hosted data | Synthetic only, unless the quant research identifies an openly licensed real dataset (Tardis is never hosted: terms §9.2). | |
| Redis | Not used. Queue is Postgres (`FOR UPDATE SKIP LOCKED`). | |

## Infrastructure inventory

| Thing | State | Notes |
|---|---|---|
| GitHub | `tanmay-alpha/BlackBox-HFT`, public, `main` only, no open issues/PRs | `gh` authenticated |
| Vercel | project `blackbox-hft` (static viewer, https://blackbox-hft.vercel.app) | CLI 65.12.1 as `tanmay-alpha`; Neon + Blob connected to the project's production env (the static build reads no non-`VITE_` vars) |
| Render | workspace `tea-d7gs7im7r5hc73dqlung` ("My Workspace", Hobby), 8 unrelated free web services in `singapore`, no Postgres/KV/disks | Official CLI v2.36.0 at `~/.blackbox/bin/render-official.exe` (the `render` on PATH is an unrelated npm templating package). Free instance hours (750/month) are shared with the other 8 services. |
| Local Postgres | 18.7 (theseus-rs build, SHA-256 verified) at `127.0.0.1:55432`, user `blackbox`, password `blackbox-local`, DBs `blackbox_dev`, `blackbox_test` | Start: `nohup ~/.blackbox/pgsql/postgresql-18.7.0-x86_64-pc-windows-msvc/bin/postgres.exe -D ~/.blackbox/pgdata > ~/.blackbox/pg.log 2>&1 &`. Local-only dev credential. |
| Local DNS quirk | The ISP resolver refuses `*.neon.tech`; public DNS resolves it | Local checks against Neon must pin `hostaddr`; Render is unaffected. Integration tests use the local cluster. |
| Vercel Blob wire protocol | Verified 2026-11-14 from Python with raw HTTP (protocol read from the official `vercel-blob` 0.6 SDK source) | `PUT https://vercel.com/api/blob?pathname=<p>` with `authorization: Bearer <BLOB_READ_WRITE_TOKEN>`, `x-api-version: 12`, `x-vercel-blob-access: private`, `x-add-random-suffix: 0`, `x-allow-overwrite: 0|1`, `x-content-type` → 200 JSON `{url, pathname, etag, …}`. Read: `GET https://<storeid lowercase>.private.blob.vercel-storage.com/<path>` with the same bearer + `x-api-version` → 200 bytes; missing → 404; **anonymous → 403**. Delete: `POST https://vercel.com/api/blob/delete` JSON `{"urls":[…]}` → 200. Store id is embedded in the token (`vercel_blob_rw_<storeId>_<secret>`). |
| Vercel Git integration | **Disconnected** 2026-11-14 | Incident: the project was connected to the GitHub repo, so the push of `5fce27d` (2026-11-13 21:15 UTC) built the repo root (nothing to build) and promoted an empty production deployment two minutes after the verified prebuilt one; https://blackbox-hft.vercel.app served 404 until the lead re-promoted `dpl_6t3iTPaE3EgcmZR6BpA1S93AUBT6` and ran `vercel git disconnect`. Reconnect only once `web/` builds the production frontend from Git (root directory `web`). |

## Phase 0 work in flight

| Agent | Model | Output |
|---|---|---|
| BaselineAudit2 | claude-opus-5-5 | `docs/engineering/BASELINE_AUDIT.md` |
| QuantResearch2 | claude-opus-5-5 | `docs/research/MARKET_MICROSTRUCTURE.md` |
| OSSEval | claude-opus-5-5 | `docs/research/OSS_TECHNOLOGY_EVALUATION.md` |
| UIDesign | claude-opus-5-5 | `docs/engineering/UI_DESIGN_SPEC.md` |

`openai-codex/gpt-6.1-sol` returned `usage_limit_reached` on 2026-11-14 for both tasks assigned to
it (audit, quant research); they were rerun on Opus 5.5. Sol will be retried for the final
independent audit.

## Next actions

1. Collect Phase 0 documents; write `TARGET_ARCHITECTURE.md`, `DATABASE_DESIGN.md`, `API_CONTRACT.md`.
2. Phase 1 correctness fixes from the audit (comparison eligibility, result integrity on load,
   single-flight execution, timestamp precision, resource budget).
3. Phase 2: Postgres schema + migrations, job queue, artefact store, versioned API.
