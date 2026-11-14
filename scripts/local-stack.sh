#!/usr/bin/env bash
# Local v1 stack without Docker, in one command (run from the repository root; Git Bash on Windows):
#   DATABASE_URL=postgres://user:password@127.0.0.1:5432/blackbox scripts/local-stack.sh
# Builds the binary and the console, applies the migrations, seeds the demo workspace (demo runs
# computed in this process, curated comparisons and saved reports; idempotent) and serves the
# console and /api/v1 on http://127.0.0.1:8787. The database must exist; it is not created here.
# DATA_DIR (default `data`) holds the datasets and run artefacts.
set -euo pipefail

: "${DATABASE_URL:?set DATABASE_URL to an existing Postgres database, e.g. postgres://user:password@127.0.0.1:5432/blackbox}"
export DATABASE_URL
data="${DATA_DIR:-data}"
bin="${CARGO_TARGET_DIR:-target}/release/blackbox"

cargo build --release --locked -p bb-app --bin blackbox
(cd web && npm ci && npm run build)
"$bin" --data-dir "$data" migrate
"$bin" --data-dir "$data" demo seed --wait
exec "$bin" --data-dir "$data" serve --web-dir web/dist
