"""Negative control: copy an adapter output with exactly one share changed.

usage: python perturb.py IN.txt OUT.txt
Adds one share to the best-bid size (field `bid_shares`) of the middle `E` record (index
floor(n/2) of n, zero-based) and prints the record number and field it changed. The comparison
of OUT against the BlackBox output must then report exactly one unequal record. Local use only.
"""
import sys

src, dst = sys.argv[1:3]
with open(src) as f:
    n_events = sum(1 for line in f if line.startswith("E "))
if n_events == 0:
    sys.exit("no E records to perturb")
target = n_events // 2
seen = 0
with open(src) as f, open(dst, "w", newline="\n") as out:
    for n, line in enumerate(f, 1):
        line = line.rstrip("\r\n")
        if line.startswith("E "):
            if seen == target:
                tokens = line.split(" ")
                tokens[5] = str(int(tokens[5]) + 1)
                line = " ".join(tokens)
                print(f"perturbed record {n} (E record {target} of {n_events}): bid_shares +1")
            seen += 1
        out.write(line + "\n")
