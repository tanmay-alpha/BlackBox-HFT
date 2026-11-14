#!/usr/bin/env bash
# SEC-19: no credential in the repository, in the commits under review, or in the built web bundle.
#
#   scripts/ci/secret-scan.sh repo [<base-rev>]   tracked files, plus every line added by the commits
#                                                 <base-rev>..HEAD when a base is given
#   scripts/ci/secret-scan.sh dist <dir>          a built bundle (web/dist) and the web sources' env use
#
# Run from the repository root. The repository legitimately names the variables (docs, config
# parsing, tests with fake values), so `repo` looks for credential *values*: a Blob token, a Neon
# password, a full-length API key, a Postgres URL with a password, or a credential variable assigned
# a literal value. The local development password `blackbox-local` (docker-compose, tests) and the
# placeholder passwords `password`/`PASSWORD`/`p`/`supersecret` are not secrets. The bundle must not
# even name them (`dist` uses the stricter SEC-19 pattern).
set -euo pipefail

# Credential values.
VALUE_RE='vercel_blob_rw_[A-Za-z0-9]{8,}_[A-Za-z0-9]{16,}'
VALUE_RE+='|npg_[A-Za-z0-9]{10,}'
VALUE_RE+='|bbk_[A-Za-z0-9_-]{43}([^A-Za-z0-9_-]|$)'
VALUE_RE+='|postgres(ql)?://[^:/@[:space:]]+:[^@[:space:]]+@'
# A credential variable assigned a literal, not a reference (`$X`, `${X}`) or a placeholder (`<...>`).
VALUE_RE+='|(BLOB_READ_WRITE_TOKEN|DATABASE_URL[A-Z_]*|BB_API_KEY_PEPPER)[[:space:]]*=[[:space:]]*["'\'']?[^[:space:]"'\''$<{`]'
# Non-secret credentials, replaced before matching.
ALLOWED_URL='postgres(ql)?://[^:/@[:space:]]+:(blackbox-local|password|PASSWORD|p|supersecret)@[^[:space:]"'\''`]*'
# Strict bundle pattern (SECURITY_REVIEW SEC-19).
DIST_RE='vercel_blob_rw_|postgres(ql)?://|npg_|bbk_[A-Za-z0-9_-]{43}|BLOB_READ_WRITE|DATABASE_URL|BB_API_KEY_PEPPER'

# stdin: "<location>:<line>:<text>" lines; prints those still holding a credential value.
leaks() {
  sed -E "s#${ALLOWED_URL}#<allowed-url>#g" | grep -E "${VALUE_RE}" || true
}

case "${1:-}" in
  repo)
    found=$(
      { git ls-files -z | xargs -0 grep -nIE "${VALUE_RE}" -- || true; } | leaks
      if [[ -n "${2:-}" ]]; then
        # Added lines of every commit in the range (a secret added then deleted is still leaked).
        git log -p --no-color --no-ext-diff --format='commit %H' "$2..HEAD" |
          awk '/^commit [0-9a-f]+$/ { c = substr($2, 1, 12); next }
               /^\+\+\+ / { f = substr($0, 7); next }
               /^\+/ { print c "/" f ":added:" substr($0, 2) }' | leaks
      fi
    )
    ;;
  dist)
    dir=${2:?usage: secret-scan.sh dist <dir>}
    test -d "$dir"
    found=$(
      grep -rnoIE "${DIST_RE}" -- "$dir" || true
      # Vite inlines every `import.meta.env.VITE_*` read into the bundle: only the public API base
      # URL may be read, and the env prefix / `define` must stay at Vite's defaults (T-SPA-1).
      grep -rnoE 'import\.meta\.env\.VITE_[A-Z0-9_]+' web/src | grep -v ':import\.meta\.env\.VITE_API_BASE_URL$' || true
      grep -HnE 'envPrefix|loadEnv|define[[:space:]]*:' web/vite.config.ts || true
    )
    ;;
  *)
    echo "usage: $0 repo [<base-rev>] | dist <dir>" >&2
    exit 2
    ;;
esac

if [[ -n "$found" ]]; then
  # Print locations only, never the matched text.
  echo "$found" | cut -d: -f1-2 | sort -u | sed 's/^/::error::possible secret at /'
  exit 1
fi
echo "secret scan ($1) ok"
