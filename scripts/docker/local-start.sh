#!/bin/sh
# Start command of the all-in-one `local` image (Dockerfile target `local`, docker-compose.yml):
# the supported v1 stack against the Postgres in DATABASE_URL.
#
#   1. apply migrations;
#   2. `demo seed --wait`: synthetic dataset, demo runs computed here, curated comparisons and
#      saved reports (idempotent: later starts create nothing new);
#   3. on the first start only, mint a research API key for workspace `lab` and print it once
#      (only its hash is stored; the marker in the data volume prevents a new key per start);
#   4. serve /api/v1 with its worker and the bundled web console on port 8787.
set -eu
: "${DATABASE_URL:?DATABASE_URL must name the Postgres database (docker compose sets it)}"
data=/data
blackbox --data-dir "$data" migrate
blackbox --data-dir "$data" demo seed --wait
if [ ! -f "$data/.local-key-minted" ]; then
    echo "Minting a research API key for workspace 'lab' (shown once; paste it into the console's API key field):"
    blackbox --data-dir "$data" keys mint --workspace lab --scopes read,run --label local-compose
    touch "$data/.local-key-minted"
fi
exec blackbox --data-dir "$data" serve --addr 0.0.0.0:8787 --web-dir /app/web
