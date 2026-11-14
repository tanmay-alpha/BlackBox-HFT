# Release runbook — rebuild/explore → main

Run from `D:/black box` in the tool bash (Git Bash builtins; plain `bash` on this box is WSL — do not use it).
Rollback image (current live): `ghcr.io/tanmay-alpha/blackbox-hft@sha256:23dee57ab010775e0402542f742ba56187173fdf4dea220b19ad6ed591a2869e`

## 0. Pre-checks (already verified at b71a46a)
- `git diff 48e277b..HEAD -- crates/bb-core crates/bb-sim crates/bb-strategies` → empty: run identity unchanged, **no `demo seed` needed**.
- No new migrations; no new `BB_*` vars in config.rs diff. Cursor HMAC uses existing `BB_API_KEY_PEPPER` → **no Render env changes**.

## 1. Merge and push
```sh
git fetch origin
git checkout main && git pull --ff-only origin main
git merge --ff-only rebuild/explore || git merge --no-ff rebuild/explore -m "Merge rebuild/explore"
git push origin main
SHA=$(git rev-parse HEAD); echo "$SHA"
```

## 2. CI → publish, get digest
```sh
gh run list --workflow ci.yml --branch main --limit 1          # note <ci_id>
gh run watch <ci_id> --exit-status
gh run list --workflow publish.yml --limit 1                   # note <pub_id> (triggered by workflow_run)
gh run watch <pub_id> --exit-status
gh run view <pub_id> --log | grep -m1 -o 'sha256:[0-9a-f]\{64\}'   # digest (also in run step summary)
# alternative via GHCR:
gh api /users/tanmay-alpha/packages/container/blackbox-hft/versions --jq '.[0] | .name, .metadata.container.tags'
DIGEST=sha256:...
```
If `RENDER_DEPLOY_HOOK_URL` is unset, publish ends "publish only (not deployed)" — expected; deploy manually below.

## 3. Render deploy by digest
```sh
R=C:/Users/TANMAY/.blackbox/bin/render-official.exe
$R deploys create srv-dbs11fm1egvs73chs7lg --image "ghcr.io/tanmay-alpha/blackbox-hft@$DIGEST" --wait --confirm -o json
# poll instead of --wait:
$R deploys list srv-dbs11fm1egvs73chs7lg -o json | jq '.[0] | {id, status, image}'
```
`--wait` exits non-zero on failed deploy.

## 4. Smoke
```sh
BASE_URL=https://blackbox-hft-api.onrender.com EXPECT_GIT_SHA=$SHA source scripts/deploy/smoke.sh
```
(source in tool bash; it `set -euo pipefail`s the shell — run in a subshell `( ... source ... )` if you want to keep the session.) Exit 0 = pass.

## 5. Vercel (Git-integrated; builds on push to main)
```sh
vercel ls blackbox-hft --scope tanmay-alphas-projects | head -5
vercel inspect <newest-production-url> --scope tanmay-alphas-projects   # alias blackbox-hft-git-main-* present
```
Verify commit in Vercel dashboard deployment "Source" (= `$SHA`) or `vercel inspect --logs`.

## Rollback
```sh
$R deploys create srv-dbs11fm1egvs73chs7lg --image ghcr.io/tanmay-alpha/blackbox-hft@sha256:23dee57ab010775e0402542f742ba56187173fdf4dea220b19ad6ed591a2869e --wait --confirm -o json
vercel rollback <previous-production-url> --scope tanmay-alphas-projects
```
