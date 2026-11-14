#!/usr/bin/env bash
# Source policy greps (SECURITY_REVIEW §16, SEC-24 and SEC-30). Run from the repository root.
#
# SEC-24: no process spawning or dynamic loading in non-test Rust code.
# SEC-30: SQL is built only from literals. sqlx 0.9 accepts only `&'static str` SQL unless it is
#   wrapped in `AssertSqlSafe`, so the compiler already rejects a runtime string; this check keeps
#   `AssertSqlSafe` inside `bb-db`, requires every `format!` there to interpolate only UPPER_CASE
#   constants or the `&'static str` fragments returned by `sort_sql` (`keyset`, `order`), and bans
#   the string-pushing builders `QueryBuilder` and `raw_sql`.
set -euo pipefail

fail=0
report() { echo "::error::$1"; fail=1; }

# Non-test Rust sources: crates/*/src, minus `tests` directories.
mapfile -t src < <(git ls-files -- 'crates/*/src/*.rs' 'crates/*/src/**/*.rs' | grep -v '/tests/')

if hits=$(grep -nE 'process::Command|Command::new|libloading|dlopen|LoadLibrary' -- "${src[@]}"); then
  report "SEC-24: process spawn or dynamic loading outside tests:"$'\n'"$hits"
fi

if hits=$(grep -nE 'QueryBuilder|raw_sql' -- "${src[@]}"); then
  report "SEC-30: string-built SQL (QueryBuilder/raw_sql):"$'\n'"$hits"
fi

if hits=$(printf '%s\n' "${src[@]}" | grep -v '^crates/bb-db/src/' | xargs -r grep -nE 'AssertSqlSafe' --); then
  report "SEC-30: AssertSqlSafe outside crates/bb-db/src:"$'\n'"$hits"
fi

# Every `format!(...)` in a bb-db file that uses AssertSqlSafe: interpolations must be constants.
for f in $(printf '%s\n' "${src[@]}" | grep '^crates/bb-db/src/' | xargs -r grep -lE 'AssertSqlSafe' --); do
  bad=$(awk -v F="$f" '
    /format!\(/ { inside = 1; depth = 0; start = NR; buf = "" }
    inside {
      buf = buf $0 "\n"
      n = split($0, ch, "")
      for (i = 1; i <= n; i++) { if (ch[i] == "(") depth++; else if (ch[i] == ")") depth-- }
      if (depth <= 0) {
        inside = 0
        while (match(buf, /\{[^{}]*\}/)) {
          name = substr(buf, RSTART + 1, RLENGTH - 2); buf = substr(buf, RSTART + RLENGTH)
          sub(/:.*/, "", name)
          if (name !~ /^[A-Z][A-Z0-9_]*$/ && name != "keyset" && name != "order")
            print F ":" start ": format! interpolates {" name "}"
        }
      }
    }' "$f")
  [[ -z "$bad" ]] || report "SEC-30: SQL built from a non-constant:"$'\n'"$bad"
done

if [[ $fail -ne 0 ]]; then exit 1; fi
echo "code policy ok (SEC-24 no spawn/dynamic loading, SEC-30 literal SQL)"
