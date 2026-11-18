"""Minimal independent ITCH 5.0 reference for ONE symbol, written from the public specification
(Nasdaq TotalView-ITCH 5.0, sections 1.3-1.6) without reading BlackBox or itch-book code paths.
Interprets R, A, F, E, C, X, D, U (P is a print and never changes the displayed book).

usage: python py_ref.py RAW SYMBOL T0_NS T1_NS OUT.txt STATS.json
OUT.txt uses the adapters' `E` record format (BBO + top-5 depth after every book event of the
symbol inside [T0, T1)). STATS.json holds aggregates only. Local use only.
"""
import heapq
import json
import sys
from collections import Counter

raw_path, symbol, t0, t1, out_path, stats_path = sys.argv[1:7]
t0, t1 = int(t0), int(t1)
data = open(raw_path, "rb").read()
target = None
orders = {}  # ref -> [is_buy, price, shares]
levels = {True: {}, False: {}}  # side -> price -> [shares, orders]
kinds = Counter()
removed = Counter()  # shares removed from the touch, by cause, inside the window
touch_moves = 0
prints_in_window = Counter()


def add(ref, buy, price, shares):
    orders[ref] = [buy, price, shares]
    lv = levels[buy].setdefault(price, [0, 0])
    lv[0] += shares
    lv[1] += 1


def reduce(ref, shares, cause, in_window):
    o = orders[ref]
    buy, price = o[0], o[1]
    side = levels[buy]
    if in_window and price == (max(side) if buy else min(side)):
        removed[cause] += shares
    o[2] -= shares
    lv = side[price]
    lv[0] -= shares
    if o[2] == 0:
        del orders[ref]
        lv[1] -= 1
    if lv[1] == 0:
        del side[price]


def top(buy):
    keys = heapq.nlargest(5, levels[buy]) if buy else heapq.nsmallest(5, levels[buy])
    return "".join(f" {p} {levels[buy][p][0]} {levels[buy][p][1]}" for p in keys)


be = int.from_bytes
want = symbol.encode().ljust(8)
pos, seq, prev_best = 0, 0, None
with open(out_path, "w", newline="\n") as out:
    while pos + 2 <= len(data):
        ln = be(data[pos:pos + 2], "big")
        if pos + 2 + ln > len(data):
            break
        f = data[pos + 2:pos + 2 + ln]
        pos += 2 + ln
        seq += 1
        t = f[0]
        if t == 0x52 and f[11:19] == want:  # R: stock directory
            target = f[1:3]
        if target is None or f[1:3] != target or t not in b"AFECXDU":
            if t == 0x50 and f[1:3] == target:  # P: non-displayed execution
                ts = be(f[5:11], "big")
                if t0 <= ts < t1:
                    prints_in_window["P"] += 1
            continue
        ts = be(f[5:11], "big")
        win = t0 <= ts < t1
        ref = be(f[11:19], "big")
        if t in b"AF":  # 1.3.1 / 1.3.2: side 19, shares 20-23, stock 24-31, price 32-35
            add(ref, f[19] == 0x42, be(f[32:36], "big"), be(f[20:24], "big"))
        elif t in b"EC":  # 1.4.1 / 1.4.2: executed shares 19-22
            reduce(ref, be(f[19:23], "big"), "execution", win)
            if win:
                prints_in_window["E" if t == 0x45 else ("C" if f[31] == 0x59 else "C-nonprintable")] += 1
        elif t == ord("X"):  # 1.4.3: cancelled shares 19-22
            reduce(ref, be(f[19:23], "big"), "partial_cancel", win)
        elif t == ord("D"):  # 1.4.4
            reduce(ref, orders[ref][2], "delete", win)
        elif t == ord("U"):  # 1.4.5: new ref 19-26, shares 27-30, price 31-34; same side
            buy = orders[ref][0]
            reduce(ref, orders[ref][2], "replace", win)
            add(be(f[19:27], "big"), buy, be(f[31:35], "big"), be(f[27:31], "big"))
        if not win:
            continue
        kinds[chr(t)] += 1
        bids, asks = levels[True], levels[False]
        bp = max(bids) if bids else 0
        ap = min(asks) if asks else 0
        best = (bp, ap)
        touch_moves += prev_best is not None and best != prev_best
        prev_best = best
        out.write(
            f"E {seq} {ts} {be(target, 'big')} {bp} {bids[bp][0] if bids else 0} {ap} "
            f"{asks[ap][0] if asks else 0} |{top(True)} |{top(False)} |\n"
        )

json.dump({
    "symbol": symbol,
    "window_ns": [t0, t1],
    "frames_read": seq,
    "book_events_in_window_by_type": dict(kinds),
    "prints_in_window": dict(prints_in_window),
    "touch_shares_removed_by_cause": dict(removed),
    "touch_price_changes": touch_moves,
    "live_orders_of_symbol_at_end": len(orders),
}, open(stats_path, "w"), indent=1)
print(open(stats_path).read())
