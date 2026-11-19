# Phase 10: entry-chunk LCP experiment

**Decision: do not ship the lazy-boundary candidate.** Median LCP improves by only
**92 ms on landing/Explore and 8 ms on Replay**, below the requested 150 ms gate.
All candidate `web/src` changes were removed. This commit contains only the
attribution/measurement tooling, two shell-interaction scenarios, and this report.

## Scope and bundle attribution

The baseline is `origin/main` at `fcd50b1f9ef1d6b5cd1bc43fdb06f7cefcd09127`.
Both builds use the same installed dependencies and `vite build --sourcemap`.
`web/perf/bundle-bytes.mjs` attributes each generated segment through the next
source-map segment to its source module, counts UTF-8 bytes, and explicitly retains
unmapped bytes. Its module totals reconcile exactly to the emitted file size;
individual module boundaries remain source-map attribution, not separate gzip sizes.
No bundle visualiser or new dependency is used. Source maps are not requested by
the browser during the performance runs.

The baseline entry is `index-DB9N5FUe.js`, **411,628 bytes (401.98 KiB)**; the
candidate is `index-B59IlTiH.js`, **400,167 bytes (390.79 KiB)**. The reduction is
**11,461 bytes (2.78%)**. The older Phase 6 entry-size estimate is not this baseline.

| Entry module | Baseline bytes | Candidate bytes |
|---|---:|---:|
| react-dom client production | 205,560 | 205,559 |
| react-resizable-panels | 49,053 | 49,045 |
| react-router core | 39,224 | 39,227 |
| Shell.tsx | 13,593 | 12,964 |
| api/normalize.ts | 9,513 | 9,513 |
| React production | 8,091 | 8,091 |
| api/http.ts | 7,551 | 7,551 |
| api/client.ts | 6,390 | 6,390 |
| StatusBar.tsx | 6,072 | 6,072 |
| components/ui.tsx | 5,818 | 5,818 |
| App.tsx | 5,487 | 5,540 |
| HelpDialog.tsx | 4,688 | 0 |
| access.tsx (includes key dialog in baseline) | 3,797 | 2,300 |
| CommandPalette.tsx | 2,516 | 0 |
| glossary.ts | 2,326 | 0 |
| Settings provider | 957 | 957 |
| Unmapped generated code | 3,386 | 3,483 |
| All remaining modules | 37,606 | 37,657 |
| **Total** | **411,628** | **400,167** |

Full per-module attribution is in local `data/perf/p10-base/entry-base.json` and
`data/perf/entry-final.json`. Reproduce with:

```sh
npx vite build --sourcemap
node perf/bundle-bytes.mjs dist
```

## Candidate boundaries

The experiment defers the command palette, Help (including arrangement settings),
inspector presentation/glossary, and research-key dialog until first opened. Small
context providers stay eager: settings are required to render the shell, access
validates identity and gates controls, health drives the status bar, and inspector
context is used by workspaces. Once opened, the palette/help/key dialog remain
mounted with their original open/closed lifecycle. The shell, routing, API client,
and existing route boundaries are unchanged. The first replay window stays **10 s**.

The largest avoidable-looking dependency, `react-resizable-panels` (49,053 bytes),
is also the shell workspace's always-mounted layout parent. Switching that parent
type on first inspector open would remount the active workspace and lose its local
state; this experiment does not make that behavioural change. React and the router
are required for first paint. Consequently the safe on-demand features remove much
less than the full entry size.

## Measurement method

`web/perf/lcp-ab.mjs` runs `web/perf/run.mjs` against a real local API. The order is
base 1, candidate 1, base 2, candidate 2, base 3, candidate 3; each build repetition
measures Explore load, Replay load, cold palette opening, and cold Help opening in
separate cold browser contexts. `/` is both the landing page and Explore; they are
not counted as two independent routes. The profile is 390 × 844, DPR 3, 4× CPU,
1.6 Mbit/s down, 750 kbit/s up, 150 ms RTT. These are local, uncompressed lab
measurements, not field INP.

Before **every navigation**, the runner executes
`delete from bb.egress_usage_daily where day >= (now() at time zone 'utc')::date - 1`
in `blackbox_e2e_ui`, starts a new server, waits for `/api/v1/status`, and stops the
server after the run. Both builds use the same copied binary
`data/e2e-ui/blackbox-perf.exe`, data directory `data/e2e-ui`, database, port 18810,
and `BB_CORS_ALLOWED_ORIGINS=http://127.0.0.1:18810`; **only `--web-dir` differs**.
The runner removes `BLOB_READ_WRITE_TOKEN`, `DATABASE_URL_UNPOOLED`, and
`BB_API_KEY_PEPPER` from the server environment. It does not touch production.

```sh
# From web/; paths are resolved relative to the worktree root.
node perf/lcp-ab.mjs \
  --bin data/e2e-ui/blackbox-perf.exe \
  --base-dir data/perf/p10-base/web/dist \
  --final-dir data/perf/p10-final \
  --psql C:/Users/TANMAY/.blackbox/pgsql/postgresql-18.7.0-x86_64-pc-windows-msvc/bin/psql.exe \
  --reps 3 --label p10-ab
```

Raw browser measurements are local `data/perf/p10-ab-{base,final}-{1,2,3}-{scenario}/mobile.json`;
per-navigation server logs are in `data/perf/p10-ab-session/`. These large local
artifacts are git-ignored. Opening time is `result_ms`, from the captured trusted
input event to the first frame containing the visible dialog controls, including
any on-demand network fetch. Event Timing `interaction_ms` is reported separately:
it does not include the entire asynchronous module download.

Aggregate the exact navigation files with `node perf/lcp-summary.mjs p10-ab 3`.

## Results

Measured on 2026-11-19, 02:27:12–02:32:15 UTC: Windows 11 x64, i7-14650HX
(24 logical CPUs), 15.7 GiB RAM, Node 24.14.1, Playwright 1.65.0, headless
Chromium 158.0.8127.18. The local Playwright suite was running concurrently;
per-run machine CPU utilisation was 29.04–39.87%, median 31.04%. The same-session
alternation limits drift but does not make these field measurements.

All timings below are **medians of 3 per build**, in ms; JS is actual CDP encoded
bytes transferred before interaction. CLS is the **maximum of 3**, unitless.
Each arrow is baseline → candidate.

| Scenario | LCP ms | FCP ms | TBT ms | JS bytes | CLS max |
|---|---:|---:|---:|---:|---:|
| Landing / Explore load | 4,072 → 3,980 | 804 → 804 | 334 → 295 | 695,418 → 683,957 | 0.0004132445 → identical |
| Replay load | 5,032 → 5,024 | 784 → 788 | 367 → 365 | 616,301 → 607,543 | 0.0013359384 → identical |
| Explore before palette | 4,056 → 4,004 | 796 → 788 | 293 → 299 | 695,418 → 683,957 | 0.0004132445 → identical |
| Explore before Help | 4,056 → 3,980 | 796 → 788 | 319 → 297 | 695,418 → 683,957 | 0.0004132445 → identical |

Raw LCP samples, in repetition order:

| Route | Baseline ms | Candidate ms |
|---|---|---|
| Landing / Explore | 4,072; 4,072; 4,012 | 4,012; 3,980; 3,972 |
| Replay | 5,008; 5,076; 5,032 | 5,056; 5,020; 5,024 |

The LCP elements remain `h1.ex-question` on Explore (32,085 px²) and
`h1.term-title` on Replay (15,980 px²). No API errors occurred in any of the 24
navigations or their interactions. CLS is exactly identical for every repetition.
Explore readiness is 5,291.4 → 5,193.5 ms; Replay readiness is
11,042.8 → 11,071.7 ms. This does not shorten the first 10 s replay payload.

### On-demand interaction cost

| Cold opening | Visible result ms | Added delay ms | Event Timing duration ms | Added JS bytes |
|---|---:|---:|---:|---:|
| Command palette (Ctrl+K) | 46.7 → 348.0 | **+301.3** | 56 → 16 | 0 → 2,879 |
| Help/settings (tap top-bar Help) | 96.7 → 400.9 | **+304.2** | 104 → 56 | 0 → 7,720 |

Palette visible-result samples: baseline 46.7, 39.2, 46.7 ms; candidate 348.0,
351.8, 342.7 ms. Help: baseline 96.7, 89.7, 102.4 ms; candidate 400.9, 407.2,
398.5 ms. These are first opens from cold contexts, not cached second opens.
Lower Event Timing durations in the candidate **do not** mean faster opening:
the asynchronous import returns control quickly, then the user waits for its
network request and dialog paint. The visible-result measurement captures that wait.

## Regressions and ship gate

- Landing/Explore saves 92 ms LCP (2.26%); Replay saves 8 ms (0.16%). **Both are
  below 150 ms**, so the candidate is not shipped.
- Cold palette and Help each become about 0.30 s slower. That trade-off is not
  justified by the measured LCP gain.
- Replay FCP is 4 ms later and readiness 28.9 ms later; palette-scenario load TBT
  is 6 ms higher. These small differences are reported, not suppressed.
- CLS is unchanged. Route APIs, first replay window, shell layout, router and
  identity providers were not changed.

Only 2.78% of entry bytes can be removed by these safe shell-dialog boundaries;
the dominant eager React/router/layout code remains. No speculative manual-chunk
split or workspace-remounting layout replacement is being shipped to inflate the
entry-size reduction.

## Verification

The candidate passed `npx tsc -b`, `npx vitest run` (**31 files, 317 tests**), and
`npm run build`. The server was built once with
`CARGO_PROFILE_DEV_DEBUG=0 cargo build -p bb-app`, then copied from
`target/debug/blackbox.exe` to `data/e2e/blackbox.exe` and
`data/e2e-ui/blackbox-perf.exe` before either was started.

`blackbox_e2e_lcp` was created in local PostgreSQL on port 55432 for the full
Playwright suite; it is separate from the performance database. The suite command,
from `web/`, was:

```sh
env -u BLOB_READ_WRITE_TOKEN -u DATABASE_URL_UNPOOLED -u BB_API_KEY_PEPPER \
  E2E_PORT=18897 E2E_DATA_DIR=data/e2e \
  DATABASE_URL=postgres://blackbox:blackbox-local@127.0.0.1:55432/blackbox_e2e_lcp \
  BLACKBOX_BIN=data/e2e/blackbox.exe npx playwright test
```

**Full Playwright suite: 79 passed (1.1 minutes), zero retries.** The first
attempt had been launched before the UI build finished and received 404s on UI
routes (34 failures, 45 not run, 900 s global timeout). The server selects API-only
mode at startup if `web/dist/index.html` is absent; [INFERENCE] that startup/build
race caused the first attempt. Starting the suite only after the completed build
resolved the setup failure without changing application code or narrowing tests.

The candidate was tested and measured before removal; the retained application
source is the unchanged baseline, not an unmeasured lazy variant.
