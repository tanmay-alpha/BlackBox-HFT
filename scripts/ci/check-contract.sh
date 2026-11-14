#!/usr/bin/env bash
# AT-14 contract drift: the committed OpenAPI document (web/openapi.json) is exactly what the
# server generates, and the committed TypeScript (web/src/api/v1.gen.ts) is exactly what
# `npm run gen:api` generates from it. Either difference fails.
#
#   scripts/ci/check-contract.sh <path to blackbox binary>
#
# Run from the repository root after `npm ci` in web/. Fix a failure with
#   blackbox openapi --out web/openapi.json && (cd web && npm run gen:api)
set -euo pipefail

bin=${1:?usage: check-contract.sh <blackbox binary>}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

"$bin" openapi --out "$tmp/openapi.json"
if ! diff -u web/openapi.json "$tmp/openapi.json" > "$tmp/openapi.diff"; then
  head -n 200 "$tmp/openapi.diff"
  echo "::error file=web/openapi.json::OpenAPI drift: regenerate with 'blackbox openapi --out web/openapi.json'"
  exit 1
fi

cp web/src/api/v1.gen.ts "$tmp/v1.gen.ts"
(cd web && npm run --silent gen:api)
if ! diff -u "$tmp/v1.gen.ts" web/src/api/v1.gen.ts > "$tmp/ts.diff"; then
  head -n 200 "$tmp/ts.diff"
  echo "::error file=web/src/api/v1.gen.ts::TypeScript drift: run 'npm run gen:api' in web/ and commit the result"
  exit 1
fi
echo "contract ok: web/openapi.json and web/src/api/v1.gen.ts match the code"
