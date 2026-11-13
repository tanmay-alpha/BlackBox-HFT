# Deployment

BlackBox has two deployment shapes, and they are deliberately different products:

| | Full simulator | Hosted viewer |
|---|---|---|
| What runs | `blackbox` binary: engine, API, web console | static files only |
| Can simulate new runs | yes | no |
| Datasets | synthetic, and vendor files you downloaded yourself | synthetic only |
| Where | your machine, or a container you control | static hosting |

Vendor market data (the Tardis sample) can only ever be used in the full simulator on your own
machine. Its terms forbid redistribution and offering it to others through a backtesting or
charting service ([OPEN_SOURCE_RESEARCH.md](OPEN_SOURCE_RESEARCH.md)), so it never appears in the
hosted viewer, in screenshots or in the repository.

## Full simulator, local

Build requirements are in [BUILDING.md](BUILDING.md).

```
cargo build --release
cd web && npm ci && npm run build && cd ..
target/release/blackbox demo          # generates the synthetic dataset on first start, then serves
```

The server listens on `127.0.0.1:8787` and serves the console and the API from the same origin.
It refuses requests addressed to any other host name or coming from another origin
([SECURITY.md](SECURITY.md)). For the Vite development server:

```
target/release/blackbox serve --allow-origin http://localhost:5173
cd web && npm run dev
```

Data lives under `--data-dir` (default `data/`): `datasets/<id>/` (manifest and event archive) and
`runs/<run_id>/` (summary and full run). Both are plain files; stopping the server loses nothing,
and a run is written to a temporary name and renamed into place, so an interrupted write never
leaves a half-written run. Deleting `data/runs` only removes cached results: every run is
reproducible from its configuration ([EXPERIMENT_REPRODUCIBILITY.md](EXPERIMENT_REPRODUCIBILITY.md)).

## Full simulator, Docker

```
docker build -t blackbox-hft .
docker run --rm -p 127.0.0.1:8787:8787 -v blackbox-data:/data blackbox-hft
```

The image builds the Rust binary and the web console in separate stages and runs as an
unprivileged user with data in the `/data` volume. Inside the container the server binds
`0.0.0.0:8787` so the published port reaches it; publish it on the host's loopback (as above)
unless you mean to share it, because the API has no authentication. If you publish a different
host port, add its name: `... blackbox-hft blackbox --data-dir /data demo --addr 0.0.0.0:8787
--web-dir /app/web --allow-host localhost:9000`.

To use vendor files you downloaded, mount them read-only and ingest inside the container:

```
docker run --rm -v blackbox-data:/data -v "$PWD/data/raw:/raw:ro" blackbox-hft \
  blackbox --data-dir /data ingest-tardis --trades /raw/.../trades.csv.gz \
  --book /raw/.../incremental_book_L2.csv.gz --id my-sample --exchange binance-futures \
  --symbol BTCUSDT --tick-size 0.01 --lot-size 0.001 --max-duration-s 600
```

The CI `docker` job builds the image and checks that a container answers `/api/health` and lists
the synthetic dataset on every push.

## Hosted viewer (static)

Live: **https://blackbox-hft.vercel.app**: a read-only viewer of 11 precomputed runs, 10
comparisons and a 6-cell sensitivity grid on the synthetic dataset. Mission Control, the three
canonical demos in Latency Lab, run overviews, order forensics, the Replay Terminal, Compare and
the grid all work. Strategy Lab, the grid's run button and run deletion are disabled with an
explanation, because nothing can be simulated there.

How it is built:

1. `blackbox export-static --out data/static-data` (`crates/bb-app/src/export_static.rs`) runs
   or reuses the demo runs and writes every API response the pages need as gzip files. It
   produces exactly the JSON the server would return for the same requests; a test checks this.
   It refuses any dataset that is not synthetic, before simulating or writing anything. The
   output goes to a content-hash directory named by `blackbox-static.json`, so the bundle files
   can be cached immutably and only that one file is revalidated.
2. `npm run build:static` (in `web/`) builds the console in `--mode static`, which switches the API
   client to read the bundle, then copies the bundle into `dist/static-data/`.
3. The output is deployed as prebuilt static files. Vercel's builders cannot produce it, because
   the bundle comes from a local simulation and `data/` is git-ignored:

```
cargo build --release
target/release/blackbox export-static --out data/static-data
cd web && npm ci && npm run build:static
# deploy web/dist together with web/vercel.json (rewrites, CSP and cache headers), e.g.
mkdir -p ../data/deploy/blackbox-hft && cp -r dist/. ../data/deploy/blackbox-hft/
jq 'del(.framework, .buildCommand, .outputDirectory)' vercel.json > ../data/deploy/blackbox-hft/vercel.json
cd ../data/deploy/blackbox-hft && vercel deploy --prod
```

Measured for the current bundle: 321 data files, 26.4 MiB gzip (337.9 MiB of JSON and Markdown
uncompressed); 325 files, 28 MB deployed. Response headers include a strict
`Content-Security-Policy` (`script-src 'self'`, no inline scripts), `X-Frame-Options: DENY`,
`nosniff` and `Referrer-Policy: no-referrer`. A missing data file returns 404 instead of the app
shell.

Viewer limits: only the exported windows and pairs exist. Replay windows are 120 s long every
60 s, plus the default and whole-dataset windows; comparisons are each demo run against that
demo's first run, both ways round; depth is limited to 25 levels. Anything else shows a "not in
the precomputed bundle" message. The run download is `export.json.gz`.

## Why there is no hosted simulator

A public simulation endpoint would need authentication, per-user isolation, quotas and a host that
can run multi-second CPU-bound jobs; free serverless platforms are a poor fit for long-running Rust
simulations, and the brief rules out paid hosting. The useful public artefact is therefore the
read-only viewer of precomputed runs above, while the full simulator stays a local tool.
