"""Aggregate scan of a raw ITCH 5.0 file (2-byte length framing): book events per symbol,
first/last timestamp, system events and per-type counts. Independent of the BlackBox decoder.

usage: python activity.py RAW [MAX_MESSAGES] > activity.json
Prints aggregates only.
"""
import json
import sys
from collections import Counter

path = sys.argv[1]
limit = int(sys.argv[2]) if len(sys.argv) > 2 else 0
data = open(path, "rb").read()
pos, n = 0, 0
types = Counter()
book = Counter()
regular = Counter()
symbols = {}
system = []
first_ts = last_ts = None
OPEN, CLOSE = 34_200 * 10**9, 57_600 * 10**9
while pos + 2 <= len(data) and (limit == 0 or n < limit):
    ln = int.from_bytes(data[pos:pos + 2], "big")
    if pos + 2 + ln > len(data):
        break
    f = data[pos + 2:pos + 2 + ln]
    pos += 2 + ln
    n += 1
    t = f[0:1]
    types[t] += 1
    loc = int.from_bytes(f[1:3], "big")
    ts = int.from_bytes(f[5:11], "big")
    first_ts = ts if first_ts is None else first_ts
    last_ts = ts
    if t in b"AFECXDU":
        book[loc] += 1
        if OPEN <= ts < CLOSE:
            regular[loc] += 1
    elif t == b"R":
        symbols[loc] = f[11:19].decode("ascii").strip()
    elif t == b"S":
        system.append((chr(f[11]), ts))


def hms(ns):
    s = ns / 1e9
    return f"{int(s // 3600):02d}:{int(s % 3600 // 60):02d}:{s % 60:06.3f}"


print(json.dumps({
    "frames": n,
    "bytes_consumed": pos,
    "trailing_bytes": len(data) - pos,
    "first_ts": hms(first_ts), "last_ts": hms(last_ts), "last_ts_ns": last_ts,
    "system_events": [(c, hms(ts)) for c, ts in system],
    "types": {k.decode(): v for k, v in sorted(types.items())},
    "directory_entries": len(symbols),
    "symbols_with_book_events": len(book),
    "top_by_book_events": [(symbols.get(l, l), c) for l, c in book.most_common(15)],
    "top_by_regular_hours_book_events": [(symbols.get(l, l), c) for l, c in regular.most_common(15)],
}, indent=1))
