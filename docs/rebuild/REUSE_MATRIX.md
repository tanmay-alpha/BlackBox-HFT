# Rebuild reuse matrix

Which outside projects the rebuild may take code, packages or ideas from, and why. Licence quotes,
provenance and data terms are in [OSS_LICENSES_AND_PROVENANCE.md](OSS_LICENSES_AND_PROVENANCE.md).
Earlier surveys still apply and are not repeated here:
[../OPEN_SOURCE_RESEARCH.md](../OPEN_SOURCE_RESEARCH.md) covers hftbacktest, NautilusTrader, ABIDES,
DBN, Lightweight Charts, Tardis and Binance.
[../research/OSS_TECHNOLOGY_EVALUATION.md](../research/OSS_TECHNOLOGY_EVALUATION.md) covers Lean,
qstrader, chart and table libraries and bundle methods (§3.6).
[../research/MARKET_MICROSTRUCTURE.md §7](../research/MARKET_MICROSTRUCTURE.md#7-real-data-a-public-hosted-service-could-serve)
covers whether real data can be hosted.

**Survey date:** 2026-11-14. Each repository was shallow-cloned to `D:/bbwt-research/<name>`. SHAs
come from `git rev-parse HEAD` and dates are the HEAD commit date. Archive status, creation date and
stars come from `gh api repos/<o>/<r>`. npm facts come from `npm view`, crates facts from
`https://crates.io/api/v1/crates/<name>`.

**BlackBox web baseline** (`web/package.json`, `web/package-lock.json`): React 19.3.1, Vite 7.3.8,
TypeScript ~5.9.3, lightweight-charts ^5.3.0, @tanstack/react-table 8.21.3, @tanstack/react-virtual
^3.14.26, react-resizable-panels ^4.22.1, react-router-dom ^7.18.5, plain CSS per feature. There is no
Tailwind or state library. The depth heatmap is hand-written canvas in `web/src/features/replay/heatmap.ts`
(1,215 lines) and `web/src/features/terminal/TerminalHeatmap.tsx`.

**Bundle method:** esbuild 0.28.2, `--bundle --minify --format=esm` with React (and, for plugins,
`lightweight-charts`) external, then `gzip -9 | wc -c`. Packages were installed in
`D:/bbwt-research/_bundle`. This is the same method as OSS_TECHNOLOGY_EVALUATION §3.6. Sizes labelled
"npm unpacked" are `npm view <pkg> dist.unpackedSize` and are not bundle size.

**Decisions:** *Adopt* = take the package or code as is. *Adapt* = port specific code with its
licence notice, or reimplement it closely. *Reference only* = read for ideas and copy no code.
*Reject* = do not use.

## 1. UI donors

| Donor | Pinned SHA (HEAD date) | Licence | Paths relevant to BlackBox | Conflicts with BlackBox web | Size | Decision |
|---|---|---|---|---|---|---|
| dylanpersonguy/OpenCharts | `399e27538d8150796571dbdecbe920a54b771de4` (2026-11-10) | MIT | `src/pages/trading/{ReplayHUD,ReplayScrubber,useReplayPlayback,useReplayChartData,DOMPanel}.tsx`, `src/services/replay.ts`, `src/lib/chart-plugins/` | lightweight-charts **^4.2.0** (we use 5); Tailwind, zustand, @tanstack/react-query, zod, viem, @nktkas/hyperliquid; React peer `>=18.2.0` | not measured (copying is not recommended) | **Reference only.** The README says the code was "extracted from a closed-source prop-trading platform", and nothing establishes the right to relicense it (see provenance doc). The chart plugins it adapted are available from TradingView upstream under Apache-2.0 for v5. |
| OpenBB-finance/workspace (`terminalpro/`) | `be00e95019a55d57af146919ee46b7e1a4859226` (2026-09-30, "Initial snapshot") | Apache-2.0 (repo) | `src/components/{GridLayout.tsx,GridLayout.hooks.tsx,Widget.tsx,DraggableCard/,Charting/,InAppOnboarding/,ui/}` | React ^18.2; `@jose-donato/react-grid-layout` fork; ag-grid/ag-charts **enterprise** (registered in `src/main.tsx`); TradingView Advanced Charts (proprietary, not shipped); Plotly; posthog; MSAL/Okta | plotly.js-dist-min 6.2 MB npm unpacked; ag-grid-enterprise 31.5 MB npm unpacked, `license: "Commercial"` | **Reject.** The repository is **archived**. Widget and card code depends on AG Grid Enterprise and a commercial key issued to OpenBB. Charting is Plotly or proprietary TradingView. GridLayout is a thin wrapper over a react-grid-layout fork. |
| Hitheshkaranth/OpenTerminalUI (`frontend/`) | `25163697b309c11ad9e9fcf14ee86b1aed0bb7e9` (2026-11-14) | MIT | `src/components/layout/{SplitPane,PanelChrome,CommandPalette,TerminalShell}.tsx`, `src/components/chart-workstation/{ChartGridContainer,LayoutSelector,ChartPanel}.tsx`, `src/workspace/{portableLayout,presets}.ts`, `src/shared/chart/{ChartSyncContext.tsx,rafBatcher.ts,replay.ts}` | React ^18.3.1, Vite ^6, Tailwind, zustand, react-grid-layout ^2.2.2, recharts, three; lightweight-charts ^5.1.0 (compatible) | react-grid-layout 2.3.0: 26,239 B gzip (measured) | **Reference only.** The useful pieces are small: `ChartSyncContext.tsx` is 55 lines and `rafBatcher.ts` 60, a rAF-coalesced crosshair/time broadcast. Reimplementing them costs less than taking Tailwind/zustand code. Do not take the `new Function(` in `shared/chart/IndicatorManager.ts:537`. |
| ordercraft-hq/terminal (`frontend/`) | `64562c97aab59d206aaaf57ab48b284af3541a2b` (2026-11-10) | MIT | `src/indicators/primitives/{FootprintPrimitive,BandFillPrimitive,DiscontinuousLinePaneView}.ts`, `src/components/ChartPane/PriceCountdownPrimitive.ts`, `src/drawings/tools/VerticalLineTool.ts` | Closest stack match: React ^19.2.8, lightweight-charts ^5.2.1, Vite ^8.3, TS ~6.0. Uses Tailwind 4, zustand, Monaco and `wickra-wasm` (4.8 MB npm unpacked). `dockview` is declared but not imported in `src/`. | not measured | **Reference only.** The repository was created 2026-11-01 with one contributor and 4 commits. Its files are monoliths (`ChartPane.tsx` 3,809 lines, `store/syncStore.ts` 2,251). It is useful as a working example of LWC v5 series primitives. |
| Kessler833/open-dom-heatmap | `f2de3cdfd549d52f3f4fc3c5043eb93c780e0764` (2026-11-07, v0.2.0) | MIT | `src/processor.js` (269 lines: colour pipeline), `src/backfill.js` + `src/worker.js` (parallel backfill), `src/book.js` (`Columnizer`, `bookToColumn`), `src/render.js` | none (zero dependencies, ES modules, `.d.ts` included); **not published on npm** (404) | whole library 18,516 B min / **7,891 B gzip** (measured) | **Adapt.** Port the colouring algorithm into `features/replay/heatmap.ts` and keep its MIT notice: local×wide geometric-mean background, percentile cut-offs recalibrated over ~40 columns, bounded per-bucket memory. The current canvas renderer stays. The project is one week old with one star, so behaviour must be checked with our own fixtures. |
| tradingview/lightweight-charts | `aebdfc8f0d85b95e43467ae79516753158d60e61` (2026-11-13); npm 5.3.0 | Apache-2.0 + NOTICE | already a dependency. Upstream also ships v5 plugin packages under `packages/`: `lwc-plugin-vertical-line`, `lwc-plugin-user-price-lines`, `lwc-plugin-heatmap-series`, `lwc-toolkit` (PluginBase, pixel helpers) | none; plugins declare peer `lightweight-charts ^5.0.0` | core `createChart`+`LineSeries` 54,042 B gzip; vertical-line 3,495; user-price-lines 3,188; heatmap-series 2,960 (all measured, LWC external) | **Adopt** (keep the core and add the official plugins as needed). Same licence and NOTICE as the existing dependency, which BlackBox already satisfies. |
| perspective-dev/perspective | `56038acfb5be1ba29d2a325f5f70567a8fa3d290` (2026-11-13) | Apache-2.0 | `packages/react`, `packages/viewer-datagrid`, `rust/perspective-viewer` | Custom element backed by a WASM engine. `@perspective-dev/viewer` 5.9.1 depends on `regular-layout`, `pro_self_extracting_wasm`; client peers `@duckdb/duckdb-wasm`, `@clickhouse/client-web` | npm unpacked: viewer 5.68 MB, client 2.51 MB, viewer-datagrid 1.58 MB, viewer-d3fc 12.0 MB | **Reject (now).** It would be a second table/chart stack beside TanStack and LWC, and multi-MB WASM on a free-tier site. Revisit only for an analyst pivot view. |
| glideapps/glide-data-grid | `0875d78cc41535a39f8b417ff88ce1ebe129c363` (2026-01-21, 6.0.4-alpha25) | MIT | `packages/core` (canvas grid) | Stable **6.0.3 peers `react ^16.12 \|\| 17 \|\| 18`**. React 19 appears only in the 6.0.4 alpha peers. Open React-19 issues #1189, #1122, #1202. Peers lodash, marked, react-responsive-carousel | `DataEditor` 316,110 B min / 106,490 B gzip (measured) | **Reject.** Stable releases do not support React 19, and TanStack Virtual already windows the tape. |

## 2. Engines and infrastructure

| Donor | Pinned SHA (HEAD date) | Licence | Paths relevant to BlackBox | Fit | Decision |
|---|---|---|---|---|---|
| Quod-Financial/quantreplay | `39111b63d24dc7e5a84fa758facbc2fbd9a49081` (2026-10-20) | Apache-2.0 | `project/{trading_system,generator,data_layer,fix,http}`, wiki `Configuring-QuantReplay` | C++ venue simulator with a FIX gateway and REST admin, plus Postgres and Liquibase. It is not a library BlackBox could link. See §4 | **Reference only**; local Docker PoC feasible (§4). |
| nautechsystems/nautilus_trader | `12982032fb970cabc085f00169c720792c3edecf` (2026-11-14, `develop`) | LGPL-3.0 | `crates/execution/src/models/latency.rs` (`LatencyModel` trait, `StaticLatencyModel`), `crates/model/src/orderbook/`, `crates/backtest/src/config.rs` | Already "learn only" in OSS_TECHNOLOGY_EVALUATION §3.1 | **Reference only** (copyleft on copied code). |
| abides-sim/abides | `c4bf157678928934417aba6073eb0651aeaf6d15` (2020-11-19) | BSD-3-Clause | `Kernel.py`, `util/model/LatencyModel.py` | Python agent-based simulator, unmaintained since 2020 | **Reference only** (latency-model concepts; already "learn only"). |
| tradingtechnologies/TT_Samples | `c79823c7934f0c2cbc3064125aeb2450ccc3614f` (2026-09-03) | BSD-style with an added "free of charge" condition and a README "not for commercial use" disclaimer | `TT_REST_API/`, `TT_CORE_SDK/`, `TT_NET_CLIENT_SIDE/`, `ADL/` | Samples for TT's proprietary platform and SDKs | **Reject.** Irrelevant to replay, with conflicting licence terms. |
| janestreet/magic-trace | `b3faf23fccfc4373d47bb6458c9d7dc79b0723fe` (2026-05-29) | MIT | README; UI is a Perfetto fork | Needs Intel PT: "VMs are mostly not supported, Intel only (Skylake or later), Linux only". It cannot run on the Windows dev box or on Render | **Reference only.** Perfetto-style timeline as a UX idea for the latency page. |
| quickfix/quickfix | `386ce46e917ae494ab6e90b1be90fd421cdbe3f9` (2026-05-20) | QuickFIX Software License 1.0 (BSD/Apache-1.1-style with acknowledgement clause) | — | BlackBox has no FIX surface. It matters only inside QuantReplay, which bundles quickfix 1.15.1 | **Reject.** |
| QuantConnect/Lean | `82d26047e95a89b53c9cbe02b67efaa3c4455ed9` (2026-11-13) | Apache-2.0 | `Common/Orders/Fills/{IFillModel,FillModel}.cs`, `Common/Orders/Slippage/` | C#/.NET; already "learn only" | **Reference only.** |
| quantstart/qstrader | `e6d86a3ac3dc507b26e27b1f20c2949a69438ef7` (2018-12-21) | MIT | — | 2018 snapshot. The maintained line is `mhallsmoore/qstrader`. Portfolio-level, no order book | **Reject** (as in OSS_TECHNOLOGY_EVALUATION §3.1). |

## 3. Data libraries and sources

Libraries and data are judged separately. A permissive library licence says nothing about the data
it reads.

| Donor | Pinned SHA / version | Licence | Relevance | Hosting verdict | Decision |
|---|---|---|---|---|---|
| databento/dbn | `a77693030595b8f29f6b4c58e63b1b1a9d899b3f` (2026-10-27); crate `dbn` 0.73.0 | Apache-2.0 | `rust/dbn` decoder; MBO (L3) and MBP schemas fit a local L3 importer | Library only; ships no data | **Adapt later.** It is a candidate dependency for a bring-your-own-file importer (local only). No code is copied now. |
| databento/databento-rs | `d75ab48a63732d70d6943e319e986c601bf0dc4f` (2026-10-27); crate `databento` 0.64.0 | Apache-2.0 | Historical and live API client. Default features include `reqwest/default-tls` (native TLS); BlackBox uses rustls | Data needs a paid account. Databento data terms are UNVERIFIED (JS-rendered) | **Reject** for the hosted service; at most a local user tool. |
| joaquinbejar/itch-rs | `029460fa265cddc22fbaf30fedacea1d3e7d31a0` (2026-11-13); workspace 0.1.1 | MIT OR Apache-2.0 | `crates/itch-protocol` (ITCH 5.0 codec, no I/O), MSRV 1.86 | Library only | **Reference only.** Not on crates.io (`itch-rs` and `itch-protocol` both return null). Created 2026-05-05, 1 star. |
| groovg/itch-book | `b502b002abf650760b79c010b3ae3a8f82896ae8` (2026-09-22) | MIT | Header-only C++23 book plus Python `itch2parquet`. The README reports agreement with Databento XNAS.ITCH on 99.7–99.9% of nanoseconds | Tool only | **Reference only.** It could serve as an offline converter to build *local* L3 validation fixtures. |
| NASDAQ ITCH samples (emi.nasdaq.com/ITCH/) | directory listing read 2026-11-14 | no licence or terms file found | Full-day `*.NASDAQ_ITCH50.gz` (3.6–5.6 GB each) | **No.** No grant found; local use only | Local-only fixture source, pending written permission. |
| Binance Vision | `binance-public-data` `d571690eb66760763b0bd6e27e8cd35603c7e244` (2026-10-16); terms v1.0, 2026-08-26 | CC BY-NC-SA 4.0 plus terms | trades, bookTicker (ended 2024-03-30), bookDepth; no L2 diffs | **No** (clause 4.3 "host") | Local-only (unchanged from MARKET_MICROSTRUCTURE §7). |
| Tardis.dev | ToS read 2026-11-14 | proprietary | L2 incremental and trades, already supported locally | **No** (clause 9.2 names "backtesting platform") | Local-only (unchanged). |
| cryptohftdata.com | Terms "Last updated: 6 October 2026" | proprietary, free, run by an individual | historical crypto HFT data | **No** ("No data resale under any circumstances"; no "embedding the data in a … dashboard, or service") | Local-only. |

## 4. QuantReplay: build and PoC feasibility

| Item | Finding | Evidence |
|---|---|---|
| Build toolchain | CMake ≥ 3.20, Conan 1.59, gcc-toolset-12 on Rocky Linux 8 (devcontainer). The wiki build guide is "tailored for Linux and macOS"; no native Windows build | `CMakeLists.txt:1`, `.devcontainer/Rocky8.Dockerfile`, wiki `QuantReplay-Build-&-Run-Guide-for-Visual-Studio-Code` |
| Bundled C++ deps | cpp-httplib, date, tl-expected, fmt, gtest, benchmark, libpqxx, pistache, **quickfix 1.15.1**, rapidjson, spdlog, tinyxml2, csv-parser, libzip, ms-gsl | `THIRD-PARTY-LICENSES.md` |
| Prebuilt images | `ghcr.io/quod-financial/deploy_marketsim:latest` (**linux/amd64 only**, 45,932,608 B compressed layers), `postgres_marketsim:latest` and `liquibase:latest` (amd64 + arm64). Tags `main`, `latest`, `develop`, `vNN` | `docker manifest inspect` (2026-11-14), README "Quickstart" |
| Runtime topology | 3 containers per Quickstart: simulator (REST :9050, FIX :9051), Postgres (:15432), one-shot Liquibase | README `docker-compose.yml` |
| CPU/RAM needs | **UNVERIFIED.** README, wiki and docs give no hardware figures. The README only says "Low footprint … single native process per simulated venue" | grep of README, `docs/`, wiki for cpu/ram/memory |
| This dev box | Docker Desktop 4.96.3, Engine 29.9.2, server `linux/amd64` (WSL2 kernel 6.18.33.2), 24 vCPU, 7.6 GiB to the VM | `docker version`, `docker info` |
| PoC feasible locally? | **Yes** [INFERENCE: amd64 image on an amd64 Linux engine; image pulled by manifest only, not run]. Port 15432 does not clash with BlackBox Postgres on 55432 | above |
| Hosted fit | **No.** Three services, including its own Postgres and a FIX listener, against the single 512 MB Render free service (OSS_TECHNOLOGY_EVALUATION §1) | — |
| What BlackBox could learn | Market phases and auction uncrossing, a seeded random order generator with reproducible runs, and CSV/DB playback with "tolerance of sparse depth data" | README "Features" |

## 5. Top frontend reuse opportunities

For the chart-first replay workspace and the latency experiment page:

1. **Official LWC v5 plugin packages** (`@tradingview/lwc-plugin-vertical-line`,
   `lwc-plugin-user-price-lines`, `lwc-toolkit`, optionally `lwc-plugin-heatmap-series`). They give the
   replay cursor and event/fill markers, order price lines, and custom primitives such as a queue-position
   overlay. Each is about 3–3.5 KB gzip, Apache-2.0, peer `^5.0.0`, and adds no new licence obligation.
   The heatmap series could put depth in the same pane and time scale as price. Whether per-cell drawing
   keeps up with 5,000 × 100 cells, compared with our `ImageData` blit, is untested [INFERENCE].
2. **open-dom-heatmap colour pipeline** (`src/processor.js`, MIT, 7.9 KB gzip for the whole library),
   ported into `features/replay/heatmap.ts` / `TerminalHeatmap.tsx`. Its local/wide background,
   percentile cut-offs and bounded memory address the "one wall rescales everything" problem of
   `size ÷ max` colouring. The causal, chunkable design fits windowed replay fetches.
3. **Synchronised multi-pane pattern for side-by-side runs** (1 ms vs 50 ms): rAF-coalesced
   crosshair/time broadcast (OpenTerminalUI `shared/chart/ChartSyncContext.tsx` + `rafBatcher.ts`)
   and portable layout presets (`workspace/portableLayout.ts`, `presets.ts`). Reimplement these on the
   existing `react-resizable-panels` and `components/SplitPane.tsx`. No new dependency is needed.
   OpenCharts' ReplayHUD/ReplayScrubber is UX reference only.
