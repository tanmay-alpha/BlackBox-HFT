# Phase 11: performance regression check against the Phase 6 baseline

These are lab measurements only. No field data, INP is not claimed, and production was not touched.

## Conditions

- Build: `main` = `80340e6` (`web/src` identical to production `3ed53cd` web code; entry `index-BqRS4VXB.js`,
  411.59 kB), built with `npm ci` + `npm run build`. Server: `data/e2e/blackbox.exe` (same code as main), data
  directory `data/e2e`, database `blackbox_e2e_auditperf` (migrated, `demo seed --wait`), port 18915,
  `BB_CORS_ALLOWED_ORIGINS=http://127.0.0.1:18915`, `BLOB_READ_WRITE_TOKEN`, `DATABASE_URL_UNPOOLED` and
  `BB_API_KEY_PEPPER` unset.
- Runner: new `web/perf/session.mjs`. Before **every run** (one profile, one repetition, all scenarios) it executes
  `delete from bb.egress_usage_daily where day >= (now() at time zone 'utc')::date - 1`, starts a new server,
  waits for `/api/v1/status`, runs `perf/run.mjs --reps 1`, and stops the server. Order: rep 1 mobile, rep 1
  desktop, rep 2 mobile, … ; 3 repetitions per profile, cold browser context per scenario. Summary:
  `web/perf/session-summary.mjs`.
- Profiles: unchanged `web/perf/profiles.mjs` (mobile 390×844 DPR 3, 4× CPU, 1.6 Mbit/s / 750 kbit/s, 150 ms RTT;
  desktop 1440×900, 10 Mbit/s, 40 ms).
- Environment: Windows 11 10.0.26300, i7-14650HX (24 logical CPUs), 15.7 GiB, Node 24.14.1, Playwright 1.65.0,
  headless Chromium 158.0.8127.18. Measured 2026-11-19 between about 05:20 and 05:50 UTC.
- **Machine load:** other audit agents were building and testing on the same machine. Per-run machine CPU
  utilisation was 0.18–0.59 (median 0.22); per scenario, the mobile Explore/Replay scenarios of repetition 1 ran at
  0.69–0.75,
  and individual repetitions show the effect (e.g. mobile Explore scenario switch LCP 12,280 ms at 0.75 vs
  4,240 / 4,696 ms at ≈0.19). The mobile profile multiplies host contention by its 4× CPU throttle.
- New scenarios (no earlier figures exist for these pages): `research-load` (`/research`, all five `li.rh-row`
  `data-state="ready"`), `studies-load` (`/studies/exec-validation`, study view rendered and SHA-256 verified),
  `queue-lab-load` (`/queue-lab`, SHA-256 verified and first content visible; on the phone the fill chart is on the
  Fills stage, so the staged Overview is the marker), `market-load` (`/market/l3-demo?tab=book`, curated example
  event selected and `.mk-ladder` drawn) with the `market-step` interaction (press "Next event (J)" until the event
  facts change).

Raw data (local, git-ignored): `data/perf/p11-main-{1,2,3}/{mobile,desktop}.{json,md}`, server logs
`data/perf/p11-session/`.

Reproduce (from `web/`):

```sh
node perf/session.mjs --bin data/e2e/blackbox.exe --db blackbox_e2e_auditperf --psql <psql.exe> \
  --build main=web/dist --data-dir data/e2e --port 18915 --reps 3 --label p11 \
  --only explore-load,explore-first-chart,explore-scenario-switch,replay-load,replay-seek,mobile-nav,research-load,studies-load,queue-lab-load,market-load
node perf/session-summary.mjs p11 main 3
```

## Results (medians of 3; CLS is the maximum of 3; samples in repetition order)

API errors: 0 in all 60 navigations and interactions.

### Mobile

| Scenario | LCP ms (samples) | FCP ms | TBT ms (samples) | CLS max | JS bytes | ready ms | LCP element |
|---|---|---:|---|---:|---:|---:|---|
| Explore load | 4,840 (4,216; 6,276; 4,840) | 1,804 | 2,935 (1,480; 2,935; 4,394) | 0.0004 | 694,665 | 9,767 | `h1.ex-question` |
| Explore first chart | 6,192 (8,744; 6,192; 4,188) | 968 | 1,183 (4,207; 1,183; 1,092) | 0.0004 | 694,665 | 7,764 | `h1.ex-question` |
| Explore scenario switch | 4,696 (12,280; 4,240; 4,696) | 1,004 | 1,338 (9,894; 992; 1,338) | 0.0004 | 694,665 | 8,352 | `h1.ex-question` |
| Replay load | 5,632 (5,632; 5,528; 5,880) | 920 | 2,251 (2,286; 1,895; 2,251) | 0.0013 | 615,548 | 12,411 | `h1.term-title` |
| Replay seek | 6,104 (11,716; 6,104; 5,620) | 1,152 | 1,978 (6,164; 1,978; 1,973) | 0.0013 | 615,548 | 12,764 | `h1.term-title` |
| Mobile nav (from `/`) | 4,556 (4,556; 4,996; 4,280) | 940 | 1,317 | 0.0004 | 694,665 | 6,442 | `h1.ex-question` |
| Research home | 3,896 (3,824; 3,896; 4,048) | 1,004 | 543 (489; 545; 543) | **0.087** | 467,681 | 5,547 | `p.note.rh-lede` |
| Studies (`exec-validation`) | 6,136 (6,060; 6,316; 6,136) | 968 | 822 | 0.0004 | 507,920 | 5,960 | `span` |
| Queue lab | 5,448 (5,448; 6,404; 5,420) | 880 | 556 | 0.0004 | 490,684 | 5,421 | `p.ql-question` |
| Market Workbench | 5,140 (5,140; 5,312; 5,036) | 888 | 557 | **0.086** | 602,783 | 5,553 | `summary` |

| Mobile interaction | visible result ms | Event Timing ms | long tasks ms |
|---|---:|---:|---:|
| Explore switch scenario | 124 | 72 | 451 |
| Replay seek step | 169 | 272 | 442 |
| Replay scrub | 57 | 104 | 247 |
| Open menu | 175 | 192 | 150 |
| Menu → Strategy Lab | 1,989 | 88 | 206 |
| Market next event | 678 | 120 | 120 |

### Desktop

| Scenario | LCP ms (samples) | FCP ms | TBT ms | CLS max | ready ms |
|---|---|---:|---:|---:|---:|
| Explore load | 900 (900; 1,028; 892) | 360 | 225 | 0.001 | 1,868 |
| Explore first chart | 1,040 | 328 | 25 | 0.002 | 2,040 |
| Explore scenario switch | 1,028 | 328 | 24 | 0.002 | 2,023 |
| Replay load | 2,632 (2,632; 5,180; 2,628) | 340 | 157 | 0.001 | 2,579 |
| Replay seek | 2,564 (2,564; 15,076; 2,488) | 332 | 194 | 0.001 | 2,507 |
| Mobile nav (rail) | 1,064 | 380 | 76 | 0.002 | 1,616 |
| Research home | 1,108 (1,340; 1,108; 788) | 404 | 32 | 0.025 | 3,182 |
| Studies | 1,372 | 368 | 98 | 0.001 | 1,311 |
| Queue lab | 1,404 | 348 | 29 | 0.001 | 1,376 |
| Market Workbench | 1,072 | 232 | 26 | 0.011 | 1,172 |

Desktop interactions: Explore switch 58 ms, Replay step 117 ms, scrub 1,327 ms (window load), rail → Strategy
Lab 518 ms, Market next event 225 ms (Event Timing ≤ 136 ms everywhere).

## Comparison with the Phase 6 "after" figures

Phase 6 figures are medians of 6 from a different session and machine state (PHASE6_PERFORMANCE.md, A/B 1 and
desktop table); they are not a same-session A/B.

| Page | Metric | Phase 6 after | Now | Δ | Flag rule |
|---|---|---:|---:|---:|---|
| Explore load (mobile) | LCP | 4,276 | 4,840 | +564 | > 300 ms: **nominal flag** |
| Explore first chart (mobile) | LCP | 4,220 | 6,192 | +1,972 | **nominal flag** |
| Explore scenario switch (mobile) | LCP | 4,126 | 4,696 | +570 | **nominal flag** |
| Replay load (mobile) | LCP | 5,232 | 5,632 | +400 | **nominal flag** |
| Replay seek (mobile) | LCP | 5,256 | 6,104 | +848 | **nominal flag** |
| Explore load (mobile) | TBT | 1,220 | 2,935 | +1,715 | > 50 ms: **nominal flag** |
| Explore / Replay (mobile) | FCP | 816 / 806 | 1,804 / 920 | | |
| Explore (mobile) | JS transferred | 675.8 KiB | 678.4 KiB (694,665 B) | +2.6 KiB | |
| Explore (desktop) | LCP | 916 | 900 | −16 | ok |
| Replay (desktop) | LCP | 2,380 | 2,632 | +252 | ok (Phase 6 spread 2,328–3,416) |
| Explore / Replay (desktop) | FCP | 228 / 216 | 360 / 340 | | |
| all pages | CLS | ≤ 0.006 mobile, ≤ 0.016 desktop | ≤ 0.087 | | none > 0.1 |
| Menu → Strategy Lab (mobile) | result | 1,930 | 1,989 | +59 | |

**Assessment of the nominal flags: not attributed to code; most likely machine load.**

- `web/src` changed after the Phase 6 measurement only in `ad2638f` (integration of the other Phase 6 web work) and
  `06e98b3` (StudiesPage only). Explore and Replay JS bytes are within 2.6 KiB of Phase 6, and the LCP elements are
  the same (`h1.ex-question`, `h1.term-title`; desktop Replay `div.empty`, as in Phase 6).
- The same build, measured the same day by PHASE10_LCP.md at 29–40 % machine load, gave Explore LCP 4,072 and TBT
  334, Replay LCP 5,032 and TBT 367 (`fcd50b1`, whose `web/src` is identical to main's). Those are at or below the
  Phase 6 figures.
- Within this session the inflation tracks load: Explore/Replay mobile repetition 1 ran at 0.69–0.75 utilisation;
  the cleaner repetitions (≈0.19) give Explore scenario switch 4,240 ms and Explore first chart 4,188 ms, i.e. within
  about 100 ms of Phase 6. TBT samples for one page vary 1,480–4,394 ms (3×).
- **Not confirmed:** a same-session A/B of `b30c936` (the Phase 6 "after" commit) against main would settle the
  Replay +400 ms (all three Replay load samples were 5,528–5,880 ms). The worktree and dependency junction were
  prepared (`data/perf/p6final/web`, same `package.json`/lock as main) but the A/B was not run (time).

## Findings without a baseline (new pages)

1. **Research home and Market Workbench mobile CLS 0.086–0.087** (all three repetitions, desktop 0.025 / 0.011).
   Below the 0.1 threshold, but about 15× every other page. The Phase 3.2 record lists a Market Workbench phone
   shift of 0.08 as fixed (meta line); the shift is back at the same size or has a new source. Not attributed:
   the harness records shift values but not sources, and attribution was stopped for time. A throwaway attribution
   script (PerformanceObserver `sources`, mobile profile) is in local `data/p11-attr.mjs`.
2. The Market "next event" interaction measured 5,353 ms (Event Timing 1,528 ms, 13 long tasks, 3,366 ms) in one
   smoke run under load, but 678 ms median (Event Timing 120 ms) in the session. Not a confirmed defect.
3. Studies mobile LCP 6,136 ms is the slowest of the new pages: the 227.7 KiB `study-result.json` (≈1.4 s on the
   profile) is fetched and SHA-256 verified before the LCP element renders.

No code fix was made: no flagged regression was confirmed as a code change.

## Harness changes

- `web/perf/scenarios.mjs`: `research-load`, `studies-load`, `queue-lab-load`, `market-load` (+ `market-step`).
- `web/perf/session.mjs`: generic same-session runner (one or more `--build name=dir`, both profiles, egress reset
  and server restart before every run).
- `web/perf/session-summary.mjs`: medians, CLS maximum, samples and interaction medians per build and profile.
