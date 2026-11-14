# Final independent audit

Date: 2026-11-14. Scope: the production research workflow, the current source checkout, BASELINE_AUDIT AT-1–AT-15, and the 32 **P0** requirements in SECURITY_REVIEW §18. This is an audit, not a certification or model calibration.

## Executive verdict

**The keyed single-run research path works, but the public research laboratory is not ready for an unqualified close-out.** A real experiment completed, its fills and accounting reconciled, a genuinely queued job was cancelled, and later reads retained the results. Anonymous demo forensics, Markdown reports and analytics work. However, the anonymous comparison workspace has no stored comparisons, one ordinary keyed comparison timed out, and response/egress controls have material implementation gaps.

| Checklist | met | partial | open |
|---|---:|---:|---:|
| Baseline AT-1–AT-15 | 5 | 7 | 3 |
| Security P0 (SEC-1–31 and SEC-34) | 11 | 17 | 4 |

**Verdict meanings:** `met` means the implementation and cited evidence satisfy the substantive requirement within the stated scope; it does **not** claim that this audit reran every referenced test. `partial` means a required subcondition, production verification, or enforcement remains missing. `open` means a material requirement is absent or contradicted. Historical criteria naming `/api/runs` or static export are evaluated against their `/api/v1`/live-demo replacements, not by requiring restoration of removed code.

### Boundaries and provenance

- Production API: `https://blackbox-hft-api.onrender.com`; console: `https://blackbox-hft.vercel.app`.
- The supplied source reference is `main=e0384a7`. Production `/api/v1/status` instead reports **`1873f4d4392c3b222d2c28e9e759c3d9b11e510d`** (R3). Source findings and observed production findings are distinguished below; newer source must not be assumed deployed.
- HTTP evidence R1–R61 was collected 10:43:24–10:47:40 UTC. There were **61 explicit HTTP requests and 27 browser-recorded requests**, 88 total to production origins. The browser was closed after the overview/comparison checks. Cloud metadata CLI reads were additional control-plane reads, not traffic/load tests.
- With the parent's explicit budget extension: **five experiment POSTs, two rejected with 422 and three resulting jobs**; two jobs completed and one was cancelled before starting. No other experiment was submitted. Two comparison resources were requested: one created, one timed out.
- The research key was read into memory only and sent only in `Authorization`. It is not reproduced here. No Tardis files were opened. No deployment, cloud-setting change, production tampering, restart, load test, build, formatter, linter, or local test suite was performed. Only this audit file was created.
- Some documentation was updated concurrently. The documentation inventory below uses the latest content actually read, and explicitly distinguishes corrected claims from remaining ones.

## 1. Production workflow and request/response evidence

The appendix maps every explicit request to its status, size, elapsed time and request id. Response excerpts below are actual returned values, not expected output.

### 1.1 Anonymous demo reads and workstation

| Check | Observed response |
|---|---|
| Liveness / readiness | R1 `200 {"status":"ok"}`; R2 `200 {"blob":"configured","db":"ok","migrations":"0007","status":"ready"}`. Readiness says configured, not that an object was fetched. |
| Demo listing | R4 `GET /api/v1/runs?limit=3` → 200, actual completed canonical runs, including `586374188aae19b2`, `6bcdf19136fd3059`, `130d67fc371bd27f`. |
| Run detail / blotter / forensics | R32 summary → 200; R33 orders `limit=1` → 200, order 0; R41 `/orders/0` → 200 with `order`, flight `events`, fills, `explanation`, and instrument `{tick_size:0.1,lot_size:0.001,contract_size:1.0}`. Its order intent is at `"100000000"`, exchange arrival at `"101000000"`, first fill at `"108869266"`. |
| Run report | R35 `/runs/586374188aae19b2/report` → 200 `text/markdown`, 25,406 bytes, title `BlackBox research report: run 586374188aae19b2`, research disclaimer and actual config/result/dataset hashes. |
| Run / dataset analytics | R34 run analytics → 200, analytics version `1.0.0`; R36 dataset analytics → 200, exchange-clock completed-batch sampling. The oversized responses are defects, not failed measurements (§4). |
| Comparison / saved-report catalogue | R6 `/comparisons?limit=3` and R7 `/reports?limit=3` → `200 {"items":[],"next_cursor":null,"truncated_by":null}`. There is no anonymous stored comparison to read. The per-run Markdown report still works. |
| Browser → API → rendering | Chromium loaded Overview, six stored demo runs and all three experiment sets over the real cross-origin API. Then `/compare/pair?a=586374188aae19b2&b=6bcdf19136fd3059` rendered **NO STORED COMPARISON**, an empty stored-comparisons table, and a disabled `Create comparison` requiring a key. No page errors or error-level console entries were recorded on these two pages. The case-sensitive wait for mixed-case `No stored comparison` timed out; reading the rendered text established the actual all-capital message. |

R10 console HTML was HTTP 200, with `script-src 'self'`, no `unsafe-inline`/`unsafe-eval`, and `connect-src 'self' https://blackbox-hft-api.onrender.com`. The browser displayed a stale idle heartbeat (81 s, later 119 s, against the advertised 15 s), despite a responsive API.

### 1.2 Authenticated experiment: submission, progress, result and later persistence

R9 `/whoami` confirmed a research workspace and `read,run` scopes, active-job quota **50**, experiments/day 100. No admin credential was used.

Starting from the exact `maker_baseline` config returned by R5 `/presets`, the successful submission changed only:

```json
{"seed":"141120260701","strategy.interval_ns":"10000000","label":"FINAL_AUDIT accounting and persistence"}
```

The notation above is a compact diff, not a literal accepted POST body. The actual body was `{config:<full preset with those changes>,label:...}`: touch-maker qty 0.01/max position 0.1/offset 0/skew 20; no-partial-fill, power_prob3 n=3, constant entry/response 1 ms, feed offset 0, maker/taker fees .0002/.0004; risk position .1/order qty .05/open orders 8/no drawdown halt; record every tick/max 2,000 queue updates.

R13 `POST /experiments` → **202**, `created:true`:

- experiment `01a1d9a8-5e51-7fdf-825c-1498e5656193`;
- job `01a1d9a8-5e4c-7257-8595-9f27e49baa3e`;
- fingerprint `bb9865efdbbb484489a79892d62e115f36e7f9b00598d8def008ddd65aeeb349`;
- `queued_at_ns="1794653052473995000"`, `started_at_ns="1794653052504628000"`;
- estimated ticks 12,001, upper tape events 84,725, upper output bytes 54,501,136;
- platform `linux-x86_64+libm-53154a4e`.

**Do not overstate transition observation:** this job had already become `running` in its first HTTP response, about 30.6 ms after its recorded queue timestamp. Its `queued` state was not sampled directly. The queue timestamp is persisted transition evidence; the separately cancelled job below was directly observed queued.

R14–R27 reported `running` with progress `{done:12000,total:12001,tape_events:60382,unit:"ticks"}`. R28 reported:

```json
{"state":"completed","run_id":"bb9865efdbbb4844","finished_at_ns":"1794653069561580000","progress":{"done":12001,"total":12001,"tape_events":60382,"unit":"ticks"}}
```

Elapsed persisted queue-to-completion time: approximately **17.09 s**. Progress was monotone over the samples, but most work finished before the first poll; a detailed increasing simulation curve was not observed.

R37 summary, R39 all 2,049 fills (560,677 bytes), R42 final unsampled equity window and R43 analytics returned 200. Summary counts: **12,000 ticks, 12,001 equity samples, 5,334 orders, 2,049 fills, 60,382 tape events**. The progress estimate includes the closing sample; it is not a claim of 12,001 strategy ticks.

R56 subsequently read the experiment with its full config. R60 subsequently read the same run with unchanged:

- `result_sha256=d5a75306c4ac4cf9613209df1f90041d3b6f763870957e55e9e7ad6a3b552f7e`;
- bundle SHA-256 `77a02f251cbac1e3acf99d058ab2e2b0b3505e2cec43abbc520a5fac65b6448f`, 2,095,726 bytes;
- `metrics.net_pnl=-123.25928999999864`.

This establishes persistence across independent HTTP reads, **not across a production process restart**. Restart was prohibited by the audit's no-cloud-change boundary; the existing restart test is `crates/bb-app/tests/api_v1.rs:577-752`.

### 1.3 Queued cancellation

R29 submitted a second job with the same preset/10 ms interval and seed `141120260702` to occupy the sole worker. R30 submitted seed `141120260703`; its response explicitly said **`queued`**, job `01a1d9a8-e9a7-7821-9929-568b00a3de52`, identity `c86efea35f40ed31`.

R31 `POST /jobs/01a1d9a8-e9a7-7821-9929-568b00a3de52/cancel {}` → **202**:

```json
{"state":"cancelled","attempt":0,"started_at_ns":null,"run_id":null,"run_ids":[],"detached":false,"queued_at_ns":"1794653088075187000","finished_at_ns":"1794653088564997000"}
```

The queued job was cancelled about **0.49 s** after enqueueing and never started. R57 later retained `cancelled`, attempt 0 and no run; R58 authenticated run lookup → 404. No direct Blob enumeration was performed, so absence of an orphan object is not independently proven by HTTP. The worker-occupancy job later completed as `3a1864f1f5010064` (R61), demonstrating continued worker availability.

### 1.4 Compare/report endpoints and security probes

- R40 created a keyed comparison of public demo runs `586374188aae19b2` (maker rebate) and `6bcdf19136fd3059` (zero fees): **201**, id `01a1d9a9-77f3-7d7d-b7ca-2c2137e8da61`, eligibility `{aligned:true,like_for_like:true,reasons:[]}`, **1,241 `same_fills_different_fees` pairs**. R52 detail, R53 first paged pair and R54 Markdown comparison report all returned 200. Report Δnet = −37.2305266 and Δfees = +37.2305266, consistent with the fee-only attribution rule.
- R59 requested a new comparison of the audit's 10 ms run with the public 100 ms rebate run. It returned **503 after 10.313 s**, `unavailable`, `details.reason="busy"`, `Retry-After:5`, request id `01a1d9ab-1e75-72f5-a769-f525bceb47f7`. Thus production interval-ineligibility output was **not** observed; the timeout is an actual failure, not a passed eligibility test. It was not rerun.
- R11/R12 seeds `9007199254740993` / `9007199254740995` → **422 invalid_config**, explicitly naming `2^53−1`. Rejections created no jobs. String ns/seed values were observed on successful responses.
- R44 anonymous read of the new private run → **404 not_found**; R45 malformed bearer on a public preset read → **401**, not silent demo fallback.
- R46 allowed-origin preflight → **204**, exact ACAO `https://blackbox-hft.vercel.app`; R47 foreign preflight → **403** without ACAO; R48 foreign-Origin GET → **403**.
- R49 research key `/readyz?probe=1` → **403**, required scope `admin`. R50 anonymous bundle export → **401**. R51 anonymous comparison mutation → **401** before meaningful body validation.

## 2. Quantitative sanity and look-ahead review

### 2.1 Accounting identity: passed

Spec: `docs/PNL_ACCOUNTING.md:54-65`, with fees deducted and contract size included. Use R42's final raw sample, not the downsampled R38 bucket:

```text
cash          = -300.3200000000028
position      = 0.009999999999999997
mid_tick      = 300002.5
tick_size     = 0.1
contract_size = 1
fees          = 122.94179000000072
mark          = 30000.25
cash + position × mark × contract_size − fees
              = -123.25929000000356
reported equity / summary net_pnl
              = -123.25928999999864
residual      = -4.916955731459893e-12 quote
```

Summing the actual 2,049 fill fees gives **122.94179000000072**, exactly the reported sampled-ledger fee total in the Python sum used here. The analytics E9 net was −123.2592900000004 and analytics fees 122.94179000000001; their tiny summation differences do not indicate an accounting discrepancy. E9's exchange-final-mark convention is explicitly distinct from simulator local-mid marking (`docs/research/VALIDATION.md:20-22`); this observation does not claim they always coincide.

### 2.2 Market analytics spot-check against fixture logic: passed

M1 specification uses `S=ask−bid`, relative spread `10,000 × S / mid` (`docs/research/MARKET_MICROSTRUCTURE.md:78-87`). The hand fixture `crates/bb-analytics/src/market_tests.rs:81-99` checks a 200/202 tick book gives 2 ticks and `40000/402` bps.

For the production clock-series point at **200,000,000 ns** (R36), R55 independently requested the replay book at that exact time: best bid **299999**, best ask **300000**. Therefore:

```text
spread ticks = 300000 − 299999 = 1
relative bps = 10000 / 299999.5 = 0.033333388888981484
```

The returned analytics point reports **1.0 ticks** and **0.033333388888981484 bps**, exactly matching the fixture's arithmetic applied to actual production data. This spot-check is not a validation of every aggregate or of the simulator against real fills.

### 2.3 Strict pre-fill rule: no look-ahead found in this rule

- `bb-data/src/book.rs:177-212` emits BBO states only after the complete equal-exchange-timestamp batch.
- `bb-analytics/src/execution.rs:736-745` selects `partition_point(|t| *t < ts) - 1` when strict, returning no reference before the first valid BBO; it does not search forward over an invalid book.
- The `ExchFill` branch calls `mid_at(book,event.ts,true,...)` at `execution.rs:1161-1166`. It does not substitute enriched `FillRecord.mid_tick`.
- Horizons use `<= t+h`, but reject horizons beyond the final book timestamp (`execution.rs:747-751,1170-1174`); these are post-hoc outputs, not strategy inputs.
- The same-timestamp-jump fixture `execution.rs:1715-1745` requires ES −0.5 rather than −10.5 and excludes a fill at the first BBO time. Its expected arithmetic was inspected, not executed in this audit.

Verdict is limited to the inspected analytics reference rule, not a proof that all strategy/model paths are look-ahead-free.

## 3. Acceptance verdicts

### 3.1 BASELINE_AUDIT AT-1–AT-15

| Item | Verdict | Current evidence and outstanding boundary |
|---|---|---|
| AT-1 compare eligibility | partial | Eligibility gate and interval fixture exist (`bb-forensics/src/compare.rs:292`, `:1187`; alignment emitted conditionally at `:986`). Production aligned fee-only case works (R40); mismatched-interval case hit 503 (R59), so end-to-end refusal was not demonstrated. |
| AT-2 timing-aware classes | met | Timing fixtures at `compare.rs:1336,1370,1551` include the original 3,831,084,401/3,859,654,055 ns case; exact-history/timing claim at `:904`. Production fees are correctly non-identical (R40/R53). Tests were inspected, not rerun. |
| AT-3 integrity on every read | partial | Detail/bundle verification and quarantine exist (`v1/runs.rs:119-175`, `artifacts.rs:302-350`); tamper/re-simulation fixture `tests/api_v1.rs:766-907`. But list summaries explicitly bypass verification (`v1/runs.rs:275-280`); cached summary-only reads do not re-read all bundle members. No production tampering performed. |
| AT-4 single-flight | met | Config/workspace row locks and live-job unique-conflict reuse (`bb-db/src/queue.rs:203-294`); explicit API concurrency test `tests/api_v1.rs:477-575`; first-label-wins via conflict update not replacing label (`queue.rs:292-293`). Eight concurrent production POSTs were deliberately not spent. |
| AT-5 budgets | partial | Admission limits (`admission.rs:39-86`), original 1800 s/1 ms rejection test (`tests/budgets.rs:81-113`), runtime-breach/cancel-store-nothing test (`:171-242`) and Linux/Windows peak check (`tests/hosted_peak_memory.rs:80-124`, CI lines 38-40/74-75). Peak fixture uses file-store pipeline, not full worker+analytics+HTTP concurrency; output-byte enforcement described at budgets.rs:203 is after the result is built, not a mid-run allocation proof. |
| AT-6 progress/cancel/recovery | partial | Production running→completed with monotone sampled progress and directly observed queued→cancelled (R13–31/R57). Lease/restart tests exist (`tests/api_v1.rs:577-752`). No production restart, running cancel or orphan-object check; first successful job's transient queued state was not sampled. |
| AT-7 platform policy | met | Platform/libm-tagged lookup (`v1/runs.rs:80-84`); `bb-core/src/platform.rs:12-34,48-64`; both CI OS jobs (`ci.yml:13,57-75`). Real run is canonical `linux-x86_64+libm-53154a4e` (R37). |
| AT-8 demo build/browser coverage | partial | Removed static mode is replaced by normal live build (`web/package.json:7-13`, `web/vercel.json:4-6`); CI builds and runs real v1 browser stories (`ci.yml:132-155`, `web/playwright.config.ts:3-6,26-34`). Production root 200/browser clean, but anonymous comparison is unavailable because no comparison is seeded. Static exporter must not be restored merely to satisfy obsolete wording. |
| AT-9 precision | met | Production unsafe seeds rejected (R11/R12), wire ns/seed strings observed. Manifest safe-number jq in CI (`ci.yml:47-50`); dataset span CHECK (`bb-db/migrations/0002_datasets.sql:73-75`). |
| AT-10 bounded/paged reads | partial | Actual fills page 560,677 bytes (R39), raw final equity window and explicit downsampling (R38/R42); DB-only list `v1/runs.rs:231-280`. Analytics violate anonymous 1 MiB and keyed 4 MiB caps (R34/R36/R43); heavy comparison/timeline/report still decode whole runs (`v1/comparisons.rs:125-126`, `v1/runs.rs:836,884`). No 10,000-row benchmark or full-server memory measurement made. |
| AT-11 auth/workspaces | partial | Real anonymous private-run 404, malformed-key 401, scope 403, CORS allow/refuse (R44–51); route-matrix and foreign-workspace fixtures (`tests/api_v1.rs:36-290`); hashed/tagged mint (`bb-db/src/keys.rs:78-108`). Unscoped DB methods remain public on the same type (`bb-db/src/lib.rs:15-21`), so requested compile-time separation is absent; no second real workspace credential or DB dump was used. |
| AT-12 multi-seed ensemble API | open | Public sweeps still take `GridSpec` (`v1/sweeps.rs:10,59`); production OpenAPI R8 has no ensemble operation. Library selection/dispersion code (`bb-forensics/src/sweep.rs:129-145,322-325`) is not an API accepting N seeds with per-seed ids and collapse/dispersion output. |
| AT-13 golden/reference coverage | met | AS, jitter/feed-offset/drawdown golden variants (`tests/golden.rs:62-76,185-214` and following table); raw same-request-stream parity test `bb-sim/tests/raw_engine_parity.rs:1-4,63-139`, included by workspace CI. Not rerun here. |
| AT-14 contract-drift gate | open | Generated OpenAPI/TS exist (`v1/mod.rs:133-155`, `web/package.json:13`, `web/src/api/v1.gen.ts:1-4`), but CI only builds the checked-in web files (`ci.yml:77-90`); no regenerate-and-diff/schema-snapshot gate. A changing Rust response can leave stale generated TS while compilation remains green. |
| AT-15 docs | open | Remaining false current claims listed in §5; no script enforces doc counts/CI claims. Some README/deployment claims were corrected during the audit; historical baseline documents should remain historical, not be silently rewritten. |

### 3.2 SECURITY_REVIEW P0 items

32 rows, including SEC-34 (P0) and excluding SEC-32/35 (P1), SEC-33/36–39 (P2). Most destructive/large-count acceptance exercises were intentionally not run on free-tier production.

| Item | Verdict | Evidence / remaining requirement |
|---|---|---|
| SEC-1 auth/scope matrix | met | Production R44–51; deny-before-handler/tag checks `v1/auth.rs:294-339`; OpenAPI-derived matrix test `tests/api_v1.rs:207-290`. This is not a claim that production revoke/expiry was exercised. |
| SEC-2 IDOR / scoped types | partial | Visible run lookup `v1/runs.rs:70-102`, caller workspace checks `auth.rs:62-81`, API foreign-id fixture `tests/api_v1.rs:78-205`; R44 private run hidden. No compile-time scoped/unscoped split (`bb-db/src/lib.rs:15-21`) and route IDOR cases are hand-enumerated rather than proven complete against every schema route. |
| SEC-3 hashed keys | met | `keys.rs:101-107` inserts SHA-256, never plaintext; `DatabaseConfig` deliberately lacks Debug (`bb-db/src/lib.rs:61-68`). No production dump was made. |
| SEC-4 CORS/Origin/Host | met | R46 exact-origin 204; R47/R48 refused; exact-host/origin guard `v1/edge.rs:128-187`, Vary/security headers `:104-123`; local/host fixture `tests/api_v1.rs:300-360`. `/healthz` deliberately has a Host exception (`edge.rs:135-136`). |
| SEC-5 body/header/encoding limits | met | Global declared-size, 16 KiB header and compressed-body checks `edge.rs:144-166`; actual body-byte cap in `respond.rs:196-205`; fixtures `tests/api_v1.rs:362-410`. No oversized production payload sent. |
| SEC-6 work/sweep budgets | partial | Admission/RunControl/peak tests as AT-5; full deployed process memory under concurrent heavy reads and analytics is not established. |
| SEC-7 labels/scripted problems bounded | partial | Main label≤100/no controls (`respond.rs:234-250`), hosted orders≤1000 (`admission.rs:19-20,40-55`), max 50 problems (`error.rs:128-145`). Scripted order labels are not bounded (`bb-core/config.rs:781-804`); validation runs before order-count rejection. Duplicating 50×300-char problems into both message and details also does not prove the required 16 KiB error bound. |
| SEC-8 atomic/fair quotas | partial | Atomic submit locks `queue.rs:203-294`, actual fair claim `api_reads.rs:233-248`, used by worker `worker.rs:360-366` (do not confuse deprecated FIFO `claim` with worker path). R9 active quota remains 50, not required hosted default 5; schema default `0001_identity.sql:55`; no per-workspace monthly upload-share enforcement in submission. |
| SEC-9 rate/global/in-flight/timeouts | partial | GCRA per-IP/global/key `auth.rs:143-210`; 2 s in-flight wait/read30s/write10s `edge.rs:203-223`. No production flood/slowloris test. The write timeout actually interrupted a legal comparison (R59). |
| SEC-10 warm anonymous paths never query DB | open | `edge.rs:363-405` is a 30 s full-URI response cache, jobs 2 s, invalidated by worker generation; cache misses reach DB handlers (`v1/runs.rs:258-268`). Varying a benign query or waiting 30 s bypasses warm cache. Egress flush can also write DB. `/healthz`, `/status`, cached readiness themselves are memory-backed, but the all-demo/zero-query acceptance requirement is not met. No 1,000-request production test performed. |
| SEC-11 Blob attempt/byte budgets | partial | Read charges precede each attempted GET (`artifacts.rs:122-145`, `bb-db/api_reads.rs:253-310`); daily pacing fixture `bb-db/tests/api_reads.rs:772` exists. Upload retries are counted at completion, not per attempt (`artifacts.rs:148-165`); per-workspace upload share absent. Whole fetch uses 8 MiB chunks (`artifacts.rs:35-36`), so >8 MiB bundles are not one GET. |
| SEC-12 egress and anonymous caps | open | Concrete cap breaches R34/R36/R43. Egress measures header Content-Length, defaulting missing length to zero (`edge.rs:325-346`), while shared JSON/Markdown constructors omit that header (`respond.rs:42-53`, `runs.rs:893-908`). Daily counters therefore cannot be relied on for these response bodies. Anonymous export 401 is met (R50). |
| SEC-13 verify/quarantine incl. demo/datasets | partial | Detail verification/quarantine and regenerated dataset event check (`runs.rs:119-175`, `registry.rs:134-143`), hash-backed analytics (`runs.rs:972-982`), test `api_v1.rs:766-907`. Lists skip summary verification (`runs.rs:275-280`), and anonymous cached responses bypass revalidation until expiry/generation change. Production corruption was not introduced. |
| SEC-14 inflate/cache bounds | met | Bounded/verify-before-inflate bundle path, member-bitflip/budget fixtures (`bb-artifacts/tests/bundle.rs:85-182`), byte-bounded bundle cache (`bb-app/artifacts.rs:173-255`). Not rerun. |
| SEC-15 storage keys | met | Filesystem traversal/symlink tests `bb-artifacts/tests/store.rs:10-77`; validation policy described in SECURITY_REVIEW §11.2 and implemented in `bb-artifacts/src/store.rs:38-147`. Not probed against production. |
| SEC-16 Blob SSRF/private store | partial | Fixed-host/no-redirect/sensitive bearer policy (`bb-artifacts/src/store.rs:264-346`), loopback mock tests in `tests/store.rs`; live public workflow does not expose Blob URLs. Anonymous GET of a private stored Blob object was not independently exercised in this audit. |
| SEC-17 generic errors | partial | Generic API errors `v1/error.rs:212-219`; worker panic/engine failures generic (`worker.rs:605-610,654-657`); actual timeout error R59 contains no internals. Full forced SQL/migration/I/O/panic leakage matrix not demonstrated. |
| SEC-18 log redaction | partial | Secret Debug redaction (`config.rs:31-46`), Blob bearer sensitivity (`bb-artifacts/store.rs:329-331`); no production log or complete sentinel-secret capture exercise. |
| SEC-19 no repo/bundle secrets | partial | Current Vite reads public base URL (`web/vite.config.ts:4-5`, `web/src/api/http.ts:9-13`); no CI repository+dist secret scan in `ci.yml`. A successful build is not evidence that the required secret scan ran. No secret exposure was observed by this audit. |
| SEC-20 Vercel project has no backend secrets | met | Explicit project inspection resolved `tanmay-alphas-projects/blackbox-hft`, root `web`. `vercel env ls --project blackbox-hft --scope tanmay-alphas-projects` listed **only VITE_API_BASE_URL (Production)**. No values were requested or downloaded. PROJECT_STATE:11 also records removal. |
| SEC-21 supply-chain CI gates | open | SHA pins/base-image policy and RustSec exist, but `ci.yml:77-115` lacks required cargo-deny, npm production audit/signature checks and pin-enforcement gate. Green CI does not satisfy this checklist. |
| SEC-22 provenance + actual digest deploy | partial | Successful image/attestation steps in publish run 43571890402; source requests SBOM/provenance/digest (`publish.yml:124-134,165-178`). That run's actual deploy-hook and smoke steps were skipped; current state records `:main`, not evidence of a deployed immutable digest. |
| SEC-23 workflow / hook protection | partial | Successful same-repo/main gate and explicit publish-job permissions (`publish.yml:20,37-42,73-77`); GitHub environments API returned `total_count:0`. No repository deploy-hook secret listed either: the hook is absent, not an exposed secret. The required protected production environment is not configured. |
| SEC-24 no user code execution | partial | Fixed strategy variants, no strategy-upload API; unwrapped-engine tests are research code, not execution of user code. Required cargo-deny/code-execution CI enforcement is absent (SEC-21); do not equate design restriction with a tested supply-chain guarantee. |
| SEC-25 transactional audit actions | partial | Mint/revoke/bootstrap insert audit rows in transactions (`bb-db/keys.rs:104-108,130-132,169-171`). HTTP submit/cancel return after mutation without audit (`v1/experiments.rs:227-313`, `v1/jobs.rs:170-190`); generic `Database::audit` is a separate pool write (`research.rs:244-256`), not transactional action logging. |
| SEC-26 key lifecycle/bootstrap | met | Tagged key/bootstrap tests `bb-db/tests/api_reads.rs:621-732`; CLI admin default30d/max90d and print-once (`main.rs:1357-1397`), 7-day non-resurrecting bootstrap (`runtime.rs:159-180`). No real production key was revoked for testing. |
| SEC-27 Neon verify-full | partial | Hosted config requires string `.contains("sslmode=verify-full")` (`config.rs:246-252`), and current state records verify-full. Substring matching does not prove the parsed effective sslmode (e.g. the text can be inside another query value); required self-signed-cert failure test not observed. |
| SEC-28 API headers/cache classes | met | Edge adds nosniff/CSP/Vary Authorization+Origin (`edge.rs:104-123`), private immutable/revalidate classes (`respond.rs:56-68`); R35/R37/R49 show appropriate cache classes and request ids. Header captures in the appendix collapse duplicate Vary fields; that must not be misread as proof Authorization is absent. |
| SEC-29 SPA CSP/rendering | met | Actual Vercel CSP R10 and source `web/vercel.json:17-29`; overview/pair Chromium checks had zero errors/error-console entries. No malicious persisted-label write was performed. |
| SEC-30 literal SQL | partial | Inspected queries are literal, with `.bind` (e.g. `queue.rs:203-294`, `api_reads.rs:247,290-308`); mandatory CI query-construction lint is absent. This is not an observed injection vulnerability. |
| SEC-31 pepper tag before DB | met | `bb-db/keys.rs:28-48` uses HMAC verify API; `v1/auth.rs:318-322` checks before key lookup; tests `bb-db/tests/api_reads.rs:18-32`, API test forged-key checks. After warm demo-workspace caching, invalid shape/tag does not need an auth query. No 10,000-request production flood performed. |
| SEC-34 platform guardrails | open | PROJECT_STATE:39-40 still says shared Render free hours with 8 services. No recorded dedicated-workspace decision/payment/usage-alert/WAF evidence in current state; GitHub/Vercel metadata checks do not establish these controls. No cloud setting was changed. |

## 4. Ranked remaining defects

No critical credential disclosure or production cross-workspace access bypass was observed. Severity below is impact-based; code-only consequences are marked **[INFERENCE]** and were not attacked on production.

### 1. High — Response/egress free-tier safety is not enforced

**Impact:** anonymous endpoints serve beyond the 1 MiB policy, keyed analytics beyond the global 4 MiB policy, and the intended daily egress brake can count such responses as zero. This is a practical availability risk on a shared free-tier service, not just a mismatch in documentation.

**Reproduction/evidence:** anonymous R34 `/runs/586374188aae19b2/analytics` → 200 **2,760,222 bytes**; anonymous R36 `/datasets/synthetic-demo/analytics` → 200 **1,266,155 bytes**. Keyed R43 `/runs/bb9865efdbbb4844/analytics` → 200 **4,568,100 bytes**, exceeding 4,194,304. `runs.rs:1025-1028` returns analytics without a byte cap; egress defaults absent Content-Length to zero (`edge.rs:325-346`); JSON/Markdown constructors never set it (`respond.rs:42-53`, `runs.rs:893-908`). **[INFERENCE]** these bodies evade accounting at that middleware point; no bandwidth-exhaustion experiment was run.

**Recommended fix:** enforce one central response-byte accounting/cap path, including cached bodies; use exact body size hints or bounded serialization rather than trusting a header attached later by HTTP transport. Atomically reserve/account egress, fail closed on persisted-counter load failure, and page large analytic series/curves while keeping aggregate scalars intact. Add small-budget tests proving nonzero counts, restart persistence, and anonymous/keyed caps without sending large production traffic.

### 2. Medium — Normal comparison hits the mutation timeout

**Impact:** a legitimate completed run cannot reliably progress into comparison/eligibility/reporting. The accepted job's bundle is only ~2.1 MB.

**Reproduction/evidence:** R59, one keyed `POST /comparisons {run_a:"bb9865efdbbb4844",run_b:"586374188aae19b2",label:"FINAL_AUDIT interval eligibility"}` → 503 `busy` after 10.313 s. Request id above. All mutations share 10 s (`edge.rs:213-223`); comparison opens/decodes both full bundles and computes before responding (`comparisons.rs:107-155`), even before returning a metrics-only result.

**Recommended fix:** determine eligibility from verified metadata before full tape loads; avoid alignment work for ineligible pairs; use a queued comparison computation or a separately bounded heavy-operation path rather than merely increasing every mutation timeout. Check whether cancelled HTTP work continues before retrying automatically. The precise production time split is unmeasured; no CPU-vs-I/O cause is asserted.

### 3. Medium — Public demo has no comparisons, saved reports or curated comparison workflow

**Impact:** the principal anonymous “why did outcomes differ?” story stops at the pair/set workspace. Visitors cannot create the missing resource; a working protected creation API does not fix their experience.

**Reproduction/evidence:** R6/R7 empty lists; real browser pair page says NO STORED COMPARISON and requires a key. `seed.rs:63-117` only queues/pins experiment runs; it never materializes comparisons/reports/sweeps promised by API_CONTRACT:84-88.

**Recommended fix:** after demo jobs finish, idempotently create the curated comparisons, representative sweep and saved reports in demo via a supported trusted seed path; add an anonymous deployed story that opens each curated comparison and report without a key. Do not weaken anonymous mutation protection.

### 4. Medium — Publish success does not mean deployment; production is behind the supplied source

**Impact:** operators/reviewers can attribute current fixes to a deployment that is actually older; CI green is not end-to-end release evidence.

**Reproduction/evidence:** R3 advertises `1873f4d…`, not supplied `e0384a7`. `gh run view 43571890402 --json jobs` showed image/attestation success but **Deploy hook not configured** success, **Trigger Render deploy** skipped, **Smoke test the deployed revision** skipped. Run 43586725211 for e0384a7 is itself skipped. `gh api .../environments` returned zero; `.../actions/secrets --jq '.secrets[].name'` returned no secret names. Source intentionally makes missing-hook publication green (`publish.yml:148-164`).

**Recommended fix:** explicitly separate publish-only from release success; configure an immutable-digest deployment with protected production environment and SHA-checked smoke gate, or document the approved manual digest-release procedure and actual deployed SHA/digest. Do not publish a “deployed” conclusion from image-only jobs. This audit did not change that configuration.

### 5. Medium — Integrity contract has read-path exceptions

**Impact:** **[INFERENCE]** a changed DB metrics row can appear in listing/overview scientific results before a verified detail read quarantines it. A warmed cached response can also mask later corruption until invalidation.

**Reproduction/evidence:** `v1/runs.rs:275-280` explicitly maps DB rows to summaries without `verified_summary`; detail invokes verification at `:313-314`. The existing tampered-metric test checks detail, not the list (`tests/api_v1.rs:809-829`). This is a source-backed gap; production tampering was not attempted.

**Recommended fix:** verify trusted row summary digests in DB-only lists without fetching artefacts (canonicalize the digest representation consistently), enforce quarantine visibility/cache invalidation, and test that a tampered summary cannot escape through any list, experiment, comparison or report path. Define precisely what verified-cache freshness guarantees mean after an external object replacement.

### 6. Medium — Warm anonymous traffic can still keep Neon active

**Impact:** **[INFERENCE]** low-rate public traffic can repeatedly wake/retain DB compute despite SEC-10's zero-query goal.

**Reproduction/evidence:** URI-keyed anonymous cache has 30 s/2 s TTL and generation invalidation (`edge.rs:363-405`); after it misses, `/runs` calls DB (`runs.rs:258-268`). A client can alternate otherwise harmless query strings without flooding. No keep-awake test was run.

**Recommended fix:** build a versioned, prewarmed demo snapshot for public read models with normalized query handling; keep health/status memory-only and measure zero DB query deltas under a bounded local/mock test. Explicitly document unavoidable startup/egress-accounting database activity rather than claiming all anonymous traffic is DB-free.

### 7. Medium — Security close-out gates are incomplete

**Impact:** shared free-tier storage can be monopolized by one key; ordinary actions have no complete audit trail; required supply-chain checks are not enforced.

**Evidence/reproduction:** R9 active-job limit 50 versus required 5; submission uses global Blob allowance but no workspace monthly share (`queue.rs:254-258`); upload retry attempts are not charged individually (`artifacts.rs:148-165`). HTTP mutation paths have no transactional audit insertion (§3 SEC-25). CI lacks cargo-deny/npm audit/signature/pin/schema-drift/secret gates (§3 SEC-19/21/24/30, AT-14). These were code-reviewed, not stress-tested.

**Recommended fix:** implement scoped upload reservations/attempt charging and the actual hosted defaults; add request/key/outcome audit inserts inside each mutation transaction; add the named CI enforcement gates and small deterministic fixtures. Retain existing fair claim instead of reintroducing deprecated FIFO.

### 8. Medium — Advertised no-database quick start serves an incompatible legacy API

**Impact:** **[INFERENCE from direct route/client tracing]** a clean new build launched by the README's quickest-look command cannot populate the current console.

**Reproduction/evidence:** current README:89-95 builds the only UI and runs `blackbox demo`; `main.rs:1511-1524` serves only `legacy_router` (`/api`), while current browser client defaults to `/api/v1` (`web/src/api/http.ts:9-20`). No static/legacy client build exists (`web/package.json:7-13`, `vite.config.ts:12-28`). Current `docs/DEPLOYMENT.md:32` still advertises that legacy server for a quick look. This local route mismatch was not smoke-run.

**Recommended fix:** make the documented quick start launch the supported local v1 stack, or implement a real supported DB-free v1 demo. Remove obsolete legacy fallback/claims only with a deliberate clean cutover; do not describe a removed static viewer as a workaround.

### 9. Low — Idle-worker heartbeat produces false stale warnings

**Impact:** healthy idle service looks broken; stale status cannot distinguish an intentionally parked in-process worker from a dead one.

**Reproduction/evidence:** actual browser showed idle heartbeat age 81 s, then 119 s, with 15 s advertised. `worker.rs:388-408` records idle heartbeat, then waits forever when no work is due in in-process mode. New submissions did start and complete.

**Recommended fix:** expose intentionally parked/idle semantics and track process liveness without DB heartbeat polling, or maintain a memory-only idle heartbeat. Do not wake Neon merely to eliminate a cosmetic stale flag.

### 10. Low — TLS policy validation checks a substring, not the effective option

**Impact:** **[INFERENCE]** an operator-supplied hosted URL containing `sslmode=verify-full` inside another parameter can pass the guard while its effective sslmode is weaker. There is no evidence current production TLS is weakened.

**Evidence/reproduction:** `config.rs:246-252` uses `.contains`; conceptual counterexample `?sslmode=require&application_name=sslmode=verify-full`. No connection attempt with weaker TLS was made.

**Recommended fix:** validate parsed SQLx connection options and add duplicate/encoded-parameter and self-signed-certificate tests. Keep verify-full on the actual production connection.

The missing multi-seed API (AT-12) and contract gate (AT-14) are also outstanding delivered-scope gaps; library-only implementations are not endpoint support.

## 5. Documentation truth inventory

This is a list of current misleading claims **and** the removed-static-viewer claims found during the audit. Historical measurements and explicitly commit-pinned audit documents are not automatically false merely because the system changed.

| Document / anchor | Claim and verdict now | Required correction |
|---|---|---|
| README originally read at lines19-21 | Public URL “precomputed … read-only” and `docs/DEPLOYMENT.md#hosted-viewer-static`. **Corrected during this audit**: latest README:19-24 describes the live API/keyed jobs. Do not report the old wording as still present. | Retain new wording; fix separate quick-start mismatch below. |
| Original docs/DEPLOYMENT.md:78-95 | `blackbox export-static`, `npm run build:static`, manual prebuilt bundle workflow. **Corrected during this audit**: latest DEPLOYMENT is 45 lines and documents the hosted stack. No `export-static` CLI variant (`main.rs:54-130`) or `build:static` script (`web/package.json:7-13`) remains. | Remove residual links to its deleted static-viewer anchor wherever used; do not run these removed commands. |
| README:89-95; latest DEPLOYMENT:32 | No-database `blackbox demo` provides a quick UI look. **Still misleading** with current v1-only client; source mismatch §4.8. | Document a working current quick-start path. |
| docs/ARCHITECTURE.md:57-59,104-105 | Current data flow uses synchronous `/api/runs`, file-store lookup and JSON export. It omits simulation revision from the depicted identity. **Outdated as primary architecture**, though offline legacy code remains. | Label historical/local-only and link the real v1 queue/bundle architecture; include simulation revision. |
| docs/ARCHITECTURE.md:158-159 | Hosted viewer and Docker image “planned work”. **False as current status**; actual production responds and CI has Docker/publish jobs. | Replace with implemented hosted design, preserving historical notes only as dated history. |
| docs/SECURITY.md:127-131 | Static public viewer “planned”, “no server … no API”, “not present”. **Not the current product**; static mode was removed and hosted API exists. | Rewrite for hosted auth/queue/storage model and link SECURITY_REVIEW plus this verified status. |
| docs/SECURITY.md:158,176-184 | No authentication/cancellation; fills/equity unpaginated; two old semaphores/current global limits. **False for hosted v1**; can apply only to clearly labelled deprecated `/api` tooling. | Separate local legacy residual risks from hosted v1. Do not erase the real response-cap defects discovered here. |
| docs/TESTING.md:14-17,131-132 | Old numeric inventory explicitly dated, but unqualified “no component or browser end-to-end tests yet”. **False**: Vitest sources and Playwright v1 stories/CI exist. | Keep measured historical counts dated or replace them with links; describe real browser/DB coverage and skip conditions. |
| docs/API.md:23-24 | Legacy router “is removed when the web client switches to /api/v1”. **Stale cutover condition**: the client already switched; legacy remains. | State actual retained compatibility scope and resolve quick-start/cutover deliberately. |
| docs/engineering/PROJECT_STATE.md:11 | SEC-20 backend-secret removal. **Confirmed** by current Vercel env-name listing; no longer a finding. | Keep explicit verification date. |
| PROJECT_STATE:12,17-21 | Image `:main`/smoke-success prose does not identify currently deployed SHA/digest. **Incomplete release provenance**: actual1873f4d vs supplied e0384a7, publish deploy skipped. | Record exact observed deployed SHA/digest and whether release was manual; separate historical smoke result from latest-source deployment. |
| PROJECT_STATE:58-61 | “AT-5 runtime-breach test missing.” **False at inspected source**: `tests/budgets.rs:171-242` exists. Full-worker peak/residual byte-allocation coverage remains incomplete. | Name the specific missing hosted-worker test, not absence of the existing pipeline fixture. |
| API_CONTRACT:84-88 | Curated demo includes comparisons, sweep, saved reports. **False in observed production**; lists empty and seed only queues runs. | Implement demo materialization or explicitly report the gap; do not silently weaken promised demo behaviour. |
| API_CONTRACT:139-150 / SECURITY_REVIEW SEC-12 | Hard 4 MiB responses / 1 MiB anonymous cap. **Contradicted by R34/R36/R43**. | Fix enforcement, then record verified bounded behaviour. |
| SECURITY_REVIEW:147-158,644-675 | “HTTP/auth not implemented”, old open/met statuses, old VITE_STATIC_DATA and static-build claims are a **dated Phase0/2 snapshot**, not today's mitigation verdicts. | Preserve historical provenance, prominently link this re-check; do not copy its unverified “met” labels as proof. |
| BASELINE_AUDIT static/export-static references | Explicitly pinned to5fce27d. **Historical evidence, not a current deployment instruction**. | Retain history; use this report for current verdicts. |

## 6. Remaining verification boundaries

No claim is made of production restart survival, arbitrary workspace isolation against every route, real-key revoke/expiry, private Blob anonymous-denial re-check, deployed image-digest attestation verification, worst-case full-server memory, strict no-DB anonymous traffic, resource-limit exhaustion, or queue-model calibration against real executions. Existing fixtures/source are cited where inspected. Those boundaries must not be converted into “passes” merely because CI is green.

## 7. Fixes after the audit

Source fixes on `main` after this audit (not yet deployed; production status must be re-checked with `/api/v1/status` `git_sha`). Verified locally: `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` against local Postgres (406 passed, 1 ignored), plus a manual run of the documented local flow (`migrate`, `demo seed --wait`, `keys mint`, `serve --web-dir web/dist`). Docker was not run locally; the compose stack is checked by CI's `docker` job.

| §4 item | Status | Commit | Change | Tests / evidence |
|---|---|---|---|---|
| 1 Response/egress caps | fixed | `4146db9` | One enforcement point (`edge::egress`): the serialized body is measured, a body above the caller's cap (anonymous 1 MiB, keyed `BB_MAX_RESPONSE_BYTES`) is refused with 500 `internal`, `details.reason = "response_too_large"`; the egress counter adds the exact body length (cached bodies included). Analytics keep every aggregate and downsample series > 100 points (`downsampled`), complete points via `/runs/{id}/analytics/series` and `/datasets/{id}/versions/{v}/analytics/series`. Anonymous replay is limited to the anonymous cap (400 with advice) | `tests/demo_workflow.rs`: anonymous analytics of all six demo runs (the production demo run ids) 462–464 KB each (was 2,760,222 B for R34), dataset analytics 259,867 B (was 1,266,155 B for R36), egress delta = body length for each, every downsampled series paged in full; `oversize_bodies_are_refused_and_egress_counts_true_bytes` (4 KiB anonymous cap → refusal; keyed and small bodies counted exactly) |
| 2 Comparison timeout | fixed | `4146db9` | Eligibility from the verified bundle trailers first (`bb_forensics::compare_metadata`); ineligible pairs never decode orders/tape. `POST /comparisons` has `BB_COMPARISON_TIMEOUT_S` (default 60 s); alignment runs in blocking work holding its heavy permit and stops (`compare_runs_until`) when the request is abandoned. Existing comparisons return without recomputation | bb-forensics `metadata_comparison_matches_the_full_one_for_ineligible_pairs_only` (identical result, cancellation stops); demo-sized aligned comparison 0.78 s in `demo_workflow`; `api_v1` interval-mismatch comparison still green |
| 3 Demo comparisons/reports | fixed (source) | `4146db9` | `blackbox demo seed [--wait]` stores 5 curated comparisons (first run vs each other run of cost of latency, queue position, gross vs net) and one saved report per demo in `demo` through the same code as `POST /comparisons`/`/reports`; prints created/existing/pending; anonymous mutation protection unchanged | `demo_workflow`: 5 + 3 created, second seed all `existing`, anonymous reads of every comparison, comparison report and saved report; manual CLI run on local Postgres. Production needs the seed re-run after deploy (DEPLOYMENT §8 step 8) |
| 4 Publish ≠ deploy | fixed (workflow + docs) | `291a04a` | Without `RENDER_DEPLOY_HOOK_URL` the `deploy` job is skipped and a `publish only (not deployed)` job posts a `Not deployed` notice and summary. Manual digest release via `POST https://api.render.com/v1/services/{id}/deploys` with `imageUrl` documented (DEPLOYMENT §2) | YAML parsed; takes effect on the next publish run. No secrets added; production environment protection (SEC-23) still open |
| 5 Integrity read-path exceptions | fixed | `0900c63`, sweep merge | Migration 0009: `runs` and `comparisons` carry `summary_row_sha256`, written by a BEFORE INSERT trigger (`bb.run_summary_digest` / `bb.comparison_summary_digest` over Postgres's canonical jsonb text, so jsonb number normalisation never mismatches) and recomputed on every read. Run, experiment, comparison and report lists check it without opening artefacts: a changed run is quarantined and left out (and the anonymous cache's read version changes with the quarantine counter); changed comparisons and report bodies are left out of lists and answer `integrity_failed` on detail. Sweep cells (`sweep_cell_views`) join only runs whose row digest matches, so sweep views and reports never show a changed run. Detail run reads check the row digest before the bundle trust root. Cache freshness: a cached anonymous response never outlives a quarantine in this process; an out-of-band DB edit is caught by the next uncached read | `tests/hardening.rs::tampered_summary_rows_never_escape_a_list`: tampered `metrics.net_pnl` absent from `/runs` (all sorts, `run_id`, `label_prefix`, `limit`), `/experiments`, `/comparisons`, `/reports`, and anonymous demo `/runs`/`/experiments` after a warmed cache; run quarantined, detail 500; tampered comparison/report rows omitted, detail 500. `sweep_views_and_reports_leave_out_tampered_runs` for sweeps |
| 6 Warm anonymous traffic keeps Neon active | **open** | – | Not delivered. A snapshot design (prebuilt demo rows answering `/runs`, `/experiments`, `/comparisons`, `/reports`, `/datasets`, `/presets`, `/demos` for any query string from memory, rebuilt on `data_generation` change or after 1 h, plus normalised anonymous-cache keys) was drafted but not wired or tested | No 1,000-varied-query zero-DB test exists; the existing SEC-10 test covers fixed URIs only |
| 7 Security close-out gates | fixed (quotas, uploads, audit) | `d4d9129`, sweep merge | `quota_active_jobs` default 5 (rows on the old 50 move to 5); live units = `count(DISTINCT coalesce(sweep_id, job_id))`, so a sweep is one admission unit (API_CONTRACT §5); `POST /sweeps` submits every run with its sweep id. `quota_uploads_per_month` (300) + `bb.workspace_uploads_monthly`: every Vercel Blob upload attempt, retries included, is reserved against the global 1,800/month and the submitting workspace's share before the PUT; a spent share or failed reservation refuses the upload (job `storage_budget`), admission refuses new runs (429 `storage`). `bb_db::Audit`: experiment.create/rename/delete, job.cancel, sweep.create/rename/cancel/delete, comparison.create/rename/delete, report.create/delete, key.revoke write one `audit_events` row (workspace, key, request id, target, `ok`) inside the mutation's transaction. CI gates were closed by `59c43c7` | `tests/hardening.rs`: `active_jobs_admit_five_units_and_a_sweep_counts_once` (8-run seed ensemble via `POST /sweeps` + 4 singles admitted, 5th single 429 limit 5 used 5), `blob_uploads_are_reserved_per_attempt_against_the_workspace_share` (503 retry charged twice, refused retry sends no PUT, 429 at the spent share, other workspace unaffected), `every_v1_mutation_writes_one_transactional_audit_row` (one row per action with key id and the response's request id, sweeps included; stale version, terminal-job cancel and repeated delete leave none), `sweep_views_and_reports_leave_out_tampered_runs` (a changed run row loses its run id and metrics in the sweep view; its report cell carries an integrity error, never the value). **Not covered:** `quota.exceeded` denial rows |
| 8 Quick start | fixed | `1d383a6` | `docker-compose.yml` (postgres:18 + `local` image: migrate, `demo seed --wait`, first-start key printed once, `serve` with the console on 127.0.0.1:8787); README and docs/DEPLOYMENT.md updated; `blackbox demo` states that the console needs `blackbox serve` (v1) | `docker compose config` valid; CI `docker` job now runs the compose stack and checks status, datasets, 5 comparisons, 3 reports, the console HTML and the printed key. Non-Docker route run manually (console 200, 5 comparisons, 3 reports) |
| 9 Idle heartbeat | fixed | `4146db9` | Memory-only liveness ticker refreshes the `/status` heartbeat while idle (no DB access); busy heartbeat unchanged. Semantics in API_CONTRACT §9.1 | `demo_workflow`: after 4 intervals idle, `worker.state = idle` and age < 3 intervals |
| 10 sslmode substring | fixed | `4146db9` | Hosted URLs validated by the effective `PgSslMode` parsed by SQLx | `config::tests::hosted_database_urls_need_the_effective_verify_full_mode` (`?sslmode=require&application_name=sslmode=verify-full` rejected, duplicate/encoded cases, valid verify-full accepted). Self-signed-certificate test still not done |

## Appendix A. Explicit HTTP request ledger

Paths beginning `/` below are relative to `/api/v1`, **except** `/healthz`, `/readyz`, `/readyz?probe=1`; the console URL is shown in full. R14–R28 all target the same job; omitted individual rows R15–R27 were 200 `running`, same sampled progress, no errors. Elapsed time is client wall time, not CPU time. Sizes are actual response-body bytes read, not claimed wire-compressed sizes.

| Evidence | Method / path | HTTP | Bytes | Seconds | Request id |
|---|---|---:|---:|---:|---|
| R1 | GET `/healthz` | 200 | 15 | 0.437 | `01a1d9a7-a1d3-7667-af34-773e906a96fa` |
| R2 | GET `/readyz` | 200 | 68 | 0.344 | `01a1d9a7-a333-7342-a7ed-f001d20e00b4` |
| R3 | GET `/status` | 200 | 963 | 0.265 | `01a1d9a7-a439-70a2-ad94-548178874616` |
| R4 | GET `/runs?limit=3` | 200 | 9,614 | 0.797 | `01a1d9a7-a522-7451-96dc-5faed23c6443` |
| R5 | GET `/presets` | 200 | 13,095 | 0.266 | `01a1d9a7-a855-746a-865a-549b225789fa` |
| R6 | GET `/comparisons?limit=3` | 200 | 51 | 0.266 | `01a1d9a7-a963-706e-95b0-37116a29f765` |
| R7 | GET `/reports?limit=3` | 200 | 51 | 0.234 | `01a1d9a7-aa51-7297-97f3-8e42f6bbb8ee` |
| R8 | GET `/openapi.json` | 200 | 331,687 | 0.469 | `01a1d9a7-ab50-7158-bb45-9198b8aa2894` |
| R9 | GET `/whoami` | 200 | 388 | 0.250 | `01a1d9a7-dff5-713f-a140-21732a4e7b1b` |
| R10 | GET `https://blackbox-hft.vercel.app/` | 200 | 897 | 0.203 | `—` |
| R11 | POST `/experiments` | 422 | 433 | 0.250 | `01a1d9a8-0941-768a-94c1-df5b63f946ba` |
| R12 | POST `/experiments` | 422 | 433 | 0.360 | `01a1d9a8-0aa6-7051-9c83-075c146d2cc9` |
| R13 | POST `/experiments` | 202 | 2,180 | 0.313 | `01a1d9a8-5e1b-707d-8b54-372f5cb2b293` |
| R14 | GET `/jobs/01a1d9a8-5e4c-7257-8595-9f27e49baa3e` | 200 | 749 | 0.281 | `01a1d9a8-5f75-7089-ab3a-b0646d134188` |
| R28 | GET `/jobs/01a1d9a8-5e4c-7257-8595-9f27e49baa3e` | 200 | 800 | 0.250 | `01a1d9a8-a4a8-7024-9457-5e573612fd10` |
| R29 | POST `/experiments` | 202 | 2,169 | 0.328 | `01a1d9a8-e779-715c-832c-c0ec749c2bfd` |
| R30 | POST `/experiments` | 202 | 1,998 | 0.578 | `01a1d9a8-e91e-7120-bddb-9763f08a090a` |
| R31 | POST `/jobs/01a1d9a8-e9a7-7821-9929-568b00a3de52/cancel` | 202 | 693 | 0.469 | `01a1d9a8-eb2b-73ac-bdd9-627caab5cdc8` |
| R32 | GET `/runs/586374188aae19b2` | 200 | 3,855 | 0.828 | `01a1d9a8-ecc9-7770-9d69-5ec9c665754f` |
| R33 | GET `/runs/586374188aae19b2/orders?limit=1` | 200 | 884 | 1.781 | `01a1d9a8-f03f-7575-b7f2-9cdf16cd3c50` |
| R34 | GET `/runs/586374188aae19b2/analytics` | 200 | 2,760,222 | 2.969 | `01a1d9a8-f6fd-75e0-951d-a18112ee0506` |
| R35 | GET `/runs/586374188aae19b2/report` | 200 | 25,406 | 5.750 | `01a1d9a9-02ab-7223-83bc-e331f80e6a46` |
| R36 | GET `/datasets/synthetic-demo/analytics` | 200 | 1,266,155 | 0.781 | `01a1d9a9-191c-73f8-b347-aca2123bb669` |
| R37 | GET `/runs/bb9865efdbbb4844` | 200 | 3,865 | 0.250 | `01a1d9a9-1c29-74c7-9d95-fe6b68c62e76` |
| R38 | GET `/runs/bb9865efdbbb4844/equity?max_points=1` | 200 | 239 | 1.531 | `01a1d9a9-1d1a-7566-aefd-654e14a0ada6` |
| R39 | GET `/runs/bb9865efdbbb4844/fills?limit=5000` | 200 | 560,677 | 0.813 | `01a1d9a9-2312-74c4-9b94-6bc22695e8a2` |
| R40 | POST `/comparisons` | 201 | 7,477 | 6.516 | `01a1d9a9-6011-72c9-9a65-670f92ce73bc` |
| R41 | GET `/runs/586374188aae19b2/orders/0` | 200 | 8,861 | 1.578 | `01a1d9a9-79ed-7237-b813-81be6f214e09` |
| R42 | GET `/runs/bb9865efdbbb4844/equity?t0_ns=119990000000&max_points=20` | 200 | 933 | 0.531 | `01a1d9a9-7fa7-7757-9c58-aa5d546954c6` |
| R43 | GET `/runs/bb9865efdbbb4844/analytics` | 200 | 4,568,100 | 2.484 | `01a1d9a9-81d4-7147-a23c-e8f7e2aec0e8` |
| R44 | GET `/runs/bb9865efdbbb4844` | 404 | 137 | 0.219 | `01a1d9a9-8b88-70e8-9e13-92294251b1c7` |
| R45 | GET `/presets` | 401 | 166 | 0.234 | `01a1d9a9-8c77-718a-af98-6fb81761f900` |
| R46 | OPTIONS `/presets` | 204 | 0 | 0.282 | `01a1d9a9-8d96-76b2-a90d-6a4ff10f173b` |
| R47 | OPTIONS `/presets` | 403 | 151 | 0.203 | `01a1d9a9-8e6c-703e-a263-e7f6a9fbb652` |
| R48 | GET `/presets` | 403 | 151 | 0.219 | `01a1d9a9-8f46-702c-af85-68277364ca15` |
| R49 | GET `/readyz?probe=1` | 403 | 168 | 0.250 | `01a1d9a9-902c-7329-baf6-8e7eb2ad121f` |
| R50 | GET `/runs/586374188aae19b2/export` | 401 | 174 | 0.234 | `01a1d9a9-9128-719c-a471-97e4d7999562` |
| R51 | POST `/comparisons` | 401 | 174 | 0.219 | `01a1d9a9-91fe-7255-b5cb-fa7e67063ec0` |
| R52 | GET `/comparisons/01a1d9a9-77f3-7d7d-b7ca-2c2137e8da61` | 200 | 7,477 | 0.562 | `01a1d9aa-31ac-71d7-b72e-4d9bbd888040` |
| R53 | GET `/comparisons/01a1d9a9-77f3-7d7d-b7ca-2c2137e8da61/pairs?limit=1` | 200 | 1,246 | 0.297 | `01a1d9aa-3372-704f-baa8-6e25837c2a0e` |
| R54 | GET `/comparisons/01a1d9a9-77f3-7d7d-b7ca-2c2137e8da61/report` | 200 | 3,719 | 0.500 | `01a1d9aa-34e2-77c4-84a0-fbab7acb3558` |
| R55 | GET `/datasets/synthetic-demo/versions/1/replay?t0_ns=200000000&t1_ns=200000001&step_ns=1&levels=1` | 200 | 407 | 0.234 | `01a1d9aa-3688-747f-8971-60625891abd1` |
| R56 | GET `/experiments/01a1d9a8-5e51-7fdf-825c-1498e5656193` | 200 | 5,201 | 0.250 | `01a1d9aa-377c-7796-b47c-c4fe800342cb` |
| R57 | GET `/jobs/01a1d9a8-e9a7-7821-9929-568b00a3de52` | 200 | 692 | 0.235 | `01a1d9aa-3878-75f2-a48c-a4b5c81d6afb` |
| R58 | GET `/runs/c86efea35f40ed31` | 404 | 137 | 0.250 | `01a1d9aa-3966-7098-aa8d-2c063a72617f` |
| R59 | POST `/comparisons` | 503 | 158 | 10.313 | `01a1d9ab-1e75-72f5-a769-f525bceb47f7` |
| R60 | GET `/runs/bb9865efdbbb4844` | 200 | 3,865 | 0.266 | `01a1d9ab-89ea-7473-82e0-0f54fab23f5f` |
| R61 | GET `/jobs/01a1d9a8-e7bf-7ab8-8989-ef374edb034a` | 200 | 800 | 0.219 | `01a1d9ab-8ad0-75c4-b1a5-a3a867f29b5b` |

### Control-plane commands (read-only)

- `vercel project inspect --non-interactive` from `web/`: `link_required`; no link was created. Explicit `vercel project inspect blackbox-hft --non-interactive` resolved the intended owner/project and root `web`.
- `vercel env ls --project blackbox-hft --scope tanmay-alphas-projects`: only `VITE_API_BASE_URL`, Production. No backend variable names; no env values were fetched.
- `gh api repos/tanmay-alpha/BlackBox-HFT/environments`: `{"total_count":0,"environments":[]}`. Repository Actions secrets listing returned no names.
- `gh run list --workflow publish --limit 3 --json conclusion,headSha,status,url`: e0384a7 publish skipped;1873f4d publish success. `gh run view 43571890402 --json jobs` showed image attestation succeeded but deployment and smoke skipped because no hook. Evidence URLs: https://github.com/tanmay-alpha/BlackBox-HFT/actions/runs/43571890402 and https://github.com/tanmay-alpha/BlackBox-HFT/actions/runs/43586725211.
