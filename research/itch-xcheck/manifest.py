"""Assemble the reproducibility manifest of one itch-validate.sh run.

usage: python manifest.py OUT_DIR
Reads OUT_DIR/context.json and OUT_DIR/stages.jsonl (written by the script), the aggregate
outputs of every stage, and hashes every file in OUT_DIR. Writes OUT_DIR/manifest.json and
prints the verdict. Exit status 0 only when every gating check passed.
The manifest holds aggregates, hashes and command lines only; no event-level content.
"""
import hashlib
import json
import os
import platform
import sys

out = sys.argv[1]


def load(name):
    path = os.path.join(out, name)
    if not os.path.exists(path):
        return None
    with open(path) as f:
        return json.load(f)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def keyvals(name):
    path = os.path.join(out, name)
    if not os.path.exists(path):
        return None
    with open(path) as f:
        text = f.read().split()
    return {k: (int(v) if v.lstrip("-").isdigit() else v)
            for k, _, v in (t.partition("=") for t in text) if _}


def last_z(name):
    """The `Z frames bytes live` record at the end of an adapter output."""
    path = os.path.join(out, name)
    if not os.path.exists(path):
        return None
    with open(path, "rb") as f:
        f.seek(0, 2)
        size = f.tell()
        f.seek(max(0, size - (1 << 16)))
        tail = f.read().decode().splitlines()
    for line in tail:
        if line.startswith("Z "):
            frames, consumed, live = map(int, line.split()[1:])
            return {"frames": frames, "bytes": consumed, "live_orders": live}
    return None


ctx = load("context.json")
stages = []
with open(os.path.join(out, "stages.jsonl")) as f:
    for line in f:
        stages.append(json.loads(line))
report = load("report.json")
activity = load("activity.json")
cmp_ref = load("compare-itchbook.json")
cmp_py = load("compare-python.json")
neg = load("negative-control.json")
pystats = load("pystats.json")
itchbook_stdout = keyvals("itchbook.stdout")
bbadapter_stdout = keyvals("bbadapter.stdout")

m = report["manifest"]
frames = {
    "independent_python_framer": activity["frames"],
    "blackbox_report": m["frames"],
    "itchbook_adapter": (last_z("ref.txt") or {}).get("frames"),
    "bbadapter": (last_z("bb.txt") or {}).get("frames"),
}
consumed = {
    "independent_python_framer": activity["bytes_consumed"],
    "blackbox_report": m["decoded_bytes"],
    "itchbook_adapter": (last_z("ref.txt") or {}).get("bytes"),
    "bbadapter": (last_z("bb.txt") or {}).get("bytes"),
}
checks = {
    "framing_agrees": len(set(frames.values())) == 1 and len(set(consumed.values())) == 1,
    "no_invalid_or_unknown_frames": not m["invalid_by_type"] and not m["unknown_by_type"],
    "itchbook_equals_blackbox": cmp_ref is not None and cmp_ref["unequal_records"] == 0,
    "python_equals_blackbox": cmp_py is not None and cmp_py["unequal_records"] == 0
    and cmp_py["event_records"] > 0,
    "negative_control_exactly_one_mismatch": neg is not None and neg["unequal_records"] == 1,
    "episodes_agree_with_book": report["quality"]["episodes_agree_with_book"],
}
informational = {
    "timestamp_regressions": m["timestamps"]["regressions"],
    "quality_checks_failed": [c["id"] for c in report["quality"]["checks"] if not c["passed"]],
    "reconstruction_checks_failed": [c["id"] for c in m["checks"] if not c["passed"]],
}

skip = {"manifest.json"}
outputs = {}
for root, _, files in os.walk(out):
    for name in sorted(files):
        path = os.path.join(root, name)
        rel = os.path.relpath(path, out).replace(os.sep, "/")
        if rel in skip:
            continue
        outputs[rel] = {"bytes": os.path.getsize(path), "sha256": sha256(path)}


def summary(c):
    if c is None:
        return None
    return {k: c[k] for k in ("records", "unequal_records", "records_by_kind", "equal_by_kind",
                              "match_rate_by_kind", "unequal_by_first_field", "event_records",
                              "event_bbo_equal", "event_bbo_match_rate", "sampled_events",
                              "sample_seed", "sampled_full_depth5_equal")}


manifest = {
    "schema": "blackbox.itch-validate.manifest/1",
    "license_note": m["license_note"],
    "input": ctx["input"],
    "truncation": {
        "decompressed_bytes": activity["bytes_consumed"] + activity["trailing_bytes"],
        "complete_frames": activity["frames"],
        "bytes_in_complete_frames": activity["bytes_consumed"],
        "trailing_partial_frame_bytes": activity["trailing_bytes"],
        "blackbox_stop": m["stop"],
        "last_timestamp_ns": m["timestamps"]["last_ns"],
    },
    "parameters": ctx["parameters"],
    "tools": ctx["tools"],
    "platform": dict(ctx["platform"], python_platform=platform.platform()),
    "stages": stages,
    "outputs": outputs,
    "results": {
        "frames": frames,
        "bytes_in_complete_frames": consumed,
        "messages_by_type": m["messages_by_type"],
        "timestamps": m["timestamps"],
        "system_events": m["system_events"],
        "book": m["book"],
        "itchbook_vs_blackbox": summary(cmp_ref),
        "python_vs_blackbox": summary(cmp_py),
        "negative_control": summary(neg),
        "itchbook_stats": itchbook_stdout,
        "bbadapter_stats": bbadapter_stdout,
        "python_reference_stats": pystats,
        "window_aggregates": cmp_ref and cmp_ref["window_aggregates"],
        "quality": report["quality"],
        "metrics": report["metrics"],
    },
    "checks": checks,
    "informational": informational,
    "passed": all(checks.values()),
}
with open(os.path.join(out, "manifest.json"), "w", newline="\n") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
for k, v in checks.items():
    print(f"{'PASS' if v else 'FAIL'} {k}")
print(f"frames {frames}")
print("passed" if manifest["passed"] else "FAILED")
sys.exit(0 if manifest["passed"] else 1)
