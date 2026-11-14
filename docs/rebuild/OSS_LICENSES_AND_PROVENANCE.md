# Rebuild donors: licences, provenance, security

Evidence behind the decisions in [REUSE_MATRIX.md](REUSE_MATRIX.md). Projects already covered in
[../OPEN_SOURCE_RESEARCH.md](../OPEN_SOURCE_RESEARCH.md) (§2 NautilusTrader, §5 ABIDES, §6c DBN, §7
Lightweight Charts, §8 Tardis, §9 Binance) are only re-pinned here. Their obligations are set out
there.

**Date:** 2026-11-14. **Method:** each repository was shallow-cloned to `D:/bbwt-research/<name>`
and its `LICENSE*`/`NOTICE*`/README files were read directly. Status comes from
`gh api repos/<o>/<r>` (`archived`, `created_at`, `pushed_at`, `license.spdx_id`) and from
`gh api repos/<o>/<r>/contributors`. Quotes are verbatim. `UNVERIFIED` = could not be confirmed; what
was tried is stated. `[INFERENCE]` = conclusion not directly observed. Not legal advice.

## 1. Licence evidence per donor

"Copyright line" is quoted from the licence file. If the file has none, the source-header line is
given, or "none".

| Donor | SHA | Last commit · archived · created · contributors | Licence file says | Copyright line | GitHub SPDX |
|---|---|---|---|---|---|
| OpenCharts | `399e27538d8150796571dbdecbe920a54b771de4` | 2026-11-10 · no · 2026-04-08 · 1 (`dylanpersonguy`, 17 commits) | `LICENSE`: "MIT License" | "Copyright (c) 2024 OpenCharts Contributors" | `MIT` |
| OpenBB workspace | `be00e95019a55d57af146919ee46b7e1a4859226` | 2026-09-30 · **yes** · 2026-09-09 · — | `LICENSE`: "Apache License Version 2.0, January 2004"; `terminalpro/package.json` `license: null` | `NOTICE`: "Copyright 2026 OpenBB, Inc." | `Apache-2.0` |
| OpenTerminalUI | `25163697b309c11ad9e9fcf14ee86b1aed0bb7e9` | 2026-11-14 · no · 2026-02-13 · 2 | "MIT License" | "Copyright (c) 2026 OpenTerminalUI Contributors" | `MIT` |
| ordercraft-hq/terminal | `64562c97aab59d206aaaf57ab48b284af3541a2b` | 2026-11-10 · no · 2026-11-01 · 1 (4 commits) | "MIT License"; `NOTICE.md` lists Lightweight Charts | "Copyright (c) 2026 OrderCraft Contributors (https://github.com/ordercraft-hq/terminal)" | `MIT` |
| open-dom-heatmap | `f2de3cdfd549d52f3f4fc3c5043eb93c780e0764` | 2026-11-07 · no · 2026-11-07 · — (1 star) | "MIT License" | "Copyright (c) 2026 Hannes Kapsch" | `MIT` |
| lightweight-charts | `aebdfc8f0d85b95e43467ae79516753158d60e61` | 2026-11-13 · no | Apache-2.0 + `NOTICE` (see OPEN_SOURCE_RESEARCH §7). Plugin packages `packages/lwc-plugin-*`, `lwc-toolkit`: `package.json` `"license": "Apache-2.0"`, same repo `LICENSE` | `LICENSE` appendix: "Copyright 2023 TradingView, Inc."; `NOTICE`: "Copyright (с) 2025 TradingView, Inc." | `Apache-2.0` |
| perspective | `56038acfb5be1ba29d2a325f5f70567a8fa3d290` | 2026-11-13 · no · 2017-11-02 | `LICENSE.md`: "Apache License _Version 2.0, January 2004_" | "Copyright 2019 The Perspective Authors"; headers: "Copyright (c) 2017, the Perspective Authors." | `Apache-2.0` |
| glide-data-grid | `0875d78cc41535a39f8b417ff88ce1ebe129c363` | 2026-01-21 (push 2026-11-13) · no · 2020-10-28 | "MIT License" | "Copyright (c) 2021 typeguard, Inc." | `MIT` |
| quantreplay | `39111b63d24dc7e5a84fa758facbc2fbd9a49081` | 2026-10-20 · no · 2025-07-09 | Apache-2.0 boilerplate; `THIRD-PARTY-LICENSES.md` lists 16 bundled libraries | none in `LICENSE`; no `Copyright` string under `project/` | `Apache-2.0` |
| nautilus_trader | `12982032fb970cabc085f00169c720792c3edecf` | 2026-11-14 · no | "GNU LESSER GENERAL PUBLIC LICENSE Version 3, 29 June 2007" | headers: "Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved." | `LGPL-3.0` |
| abides | `c4bf157678928934417aba6073eb0651aeaf6d15` | 2020-11-19 (push 2023-07-06) · no | `LICENSE.txt`: "BSD 3-Clause License" | "Copyright (c) 2019 Georgia Tech Research Corporation" | `NOASSERTION` |
| TT_Samples | `c79823c7934f0c2cbc3064125aeb2450ccc3614f` | 2026-09-03 · no | BSD-style with extra clause: "Redistributions of source or binary code must be free of charge." | "Copyright © 2018-2019 Trading Technologies International, Inc. All Rights Reserved Worldwide" | `NOASSERTION` |
| magic-trace | `b3faf23fccfc4373d47bb6458c9d7dc79b0723fe` | 2026-05-29 · no | `LICENSE.md`: "The MIT License" | "Copyright (c) 2022 Jane Street Group, LLC <opensource@janestreet.com>" | `MIT` |
| quickfix | `386ce46e917ae494ab6e90b1be90fd421cdbe3f9` | 2026-05-20 · no | "The QuickFIX Software License, Version 1.0" | "Copyright (c) 2001-2020 Oren Miller" | `NOASSERTION` |
| Lean | `82d26047e95a89b53c9cbe02b67efaa3c4455ed9` | 2026-11-13 · no | Apache-2.0 | "Copyright 2014 QuantConnect Corporation" | `Apache-2.0` |
| qstrader (quantstart) | `e6d86a3ac3dc507b26e27b1f20c2949a69438ef7` | 2018-12-21 (push 2019-03-08) · no | "The MIT License (MIT)" | "Copyright (c) 2015-2018 QuantStart.com" | `MIT` |
| dbn | `a77693030595b8f29f6b4c58e63b1b1a9d899b3f` | 2026-10-27 · no; crate 0.73.0 `Apache-2.0` | Apache-2.0 boilerplate | none | `Apache-2.0` |
| databento-rs | `d75ab48a63732d70d6943e319e986c601bf0dc4f` | 2026-10-27 · no; crate 0.64.0 `Apache-2.0` | Apache-2.0 boilerplate | none | — |
| itch-rs | `029460fa265cddc22fbaf30fedacea1d3e7d31a0` | 2026-11-13 · no · 2026-05-05 · (1 star) | `LICENSE-MIT` + `LICENSE-APACHE`; `Cargo.toml` `license = "MIT OR Apache-2.0"` | `LICENSE-MIT`: "Copyright (c) 2026 Joaquin Bejar" | — |
| itch-book | `b502b002abf650760b79c010b3ae3a8f82896ae8` | 2026-09-22 · no · 2026-07-07 · (3 stars) | "MIT License" | "Copyright (c) 2026 groovg" | — |

Obligations if code is ported: MIT and BSD require keeping the copyright line and licence text with
the copied file. Apache-2.0 additionally requires stating changes and carrying `NOTICE` (§4(b),(d)).
For the two *Adapt* items: the open-dom-heatmap port must keep "Copyright (c) 2026 Hannes Kapsch" plus
the MIT text in the ported file header and in the repo `NOTICE`. Adopting LWC plugin packages adds no
new notice, because the existing TradingView NOTICE and attribution (OPEN_SOURCE_RESEARCH §7) already
cover them [INFERENCE: same licensor, same `NOTICE`].

## 2. Provenance concerns

### OpenCharts: "extracted from a closed-source prop-trading platform"

README, lines 65–67:

> "The codebase was extracted from a closed-source prop-trading platform, so a few panels are gated
> off or stubbed rather than removed — those are called out explicitly below rather than advertised as
> features."

The README's source tree also says `src/vendor/` holds "types inlined from the original platform's
shared package" and `src/services/api/` holds "legacy HTTP clients from the original platform".

What establishes or fails to establish the right to relicense it under MIT:

- Not established: the platform is not named. No statement, licence grant or contributor agreement
  from the platform's owner appears in `README.md`, `NOTICE`, `LICENSE` or `CONTRIBUTING.md`. `NOTICE`
  only covers TradingView's Lightweight Charts and fancy-canvas.
- The copyright holder is the generic "OpenCharts Contributors" with year **2024**. The repository was
  created **2026-04-08**, and its first commit (`d234858d3c`, 2026-04-08, "Initial commit: OpenCharts -
  Open Source TradingView Alternative") arrived as a whole codebase. A single account
  (`dylanpersonguy`) authored all 17 commits (`gh api …/commits`, `…/contributors`).
- Possible but unproven: the author owned the closed platform. **UNVERIFIED.** No public statement
  was found in the repository.
- Clean: `src/lib/chart-plugins/{plugin-base.ts,tooltip/,delta-tooltip/,session-highlighting/,session-breaks/,bands-indicator/,highlight-bar-crosshair/,helpers/}`
  are, per `NOTICE`, "adapted from the plugin examples in" Lightweight Charts (Apache-2.0). BlackBox
  should take those from upstream `tradingview/lightweight-charts/plugin-examples` or `packages/`, not
  from OpenCharts.

**Judgement:** redistribution rights are **not established** for any OpenCharts component outside
the TradingView-derived plugins: `pages/trading/*`, `services/replay.ts`, `vendor/`, `services/api/`.
Use it as UX reference only and copy no code.

### OpenBB workspace: proprietary and commercial components

- `NOTICE`: "The TradingView Advanced Charts library is proprietary to TradingView, Inc. and is NOT
  licensed under the Apache License 2.0. It is not distributed with this repository. To build with
  it, obtain the library under TradingView's own license agreement and place it in
  terminalpro/src/lib/charting_library/." Files referencing it include
  `src/components/Charting/TVIndicatorsDialog.tsx`, `src/components/Charting/constants.ts` and
  `src/components/Widgets/TVChartContainer/TVChartContainerFunc.tsx`.
- AG Grid and AG Charts **Enterprise** 34.1.1 / 12.1.1 are runtime dependencies. `npm view
  ag-grid-enterprise license` → `"Commercial"`. `src/main.tsx:109` calls
  `LicenseManager.setLicenseKey(…)` with a key string naming `{OpenBB}` as licensee for a
  `{Single_Application}` (`{OpenBB-Pro}`), then registers `AllEnterpriseModule`. 23 files under `src/`
  import an enterprise package. The Apache-2.0 repo licence does not relicense AG's commercial
  product, and the key is not transferable to BlackBox [INFERENCE from the key's own wording].
- The repository is archived ("Initial snapshot of OpenBB Workspace", single commit). README: "Internal
  deployment pipelines are maintained separately."

### Other provenance notes

- **ordercraft-hq/terminal** README: "a 100% original, permissive open-source … workstation built from
  first principles". No contrary evidence was found. The project is 13 days old with one contributor.
- **open-dom-heatmap** README: "Not affiliated with Bookmap or ATAS … the algorithm is an independent
  implementation". `src/processor.js` header: "Inspired by the settings Bookmap documents publicly
  (percentile cut-offs that are recalibrated over time); the algorithm itself is an independent design."
  The repository includes `AGENTS.md`, "Instructions for AI coding agents". This is not a licence issue.
- **TT_Samples**: the licence says "BSD-style" but adds "must be free of charge". The README adds:
  "Sample code is provided on the website subject to a limited, revocable, non-transferable license
  solely for testing purposes and not for commercial use." The two statements conflict, and neither is
  OSI-approved.
- **QuickFIX** licence clause 3 requires end-user docs to include "This product includes software
  developed by quickfixengine.org (http://www.quickfixengine.org/)." This is relevant only if
  QuantReplay binaries were ever redistributed, since it bundles quickfix 1.15.1.
- **QuantReplay** carries no project copyright line anywhere checked. The Apache-2.0 grant comes from
  `LICENSE` plus the GitHub SPDX. Its third-party list is in `THIRD-PARTY-LICENSES.md`.

## 3. Dependency and proprietary-component exclusions

| Exclude | Where it comes from | Why |
|---|---|---|
| TradingView **Advanced Charts** / `charting_library` | OpenBB `terminalpro` | Proprietary (OpenBB `NOTICE`). Distinct from Lightweight Charts |
| `ag-grid-enterprise`, `ag-charts-enterprise` | OpenBB `terminalpro` | npm licence "Commercial"; licence key issued to OpenBB |
| TradingView embed widgets (`s3.tradingview.com/external-embedding/*.js`) | OpenCharts `src/components/TradingViewWidgets.tsx` | Third-party script and live data from TradingView's CDN |
| `@nktkas/hyperliquid`, `viem` | OpenCharts | Live-exchange trading adapters; BlackBox never trades |
| `posthog-js`, MSAL, Okta | OpenBB | Telemetry and auth for OpenBB's own service |
| lightweight-charts **4.x** code | OpenCharts | BlackBox is on 5.x; v4 plugin code needs porting anyway |

React 19 compatibility: OpenBB (React ^18.2), OpenTerminalUI (^18.3.1) and OpenCharts (peer `>=18.2.0`,
dev React not checked) target React 18. The ordercraft terminal targets React ^19.2.8. glide-data-grid
6.0.3 rejects React 19 in its peer range. open-dom-heatmap and the LWC plugins have no React dependency.

## 4. Security notes

| Donor | Observation | Location |
|---|---|---|
| OpenBB workspace | Commercial AG Grid licence key committed in source | `terminalpro/src/main.tsx:109` |
| OpenBB workspace | `.env` committed with 79 `VITE_*` assignments including a PostHog key. Its own header says "ALL THESE VARIABLES ARE PUBLIC AND SHOULD NOT BE USED FOR SENSITIVE INFORMATION" | `terminalpro/.env` |
| OpenBB workspace | Optional runtime script from `//unpkg.com/react-scan/dist/auto.global.js` when `VITE_ENABLE_RENDER_SCANNING=true` | `terminalpro/src/main.tsx` |
| OpenTerminalUI | `new Function(` builds user indicator code at runtime | `frontend/src/shared/chart/IndicatorManager.ts:537` |
| OpenTerminalUI | `dangerouslySetInnerHTML={{ __html: highlightSource(source) }}`; escaping inside `highlightSource` not reviewed (UNVERIFIED) | `frontend/src/components/chart/ScriptEditor.tsx:324` |
| OpenCharts | Loads third-party scripts from `s3.tradingview.com`; would need CSP `script-src` exceptions | `src/components/TradingViewWidgets.tsx` |
| perspective | Viewer depends on `pro_self_extracting_wasm` 0.0.11 (Apache-2.0, `prospectiveco/pro-self-extracting-wasm`): a third-party WASM loader in the supply chain | `npm view @perspective-dev/viewer dependencies` |
| QuantReplay | Quickstart uses `POSTGRES_PASSWORD=postgres`; wiki: image defines user `sim` / password `sim`; publishes FIX :9051 and REST :9050. A local PoC should bind to 127.0.0.1 | README Quickstart; wiki `Overview` |
| magic-trace | Drives `perf` with Intel PT and needs perf privileges on Linux | README |
| databento-rs | Default feature `reqwest/default-tls` (native TLS). Use `default-features = false` and rustls if ever linked | `Cargo.toml` |
| open-dom-heatmap | Zero dependencies; Web Workers for backfill; the live demo connects to Binance (demo only) | `package.json`, `demo/live.js` |

## 5. Data sources: hosting and redistribution

The rule from MARKET_MICROSTRUCTURE §7.2 stands: **the hosted service serves synthetic data only.**
New or re-checked evidence:

| Source | Terms (URL, date) | Quoted clause | Verdict |
|---|---|---|---|
| NASDAQ ITCH samples | <https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/> (listing read 2026-11-14). No licence, terms or README file in `/`, `/ITCH/` or `/ITCH/Nasdaq ITCH/`; `README.txt` → 404 | No grant found. Nasdaq's *US Equities and Options Data Policies* v2.6 (<https://www.nasdaqtrader.com/content/AdministrationSupport/Policy/USEquitiesandOptionsDataPolicies.pdf>) opens: "Under the terms of the Nasdaq Global Data Agreement, Distributors of Nasdaq information must adhere to the following Nasdaq Data Policies". It defines Information to include "any element of information used or processed in such a way that Exchange Information or a substitute for such Information can be identified, recalculated or re-engineered" | **Do not host.** Local use only. Whether the policies apply to the free sample files is UNVERIFIED and needs written confirmation from Nasdaq |
| Binance Vision | <https://raw.githubusercontent.com/binance/binance-public-data/master/TERMS_AND_CONDITIONS.md>, "Version: 1.0", "Last Updated: August 26, 2026" (re-read 2026-11-14) | 4.3: "You shall not license, sub-license, sell, rent, lease, host, or commercially exploit the Datasets or any derivative real-time or historical data feeds." | **Do not host** (unchanged; full analysis in OPEN_SOURCE_RESEARCH §9) |
| Tardis.dev | <https://docs.tardis.dev/legal/terms-of-service> (re-read 2026-11-14) | 9.2: the Customer shall not "use the Data or Manipulated Data to provide a product or service that gives anyone other than Customer Users access to them or to functionality that uses them, such as a data feed, an API, a downloadable dataset or an analytics, charting or backtesting platform". 9.3 allows Derived Data only "in charts, statistics, research reports … but not in a dataset, data feed, API, analytics, charting or backtesting platform", or "if it is aggregated over periods of at least 10 minutes" | **Do not host** (unchanged) |
| cryptohftdata.com | <https://www.cryptohftdata.com/terms>, "Last updated: 6 October 2026" | "Internal use only. No data resale under any circumstances." · "You may not resell, sublicense, redistribute, republish, share, or otherwise make the data available to any third party, whether free of charge or for payment." · "Repackaging, re-aggregating, white-labelling, or embedding the data in a downstream product, feed, dataset, dashboard, or service is not allowed under the standard licence." · "CryptoHFTData is currently run by an individual and is not incorporated." | **Do not host.** Local research use is permitted. Outputs are allowed only "as long as nobody can reconstruct the data from them" |
| Databento (data, via dbn / databento-rs) | <https://databento.com/legal/databento-user-agreement/platform-access>: the page is client-rendered, and the fetched HTML contained no agreement text. `/terms` and `/licensing` → 404 | — | **UNVERIFIED.** Treat as not hostable. The libraries are Apache-2.0, but that says nothing about the data |

## 6. UNVERIFIED items

| Item | What was tried |
|---|---|
| OpenCharts author's ownership of the source platform | README, NOTICE, LICENSE, CONTRIBUTING, commit list and contributors API; no statement found |
| QuantReplay CPU/RAM requirements | grep for cpu/ram/memory/hardware in README, `docs/`, wiki clone; none. The image was not run |
| Nasdaq sample-file terms | EMI directory listings; Nasdaq data-policy PDF; web search. No sample-specific terms found |
| Databento data licence text | three URLs (404 or JS-only) and a web search; agreement text not retrievable without a browser |
| `highlightSource` escaping in OpenTerminalUI | not reviewed |

Reproduce: `git -C D:/bbwt-research/<name> rev-parse HEAD`; `gh api repos/<o>/<r> --jq
'.archived,.created_at,.pushed_at,.license.spdx_id'`; bundle sizes per REUSE_MATRIX "Bundle method".
