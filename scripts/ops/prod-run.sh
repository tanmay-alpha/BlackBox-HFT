#!/usr/bin/env bash
# Runs ONE production maintenance command with production credentials that exist only in that
# command's environment (docs/OPERATOR_PRODUCTION.md).
#
#   scripts/ops/prod-run.sh <env-file> <command> [args...]
#
#   scripts/ops/prod-run.sh data/secrets/prod-ops.env \
#       target/release/blackbox --data-dir data/ops keys mint --workspace lab --scopes read,run,admin --label lead
#   scripts/ops/prod-run.sh data/secrets/prod-ops.env \
#       sh -c 'pg_dump --format=custom --schema=bb --file "data/secrets/backups/bb-$(date -u +%F).dump" "$DATABASE_URL_UNPOOLED"'
#
# <env-file> holds KEY=VALUE lines (blank lines and `#` comments ignored, an optional `export `
# prefix and one layer of matching quotes are stripped; no shell expansion, nothing is executed).
# The file is read in a subshell: the variables are exported to <command> only, never to the
# calling shell, and no value is printed. Production credentials already present in the caller's
# environment are dropped first, so the file is the only source. BB_ALLOW_CLOUD_CREDENTIALS=true is
# set for the child so a local-profile `blackbox` accepts them for this one command; the variable
# is never set anywhere else.
set -euo pipefail
set +x

die() {
    echo "prod-run: $*" >&2
    exit 2
}

[ $# -ge 2 ] || die "usage: scripts/ops/prod-run.sh <env-file> <command> [args...]"
file=$1
shift
[ -f "$file" ] && [ -r "$file" ] || die "$file is not a readable file"

# Owner-only permissions where POSIX modes mean something (not NTFS under Git Bash/MSYS/Cygwin).
case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN*) ;;
*)
    mode=$(stat -c %a "$file" 2>/dev/null || stat -f %Lp "$file")
    case "$mode" in
    *00) ;;
    *) die "$file is readable by group or others (mode $mode); run: chmod 600 $file" ;;
    esac
    ;;
esac

(
    unset BLOB_READ_WRITE_TOKEN DATABASE_URL DATABASE_URL_UNPOOLED BB_API_KEY_PEPPER \
        BB_ADMIN_BOOTSTRAP_KEY_SHA256 BB_ALLOW_CLOUD_CREDENTIALS
    names=()
    lineno=0
    while IFS= read -r line || [ -n "$line" ]; do
        lineno=$((lineno + 1))
        line=${line%$'\r'}
        line=${line#"${line%%[![:space:]]*}"}
        case "$line" in
        '' | '#'*) continue ;;
        esac
        line=${line#export }
        key=${line%%=*}
        if [[ "$line" != *=* || ! "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
            die "$file:$lineno is not a KEY=VALUE line"
        fi
        value=${line#*=}
        if [ ${#value} -ge 2 ]; then
            first=${value:0:1}
            last=${value: -1}
            if [[ ("$first" == '"' || "$first" == "'") && "$last" == "$first" ]]; then
                value=${value:1:${#value}-2}
            fi
        fi
        export "$key=$value"
        names+=("$key")
    done <"$file"
    [ ${#names[@]} -gt 0 ] || die "$file defines no variables"
    export BB_ALLOW_CLOUD_CREDENTIALS=true
    echo "prod-run: running $1 with ${names[*]} from $file (values not shown)" >&2
    exec "$@"
)
