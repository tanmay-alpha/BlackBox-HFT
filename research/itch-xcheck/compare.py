"""Compare two adapter outputs record by record; write aggregates and an exact mismatch report.

usage: python compare.py REFERENCE.txt BLACKBOX.txt SUMMARY.json
                         [--sample N] [--only KINDS] [--mismatches MISMATCHES.json]

Records are the text lines of itchbook_adapter.cpp / bbadapter / py_ref.py (`T`, `L`, `E`, `C`,
`Z`). Lines are paired by position after optionally keeping only record kinds in KINDS (for
example `E` for the Python reference, which only writes `E` records). Equality is exact on the
text after removing the line ending.

SUMMARY.json holds counts and match rates, plus window aggregates computed from the BlackBox `E`
records. MISMATCHES.json (only written when given) lists the first 100 unequal pairs with the
first differing field: it is event-level and must stay with the local data (data/itch/).
Local use only: both inputs are event-level.
"""
import argparse
import json
import random
import statistics
from collections import Counter
from itertools import zip_longest

FIELDS = {
    "E": ["kind", "seq", "ts_ns", "locate", "bid_price4", "bid_shares", "ask_price4", "ask_shares"],
    "T": ["kind", "seq", "ts_ns", "locate", "source", "side", "price4", "shares", "match", "cross"],
    "L": ["kind", "seq", "locate", "side", "price4", "shares", "orders"],
    "Z": ["kind", "frames", "bytes", "live_orders"],
    "C": ["kind", "type", "count"],
}
MAX_MISMATCHES = 100


def field_name(kind, tokens, index):
    names = FIELDS.get(kind, [])
    if index < len(names):
        return names[index]
    if kind == "E":
        # After the BBO: "|", bid levels as (price4, shares, orders), "|", ask levels, "|".
        side, pos = None, 0
        for j in range(len(names), min(index, len(tokens) - 1) + 1):
            if tokens[j] == "|":
                side, pos = ("bid" if side is None else "ask"), 0
                if j == index:
                    return "separator"
                continue
            if j == index:
                return f"{side}{pos // 3 + 1}_{('price4', 'shares', 'orders')[pos % 3]}"
            pos += 1
    return f"field{index}"


def records(path, only):
    with open(path) as f:
        for line in f:
            line = line.rstrip("\r\n")
            if only is None or line[:1] in only:
                yield line


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("reference")
    ap.add_argument("blackbox")
    ap.add_argument("summary")
    ap.add_argument("--sample", type=int, default=1000)
    ap.add_argument("--only")
    ap.add_argument("--mismatches")
    a = ap.parse_args()
    only = set(a.only) if a.only else None

    kinds, equal, bbo_equal = Counter(), Counter(), Counter()
    unequal_by_field = Counter()
    mismatches = []
    events = []  # (equal, BlackBox E tokens), in memory for aggregates only
    counts, z = {}, {}
    n = 0
    for n, (ra, rb) in enumerate(zip_longest(records(a.reference, only), records(a.blackbox, only)), 1):
        ra, rb = ra or "", rb or ""
        kind = (ra or rb)[:1]
        kinds[kind] += 1
        same = ra == rb
        if same:
            equal[kind] += 1
        else:
            ta, tb = ra.split(), rb.split()
            idx = next((i for i, (x, y) in enumerate(zip_longest(ta, tb)) if x != y), 0)
            name = field_name(kind, tb if len(tb) >= len(ta) else ta, idx)
            unequal_by_field[f"{kind}.{name}"] += 1
            if len(mismatches) < MAX_MISMATCHES:
                mismatches.append({
                    "record": n,
                    "kind": kind,
                    "message_seq": (ta[1:2] or tb[1:2] or [None])[0] if kind in "ETL" else None,
                    "first_differing_field": name,
                    "field_index": idx,
                    "reference_value": ta[idx] if idx < len(ta) else None,
                    "blackbox_value": tb[idx] if idx < len(tb) else None,
                    "reference_record": ra,
                    "blackbox_record": rb,
                })
        if kind == "E":
            pa, pb = ra.split("|")[0].split(), rb.split("|")[0].split()
            bbo_equal["E"] += pa == pb
            events.append((same, pb))
        elif kind == "C" and rb:
            p = rb.split()
            counts[chr(int(p[1]))] = int(p[2])
        elif kind == "Z" and rb:
            z = dict(zip(["frames", "bytes", "live_orders"], map(int, rb.split()[1:])))

    rng = random.Random(20261117)
    idx = sorted(rng.sample(range(len(events)), min(a.sample, len(events)))) if events else []
    sample_full = sum(events[i][0] for i in idx)

    spreads, touch = [], []
    two_sided = same_ns = bbo_changes = 0
    prev_ts, prev_bbo = None, None
    for _, p in events:
        if len(p) < 8:
            continue
        ts = int(p[2])
        bbo = tuple(map(int, p[4:8]))
        bid_px, bid_sz, ask_px, ask_sz = bbo
        same_ns += ts == prev_ts
        bbo_changes += prev_bbo is not None and bbo != prev_bbo
        prev_ts, prev_bbo = ts, bbo
        if bid_px and ask_px:
            two_sided += 1
            spreads.append(ask_px - bid_px)
            touch.append(bid_sz + ask_sz)
    spreads.sort()

    def q(xs, f):
        return xs[min(len(xs) - 1, int(f * len(xs)))] if xs else None

    total = sum(kinds.values())
    unequal = total - sum(equal.values())
    summary = {
        "records": total,
        "unequal_records": unequal,
        "records_by_kind": dict(sorted(kinds.items())),
        "equal_by_kind": dict(sorted(equal.items())),
        "match_rate_by_kind": {k: equal[k] / kinds[k] for k in sorted(kinds)},
        "unequal_by_first_field": dict(sorted(unequal_by_field.items())),
        "event_records": len(events),
        "event_bbo_equal": bbo_equal["E"],
        "event_bbo_match_rate": bbo_equal["E"] / len(events) if events else None,
        "sampled_events": len(idx),
        "sample_seed": 20261117,
        "sampled_full_depth5_equal": sample_full,
        "message_counts": dict(sorted(counts.items())),
        "end": z,
        "window_aggregates": {
            "two_sided_events": two_sided,
            "spread_price4_p10_p50_p90": [q(spreads, 0.1), q(spreads, 0.5), q(spreads, 0.9)],
            "spread_price4_mean": statistics.fmean(spreads) if spreads else None,
            "touch_shares_median": statistics.median(touch) if touch else None,
            "locked_or_crossed_events": sum(1 for s in spreads if s <= 0),
            "events_sharing_previous_event_ns": same_ns,
            "events_changing_bbo_price_or_size": bbo_changes,
        },
    }
    with open(a.summary, "w", newline="\n") as f:
        json.dump(summary, f, indent=2)
        f.write("\n")
    if a.mismatches:
        with open(a.mismatches, "w", newline="\n") as f:
            json.dump({"unequal_records": unequal, "first": mismatches}, f, indent=2)
            f.write("\n")
    print(f"records={total} unequal={unequal} events={len(events)} "
          f"sampled_equal={sample_full}/{len(idx)}")
    for m in mismatches[:5]:
        print(f"  mismatch record {m['record']} kind {m['kind']} seq {m['message_seq']}: "
              f"{m['first_differing_field']} reference={m['reference_value']} "
              f"blackbox={m['blackbox_value']}")


if __name__ == "__main__":
    main()
