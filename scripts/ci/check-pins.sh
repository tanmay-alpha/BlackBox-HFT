#!/usr/bin/env bash
# SEC-21 (T-SC-3, T-SC-4): every action a workflow uses is pinned to a full 40-hex commit SHA, and
# every Dockerfile base-image ARG is pinned to an index digest. Run from the repository root.
set -euo pipefail

fail=0

# `uses:` values: owner/repo[/path]@<40 hex>, a local `./` action, or `docker://...@sha256:<64 hex>`.
while IFS= read -r hit; do
  file=${hit%%:*}; rest=${hit#*:}; line=${rest%%:*}; text=${rest#*:}
  ref=$(sed -E 's/^[[:space:]-]*uses:[[:space:]]*//; s/[[:space:]]+#.*$//; s/^["'\'']//; s/["'\'']$//' <<<"$text")
  case "$ref" in
    ./*) continue ;;
    docker://*@sha256:*) [[ $ref =~ @sha256:[0-9a-f]{64}$ ]] && continue ;;
    *) [[ $ref =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$ ]] && continue ;;
  esac
  echo "::error file=$file,line=$line::action not pinned to a full commit SHA: $ref"
  fail=1
done < <(git ls-files -- '.github/workflows/*.yml' '.github/workflows/*.yaml' '.github/actions/**' |
  xargs -r grep -nE '^[[:space:]-]*uses:' /dev/null)

# Dockerfile base images: `ARG <NAME>_IMAGE=<image>@sha256:<64 hex>`, and any literal image named
# in `FROM` (a registry path or tag, not an earlier stage or an ARG) carries a digest too.
while IFS= read -r hit; do
  file=${hit%%:*}; rest=${hit#*:}; line=${rest%%:*}; text=${rest#*:}
  if [[ $text =~ ^ARG ]]; then
    [[ $text =~ @sha256:[0-9a-f]{64}([[:space:]]|$) ]] && continue
  else
    image=$(awk '{for (i = 2; i <= NF; i++) if ($i !~ /^--/) { print $i; exit }}' <<<"$text")
    [[ $image == *'$'* || ( $image != *:* && $image != */* ) || $image =~ @sha256:[0-9a-f]{64}$ ]] && continue
  fi
  echo "::error file=$file,line=$line::base image not pinned to a digest: $text"
  fail=1
done < <(git ls-files -- 'Dockerfile' '*/Dockerfile' '*.Dockerfile' |
  xargs -r grep -nE '^(ARG[[:space:]]+[A-Z0-9_]*_IMAGE=|FROM[[:space:]])' /dev/null)

if [[ $fail -ne 0 ]]; then exit 1; fi
echo "pins ok: every action is SHA-pinned and every base image digest-pinned"
