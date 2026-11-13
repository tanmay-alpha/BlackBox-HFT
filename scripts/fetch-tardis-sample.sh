#!/usr/bin/env bash
# Downloads one day of Tardis.dev sample data (trades + incremental L2 book) for local use.
#
# Tardis.dev publishes these files; BlackBox does not redistribute them. Read the vendor's terms
# before using the data, and never commit or publish anything under data/raw or data/datasets
# (both are git-ignored). See docs/OPEN_SOURCE_RESEARCH.md.
#
# Usage: scripts/fetch-tardis-sample.sh [exchange] [symbol] [YYYY-MM-DD]
# Defaults: binance-futures BTCUSDT 2020-02-01 (the book file is about 250 MB).
set -euo pipefail

exchange="${1:-binance-futures}"
symbol="${2:-BTCUSDT}"
day="${3:-2020-02-01}"

if [[ ! "$day" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
  echo "date must be YYYY-MM-DD, got: $day" >&2
  exit 2
fi
# Both become path components and URL segments: no separators, dots-only names or URL syntax.
if [[ ! "$exchange" =~ ^[a-z0-9][a-z0-9-]*$ ]]; then
  echo "exchange must match [a-z0-9][a-z0-9-]*, got: $exchange" >&2
  exit 2
fi
if [[ ! "$symbol" =~ ^[A-Za-z0-9][A-Za-z0-9_-]*$ ]]; then
  echo "symbol must match [A-Za-z0-9][A-Za-z0-9_-]*, got: $symbol" >&2
  exit 2
fi
ymd="${day//-//}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/data/raw/$exchange/$symbol/$day"
mkdir -p "$out"

for kind in trades incremental_book_L2; do
  url="https://datasets.tardis.dev/v1/$exchange/$kind/$ymd/$symbol.csv.gz"
  dest="$out/$kind.csv.gz"
  if [[ -s "$dest" ]]; then
    echo "exists, skipping: $dest"
  else
    echo "downloading $url"
    # --fail: a 401/404 (day not available without an API key) must not leave an error page
    # behind that would later be mistaken for data.
    curl --fail --location --retry 3 --silent --show-error -o "$dest.part" "$url"
    # Only a complete, valid gzip stream is promoted to the final name.
    gzip -t "$dest.part"
    mv "$dest.part" "$dest"
  fi
  # A truncated download fails this integrity test instead of failing later during ingestion.
  gzip -t "$dest"
  sha256sum "$dest"
done

cat <<EOF

Saved to: $out
Next:
  blackbox ingest-tardis \\
    --trades "$out/trades.csv.gz" \\
    --book "$out/incremental_book_L2.csv.gz" \\
    --id ${exchange}-$(echo "$symbol" | tr '[:upper:]' '[:lower:]')-${day} \\
    --exchange $exchange --symbol $symbol \\
    --tick-size <tick> --lot-size <lot> --max-duration-s 600
EOF
