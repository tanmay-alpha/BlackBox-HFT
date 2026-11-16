# HTTP API

The API is `/api/v1`. Its contract is [engineering/API_CONTRACT.md](engineering/API_CONTRACT.md); the normative
OpenAPI 3.1 document is generated from the code:

```sh
blackbox openapi --out openapi.json        # offline, no database
curl http://127.0.0.1:8787/api/v1/openapi.json
```

TypeScript types for the web client: `cd web && npm run gen:api` (writes `src/api/v1.gen.ts`).

The public Market Data Workbench routes (`/api/v1/market/datasets/...`, synthetic `l3-demo` only) are in
[API_CONTRACT.md §8.2a](engineering/API_CONTRACT.md#82a-market-data-workbench-l3-synthetic-only).

Local server (Postgres from docs/engineering/PROJECT_STATE.md):

```sh
export DATABASE_URL=postgres://blackbox:blackbox-local@127.0.0.1:55432/blackbox_dev
blackbox migrate
blackbox demo seed --wait
blackbox keys mint --workspace lab --scopes read,run --label laptop
BB_CORS_ALLOWED_ORIGINS=http://localhost:5173 blackbox serve
```

The web client already uses `/api/v1`. The unversioned `/api/*` router remains only as deprecated
file-store tooling (the local-profile fallback of `serve`); hosted `serve` does not expose it.
