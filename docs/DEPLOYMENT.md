# Deployment

Two shapes, same code:

| | Hosted research system | Local |
|---|---|---|
| What runs | Vercel SPA → Render web service (`blackbox serve`: `/api/v1` + in-process worker) → Neon Postgres + Vercel Blob | `blackbox serve` (console + `/api/v1` + worker) against a local Postgres and a filesystem artefact store; `docker compose up` runs both |
| URLs | https://blackbox-hft.vercel.app (console), https://blackbox-hft-api.onrender.com (API) | http://127.0.0.1:8787 |
| Access | anonymous visitors: read-only demo workspace; research: `Authorization: Bearer bbk_…` keys | same model; keys minted with `blackbox keys mint` |
| Data | synthetic only (vendor terms forbid hosting Tardis data, [OPEN_SOURCE_RESEARCH.md](OPEN_SOURCE_RESEARCH.md)) | synthetic, and vendor files you downloaded yourself |
| Cost | $0 (free tiers; limits below) | — |

The operator runbook — environment contract, first-time setup, GHCR image publishing, Render
service, Vercel project settings, migrations, rollback, backups and smoke tests — is
[engineering/DEPLOYMENT.md](engineering/DEPLOYMENT.md). Current infrastructure state and incidents
are in [engineering/PROJECT_STATE.md](engineering/PROJECT_STATE.md).

## Local

Quickest: `docker compose up --build` ([docker-compose.yml](../docker-compose.yml)). It starts `postgres:18` and the
Dockerfile's `local` image, whose start command ([scripts/docker/local-start.sh](../scripts/docker/local-start.sh))
runs `migrate`, `demo seed --wait` (synthetic dataset, demo runs computed in that process, curated comparisons and
saved reports; idempotent), mints one research key for workspace `lab` on the first start only (printed once in
`docker compose logs app`) and then `serve` with the built console on http://127.0.0.1:8787 (host loopback only).
`docker compose down -v` deletes the data. CI's `docker` job runs exactly this file and checks the console, the
five curated comparisons, the three saved reports and the printed key.

Without Docker (build requirements in [BUILDING.md](BUILDING.md)), against an existing Postgres database:
`scripts/local-stack.sh` (bash) or `scripts/local-stack.ps1` (Windows PowerShell) runs the steps below except the key
mint, reading `DATABASE_URL` and optionally `DATA_DIR` (default `data`) and `CARGO_TARGET_DIR`. The steps:

```
cargo build --release
cd web && npm ci && npm run build && cd ..
export DATABASE_URL=postgres://user:password@127.0.0.1:5432/blackbox
target/release/blackbox --data-dir data migrate
target/release/blackbox --data-dir data demo seed --wait    # dataset, demo runs, curated comparisons + reports
target/release/blackbox --data-dir data keys mint --workspace lab --scopes read,run --label me
target/release/blackbox --data-dir data serve --web-dir web/dist
```

`demo seed` without `--wait` only queues the demo runs; the worker of `serve` computes them, and a second
`demo seed` then stores the curated comparisons and reports (it prints what is still pending).

Anonymous demo lists and catalogues (`/runs`, `/experiments`, `/comparisons`, `/reports`, `/datasets`, `/presets`,
`/demos`) are answered from an in-memory snapshot that `serve` builds at start and rebuilds after any change it makes
itself, so warm anonymous traffic leaves Postgres idle. Rows written by another process (`demo seed`, `datasets`
commands, a separate `worker`) are noticed by a cheap demo-data version check (`bb-db` `demo_data_version`) that one
anonymous reader runs at most every 15 minutes; a change rebuilds the snapshot. Worst-case staleness is 15 minutes plus
the 30 s anonymous response cache plus one rebuild ([rebuild/PHASE6_RELIABILITY.md](rebuild/PHASE6_RELIABILITY.md)).
Keyed reads always use the database.

The `hosted` Docker target is the API image that CI publishes to `ghcr.io/tanmay-alpha/blackbox-hft`.

## Free-tier limits that can affect the hosted demo

- Render free web service: 0.1 CPU, 512 MB; spins down after 15 minutes without traffic and takes
  about a minute to wake (the console shows a WAKING state). One worker slot, so jobs run one at
  a time. Free instance hours are shared with other services in the same Render workspace.
- Neon free: small storage and monthly compute-hour quota; the database suspends when idle.
- Vercel Blob (Hobby): 1 GB stored, 2,000 uploads and 10,000 uncached reads per month; access is
  blocked (never billed) when exceeded. One artefact bundle is uploaded per completed run.
- Hosted runs are admitted under the `hosted` execution budget (see
  [EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md)); larger experiments run locally.
