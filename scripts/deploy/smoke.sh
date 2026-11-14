#!/usr/bin/env bash
# Smoke test of a running BlackBox API (hosted on Render, or local). Read-only: it never creates
# anything. Checks, in order:
#   1. GET /healthz answers 200 JSON (retried while the service wakes up: a spun-down free Render
#      service takes about a minute and serves its own loading page meanwhile)
#   2. GET /api/v1/status answers 200 JSON; with EXPECT_GIT_SHA set, waits until the reported
#      git_sha is that commit (the new deploy is live, not the previous instance)
#   3. GET /readyz answers 200 (database reachable, migrations applied, storage configured)
#   4. CORS preflight from ALLOWED_ORIGIN is allowed for exactly that origin
#   5. CORS preflight from a foreign origin gets no Access-Control-Allow-Origin
#   6. a GET carrying a foreign Origin is refused with 403
#   7. an unauthenticated POST to a mutating endpoint gets 401 `unauthorized`
#   8. a malformed bearer key on a read gets 401 (no silent fallback to the demo workspace)
#
# Usage: BASE_URL=https://blackbox-hft-api.onrender.com scripts/deploy/smoke.sh
# Environment:
#   BASE_URL         required; service origin without a trailing slash
#   ALLOWED_ORIGIN   origin that must pass CORS (default https://blackbox-hft.vercel.app)
#   FOREIGN_ORIGIN   origin that must fail CORS (default https://smoke-test.invalid)
#   EXPECT_GIT_SHA   optional; full or abbreviated commit the service must report
#   WAIT_SECONDS     how long to keep retrying steps 1-3 (default 180)
# Needs bash, curl and grep only. Exit status 0 = every check passed.
set -euo pipefail

BASE_URL=${BASE_URL:?set BASE_URL to the service origin, e.g. https://blackbox-hft-api.onrender.com}
BASE_URL=${BASE_URL%/}
ALLOWED_ORIGIN=${ALLOWED_ORIGIN:-https://blackbox-hft.vercel.app}
FOREIGN_ORIGIN=${FOREIGN_ORIGIN:-https://smoke-test.invalid}
EXPECT_GIT_SHA=${EXPECT_GIT_SHA:-}
WAIT_SECONDS=${WAIT_SECONDS:-180}
MUTATING_PATH=/api/v1/experiments

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
body=$tmp/body
headers=$tmp/headers
deadline=$((SECONDS + WAIT_SECONDS))
failures=0

log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*"; }
pass() { log "PASS $*"; }
fail() { log "FAIL $*"; failures=$((failures + 1)); }

# request METHOD PATH [curl args...] -> sets $code; body in $body, headers in $headers.
request() {
  local method=$1 path=$2
  shift 2
  : >"$body"
  : >"$headers"
  code=$(curl -sS -X "$method" -o "$body" -D "$headers" -w '%{http_code}' \
    --connect-timeout 10 --max-time 30 "$@" "$BASE_URL$path" 2>"$tmp/curl.err") || code=000
}

# The `|| true` guards keep `set -e -o pipefail` from aborting when nothing matches.
header() { # header NAME -> value of the last occurrence (case-insensitive), CR stripped
  { grep -i "^$1:" "$headers" || true; } | tail -n 1 | cut -d: -f2- | tr -d '\r' | sed 's/^ *//; s/ *$//'
}

is_json() { header content-type | grep -qi 'application/json'; }

# json_string KEY -> first "KEY":"value" in the body (flat string fields only).
json_string() {
  { grep -o "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" "$body" || true; } | head -n 1 | sed 's/.*:[[:space:]]*"//; s/"$//'
}

# wait_for NAME METHOD PATH PREDICATE: retry until PREDICATE (a function) succeeds or the deadline.
wait_for() {
  local name=$1 method=$2 path=$3 predicate=$4 attempt=0 delay=5
  while :; do
    attempt=$((attempt + 1))
    request "$method" "$path"
    if "$predicate"; then
      pass "$name ($method $path -> $code, attempt $attempt)"
      return 0
    fi
    if [ "$SECONDS" -ge "$deadline" ]; then
      fail "$name ($method $path -> $code after $attempt attempts, ${WAIT_SECONDS}s)"
      head -c 400 "$body" 2>/dev/null || true
      echo
      return 1
    fi
    log "wait $name: HTTP $code$(is_json || echo ', not JSON (service waking?)'); retry in ${delay}s"
    sleep "$delay"
    if [ "$delay" -lt 20 ]; then delay=$((delay + 5)); fi
  done
}

healthy() { [ "$code" = 200 ] && is_json; }

status_current() {
  if [ "$code" != 200 ] || ! is_json; then return 1; fi
  if [ -z "$EXPECT_GIT_SHA" ]; then return 0; fi
  local got
  got=$(json_string git_sha)
  # Either side may be abbreviated (at least 7 hex characters).
  if [ "${#got}" -lt 7 ]; then return 1; fi
  case "$EXPECT_GIT_SHA" in "$got"*) return 0 ;; esac
  case "$got" in "$EXPECT_GIT_SHA"*) [ "${#EXPECT_GIT_SHA}" -ge 7 ] && return 0 ;; esac
  log "status reports git_sha=$got, waiting for $EXPECT_GIT_SHA"
  return 1
}

ready() { [ "$code" = 200 ]; }

log "smoke test of $BASE_URL"
wait_for "liveness" GET /healthz healthy || exit 1
wait_for "status" GET /api/v1/status status_current || exit 1
wait_for "readiness" GET /readyz ready || exit 1

# 4. Preflight from the allowed origin: 2xx and the exact origin echoed back.
request OPTIONS "$MUTATING_PATH" \
  -H "Origin: $ALLOWED_ORIGIN" \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: authorization,content-type'
acao=$(header access-control-allow-origin)
if [[ "$code" =~ ^2 ]] && [ "$acao" = "$ALLOWED_ORIGIN" ]; then
  pass "CORS preflight from $ALLOWED_ORIGIN ($code, allow-origin=$acao)"
else
  fail "CORS preflight from $ALLOWED_ORIGIN (HTTP $code, allow-origin='$acao')"
fi

# 5. Preflight from a foreign origin: no allow-origin header (the browser then blocks the call).
request OPTIONS "$MUTATING_PATH" \
  -H "Origin: $FOREIGN_ORIGIN" \
  -H 'Access-Control-Request-Method: POST' \
  -H 'Access-Control-Request-Headers: authorization,content-type'
acao=$(header access-control-allow-origin)
if [ -z "$acao" ]; then
  pass "CORS preflight from $FOREIGN_ORIGIN refused (HTTP $code, no allow-origin)"
else
  fail "CORS preflight from $FOREIGN_ORIGIN answered allow-origin='$acao'"
fi

# 6. A simple request carrying a foreign Origin is refused outright (API_CONTRACT §4, AT-11).
request GET /api/v1/status -H "Origin: $FOREIGN_ORIGIN"
if [ "$code" = 403 ]; then
  pass "GET with foreign Origin refused (403)"
else
  fail "GET with foreign Origin answered HTTP $code, expected 403"
fi

# 7. Anonymous mutation: 401 unauthorized with a Bearer challenge.
request POST "$MUTATING_PATH" -H 'Content-Type: application/json' --data '{}'
err=$(json_string code)
challenge=$(header www-authenticate)
if [ "$code" = 401 ] && [ "$err" = unauthorized ] && [[ "$challenge" == Bearer* ]]; then
  pass "anonymous POST $MUTATING_PATH refused (401 $err)"
else
  fail "anonymous POST $MUTATING_PATH answered HTTP $code code='$err' www-authenticate='$challenge'"
fi

# 8. A malformed key is rejected even on a read (API_CONTRACT §3: no fallback to the demo workspace).
request GET /api/v1/whoami -H 'Authorization: Bearer bbk_not-a-real-key'
if [ "$code" = 401 ]; then
  pass "malformed bearer key refused (401)"
else
  fail "malformed bearer key answered HTTP $code, expected 401"
fi

if [ "$failures" -gt 0 ]; then
  log "$failures check(s) failed"
  exit 1
fi
log "all checks passed"
