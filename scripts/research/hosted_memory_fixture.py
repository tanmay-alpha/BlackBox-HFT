#!/usr/bin/env python3
"""Helpers of the hosted full-process memory fixture (scripts/research/hosted-memory.sh,
docs/rebuild/HOSTED_MEMORY.md). Standard library only; runs inside throwaway containers.

  blob   A local fake of the Vercel Blob API subset `VercelBlobStore` uses (PUT with ?pathname=,
         ranged GET, HEAD, POST /delete) over HTTPS with a fixture-only certificate. It answers
         under the network aliases `vercel.com` and `<store>.private.blob.vercel-storage.com`, so
         the hosted image runs its real Blob client unmodified. No real Blob store is contacted.
  drive  Shares the server container's PID namespace, samples the `blackbox serve` process's
         /proc status (VmRSS, VmHWM, RssAnon, RssFile, threads) every SAMPLE_MS, and drives the
         measured phases over HTTP. Prints one JSON document (the result) on stdout.

Nothing here contains or reads a real credential: the Blob token, API-key pepper and API key
are generated per fixture run by the shell script.
"""

import hashlib
import http.client
import http.server
import json
import os
import random
import socketserver
import ssl
import sys
import threading
import time
import urllib.parse

# --------------------------------------------------------------------------------------------
# Fake Blob store.


def blob() -> None:
    store = os.environ["BLOB_STORE_ID"].lower()
    root = "/tmp/blob"
    os.makedirs(root, exist_ok=True)

    def path_of(key: str) -> str:
        return os.path.join(root, hashlib.sha256(key.encode()).hexdigest())

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):  # quiet
            pass

        def reply(self, code, body=b"", headers=()):
            self.send_response(code)
            for name, value in headers:
                self.send_header(name, value)
            if not any(name.lower() == "content-length" for name, _ in headers):
                self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if body and self.command != "HEAD":
                self.wfile.write(body)

        def drain(self) -> bytes:
            n = int(self.headers.get("Content-Length") or 0)
            return self.rfile.read(n) if n else b""

        def object_key(self) -> str:
            return urllib.parse.unquote(urllib.parse.urlsplit(self.path).path.lstrip("/"))

        def do_PUT(self):
            url = urllib.parse.urlsplit(self.path)
            key = urllib.parse.parse_qs(url.query).get("pathname", [""])[0]
            if url.path.rstrip("/") != "/api/blob" or not key:
                self.drain()
                return self.reply(404)
            remaining = int(self.headers.get("Content-Length") or 0)
            target = path_of(key)
            exists = os.path.exists(target)
            digest = hashlib.md5()
            with open(target + ".part", "wb") as out:
                while remaining:
                    chunk = self.rfile.read(min(remaining, 1 << 20))
                    if not chunk:
                        break
                    remaining -= len(chunk)
                    digest.update(chunk)
                    out.write(chunk)
            if exists and self.headers.get("x-allow-overwrite") == "0":
                os.remove(target + ".part")
                return self.reply(409, b'{"error":{"code":"conflict"}}')
            os.replace(target + ".part", target)
            body = json.dumps(
                {
                    "pathname": key,
                    "url": f"https://{store}.private.blob.vercel-storage.com/"
                    + urllib.parse.quote(key),
                    "etag": f'"{digest.hexdigest()}"',
                }
            ).encode()
            self.reply(200, body, [("Content-Type", "application/json")])

        def object(self):
            target = path_of(self.object_key())
            if not os.path.exists(target):
                self.reply(404)
                return None
            return target

        def do_HEAD(self):
            target = self.object()
            if target:
                size = os.path.getsize(target)
                self.reply(200, b"", [("Content-Length", str(size)), ("ETag", '"x"')])

        def do_GET(self):
            target = self.object()
            if not target:
                return
            size = os.path.getsize(target)
            spec = self.headers.get("Range", "")
            if not spec.startswith("bytes="):
                with open(target, "rb") as f:
                    return self.reply(200, f.read())
            first, last = spec[len("bytes=") :].split("-")
            first, last = int(first), min(int(last), size - 1)
            if first > last:
                return self.reply(416)
            with open(target, "rb") as f:
                f.seek(first)
                data = f.read(last - first + 1)
            self.reply(
                206,
                data,
                [("Content-Range", f"bytes {first}-{last}/{size}"), ("Content-Encoding", "identity")],
            )

        def do_POST(self):
            body = self.drain()
            if urllib.parse.urlsplit(self.path).path.rstrip("/") != "/api/blob/delete":
                return self.reply(404)
            for url in json.loads(body or b"{}").get("urls", []):
                key = urllib.parse.unquote(urllib.parse.urlsplit(url).path.lstrip("/"))
                try:
                    os.remove(path_of(key))
                except FileNotFoundError:
                    pass
            self.reply(200, b"{}", [("Content-Type", "application/json")])

    class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
        daemon_threads = True

    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain("/fixture/blob.crt", "/fixture/blob.key")
    server = Server(("0.0.0.0", 443), Handler)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    print("fake blob listening on :443", flush=True)
    server.serve_forever()


# --------------------------------------------------------------------------------------------
# Driver.

BASE_HOST = os.environ.get("APP_HOST", "app")
BASE_PORT = int(os.environ.get("APP_PORT", "8787"))
SAMPLE_S = int(os.environ.get("SAMPLE_MS", "20")) / 1000.0
CLIENTS = int(os.environ.get("CLIENTS", "32"))
LOAD_S = float(os.environ.get("LOAD_S", "180"))
LOAD_SERVED = int(os.environ.get("LOAD_SERVED", "600"))
KEY = os.environ.get("BB_FIXTURE_KEY", "")


def log(*args):
    print(*args, file=sys.stderr, flush=True)


def request(method, path, key=None, body=None, ip=None, timeout=180):
    """Returns (status, body bytes); (0, b"") on a transport error."""
    headers = {"Accept": "application/json"}
    if key:
        headers["Authorization"] = f"Bearer {key}"
    if ip:
        headers["X-Forwarded-For"] = ip
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    conn = http.client.HTTPConnection(BASE_HOST, BASE_PORT, timeout=timeout)
    try:
        conn.request(method, path, body=data, headers=headers)
        response = conn.getresponse()
        return response.status, response.read()
    except (OSError, http.client.HTTPException):
        return 0, b""
    finally:
        conn.close()


def get_json(path, key=None, ip=None):
    status, body = request("GET", path, key=key, ip=ip)
    if status != 200:
        raise RuntimeError(f"GET {path}: HTTP {status}: {body[:300]!r}")
    return json.loads(body)


def server_pid() -> int:
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/cmdline", "rb") as f:
                argv = f.read().split(b"\0")
        except OSError:
            continue
        if argv and argv[0].endswith(b"blackbox") and b"serve" in argv:
            return int(entry)
    raise RuntimeError("no `blackbox serve` process in the shared PID namespace")


class Memory:
    FIELDS = ("VmRSS", "VmHWM", "RssAnon", "RssFile", "Threads")

    def __init__(self, pid):
        self.pid = pid
        self.lock = threading.Lock()
        self.samples = []  # (t, rss_kib, anon_kib, cgroup_bytes or None)
        self.phase_max = {}
        self.phase = None
        self.stop = threading.Event()
        self.cgroup_dir = f"/proc/{pid}/root/sys/fs/cgroup"
        self.t0 = time.monotonic()

    def status(self):
        values = {}
        with open(f"/proc/{self.pid}/status") as f:
            for line in f:
                name, _, rest = line.partition(":")
                if name in self.FIELDS:
                    values[name] = int(rest.split()[0])
        return values

    def cgroup(self, name):
        try:
            with open(f"{self.cgroup_dir}/{name}") as f:
                return int(f.read().split()[0])
        except (OSError, ValueError):
            return None

    def cgroup_stat(self):
        """`anon` and `file` (page cache, e.g. bundles written to the disk cache) of the
        server's cgroup in MiB; the page cache counts against the container limit but is
        reclaimable, unlike the process's anonymous memory."""
        out = {}
        try:
            with open(f"{self.cgroup_dir}/memory.stat") as f:
                for line in f:
                    name, _, value = line.partition(" ")
                    if name in ("anon", "file"):
                        out[f"cgroup_{name}_mib"] = round(int(value) / 1048576.0, 1)
        except (OSError, ValueError):
            pass
        return out

    def reset_hwm(self) -> bool:
        try:
            with open(f"/proc/{self.pid}/clear_refs", "w") as f:
                f.write("5")
            return True
        except OSError:
            return False

    def run(self):
        while not self.stop.is_set():
            try:
                s = self.status()
            except OSError:
                break
            if "VmRSS" not in s:  # the server exited (zombie)
                break
            cg = self.cgroup("memory.current")
            t = time.monotonic() - self.t0
            with self.lock:
                self.samples.append((round(t, 3), s["VmRSS"], s["RssAnon"], cg))
                if self.phase:
                    m = self.phase_max.setdefault(self.phase, [0, 0])
                    m[0] = max(m[0], s["VmRSS"])
                    m[1] = max(m[1], cg or 0)
            time.sleep(SAMPLE_S)


def mib(kib):
    return round(kib / 1024.0, 1)


class Phase:
    def __init__(self, memory, results, name):
        self.memory, self.results, self.name = memory, results, name

    def __enter__(self):
        m = self.memory
        self.hwm_reset = m.reset_hwm()
        self.start = m.status()
        self.t = time.monotonic()
        with m.lock:
            m.phase = self.name
        log(f"phase {self.name}: start RSS {mib(self.start['VmRSS'])} MiB")
        return self

    def __exit__(self, *exc):
        m = self.memory
        time.sleep(2 * SAMPLE_S)
        end = m.status()
        with m.lock:
            m.phase = None
            sampled, cg = m.phase_max.get(self.name, [0, 0])
        record = {
            "seconds": round(time.monotonic() - self.t, 1),
            "start_rss_mib": mib(self.start["VmRSS"]),
            "end_rss_mib": mib(end["VmRSS"]),
            "sampled_peak_rss_mib": mib(max(sampled, end["VmRSS"])),
            "vmhwm_mib": mib(end["VmHWM"]) if self.hwm_reset else None,
            "end_rss_anon_mib": mib(end["RssAnon"]),
            "end_rss_file_mib": mib(end["RssFile"]),
            "end_threads": end["Threads"],
            "sampled_peak_cgroup_mib": round(cg / 1048576.0, 1) if cg else None,
        }
        record.update(m.cgroup_stat())
        record.update(getattr(self, "extra", {}))
        self.results[self.name] = record
        log(f"phase {self.name}: {json.dumps(record)}")
        return False


def anonymous_reads(rng):
    """Distinct heavy anonymous URIs over the seeded demo workspace."""
    runs = get_json("/api/v1/runs?limit=100")["items"]
    comparisons = get_json("/api/v1/comparisons?limit=20")["items"]
    datasets = get_json("/api/v1/datasets")
    datasets = datasets.get("items", datasets) if isinstance(datasets, dict) else datasets
    market = get_json("/api/v1/market/datasets/l3-demo")
    events = int(market["events"])
    urls = []
    spans = {}
    for d in datasets:
        detail = get_json(f"/api/v1/datasets/{d['id']}")
        manifest = (detail.get("current") or detail).get("manifest") or detail.get("manifest") or {}
        start, end = int(manifest.get("start", 0) or 0), int(manifest.get("end", 0) or 0)
        spans[d["id"]] = (start, end)
        for _ in range(40):
            if end > start:
                t0 = rng.randrange(start, max(start + 1, end - 5_000_000_000))
                urls.append(
                    f"/api/v1/datasets/{d['id']}/replay?t0_ns={t0}&t1_ns={t0 + 5_000_000_000}"
                    f"&step_ns={rng.choice([50, 100, 250])}000000&levels=10&max_trades=500"
                )
    for r in runs:
        rid = r["run_id"] if "run_id" in r else r["id"]
        base = f"/api/v1/runs/{rid}"
        urls += [
            base,
            base + "/analytics",
            base + "/report",
            base + "/engine-check",
            base + "/equity",
            base + "/fills",
            base + "/orders",
            base + "/tape?limit=500",
            base + "/timeline",
        ]
        start, end = spans.get(r.get("dataset_id") or "", (0, 0))
        for _ in range(8):
            if end > start:
                t0 = rng.randrange(start, end - 2_000_000_000)
                urls.append(f"{base}/timeline?t0_ns={t0}&t1_ns={t0 + 2_000_000_000}")
            urls.append(f"{base}/tape?limit=500&from_seq={rng.randrange(0, 50_000)}")
    for c in comparisons:
        base = f"/api/v1/comparisons/{c['id']}"
        urls += [base, base + "/pairs?limit=200", base + "/report"]
        for cls in ("matched", "a_only", "b_only"):
            urls.append(f"{base}/pairs?limit=100&class={cls}")
    for _ in range(400):
        index = rng.randrange(0, events)
        urls.append(
            f"/api/v1/market/datasets/l3-demo/events/{index}?levels={rng.randint(5, 10)}"
            f"&orders_per_level=100"
        )
        urls.append(f"/api/v1/market/datasets/l3-demo/book?event_index={index}&levels=20&orders_per_level=100")
    urls += [
        "/api/v1/market/datasets/l3-demo/examples",
        "/api/v1/market/datasets/l3-demo/comparison",
        "/api/v1/market/datasets/l3-demo/metrics?max_points=500",
    ]
    return urls, runs, comparisons


class Load:
    """CLIENTS threads issue random distinct anonymous heavy reads, each from a new forwarded
    client address (many visitors; the global anonymous limit still applies). Stops after
    `seconds`, on `stop`, or once `served` requests were answered by a handler (anything but a
    transport error or a 429 rate refusal), so runs on slower or busier machines do the same
    amount of heavy work."""

    def __init__(self, urls, seconds, seed, served=None):
        self.urls, self.seconds, self.seed, self.target = urls, seconds, seed, served
        self.statuses = {}
        self.bytes = 0
        self.requests = 0
        self.served = 0
        self.lock = threading.Lock()
        self.threads = []
        self.stop = threading.Event()

    def worker(self, n):
        rng = random.Random(self.seed * 1000 + n)
        deadline = time.monotonic() + self.seconds
        while time.monotonic() < deadline and not self.stop.is_set():
            ip = f"10.{rng.randrange(256)}.{rng.randrange(256)}.{rng.randrange(1, 255)}"
            status, body = request("GET", rng.choice(self.urls), ip=ip, timeout=60)
            with self.lock:
                self.statuses[status] = self.statuses.get(status, 0) + 1
                self.bytes += len(body)
                self.requests += 1
                if status not in (0, 429):
                    self.served += 1
                    if self.target and self.served >= self.target:
                        self.stop.set()
            if status == 429:
                time.sleep(0.05)

    def start(self):
        for n in range(CLIENTS):
            t = threading.Thread(target=self.worker, args=(n,), daemon=True)
            t.start()
            self.threads.append(t)
        return self

    def join(self):
        for t in self.threads:
            t.join()
        return {
            "clients": CLIENTS,
            "requests": self.requests,
            "served": self.served,
            "statuses": {str(k): v for k, v in sorted(self.statuses.items())},
            "body_mib": round(self.bytes / 1048576.0, 1),
        }


def job_config(preset_config, candidate, seed):
    """`candidate` = (dataset, preset, interval_ns, offset_ns, max_queue_updates_per_order)."""
    config = json.loads(json.dumps(preset_config))
    config["strategy"]["interval_ns"] = candidate[2]
    config["record"]["sample_every_ticks"] = 1
    config["record"]["max_queue_updates_per_order"] = candidate[4]
    config["execution"]["feed_latency_offset_ns"] = candidate[3]
    config["seed"] = seed
    return config


def parse_candidates(text):
    """`none` skips the job phases (3 to 5)."""
    out = []
    for entry in text.split():
        if entry == "none":
            continue
        dataset, preset, interval, offset, cap = entry.split("/")
        out.append((dataset, preset, int(interval), int(offset), int(cap)))
    return out


def submit(config, label):
    status, body = request("POST", "/api/v1/experiments", key=KEY, body={"config": config, "label": label})
    if status not in (200, 202):
        raise RuntimeError(f"submit {label}: HTTP {status}: {body[:400]!r}")
    return json.loads(body)


def wait_job(job_id, until, timeout=1800):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        job = get_json(f"/api/v1/jobs/{job_id}", key=KEY)
        if job["state"] in until:
            return job
        time.sleep(0.1)
    raise RuntimeError(f"job {job_id} did not reach {until}")


def drive() -> None:
    candidates = parse_candidates(os.environ["JOB_CANDIDATES"])
    rng = random.Random(20261118)

    deadline = time.monotonic() + 300
    while request("GET", "/readyz", timeout=5)[0] != 200:
        if time.monotonic() > deadline:
            raise RuntimeError("server never became ready")
        time.sleep(0.5)
    memory = Memory(server_pid())
    sampler = threading.Thread(target=memory.run, daemon=True)
    sampler.start()
    results = RESULTS
    results.update({"phases": {}, "hwm_reset_supported": memory.reset_hwm()})
    phases = results["phases"]
    time.sleep(3)
    results["boot"] = {k: (mib(v) if k != "Threads" else v) for k, v in memory.status().items()}
    # Also in driver.log: an OOM kill of the server ends the shared PID namespace, so the driver
    # dies with it and result.json stays empty; the per-phase log lines are what remains.
    log(f"boot: {json.dumps(results['boot'])}")

    preset_configs = {}
    for dataset in sorted({c[0] for c in candidates}):
        for p in get_json(f"/api/v1/presets?dataset_id={dataset}", key=KEY):
            preset_configs[(dataset, p["id"])] = p["config"]

    # 1. Cache fill: one client touches every heavy read once (lazy market demo, dataset
    #    regeneration, parsed-run cache, demo snapshot, anonymous response cache).
    urls, runs, comparisons = anonymous_reads(rng)
    results["read_set"] = {"uris": len(urls), "runs": len(runs), "comparisons": len(comparisons)}
    with Phase(memory, phases, "1_cache_fill_sequential") as p:
        statuses = {}
        for i, url in enumerate(urls[::7]):
            status, _ = request("GET", url, ip=f"10.1.{i // 250}.{i % 250 + 1}")
            statuses[status] = statuses.get(status, 0) + 1
        p.extra = {"requests": len(urls[::7]), "statuses": {str(k): v for k, v in statuses.items()}}
    time.sleep(5)

    # 2. Concurrent anonymous heavy reads alone.
    with Phase(memory, phases, "2_concurrent_anonymous_reads") as p:
        p.extra = Load(urls, LOAD_S, 1, LOAD_SERVED).start().join()
    time.sleep(5)

    if candidates:
        job_phases(memory, phases, results, candidates, preset_configs, urls)

    # 6. Recovery: idle past the 30 s anonymous response-cache lifetime.
    with Phase(memory, phases, "6_idle_recovery") as p:
        time.sleep(float(os.environ.get("RECOVERY_S", "40")))
        p.extra = {}

    memory.stop.set()
    sampler.join()
    final = memory.status()
    results["process_lifetime_vmhwm_note"] = (
        "VmHWM is reset per phase when clear_refs is writable; the per-phase vmhwm_mib values "
        "are exact peaks of the server process, sampled values are every SAMPLE_MS"
    )
    results["final"] = {k: (mib(v) if k != "Threads" else v) for k, v in final.items()}
    results["cgroup"] = {
        "memory.peak_mib": round((memory.cgroup("memory.peak") or 0) / 1048576.0, 1),
        "memory.current_mib": round((memory.cgroup("memory.current") or 0) / 1048576.0, 1),
    }
    results["sampled_overall_peak_rss_mib"] = mib(max(s[1] for s in memory.samples))
    results["samples"] = len(memory.samples)
    # Downsampled series (t, RSS MiB) for the document's plot-free summary.
    step = max(1, len(memory.samples) // 400)
    results["series"] = [(s[0], mib(s[1])) for s in memory.samples[::step]]
    results["summary"] = summarize(results)
    log("summary: " + json.dumps(results["summary"]))
    print(json.dumps(results, indent=1))


def job_phases(memory, phases, results, candidates, preset_configs, urls):
    # 3. Every candidate job alone, one after another (the largest admitted shape of every
    #    preset on every seeded dataset); the one with the highest peak is the largest job.
    scan = []
    for n, c in enumerate(candidates):
        name = f"3_job_{n:02d}_{c[0]}_{c[1]}"
        with Phase(memory, phases, name) as p:
            config = job_config(preset_configs[(c[0], c[1])], c, 1)
            status, body = request("POST", "/api/v1/experiments", key=KEY,
                                   body={"config": config, "label": f"hosted-memory: {c[0]}/{c[1]}"})
            if status not in (200, 202):
                # Recorded, not fatal: the submission itself is the finding (see the document).
                p.extra = {"candidate": "/".join(map(str, c)), "job_state": f"submit_http_{status}",
                           "error": body[:300].decode(errors="replace")}
                continue
            submitted = json.loads(body)
            job = wait_job(submitted["job"]["id"], {"completed", "failed", "cancelled", "expired"})
            run = {}
            if job["state"] == "completed":
                detail = get_json(f"/api/v1/runs/{job['run_id']}", key=KEY)
                run = {"ticks": detail.get("ticks"), "tape_len": detail.get("tape_len"),
                       "bundle_bytes": (detail.get("bundle") or {}).get("size_bytes")}
            p.extra = {"candidate": "/".join(map(str, c)), "job_state": job["state"],
                       "error": job.get("error"), "estimate": submitted["experiment"].get("estimate"),
                       "run_id": job.get("run_id"), "run": run}
        scan.append((phase_peak(phases[name]), name, c, job))
        time.sleep(2)
    scan.sort(key=lambda s: -s[0])
    worst = scan[0]
    completed = [s for s in scan if s[3]["state"] == "completed"]
    results["largest_job"] = {"phase": worst[1], "peak_mib": worst[0],
                              "candidate": "/".join(map(str, worst[2]))}
    largest = worst[2]
    largest_config = preset_configs[(largest[0], largest[1])]
    time.sleep(3)

    # 4. Everything at once: the largest job runs again (new seed, so a new run identity) while
    #    anonymous heavy reads hit every demo route and the keyed client reads the largest
    #    completed run whole (timeline, analytics, report, pages); once the job ends, the keyed
    #    client compares the two largest runs (both decoded at once) while the load continues.
    run_a = completed[0][3]["run_id"] if completed else None
    with Phase(memory, phases, "4_job_plus_concurrent_reads") as p:
        load = Load(urls, 3600, 2).start()
        time.sleep(1)
        submitted = submit(job_config(largest_config, largest, 2), "hosted-memory: largest again")
        keyed = {}

        def keyed_reads():
            for path in ("", "/analytics", "/report", "/timeline", "/equity", "/fills", "/orders",
                         "/tape?limit=5000"):
                status, _ = request("GET", f"/api/v1/runs/{run_a}{path}", key=KEY)
                keyed[path or "/"] = status

        reader = threading.Thread(target=keyed_reads)
        if run_a:
            reader.start()
        job_b = wait_job(submitted["job"]["id"], {"completed", "failed", "cancelled", "expired"})
        if run_a:
            reader.join()
        run_b = job_b.get("run_id") if job_b["state"] == "completed" else None
        run_b = run_b or (completed[1][3]["run_id"] if len(completed) > 1 else None)
        compare = {"run_a": run_a, "run_b": run_b}
        if run_a and run_b:
            status, body = request("POST", "/api/v1/comparisons", key=KEY,
                                   body={"run_a": run_a, "run_b": run_b, "label": "hosted-memory largest pair"})
            compare["create"] = status
            if status in (200, 201):
                cid = json.loads(body)["id"]
                for path in ("", "/pairs?limit=1000", "/report"):
                    compare[path or "/"] = request("GET", f"/api/v1/comparisons/{cid}{path}", key=KEY)[0]
        load.stop.set()
        p.extra = {"load": load.join(), "job_state": job_b["state"], "job_error": job_b.get("error"),
                   "keyed_reads": keyed, "compare": compare}
    time.sleep(5)

    # 5. Cancellation of a running largest job; nothing may be stored.
    with Phase(memory, phases, "5_cancel_running_job") as p:
        submitted = submit(job_config(largest_config, largest, 3), "hosted-memory: cancelled")
        job_id = submitted["job"]["id"]
        running = wait_job(job_id, {"running", "completed", "failed", "cancelled", "expired"})
        # Let the simulation build up state before cancelling it.
        time.sleep(float(os.environ.get("CANCEL_AFTER_S", "0.5")))
        cancel_status = request("POST", f"/api/v1/jobs/{job_id}/cancel", key=KEY)[0]
        final = wait_job(job_id, {"completed", "failed", "cancelled", "expired"})
        p.extra = {"state_before_cancel": running["state"], "cancel_http": cancel_status,
                   "final_state": final["state"], "run_id": final.get("run_id"),
                   "error": final.get("error"), "progress": final.get("progress")}
    time.sleep(2)


def phase_peak(p):
    """Peak RSS of a phase in MiB: the exact VmHWM when it could be reset, else sampled."""
    return max(p["vmhwm_mib"] or 0, p["sampled_peak_rss_mib"])


def summarize(results):
    """The figures docs/rebuild/HOSTED_MEMORY.md reports, all in MiB of the server process
    (None for phases that did not run)."""
    phases = results["phases"]

    def peak(name):
        return phase_peak(phases[name]) if name in phases else None

    def above_start(name):
        return round(peak(name) - phases[name]["start_rss_mib"], 1) if name in phases else None

    def end(name):
        return phases[name]["end_rss_mib"] if name in phases else None

    jobs = [name for name in phases if name.startswith("3_job_")]
    largest = results.get("largest_job", {})
    summary = {
        "rss_peak_mib": max(phase_peak(p) for p in phases.values()),
        "boot_rss_mib": results["boot"]["VmRSS"],
        "cache_contribution_mib": round(
            end("1_cache_fill_sequential") - results["boot"]["VmRSS"], 1
        ),
        "cache_fill_peak_mib": peak("1_cache_fill_sequential"),
        "concurrent_reads_peak_mib": peak("2_concurrent_anonymous_reads"),
        "concurrent_overhead_mib": above_start("2_concurrent_anonymous_reads"),
        "largest_job": largest.get("candidate"),
        "simulation_peak_mib": peak(largest["phase"]) if largest else None,
        "simulation_increment_mib": above_start(largest["phase"]) if largest else None,
        "jobs_completed": sum(phases[n]["job_state"] == "completed" for n in jobs),
        "jobs_measured": len(jobs),
        "combined_peak_mib": peak("4_job_plus_concurrent_reads"),
        "combined_increment_mib": above_start("4_job_plus_concurrent_reads"),
        "cancel_peak_mib": peak("5_cancel_running_job"),
        "after_cancel_rss_mib": end("5_cancel_running_job"),
        "recovered_rss_mib": end("6_idle_recovery"),
        "recovered_above_pre_jobs_mib": (
            round(end("6_idle_recovery") - phases[jobs[0]]["start_rss_mib"], 1) if jobs else None
        ),
        "cgroup_memory_peak_mib": results["cgroup"]["memory.peak_mib"],
    }
    return summary


# Whatever the driver measured so far; printed even when a phase fails.
RESULTS = {}

if __name__ == "__main__":
    if sys.argv[1] == "blob":
        blob()
    else:
        try:
            drive()
        except Exception as error:  # noqa: BLE001 - report partial measurements, then fail
            RESULTS["error"] = f"{type(error).__name__}: {error}"
            print(json.dumps(RESULTS, indent=1))
            raise
