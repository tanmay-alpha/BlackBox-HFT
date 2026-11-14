# Deployment

Two shapes, same code:

| | Hosted research system | Local |
|---|---|---|
| What runs | Vercel SPA → Render web service (`blackbox serve`: `/api/v1` + in-process worker) → Neon Postgres + Vercel Blob | `blackbox serve` against a local Postgres and a filesystem artefact store (or the legacy all-in-one `blackbox demo`) |
| URLs | https://blackbox-hft.vercel.app (console), https://blackbox-hft-api.onrender.com (API) | http://127.0.0.1:8787 |
| Access | anonymous visitors: read-only demo workspace; research: `Authorization: Bearer bbk_…` keys | same model; keys minted with `blackbox keys mint` |
| Data | synthetic only (vendor terms forbid hosting Tardis data, [OPEN_SOURCE_RESEARCH.md](OPEN_SOURCE_RESEARCH.md)) | synthetic, and vendor files you downloaded yourself |
| Cost | $0 (free tiers; limits below) | — |

The operator runbook — environment contract, first-time setup, GHCR image publishing, Render
service, Vercel project settings, migrations, rollback, backups and smoke tests — is
[engineering/DEPLOYMENT.md](engineering/DEPLOYMENT.md). Current infrastructure state and incidents
are in [engineering/PROJECT_STATE.md](engineering/PROJECT_STATE.md).

## Local

Build requirements are in [BUILDING.md](BUILDING.md).

```
cargo build --release
cd web && npm ci && npm run build && cd ..
export DATABASE_URL=postgres://user:password@127.0.0.1:5432/blackbox
target/release/blackbox --data-dir data migrate
target/release/blackbox --data-dir data demo seed          # synthetic dataset + demo experiments
target/release/blackbox --data-dir data keys mint --workspace lab --scopes read,run --label me
target/release/blackbox --data-dir data serve --web-dir web/dist
```

`blackbox demo` still starts the deprecated file-based server without Postgres for a quick look.
Docker: `docker build --target local -t blackbox-hft .` builds the all-in-one image; the `hosted`
target is the API image that CI publishes to `ghcr.io/tanmay-alpha/blackbox-hft`.

## Free-tier limits that can affect the hosted demo

- Render free web service: 0.1 CPU, 512 MB; spins down after 15 minutes without traffic and takes
  about a minute to wake (the console shows a WAKING state). One worker slot, so jobs run one at
  a time. Free instance hours are shared with other services in the same Render workspace.
- Neon free: small storage and monthly compute-hour quota; the database suspends when idle.
- Vercel Blob (Hobby): 1 GB stored, 2,000 uploads and 10,000 uncached reads per month; access is
  blocked (never billed) when exceeded. One artefact bundle is uploaded per completed run.
- Hosted runs are admitted under the `hosted` execution budget (see
  [EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md)); larger experiments run locally.
