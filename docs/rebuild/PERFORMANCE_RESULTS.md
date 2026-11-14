# Performance results (rebuild)

Every row states where it was measured. Local numbers are not production numbers.

## Web bundle (`npm run build`, Vite 7, local)

| Build | Entry JS | gzip | Note |
|---|---|---|---|
| Baseline `48e277b` | 1,068.43 kB | 327.90 kB | one chunk for all routes |
| `c338afc` route-level `lazy()` | 399.78 kB | 129.29 kB | lightweight-charts (175 kB) and DataTable (91 kB) split into shared chunks loaded on demand |

## API payloads

| Request | Before | After | Where |
|---|---|---|---|
| Anonymous `/runs/253bbc23b342f373/timeline` default window | 500 `response_too_large` | 400 `window_too_large` with a hint; the terminal requests 10 s windows | production (before), local (after) |
| Timeline, 10 s window, 2,238 events | 200, 792,673 B, 3.07 s | paged: first page ≤ 1 MiB, follow `next_cursor` | production / local |
| Anonymous run analytics (`586374188aae19b2`) | 2,760,222 B (FINAL_AUDIT, older production build) | 462,842 B; series halve until they fit, else 422 | local, `6f84791` |
| Keyed run analytics, 128 KiB cap | 500 `internal` | 200, 96–97 kB | local, `6f84791` |

## Public page payload vs. the anonymous egress budget

The anonymous budget is 60,000,000 B/day of uncompressed response bodies (`config.rs`). On the
wire, production responses are gzip-encoded (`Content-Encoding: gzip`, observed), so bytes
actually sent are lower than bytes counted. Decision (user, 2026-11-14): keep the budget and shrink
the page instead.

| Page | API body bytes per fresh visit | Visits/day within budget | Method |
|---|---|---|---|
| Prototype A | ~880 KB | ~68 | review, Performance API, local |
| Prototype B | ~637 KB | ~94 | review, Performance API, local |
| Final Explore page | see PRODUCTION_ACCEPTANCE / T6 report | — | same method |

## ITCH 5.0 reconstruction (local, release build, i7-14650HX, Windows 11, single thread)

First 5,000,000 messages of a NASDAQ sample prefix, kept local only. Median of 5 runs, file in the
OS cache, timing includes SHA-256 of the input:

| Mode | Wall | Messages/s |
|---|---|---|
| Decode only | 0.161 s | 31.1 M |
| L3 books, all 8,884 stocks | 0.516 s | 9.68 M |
| L3 + L2 rows for 3 symbols | 0.541 s | 9.25 M |
| Same, from gzip | 0.749 s | 6.67 M |

## Analytics grid decision (M6)

Not adopted: Perspective (multi-MB WASM and a second table stack) and Glide Data Grid (stable 6.0.3
peer-depends on React ≤ 18, 106 KB gzip). Evidence is in REUSE_MATRIX.md. The existing TanStack
Table + Virtual stack stays. No measured use case exceeds it: the orders blotter is server-paged
(100 rows a page), and the fills export streams by cursor. Revisit only with a concrete workload,
such as a sweep with hundreds of cells, and a benchmark.
