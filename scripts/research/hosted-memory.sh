#!/usr/bin/env bash
# Hosted full-process memory fixture (audit AT-5 / SEC-6; docs/rebuild/HOSTED_MEMORY.md).
#
# Runs the `hosted` Docker image exactly as Render does (BB_PROFILE=hosted, BB_WORKER_SLOTS=1,
# the hosted admission limits, migrations on start, TLS verify-full to Postgres, the real Vercel
# Blob client) inside a 512 MB container, next to throwaway local stand-ins:
#
#   bbmem-db      postgres:18 with a fixture-only TLS certificate (alias db.bbmem.test)
#   bbmem-blob    a fake of the Vercel Blob API (scripts/research/hosted_memory_fixture.py blob)
#                 under the aliases vercel.com and memstore.private.blob.vercel-storage.com,
#                 trusted through SSL_CERT_FILE; no request leaves the Docker network
#   bbmem-app     `blackbox serve` from the hosted image (--memory 512m, no swap)
#   bbmem-driver  shares bbmem-app's PID namespace: samples the server's /proc status, resets
#                 VmHWM per phase (clear_refs) and drives the phases over HTTP
#
# The demo is seeded first in one-off containers (`migrate`, `demo seed --wait`, `keys mint`),
# so the measured server process starts from a cold boot. Phases: cache fill, concurrent
# anonymous heavy reads, the largest admitted hosted job, that job plus concurrent reads plus a
# comparison of the two largest runs, cancellation of a running job, idle recovery.
#
# The Blob token, API-key pepper and API key are generated per run (Postgres uses the local-only default password)
# and exists only inside the throwaway containers and a temporary env file that is deleted on
# exit. Nothing real is read or contacted.
#
# Usage (repository root; Docker required; Git Bash on Windows works):
#   scripts/research/hosted-memory.sh            # build the image if missing, run, clean up
#   BBMEM_BUILD=1 scripts/research/hosted-memory.sh   # rebuild the hosted image first
# Knobs: BBMEM_IMAGE, BBMEM_PG_IMAGE, BBMEM_PY_IMAGE, BBMEM_MEMORY (512m), BBMEM_CPUS (unset:
# no CPU quota), BBMEM_CLIENTS (32), BBMEM_LOAD_SERVED (600 handler-answered reads in phase 2),
# BBMEM_LOAD_S (180, phase 2 time cap), BBMEM_CANDIDATES (`none`: reads only), BBMEM_APP_ENV,
# BBMEM_HEAPTRACK=1, BBMEM_OUT, BBMEM_KEEP=1 (keep containers).
# Output: $BBMEM_OUT/result.json (phases, summary, RSS series), driver.log, app.log.
# Exit status: 0 when the measured peak (VmHWM) stays under 300 MB and no container was
# OOM-killed, 1 otherwise.
set -euo pipefail
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

cd "$(dirname "$0")/../.."
IMAGE=${BBMEM_IMAGE:-blackbox-hft:hosted-mem}
PG_IMAGE=${BBMEM_PG_IMAGE:-postgres:18}
PY_IMAGE=${BBMEM_PY_IMAGE:-python:3.12-slim}
MEMORY=${BBMEM_MEMORY:-512m}
OUT=${BBMEM_OUT:-target/hosted-memory/$(date -u +%Y%m%dT%H%M%SZ)}
LIMIT_BYTES=300000000

# Candidate jobs, `dataset/preset/interval_ns/feed_latency_offset_ns/max_queue_updates_per_order`:
# the largest shape the hosted profile admits of every preset on every seeded dataset (equity
# sampled every tick). Generated and checked against the admission code by
# crates/bb-app/tests/hosted_full_process_memory.rs; the fixture runs each one and measures it.
JOB_CANDIDATES="
    synthetic-demo/maker_baseline/1000000/22549000000/2000
    synthetic-demo/maker_baseline/1000000/22549000000/4294967295
    synthetic-demo/maker_slow/1000000/22549000000/2000
    synthetic-demo/maker_slow/1000000/22549000000/4294967295
    synthetic-demo/maker_conservative_queue/1000000/22549000000/2000
    synthetic-demo/maker_conservative_queue/1000000/22549000000/4294967295
    synthetic-demo/maker_rebate/1000000/22549000000/2000
    synthetic-demo/maker_rebate/1000000/22549000000/4294967295
    synthetic-demo/avellaneda_stoikov/1000000/22549000000/2000
    synthetic-demo/avellaneda_stoikov/1000000/22549000000/4294967295
    synthetic-demo/imbalance_taker/2000000/60000000000/2000
    synthetic-demo/imbalance_taker/2000000/60000000000/4294967295
    synthetic-demo/forensics_probe/1000000/29997000000/2000
    synthetic-demo/latency_lognormal/1000000/22549000000/2000
    synthetic-demo/latency_lognormal/1000000/22549000000/4294967295
    synthetic-demo/latency_heavy_tail/1000000/22549000000/2000
    synthetic-demo/latency_heavy_tail/1000000/22549000000/4294967295
    synthetic-demo/glft_baseline/1000000/22549000000/2000
    synthetic-demo/glft_baseline/1000000/22549000000/4294967295
    synthetic-stress-depletion/maker_baseline/1000000/22099000000/2000
    synthetic-stress-depletion/maker_baseline/1000000/22099000000/4294967295
    synthetic-stress-depletion/maker_slow/1000000/22099000000/2000
    synthetic-stress-depletion/maker_slow/1000000/22099000000/4294967295
    synthetic-stress-depletion/maker_conservative_queue/1000000/22099000000/2000
    synthetic-stress-depletion/maker_conservative_queue/1000000/22099000000/4294967295
    synthetic-stress-depletion/maker_rebate/1000000/22099000000/2000
    synthetic-stress-depletion/maker_rebate/1000000/22099000000/4294967295
    synthetic-stress-depletion/avellaneda_stoikov/1000000/22099000000/2000
    synthetic-stress-depletion/avellaneda_stoikov/1000000/22099000000/4294967295
    synthetic-stress-depletion/imbalance_taker/2000000/60000000000/2000
    synthetic-stress-depletion/imbalance_taker/2000000/60000000000/4294967295
    synthetic-stress-depletion/forensics_probe/1000000/29997000000/2000
    synthetic-stress-depletion/latency_lognormal/1000000/22099000000/2000
    synthetic-stress-depletion/latency_lognormal/1000000/22099000000/4294967295
    synthetic-stress-depletion/latency_heavy_tail/1000000/22099000000/2000
    synthetic-stress-depletion/latency_heavy_tail/1000000/22099000000/4294967295
    synthetic-stress-depletion/glft_baseline/1000000/22099000000/2000
    synthetic-stress-depletion/glft_baseline/1000000/22099000000/4294967295
    synthetic-stress-wide-spreads/maker_baseline/1000000/22751000000/2000
    synthetic-stress-wide-spreads/maker_baseline/1000000/22751000000/4294967295
    synthetic-stress-wide-spreads/maker_slow/1000000/22751000000/2000
    synthetic-stress-wide-spreads/maker_slow/1000000/22751000000/4294967295
    synthetic-stress-wide-spreads/maker_conservative_queue/1000000/22751000000/2000
    synthetic-stress-wide-spreads/maker_conservative_queue/1000000/22751000000/4294967295
    synthetic-stress-wide-spreads/maker_rebate/1000000/22751000000/2000
    synthetic-stress-wide-spreads/maker_rebate/1000000/22751000000/4294967295
    synthetic-stress-wide-spreads/avellaneda_stoikov/1000000/22751000000/2000
    synthetic-stress-wide-spreads/avellaneda_stoikov/1000000/22751000000/4294967295
    synthetic-stress-wide-spreads/imbalance_taker/2000000/60000000000/2000
    synthetic-stress-wide-spreads/imbalance_taker/2000000/60000000000/4294967295
    synthetic-stress-wide-spreads/forensics_probe/1000000/29997000000/2000
    synthetic-stress-wide-spreads/latency_lognormal/1000000/22751000000/2000
    synthetic-stress-wide-spreads/latency_lognormal/1000000/22751000000/4294967295
    synthetic-stress-wide-spreads/latency_heavy_tail/1000000/22751000000/2000
    synthetic-stress-wide-spreads/latency_heavy_tail/1000000/22751000000/4294967295
    synthetic-stress-wide-spreads/glft_baseline/1000000/22751000000/2000
    synthetic-stress-wide-spreads/glft_baseline/1000000/22751000000/4294967295
"

STORE=memstore
NET=bbmem
hostpath() { if command -v cygpath > /dev/null; then cygpath -w "$1"; else printf '%s' "$1"; fi; }
rand_hex() { openssl rand -hex "$1"; }

work=$(mktemp -d)
cleanup() {
    status=$?
    if [ "${BBMEM_KEEP:-0}" != 1 ]; then
        docker rm -f bbmem-driver bbmem-app bbmem-oneoff bbmem-blob bbmem-db > /dev/null 2>&1 || true
        docker network rm "$NET" > /dev/null 2>&1 || true
    fi
    rm -rf "$work"
    exit "$status"
}
trap cleanup EXIT
mkdir -p "$OUT"

if [ "${BBMEM_BUILD:-0}" = 1 ] || ! docker image inspect "$IMAGE" > /dev/null 2>&1; then
    echo "building $IMAGE (Dockerfile target hosted)"
    docker build --target hosted --build-arg "GIT_SHA=$(git rev-parse --short HEAD)" -t "$IMAGE" .
fi

# --- fixture-only certificates ------------------------------------------------------------------
certs="$work/certs"
mkdir -p "$certs"
# A self-contained config, so an OPENSSL_CONF inherited from the host cannot break `req`.
printf '[req]\ndistinguished_name = dn\n[dn]\n' > "$certs/openssl.cnf"
export OPENSSL_CONF="$certs/openssl.cnf"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=bbmem fixture CA" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -keyout "$certs/ca.key" -out "$certs/ca.crt" 2> /dev/null
leaf() { # name, comma-separated DNS names
    local sans
    sans=$(printf 'DNS:%s,' ${2//,/ })
    openssl req -newkey rsa:2048 -nodes -subj "/CN=${2%%,*}" \
        -keyout "$certs/$1.key" -out "$certs/$1.csr" 2> /dev/null
    printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=%s\n' \
        "${sans%,}" > "$certs/$1.ext"
    openssl x509 -req -in "$certs/$1.csr" -CA "$certs/ca.crt" -CAkey "$certs/ca.key" \
        -CAcreateserial -days 2 -extfile "$certs/$1.ext" -out "$certs/$1.crt" 2> /dev/null
}
leaf server db.bbmem.test
leaf blob "vercel.com,$STORE.private.blob.vercel-storage.com"
trust="$work/trust"
fixture="$work/fixture"
mkdir -p "$trust" "$fixture"
cp "$certs/ca.crt" "$trust/ca.crt"
cp "$certs/blob.crt" "$certs/blob.key" scripts/research/hosted_memory_fixture.py "$fixture/"

# --- network, Postgres, fake Blob ---------------------------------------------------------------
docker rm -f bbmem-driver bbmem-app bbmem-oneoff bbmem-blob bbmem-db > /dev/null 2>&1 || true
docker network rm "$NET" > /dev/null 2>&1 || true
docker network create "$NET" > /dev/null

# The repository's local-only default (docker-compose.yml); the database is on a private network.
pg_password=blackbox-local
docker create --name bbmem-db --network "$NET" --network-alias db.bbmem.test \
    -e POSTGRES_USER=blackbox -e "POSTGRES_PASSWORD=$pg_password" -e POSTGRES_DB=blackbox \
    --entrypoint sh "$PG_IMAGE" -c '
        install -d -o postgres -m 700 /pgcerts &&
        install -o postgres -m 600 /certs-in/server.key /pgcerts/server.key &&
        install -o postgres -m 644 /certs-in/server.crt /pgcerts/server.crt &&
        exec docker-entrypoint.sh postgres -c ssl=on \
            -c ssl_cert_file=/pgcerts/server.crt -c ssl_key_file=/pgcerts/server.key' > /dev/null
mkdir -p "$work/pgcerts"
cp "$certs/server.crt" "$certs/server.key" "$work/pgcerts/"
docker cp "$(hostpath "$work/pgcerts")" bbmem-db:/certs-in > /dev/null
docker start bbmem-db > /dev/null

docker create --name bbmem-blob --network "$NET" \
    --network-alias vercel.com --network-alias "$STORE.private.blob.vercel-storage.com" \
    -e "BLOB_STORE_ID=$STORE" "$PY_IMAGE" python3 /fixture/hosted_memory_fixture.py blob > /dev/null
docker cp "$(hostpath "$fixture")" bbmem-blob:/fixture > /dev/null
docker start bbmem-blob > /dev/null

echo "waiting for Postgres"
for _ in $(seq 120); do
    if docker logs bbmem-db 2>&1 | grep -q 'PostgreSQL init process complete' &&
        docker exec bbmem-db pg_isready -q -U blackbox -d blackbox; then
        break
    fi
    sleep 1
done
docker exec bbmem-db pg_isready -q -U blackbox -d blackbox

# --- the hosted environment (Render's render.yaml values; generated throwaway secrets) ----------
db_url="postgres://blackbox:blackbox-local@db.bbmem.test:5432/blackbox?sslmode=verify-full&sslrootcert=/etc/bbmem/ca.crt"
# A fake token of the shape the hosted profile requires; only the fake Blob store ever sees it.
blob_token="vercel_blob_rw_${STORE}_$(rand_hex 16)"
env_file="$work/hosted.env"
cat > "$env_file" << EOF
BB_PROFILE=hosted
DATABASE_URL=$db_url
DATABASE_URL_UNPOOLED=$db_url
BLOB_READ_WRITE_TOKEN=$blob_token
BB_API_KEY_PEPPER=$(rand_hex 24)
BB_CORS_ALLOWED_ORIGINS=https://blackbox-hft.vercel.app
BB_ALLOWED_HOSTS=app
BB_MIGRATE_ON_START=true
BB_WORKER_SLOTS=1
BB_SHUTDOWN_GRACE_MS=25000
RUST_LOG=info
SSL_CERT_FILE=/etc/bbmem/ca.crt
EOF

oneoff() { # blackbox arguments; prints the command's output
    docker create --name bbmem-oneoff --network "$NET" --env-file "$(hostpath "$env_file")" \
        "$IMAGE" blackbox --data-dir /var/lib/blackbox "$@" > /dev/null
    docker cp "$(hostpath "$trust")" bbmem-oneoff:/etc/bbmem > /dev/null
    local code=0
    docker start -a bbmem-oneoff > "$work/oneoff.log" 2>&1 || code=$?
    docker rm bbmem-oneoff > /dev/null
    if [ "$code" != 0 ]; then
        grep -v 'bbk_' "$work/oneoff.log" | tail -40 >&2
        echo "blackbox $1 failed ($code)" >&2
        return "$code"
    fi
}
echo "migrate + demo seed --wait (one-off containers)"
oneoff migrate
oneoff demo seed --wait
grep -v 'bbk_' "$work/oneoff.log" | tail -3
oneoff keys mint --workspace lab --scopes read,run --label hosted-memory
api_key=$(grep -o 'bbk_[A-Za-z0-9_-]*' "$work/oneoff.log" | head -1)
[ -n "$api_key" ] || { echo "no API key minted" >&2; exit 1; }

# --- the measured server -------------------------------------------------------------------------
# BBMEM_HEAPTRACK=1 runs the same binary under heaptrack (Debian package, added to a derived image)
# to attribute the peak to allocation sites; its RSS figures include heaptrack's own overhead, so
# they are diagnostic only. The memory limit defaults to 2g in that mode so the peak is reached.
app_image=$IMAGE
app_cmd=()
if [ "${BBMEM_HEAPTRACK:-0}" = 1 ]; then
    app_image="$IMAGE-heaptrack"
    MEMORY=${BBMEM_MEMORY:-2g}
    printf 'FROM %s\nUSER root\nRUN apt-get update && apt-get install -y --no-install-recommends heaptrack && rm -rf /var/lib/apt/lists/*\nUSER 10001:10001\n' \
        "$IMAGE" | docker build -q -t "$app_image" - > /dev/null
    app_cmd=(heaptrack -o /var/lib/blackbox/heaptrack.serve blackbox --data-dir /var/lib/blackbox serve)
fi
cpus=()
[ -n "${BBMEM_CPUS:-}" ] && cpus=(--cpus "$BBMEM_CPUS")
# BBMEM_APP_ENV: extra NAME=VALUE words for the server only (experiments, e.g. allocator tuning).
app_env=()
for assignment in ${BBMEM_APP_ENV:-}; do app_env+=(-e "$assignment"); done
docker create --name bbmem-app --network "$NET" --network-alias app \
    --memory "$MEMORY" --memory-swap "$MEMORY" "${cpus[@]}" "${app_env[@]}" \
    --env-file "$(hostpath "$env_file")" "$app_image" "${app_cmd[@]}" > /dev/null
docker cp "$(hostpath "$trust")" bbmem-app:/etc/bbmem > /dev/null
docker start bbmem-app > /dev/null

printf 'BB_FIXTURE_KEY=%s\n' "$api_key" > "$work/driver.env"
docker create --name bbmem-driver --network "$NET" --pid container:bbmem-app --user 10001:10001 \
    --env-file "$(hostpath "$work/driver.env")" \
    -e APP_HOST=app -e "JOB_CANDIDATES=${BBMEM_CANDIDATES:-$JOB_CANDIDATES}" \
    -e "CLIENTS=${BBMEM_CLIENTS:-32}" \
    -e "LOAD_S=${BBMEM_LOAD_S:-180}" -e "LOAD_SERVED=${BBMEM_LOAD_SERVED:-600}" \
    "$PY_IMAGE" python3 /fixture/hosted_memory_fixture.py drive > /dev/null
docker cp "$(hostpath "$fixture")" bbmem-driver:/fixture > /dev/null
echo "driving phases (results in $OUT)"
driver_code=0
docker start -a bbmem-driver > "$OUT/result.json" 2> "$OUT/driver.log" || driver_code=$?
oom=$(docker inspect -f '{{.State.OOMKilled}}' bbmem-app)
running=$(docker inspect -f '{{.State.Running}}' bbmem-app)
if [ "${BBMEM_HEAPTRACK:-0}" = 1 ]; then
    # SIGTERM: the server drains and exits, heaptrack writes its data file.
    docker stop -t 60 bbmem-app > /dev/null
    for suffix in zst gz; do
        docker cp "bbmem-app:/var/lib/blackbox/heaptrack.serve.$suffix" "$(hostpath "$OUT")" > /dev/null 2>&1 &&
            docker create --name bbmem-oneoff "$app_image" heaptrack_print -f "/tmp/heaptrack.serve.$suffix" \
                --print-peaks 1 --print-allocators 0 --print-temporary 0 --print-leaks 0 \
                --print-histogram 0 -n 30 > /dev/null &&
            docker cp "$(hostpath "$OUT/heaptrack.serve.$suffix")" bbmem-oneoff:/tmp/ > /dev/null &&
            { docker start -a bbmem-oneoff > "$OUT/heaptrack.txt" 2>&1 || true; } &&
            docker rm bbmem-oneoff > /dev/null && echo "heaptrack report: $OUT/heaptrack.txt" && break
    done
fi
docker logs bbmem-app > "$OUT/app.log" 2>&1 || true
grep -E '^(boot|phase|summary)' "$OUT/driver.log" || true
echo "server container: running=$running oom_killed=$oom"
if [ "$driver_code" != 0 ]; then
    tail -20 "$OUT/driver.log" >&2
    echo "driver failed ($driver_code)" >&2
    exit 1
fi
peak_mib=$(grep -o '"rss_peak_mib": [0-9.]*' "$OUT/result.json" | tail -1 | grep -o '[0-9.]*$')
peak_bytes=$(awk -v m="$peak_mib" 'BEGIN { printf "%d", m * 1048576 }')
echo "server RSS peak: $peak_mib MiB ($peak_bytes bytes; limit $LIMIT_BYTES)"
[ "$oom" = false ] && [ "$peak_bytes" -lt "$LIMIT_BYTES" ]
