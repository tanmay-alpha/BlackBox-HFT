# QuantReplay exchange-lab evaluation (M8, optional)

Should BlackBox add an interactive exchange simulator next to its historical replay? This page records a
local proof of concept with [Quod-Financial/quantreplay](https://github.com/Quod-Financial/quantreplay)
(Apache-2.0, source `39111b63`, 2026-10-20). Licence and reuse context:
[REUSE_MATRIX.md §4](REUSE_MATRIX.md#4-quantreplay-build-and-poc-feasibility) and
[OSS_LICENSES_AND_PROVENANCE.md](OSS_LICENSES_AND_PROVENANCE.md).

**Decision: no-go for product integration. Keep it as reference only.** The PoC worked, and with a
fixed seed it gave identical runs. But the order flow it generates is random and synthetic, and it is
paced by the wall clock. It does not model fees. It needs Docker, its own Postgres and a
Liquibase migration step, plus a FIX client. It cannot run on the hosted 512 MB tier. Details are in §6.

Everything below is simulated. No real exchange or real money was involved, and no market impact
was measured on any real market.

## 1. What was run

All PoC files live outside the repository, in `D:/bbwt-research/quantreplay-poc/`. Nothing was added
to BlackBox code.

| File | Purpose |
|---|---|
| `docker-compose.yml` | Isolated compose project `bbqr-poc`. Services: simulator `XETRA`, its Postgres, a one-shot Liquibase, and a one-shot `poc-setup` psql step. REST and FIX are bound to `127.0.0.1:19050/19051`; Postgres is not published |
| `setup.sql` | Makes one synthetic venue with one instrument: `order_on_startup=false` on venue `XETRA`, and every demo listing except `VOW` disabled |
| `client.py` | Dependency-free FIXT.1.1 client over a socket (about 150 lines, standard library only) |
| `run.py` | One bounded run: fresh volumes, start, `docker stats`, client, logs, `down -v` |
| `compare.py` | Normalises two raw FIX logs and compares them (SHA-256 per phase, first difference) |
| `out/run{1,2,3}.*` | Raw FIX logs, normalised traces, metrics, simulator logs, FIX session store |

The images are `ghcr.io/quod-financial/{deploy_marketsim,postgres_marketsim,liquibase}:latest`, at digests
`cb46204be6f9…`, `5f18a11e5837…` and `bce77d1719d2…`. The simulator reports version
`12.1792505299_39111b63(2026-10-20 17:08:19 +0300)`. The venue and instrument come from the upstream
demo data: "XETRA" is a synthetic venue name, and `VOW` has price seeds bid 98 / mid 100 / offer 102.

Each run used the same script:

1. Log on with FIX (`35=A`, `141=Y`) and subscribe to full-book market data for `VOW` (`35=V`, `263=1`).
2. Send 4 GTC limit orders: buy 100 @ 99, buy 50 @ 98, sell 30 @ 101, then sell 80 @ 99. The last one crosses.
3. Send 3 cancels (`35=F`): the buy remainder at 99, the buy at 98 and the sell at 101.
4. Start the seeded random order generator with `POST /api/genstart {"seed": "<seed>"}` and let it run for
   6 s, then call `POST /api/genstop`. Log off.

### Observed matching and market data (run 1)

These execution reports are from `out/run1.raw.log`, with header tags trimmed:

```
35=8 11=b1 150=0 39=0 38=100 44=99 151=100                  new, rests
35=8 11=s2 150=F 39=2 31=99 32=80 14=80 151=0 6=99          aggressor filled 80 @ 99
35=8 11=b1 150=F 39=1 31=99 32=80 14=80 151=20 6=99         resting buy partially filled
35=8 11=c1 41=b1 150=4 39=4 14=80 151=0                     remainder cancelled
35=W 269=2 270=99 271=80 2446=2 | 269=0 99x20 | 269=0 98x50 | 269=1 101x30   trade + book after match
```

- Matching is price-time. The fill priced at the resting order's limit (99), not the aggressor's limit.
  The trade went out on market data with the aggressor side (`2446=2`, sell).
- All cancels were acknowledged (`150=4`), and the book returned to empty (`268=0`).
- After `genstart`, generated parties (`288=CP1`, `289=CP2`) placed quotes around the price seed and
  traded with each other: 12 market-data messages in 6 s with seed `blackbox-42`.
- Execution reports carry no fee or commission field. The FIX Rules of Engagement list none either
  (`docs/FIXRulesOfEngagement/TradeMessages`).

## 2. Measurements

Measured on this machine: Docker Desktop 4.96.3, Engine 29.9.2 (linux/amd64, WSL2), 24 vCPU,
7.6 GiB given to the VM. Each `docker stats` figure is a single `--no-stream` sample, so treat it as
an order of magnitude, not a benchmark.

| Measure | Result |
|---|---|
| Image size (uncompressed, `docker image inspect`) | simulator 168.4 MB; Postgres 636.3 MB; Liquibase 439.6 MB; **1.24 GB in total**. The simulator is about 45.9 MB compressed (REUSE_MATRIX §4) |
| Pull time (sequential, first pull) | 30.8 s, 62.1 s and 85.3 s (about 3 min) |
| Startup, cached images (`compose up --wait` → REST `/api/venuestatus` answers) | 25.0 s, 24.6 s, 18.4 s. This includes Postgres initdb on a fresh volume and 269 Liquibase changesets |
| Idle, 5 s after ready | simulator 14.0–15.3 MiB at 0.04–0.07 % CPU; Postgres 36.0–38.3 MiB at 0.05–7.95 % CPU |
| Active (generator running, one listing) | simulator 15.6–16.1 MiB at 0.44–0.54 % CPU; Postgres 36.9–37.7 MiB at 0.05–9.67 % CPU |
| Liquibase (Java, one-shot) memory | **Not measured.** It exits before the stats sample |
| One full run (start → trade → generate for 6 s → teardown) | about 45 s wall time; `down -v` 1.9–2.8 s |
| Cleanup | After the last run: 0 containers, 0 volumes and 0 networks matching `bbqr`, and the bind-mount directory was removed |

### Determinism

`compare.py` drops tags that carry the wall clock or session state before hashing:
`8, 9, 10, 34, 49, 52, 56, 60, 779, 272, 273` (headers and timestamps), `37` and `17` (OrderID and
ExecID embed the wall-clock time, e.g. `261114150651000000`), and `278` (MDEntryID embeds the epoch).

| Comparison | Manual phase (17 msgs) | Generator phase |
|---|---|---|
| run 1 vs run 2, same seed `blackbox-42` | identical, `614695a9b007d753` | identical, 12 vs 12 msgs, `69de9eedbc7abcfe` |
| run 1 vs run 3, seed `blackbox-43` | identical, `614695a9b007d753` | differs from the first message (12 vs 11 msgs; `8ec99e1dcde7325f`) |

Same seed gave the same normalised trace twice (n = 2), and a different seed changed it. What this
does not show:

- **Pacing follows the wall clock.** `random_orders_rate` is orders per second of real time, so how
  many events land in a time window depends on scheduling. Matching identical prefixes is what
  holds; equal counts were not guaranteed.
- **The upstream docs limit the guarantee to one listing:** "deterministic generation … is only
  guaranteed within the sequence of updates per each listing. Since each listing is managed in a
  separate thread, overall updates mixed across all listings will still be produced with
  non-deterministic results" (`docs/RESTAPI/AdministrativeCommands`). This PoC used one listing on
  purpose.
- Raw timestamps and IDs never repeat between runs. Byte-identical artefacts would need the
  normalisation above.
- Seeding happens only through `genstart`. A venue with `order_on_startup=true` begins generating
  from an unseeded PRNG.

### Setup errors met (and fixed)

- A named volume for the simulator's data directory failed with
  `cp: cannot create regular file '/market-simulator/quod/data/cfg/configSim.txt': Permission denied`
  (the container exited with status 1). A bind mount, as in the upstream Quickstart, works.
- The tables are in schema `sim`, not `public`, so `setup.sql` needs `SET search_path TO sim`.

## 3. Adapter contract (design only; nothing implemented)

If an external engine ever produced BlackBox experiments, its run metadata would sit next to the
existing `RunManifest` (`crates/bb-core/src/result.rs`). The `engine` field would carry
`EngineInfo { name: "quantreplay", version, features }`. A new `external_engine` block would record
the rest. Every field is required unless marked optional.

```jsonc
"external_engine": {
  "engine_id": "quantreplay",
  "engine_version": "12.1792505299_39111b63",           // from GET /api/venuestatus
  "kind": "interactive_matching_engine",                // vs "historical_replay" (BlackBox native)
  "images": { "simulator": "sha256:cb46…", "database": "sha256:5f18…", "migrations": "sha256:bce7…" },
  "venue": {
    "id": "XETRA", "synthetic": true, "engine_type": "Matching", "timezone": "UTC",
    "tif_supported": ["Day", "IOC", "FOK", "GTC (accepted in PoC)"], "include_own_orders": true,
    "phases": "demo default; market phases not configured or checked in PoC"
  },
  "instrument": { "symbol": "VOW", "tick_size": null, "qty_min": …, "qty_max": …,
                  "price_seed": { "bid": "98", "mid": "100", "offer": "102" } },
  "config": {
    "sha256": "…",                     // over setup.sql + venue/listing rows read back via REST
    "order_on_startup": false,
    "generator": { "enabled": true, "rate_per_wall_second": 3, "depth_levels": 5,
                   "run_wall_seconds": 6 },
    "historical_playback": null         // datasource config, if used (untested)
  },
  "clock": {
    "semantics": "wall_clock",          // engine has no simulated/virtual clock control
    "source": "container system clock (UTC)",
    "timestamp_precision": "ms in SendingTime(52); µs in TransactTime/logs",
    "pacing": "generator events per real second; host scheduling affects counts",
    "blackbox_latency_model": "none: local loopback TCP; not exchange latency"
  },
  "seed": { "value": "blackbox-42", "scope": "per-listing generator sequence",
            "mechanism": "POST /api/genstart {seed}; engine hashes it with listing hash" },
  "events": { "artefact": "fix_trace.normalised.jsonl", "sha256": "…",
              "normalisation": ["drop 8,9,10,34,49,52,56,60,779,272,273,37,17,278"] },
  "orders": [ { "client_order_id": "b1", "side": "buy", "type": "limit", "qty": "100",
                "price": "99", "tif": "GTC", "sent_wall_ns": "…" } ],
  "fills": [ { "client_order_id": "b1", "exec_type": "trade", "last_px": "99", "last_qty": "80",
               "liquidity": "maker (derived: resting order)", "engine_exec_id": "…" } ],
  "fees": { "modelled_by_engine": false, "applied_by": "blackbox", "schedule": "<fee config ref>" },
  "artefacts": [ "fix_trace.raw.log", "fix_trace.normalised.jsonl", "simulator.log", "metrics.txt" ],
  "reproducibility": { "repeat_runs": 2, "normalised_trace_equal": true },
  "limitations": [
    "Synthetic venue and random order flow; not calibrated to any real market",
    "Wall-clock pacing: event counts per window are not reproducible",
    "Determinism guaranteed per listing only",
    "No fees in the engine; fees applied after the fact by BlackBox",
    "Local loopback latency only"
  ]
}
```

The run identity rules would need to change. BlackBox's `config_sha256` → `result_sha256` promise
("equal config on the same platform class gives an equal result") cannot be made for a wall-clock
engine. An adapter would store `result_sha256` over the normalised trace and report the number of
repeat runs that matched. It would never claim content-addressed reuse.

## 4. What each approach can and cannot establish

| | Historical replay (BlackBox today) | Interactive matching engine (QuantReplay) |
|---|---|---|
| Market data | Real recorded events in their real order | Synthetic random quotes around a price seed. Optional CSV/DB playback exists upstream (untested here) |
| Own orders change the book | **No.** No market impact; simulated orders never consume replayed depth | **Yes, inside the synthetic book.** Our sell 80 @ 99 consumed the resting buy, and the market data showed it |
| Matching mechanics | Fills inferred from replayed trades and a queue-position model | Exact price-time matching among the orders sent to it, with partial fills, cancels and TIF |
| Order lifecycle and protocol | Simulated statuses | Real FIX acks, rejects and cancel/replace message flows (FIX 5.0 SP2 over FIXT.1.1) |
| Other participants react | No | Only as random generator agents. They are not behavioural, and not calibrated |
| Queue position | Modelled, never observed in L2 | Known exactly, but only within the synthetic book |
| Latency | Configurable model (e.g. 1 ms vs 50 ms) | Whatever the local TCP loop happens to be. Not a modelled latency |
| Fees | BlackBox fee schedules | None |
| Reproducibility | Content-addressed: same config gives the same result | Same seed gives the same normalised trace for one listing; event counts depend on the wall clock |

**Neither establishes these without calibration against real venue data:** the size of market impact
on a real market; how real participants would react to our orders; true queue position and fill
probability at a real venue; adverse selection and realistic P&L; real venue latency; real fee and
rebate outcomes; hidden or iceberg liquidity. An interactive engine shows that the *mechanics* are
right (orders cross, partial fills happen, the book updates). It says nothing about whether the
*market* behaves like this.

## 5. Cost and maintenance

- **Money:** $0 to run locally. No paid service is involved.
- **Footprint:** 1.24 GB of images; about 52 MiB of RAM for the simulator plus Postgres at steady
  state; about 20–25 s to start from cached images.
- **Maintenance:** the project is active (v12, last commit 2026-10-20, images three weeks old). That
  means `latest` keeps moving, so an adapter must pin image digests. The simulator image is amd64
  only (REUSE_MATRIX §4), so arm64 machines would need emulation. BlackBox would also have to own a FIX
  client, a trace normaliser, a schema-aware setup step and a second run-identity model. None of
  this exists in BlackBox, and none of it could be shared with the replay engine.
- **Licence:** Apache-2.0. Running unmodified images locally creates no redistribution obligation.
  Redistributing images would carry the bundled quickfix 1.15.1 notice
  (OSS_LICENSES_AND_PROVENANCE).

## 6. Go / no-go

**No-go for integration into the product or the hosted service.**

1. **Hosted fit: infeasible.** The product runs one Render free web service with 512 MB RAM and
   "less than 1 CPU" (`docs/research/OSS_TECHNOLOGY_EVALUATION.md` §1). QuantReplay needs a second
   service with its own Postgres and a migration job, which breaks the "no new databases or
   microservices" rule. It also needs a raw-TCP FIX port and a long-lived process. Steady-state RAM
   (about 52 MiB) would fit, but the topology would not. [INFERENCE: a Render free web service
   exposes HTTP only, so the FIX port could not be reached; this was not tested.]
2. **Scientific value: low for BlackBox's questions.** BlackBox asks how latency, queue assumptions
   and fees change outcomes on *real* recorded markets. QuantReplay adds own-order impact only inside
   a random synthetic book, and it has no fees and no modelled latency. Its results could not be
   compared with BlackBox replay runs without calibration that neither tool provides (§4).
3. **Reproducibility is weaker than BlackBox's.** Same-seed equality held for one listing on
   normalised traces (2 runs). Wall-clock pacing and per-listing-only guarantees rule out BlackBox's
   content-addressed run identity.
4. **Cost is low, but maintenance is not.** It is free to run. Integration would still mean a FIX
   client, digest pinning, an amd64-only image and a second identity model.

**When to revisit:** if a future milestone needs to test the *order-lifecycle protocol* (FIX
acks, rejects, cancel/replace) or needs to show own-order impact on a toy book for teaching. In that
case, offer it as a local-only "exchange lab", clearly labelled synthetic, using the §3 contract.
The PoC in `D:/bbwt-research/quantreplay-poc/` is the starting runbook.

## 7. Gaps

- Liquibase memory and peak Postgres memory during migration were not measured.
- The determinism check used 2 same-seed runs, one listing and one 6 s window. Multi-listing runs
  and long runs were not tested.
- Historical playback through QuantReplay datasources (CSV/DB) and market phases or auctions were
  not exercised.
- Cold start without cached images was about 3 min of pulls plus startup, measured only for
  sequential pulls.
