# Production acceptance (M10)

Date: 2026-11-15. Release commit: `d85f743719ae9d46d29c7c9e0df58f476e8bff42` (`main`). Later releases are appended
below; the current suite and its latest production result (`a24ccf9`) are in "Phase 6/7 production suite".

## Release evidence

| Step | Evidence |
|---|---|
| CI `ci` run 43691546229 | rust, web, e2e (Playwright on the real binary), contract drift, audit, policy, golden-windows and docker all succeeded. The first `web` attempt failed at `npm audit signatures` (`EMISSINGSIGNATUREKEY` for postcss@8.5.32 attestations). The lockfile is unchanged and the check passed locally, and the rerun passed. |
| `publish` run 43692117407 | image index `ghcr.io/tanmay-alpha/blackbox-hft@sha256:1d35116cd5927c91df7c658edc9182f8c23076055edf8de6e675bff9e002b237`, attested; `deploy` skipped because no hook is configured, so the manual digest release followed |
| Render deploy | `render deploys create srv-dbs11fm1egvs73chs7lg --image …@sha256:1d35116c…`. Final deploy `dep-dbsmfrjl550s738ssrmg` is `live` |
| Config correction | The service carried a manual `BB_GIT_SHA=7840bc6…` env override, which made `/status` report the old commit after the new image went live. Removed (HTTP 204) so the image's baked value applies, as DEPLOYMENT.md §5 specifies. Production now reports `git_sha=d85f743…` |
| Smoke (`scripts/deploy/smoke.sh`, `EXPECT_GIT_SHA=d85f743…`) | 8/8 PASS: liveness, status SHA, readiness (`migrations 0009`, db ok, blob configured), CORS allow/refuse, foreign-Origin 403, anonymous mutation 401, malformed key 401 |
| Vercel | Production deployment `blackbox-do96bv953…` Ready, created 2026-11-15 07:48 UTC, after the push to `main` |
| Worker | `/status`: `idle`, heartbeat age 5.6 s against a 15 s interval |
| Demo data | No reseed needed (`bb-core`/`bb-sim`/`bb-strategies` unchanged since `48e277b`). Anonymous `/comparisons` returns the 5 curated comparisons |

## Browser acceptance against production (`npm run test:prod`, headless Chromium, this workstation)

7/7 passed in 21.1 s:

1. `/`: the latency question is visible; the result numbers equal the stored comparison from the API; the first-divergence jump shows both scenarios' lifecycle; forensics opens and Back restores the page.
2. `/book/runs/253bbc23b342f373`: the replay window loads with no error box. The baseline was a 500 `response_too_large` (PRODUCT_BASELINE D1).
3. `/experiments`, `/compare/set?runs=…&demo=cost_of_latency` and `/system` render with no console errors.
4. At 768×1024 and 390×844, `/` has `scrollWidth` equal to the viewport. The baseline was 1405 px at 390 (D4).

Screenshots: `docs/rebuild/img/prod-*.png`.

## Rollback

```
render deploys create srv-dbs11fm1egvs73chs7lg --image ghcr.io/tanmay-alpha/blackbox-hft@sha256:23dee57ab010775e0402542f742ba56187173fdf4dea220b19ad6ed591a2869e --wait --confirm
```

To roll the frontend back, promote the previous Vercel production deployment. No migration was added, so rollback needs no database step.

## Not verified in production

- Keyed submission → result was verified locally (RESEARCH_JOURNEY.md, Playwright `research.spec.ts` in CI), not against production, to save free-tier budget. (Verified on production later the same day: "Authenticated production research journey" below.)
- Render cold start was not measured: the instance was warm.

## Second release: UI polish (2026-11-15)

Commit `a665991` (refined design tokens and shell; calmer Explore hero with a three-metric result strip).
CI run 43693354372 all green; image `sha256:212495fa…c659cc1` deployed to Render (`live`); smoke 8/8
PASS; `/status` reports `git_sha=a665991…`; Vercel production `blackbox-ju4q5fzqt…` Ready;
`npm run test:prod` 7/7 PASS. Local Playwright 7/7, vitest 111/111, `tsc` clean.

## Authenticated production research journey (2026-11-15 09:54–09:55 UTC)

Spec: `web/e2e-prod/research.prod.spec.ts`; the key is taken from `PROD_RESEARCH_KEY`, typed into the access dialog
and never logged. Raw evidence: [prod-research-evidence.json](prod-research-evidence.json). Screenshots: `img/prodj-0*.png`.
1 passed (26.4 s). Backend `a665991`.

| Step | Result |
|---|---|
| Strategy Lab, key sign-in, `synthetic-demo`, preset "Touch maker: baseline", entry latency 5 ms, seed 2026839111 | validation `valid`; changed-vs-preset shows `5 ms` |
| Submit → job `01a1dea1-7627-79c1-a2e4-263821cc0a6b` | UI states: running (1.2 s) → completed (10.0 s); API: queued→started 0.20 s, started→finished 7.17 s, attempt 1, 1,201/1,201 ticks, worker platform `linux-x86_64+libm-53154a4e` |
| Run `e2321a7eec3e23e1` | `GET /runs/{id}` (bundle-verified detail read) 200; result sha256 `bb34d8c567022f805414e125430c9eb45cac0a7758d21a2ef7539e1ab865164c`; 2,051 orders, 1,182 fills, net P&L −71.169 (simulated) |
| Order blotter, one order's forensics, Order Book Replay | rendered; replay showed "Depth at cursor" with no error box |
| Comparison vs demo run `253bbc23b342f373` | 201, id `01a1dea1-bd3b-765f-…`, aligned and like-for-like; diffs `execution.order_latency.entry_ns`, `seed` |
| Report `GET /runs/{id}/report` | 200, 25,426 B, research report title |
| Reload | key kept in the tab; run still shown |
| Sign out | badge DEMO; UI "Run … was not found"; anonymous `GET /runs/{id}` → 404 |
| Sign in again | blotter visible again |
| Page errors | none |

Production state written: one experiment/job/run, one comparison, in workspace `research`. The test key was the
existing research key (scopes `read,run`), so nothing was minted and nothing needs revoking.

## Phase 2 release (2026-11-15 10:47 UTC)

Commit `96a3ccb4f804dc327e6297266bf445b7dae0e883` (merge of `phase2/ui`). CI run 43704539393: all 8 jobs green.
Image `ghcr.io/tanmay-alpha/blackbox-hft@sha256:565fc4041b13789b069e6a793d2200ad5bced103b33e7991f090cded294c2d86`,
Render deploy `dep-dbsoram7bikc73c2i3j0` live; `/status` `git_sha=96a3ccb…`, worker idle, `readyz` migrations 0009.
Vercel production `blackbox-4dpe08p88…` Ready. Smoke 8/8 PASS. Anonymous acceptance 7/7 PASS.
Authenticated journey PASS again: job `01a1ded2-4b4d-74b8-875a-baac7d2a45e1`, run `b0f713f6a8cb46af`, started→finished
7.93 s on the worker, comparison 201, report 200, anonymous read 404, re-auth OK, 0 page errors. The result sha256
`bb34d8c5…` is identical to run `e2321a7eec3e23e1` under the previous release `a665991`. The only config difference is
the seed, and the touch-maker strategy uses no random draws. The phase-2 backend changes therefore did not change
simulation output.

Rollback for this release: redeploy `sha256:212495fa724420186e871fdea9a44cbf09b647661a0e451de3e442cc3c659cc1`
(release `a665991`). No migration was added.

## Phase 6/7 production suite (`web/e2e-prod/`)

Current suite: 16 tests in 6 files (`npx playwright test -c playwright.prod.config.ts --list`): `acceptance.spec.ts`
(the 7 tests above), the four Phase 6/7 specs below (8 tests) and the keyed `research.prod.spec.ts` (1 test, skipped
unless `PROD_RESEARCH_KEY` is set). Latest result, production `a24ccf9` (2026-11-19, [PHASE67_RELEASE.md](PHASE67_RELEASE.md)
follow-up 5): 16/16 passed; `market.prod.spec.ts` needed its ladder assertion updated for the cells' screen-reader text
and passed 3/3 on rerun. Keyed journey: job `01a1f33e-ce13-7966-95b0-7b1839413274`, run `299f836aa96bd57c`, result
sha256 `bb34d8c5…` (equal to the 2026-11-15 and `3ed53cd` keyed runs), comparison 201, report 200 (25,696 B), anonymous
read 404, re-authentication OK, 0 page errors ([prod-research-evidence.json](prod-research-evidence.json)). Previous:
`3ed53cd`, 15 anonymous passed and keyed run `c6d47c551e17c5af`.

The Phase 6/7 specs are anonymous and read-only. Every on-screen number is compared with the API or
artefact response fetched in the same test; each full page load is paced (≥ 2 s) and recorded as a test annotation;
console errors and failed requests fail the test unless expected.

| Spec | Page loads | Checks |
|---|---:|---|
| `market.prod.spec.ts` (3 tests) | 3 | Workbench `l3-demo`: synthetic badge, validation counts, curated example, L2/L3 before/after values vs event-detail API, J/K/Next/Previous and URL, metrics vs API, Back, out-of-range event notice, 390×844 overflow |
| `studies.prod.spec.ts` (1) | 1 | study artefact SHA-256 = published `.sha256` = badge; net P&L matrix = artefact; stored runs link to existing `/forensics/{id}`; unstored rows show the reproduction command |
| `compare-set.prod.spec.ts` (1) | 1 | `execution_validation` pairs: aligned ×3, metrics-only ×2, compared elsewhere ×1 (classified independently from `/demos` + `/comparisons`); never "missing"; zero `GET /runs/{id}`; a 503 is accepted only as the explicit busy state with Retry |
| `errors.prod.spec.ts` (3) | 3 | unknown run 404 page; unsupported study contract (intercepted) shows a contract error; blocked dataset API shows the unreachable state and Retry recovers |

With the existing acceptance spec (7 loads) a full run is about 15 production page loads (≤ 30 with one retry each),
well under the Vercel bot-checkpoint threshold. Targets come from `PROD_WEB_URL` and `PROD_API_URL` (origins; default
production), so the same suite runs locally:
`PROD_WEB_URL=http://127.0.0.1:18880 PROD_API_URL=http://127.0.0.1:18880 npx playwright test -c playwright.prod.config.ts`.
Screenshots are written to `docs/rebuild/img/` only when the target is production.
