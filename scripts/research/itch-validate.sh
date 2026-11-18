#!/usr/bin/env bash
# Reproducible validation of a NASDAQ TotalView-ITCH 5.0 file against two independent
# implementations. See docs/research/ITCH_VALIDATION_PIPELINE.md.
#
# usage: scripts/research/itch-validate.sh ITCH_FILE SYMBOL WINDOW OUT_DIR
#   ITCH_FILE  raw BinaryFILE (2-byte length-prefixed) or its gzip (a truncated prefix is fine)
#   SYMBOL     Stock Directory symbol, e.g. QQQ
#   WINDOW     exchange time HH:MM:SS-HH:MM:SS (nanoseconds since midnight ET if all digits:
#              T0-T1); messages with T0 <= ts < T1 are compared and measured
#   OUT_DIR    created if needed; inside this repository it must be under data/ (git-ignored),
#              because it receives event-level files derived from NASDAQ data
#
# Environment: PYTHON (python), CXX (g++), ITCH_BOOK_DIR (data/itch/tools/itch-book; cloned at
# the pinned commit when absent), CARGO_TARGET_DIR (target/), SAMPLE_EVERY (1000000),
# SAMPLE_N (1000), MIDNIGHT_EPOCH_NS (0; recorded only), KEEP_EVENT_FILES (1).
#
# Exit status: 0 when every gating check in manifest.json passed, 1 when one failed, 2 on usage
# or tool errors.
set -euo pipefail

ITCH_BOOK_URL=https://github.com/groovg/itch-book
ITCH_BOOK_PIN=b502b002abf650760b79c010b3ae3a8f82896ae8
# itch-book's only dependency, pinned by its CMakeLists.txt at that commit (FetchContent).
FIXED_DECIMAL_COMMIT=a25e15801e32977df9abbea6a1819eecc9befaa9
FIXED_DECIMAL_URL=https://github.com/groovg/fixed-decimal/archive/$FIXED_DECIMAL_COMMIT.tar.gz
FIXED_DECIMAL_SHA256=aedd636c8ba9874f7fdeda5b7e7d08f4a6a38c9cb0cc330153865bfe7202d5fa

die() { echo "itch-validate: $*" >&2; exit 2; }
[ $# -eq 4 ] || die "usage: $0 ITCH_FILE SYMBOL WINDOW OUT_DIR"

# Absolute paths in the form native tools accept (D:/... under Git Bash, /... elsewhere).
abspath() { (cd "$1" && { pwd -W 2>/dev/null || pwd; }); }
REPO=$(abspath "$(dirname "$0")/../..")
XC="$REPO/research/itch-xcheck"
INPUT=$1 SYMBOL=$2 WINDOW=$3 OUT=$4
PY=${PYTHON:-python}
CXX=${CXX:-g++}
ITCH_BOOK_DIR=${ITCH_BOOK_DIR:-$REPO/data/itch/tools/itch-book}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$REPO/target}
SAMPLE_EVERY=${SAMPLE_EVERY:-1000000}
SAMPLE_N=${SAMPLE_N:-1000}
MIDNIGHT_EPOCH_NS=${MIDNIGHT_EPOCH_NS:-0}
KEEP_EVENT_FILES=${KEEP_EVENT_FILES:-1}

[ -f "$INPUT" ] || die "no such file: $INPUT"
mkdir -p "$OUT"
OUT=$(abspath "$OUT")
case "$OUT/" in
"$REPO/data/"*) ;;
"$REPO/"*) die "OUT_DIR inside the repository must be under data/ (git-ignored): $OUT" ;;
esac
mkdir -p "$OUT/bin"
: >"$OUT/stages.jsonl"

read -r T0 T1 < <("$PY" - "$WINDOW" <<'EOF'
import re, sys
w = sys.argv[1]
m = re.fullmatch(r"(\d+)-(\d+)", w)
if m:
    t0, t1 = int(m.group(1)), int(m.group(2))
else:
    m = re.fullmatch(r"(\d\d):(\d\d):(\d\d(?:\.\d{1,9})?)-(\d\d):(\d\d):(\d\d(?:\.\d{1,9})?)", w)
    if not m:
        sys.exit("WINDOW must be HH:MM:SS-HH:MM:SS or T0_NS-T1_NS")
    def ns(h, mi, s):
        whole, _, frac = s.partition(".")
        return ((int(h) * 60 + int(mi)) * 60 + int(whole)) * 10**9 + int(frac.ljust(9, "0") or 0)
    t0, t1 = ns(*m.group(1, 2, 3)), ns(*m.group(4, 5, 6))
if not t0 < t1:
    sys.exit("WINDOW must have T0 < T1")
print(t0, t1)
EOF
)
T0=${T0%$'\r'} T1=${T1%$'\r'}

# Runs one stage: records its command line, exit status and wall time in stages.jsonl.
stage() {
  local name=$1 out=$2
  shift 2
  local start end status=0
  start=$(date +%s.%N)
  echo "[$name] $*" >&2
  if [ "$out" = - ]; then "$@" || status=$?; else "$@" >"$out" || status=$?; fi
  end=$(date +%s.%N)
  "$PY" - "$name" "$status" "$start" "$end" "$REPO" "$OUT" "$@" >>"$OUT/stages.jsonl" <<'EOF'
import json, sys
name, status, start, end, repo, out, *argv = sys.argv[1:]
rel = lambda a: a.replace(out, "$OUT").replace(repo, "$REPO")
print(json.dumps({"stage": name, "argv": [rel(a) for a in argv], "exit": int(status),
                  "seconds": round(float(end) - float(start), 3)}))
EOF
  return "$status"
}

# ---- 0. tools -----------------------------------------------------------------------------
if [ ! -d "$ITCH_BOOK_DIR/.git" ]; then
  mkdir -p "$(dirname "$ITCH_BOOK_DIR")"
  git clone --quiet "$ITCH_BOOK_URL" "$ITCH_BOOK_DIR"
  git -C "$ITCH_BOOK_DIR" -c advice.detachedHead=false checkout --quiet "$ITCH_BOOK_PIN"
fi
HEAD_SHA=$(git -C "$ITCH_BOOK_DIR" rev-parse HEAD)
[ "$HEAD_SHA" = "$ITCH_BOOK_PIN" ] || die "itch-book at $HEAD_SHA, expected $ITCH_BOOK_PIN"
[ -z "$(git -C "$ITCH_BOOK_DIR" status --porcelain)" ] || die "itch-book checkout is modified"
grep -q "$FIXED_DECIMAL_COMMIT" "$ITCH_BOOK_DIR/CMakeLists.txt" ||
  die "itch-book CMakeLists.txt no longer pins fixed-decimal $FIXED_DECIMAL_COMMIT"
FD_DIR="$ITCH_BOOK_DIR-deps/fixed-decimal-$FIXED_DECIMAL_COMMIT"
FD_TGZ="$FD_DIR.tar.gz"
mkdir -p "$FD_DIR"
[ -f "$FD_TGZ" ] || curl -fsSL -o "$FD_TGZ" "$FIXED_DECIMAL_URL"
FD_SHA=$(sha256sum "$FD_TGZ" | cut -d' ' -f1)
[ "$FD_SHA" = "$FIXED_DECIMAL_SHA256" ] || die "fixed-decimal archive SHA-256 $FD_SHA, expected $FIXED_DECIMAL_SHA256"
[ -f "$FD_DIR/cpp/include/fixed_decimal.hpp" ] ||
  (cd "$FD_DIR" && tar -xzf "../$(basename "$FD_TGZ")" --strip-components=1)

stage build-itchbook - "$CXX" -std=c++23 -O2 -static -I"$ITCH_BOOK_DIR/include" \
  -I"$FD_DIR/cpp/include" "$XC/itchbook_adapter.cpp" -o "$OUT/bin/itchbook_adapter.exe"
(cd "$REPO" && stage build-blackbox - cargo build --release --locked --quiet \
  --manifest-path "$XC/bbadapter/Cargo.toml")
cp "$CARGO_TARGET_DIR/release/bbadapter.exe" "$CARGO_TARGET_DIR/release/bbreport.exe" "$OUT/bin/" 2>/dev/null ||
  cp "$CARGO_TARGET_DIR/release/bbadapter" "$CARGO_TARGET_DIR/release/bbreport" "$OUT/bin/"
BB="$OUT/bin/bbadapter"; [ -f "$BB.exe" ] && BB="$BB.exe"
BR="$OUT/bin/bbreport"; [ -f "$BR.exe" ] && BR="$BR.exe"

# ---- 1. input -----------------------------------------------------------------------------
GZIP_STATUS=null GZIP_MESSAGE=""
if [ "$(head -c 2 "$INPUT" | od -An -tx1 | tr -d ' \n')" = 1f8b ]; then
  RAW="$OUT/input.raw"
  set +e
  gzip -dc "$INPUT" >"$RAW" 2>"$OUT/gzip.stderr"
  GZIP_STATUS=$?
  set -e
  GZIP_MESSAGE=$(tr -d '\r' <"$OUT/gzip.stderr" | head -c 400)
  # A cut gzip prefix exits 1 ("unexpected end of file"); anything else is an error.
  [ "$GZIP_STATUS" -le 1 ] || die "gzip failed ($GZIP_STATUS): $GZIP_MESSAGE"
else
  RAW="$INPUT"
fi

# ---- 2. framing (independent Python framer) -----------------------------------------------
stage framing "$OUT/activity.json" "$PY" "$XC/activity.py" "$RAW"

# ---- 3. decode, timestamps, sequence, quality report, metrics -----------------------------
stage report "$OUT/report.stdout" "$BR" "$RAW" "$SYMBOL" "$T0" "$T1" "$OUT/report.json" \
  "$MIDNIGHT_EPOCH_NS"

# ---- 4. reconstruction by three implementations -------------------------------------------
stage itchbook "$OUT/itchbook.stdout" "$OUT/bin/itchbook_adapter.exe" "$RAW" "$OUT/ref.txt" 0 \
  "$T0" "$T1" "$SAMPLE_EVERY" "$SYMBOL"
stage bbadapter "$OUT/bbadapter.stdout" "$BB" "$RAW" "$OUT/bb.txt" 0 "$T0" "$T1" \
  "$SAMPLE_EVERY" "$SYMBOL"
stage python-reference "$OUT/pyref.stdout" "$PY" "$XC/py_ref.py" "$RAW" "$SYMBOL" "$T0" "$T1" \
  "$OUT/py.txt" "$OUT/pystats.json"

# ---- 5. three-way comparison ----------------------------------------------------------------
stage compare-itchbook "$OUT/compare-itchbook.stdout" "$PY" "$XC/compare.py" "$OUT/ref.txt" \
  "$OUT/bb.txt" "$OUT/compare-itchbook.json" --sample "$SAMPLE_N" \
  --mismatches "$OUT/mismatches-itchbook.json"
stage compare-python "$OUT/compare-python.stdout" "$PY" "$XC/compare.py" "$OUT/py.txt" \
  "$OUT/bb.txt" "$OUT/compare-python.json" --only E --sample "$SAMPLE_N" \
  --mismatches "$OUT/mismatches-python.json"

# ---- 6. negative control ------------------------------------------------------------------
stage perturb "$OUT/perturb.stdout" "$PY" "$XC/perturb.py" "$OUT/ref.txt" "$OUT/ref-perturbed.txt"
stage negative-control "$OUT/negative-control.stdout" "$PY" "$XC/compare.py" \
  "$OUT/ref-perturbed.txt" "$OUT/bb.txt" "$OUT/negative-control.json" --sample "$SAMPLE_N" \
  --mismatches "$OUT/mismatches-negative-control.json"
rm -f "$OUT/ref-perturbed.txt"
[ "$KEEP_EVENT_FILES" = 1 ] || rm -f "$OUT/ref.txt" "$OUT/bb.txt" "$OUT/py.txt"

# ---- 7. reproducibility manifest ------------------------------------------------------------
version() { "$@" 2>&1 | head -n 1 | tr -d '\r'; }
REPO_COMMIT=$(git -C "$REPO" rev-parse HEAD)
REPO_DIRTY=$(git -C "$REPO" status --porcelain -- crates research scripts | wc -l | tr -d ' ')
"$PY" - "$OUT/context.json" <<EOF
import json, sys, hashlib, os
def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for c in iter(lambda: f.read(1 << 20), b""):
            h.update(c)
    return h.hexdigest()
inp, raw = r"""$INPUT""", r"""$RAW"""
ctx = {
  "input": {
    "name": os.path.basename(inp),
    "bytes": os.path.getsize(inp),
    "sha256": sha(inp),
    "compression": "gzip" if raw != inp else "none",
    "gzip_exit_status": $GZIP_STATUS,
    "gzip_message": r"""$GZIP_MESSAGE""".strip() or None,
    "decompressed_bytes": os.path.getsize(raw),
    "decompressed_sha256": sha(raw) if raw != inp else None,
  },
  "parameters": {
    "symbol": "$SYMBOL", "window": "$WINDOW", "window_ns": [$T0, $T1],
    "clock": "ITCH exchange timestamp, nanoseconds since midnight Eastern",
    "sample_every_messages": $SAMPLE_EVERY, "sample_events": $SAMPLE_N, "sample_seed": 20261117,
    "midnight_epoch_ns": $MIDNIGHT_EPOCH_NS,
    "markout_horizons_ns": [100000000, 1000000000, 5000000000], "ofi_bucket_ns": 1000000000,
  },
  "tools": {
    "blackbox_commit": "$REPO_COMMIT",
    "blackbox_uncommitted_paths": $REPO_DIRTY,
    "itch_book": {"url": "$ITCH_BOOK_URL", "commit": "$HEAD_SHA", "licence": "MIT"},
    "fixed_decimal": {"url": "$FIXED_DECIMAL_URL", "commit": "$FIXED_DECIMAL_COMMIT",
                      "archive_sha256": "$FD_SHA", "licence": "MIT"},
    "rustc": r"""$(version rustc --version)""",
    "cargo": r"""$(version cargo --version)""",
    "cxx": r"""$(version "$CXX" --version)""",
    "python": r"""$(version "$PY" --version)""",
    "gzip": r"""$(version gzip --version)""",
    "git": r"""$(version git --version)""",
  },
  "platform": {"uname": r"""$(uname -srm)"""},
}
json.dump(ctx, open(sys.argv[1], "w", newline="\n"), indent=2)
EOF
[ "$RAW" = "$INPUT" ] || [ "$KEEP_EVENT_FILES" = 1 ] || rm -f "$RAW"
set +e
"$PY" "$XC/manifest.py" "$OUT"
status=$?
set -e
echo "manifest: $OUT/manifest.json" >&2
exit "$status"
