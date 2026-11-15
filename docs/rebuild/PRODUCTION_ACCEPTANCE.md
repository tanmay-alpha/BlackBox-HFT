# Production acceptance (M10)

Date: 2026-11-15. Release commit: `d85f743719ae9d46d29c7c9e0df58f476e8bff42` (`main`).

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

- Keyed submission → result was verified locally (RESEARCH_JOURNEY.md, Playwright `research.spec.ts` in CI), not against production, to save free-tier budget.
- Render cold start was not measured: the instance was warm.

## Second release: UI polish (2026-11-15)

Commit `a665991` (refined design tokens and shell; calmer Explore hero with a three-metric result strip).
CI run 43693354372 all green; image `sha256:212495fa…c659cc1` deployed to Render (`live`); smoke 8/8
PASS; `/status` reports `git_sha=a665991…`; Vercel production `blackbox-ju4q5fzqt…` Ready;
`npm run test:prod` 7/7 PASS. Local Playwright 7/7, vitest 111/111, `tsc` clean.
