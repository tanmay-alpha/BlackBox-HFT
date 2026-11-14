# Target product architecture

This document covers only what the rebuild changes. The system architecture
(`docs/ARCHITECTURE.md`, `docs/engineering/TARGET_ARCHITECTURE.md`) stays as it is: the Rust
workspace, the axum `/api/v1`, the Postgres queue, Blob artefacts, one in-process worker on Render
Free, and the Vite SPA on Vercel. The rebuild adds no service, no database and no paid resource.

## 1. Principles

- One frontend and one design system (`web/src/styles/tokens.css`); no second app.
- Public = read-only stored results from the `demo` workspace. Computation needs a key, as it
  does today. The public demo never submits simulations.
- Every number shown comes from an API field. Browser-side arithmetic is limited to stated
  identities (for example net = realized + unrealized − fees), and each one is checked against
  the stored value before it is shown (`features/explore/story.ts::pnlBreakdown`).
- Every replay window is complete or labelled incomplete. A cap is never shown as if it were the
  whole window (D1/D2 in PRODUCT_BASELINE).

## 2. Information architecture

| Section | Workspaces (route) | Audience |
|---|---|---|
| Explore | Guided latency experiment (`/explore`, planned to become `/`); Experiments catalogue (`/`) | anonymous |
| Research Studio | Strategy Lab (`/strategy`), Jobs & Sweeps (`/sim`), Reports (`/reports`) | keyed (read-only for anonymous visitors) |
| Market Lab | Datasets (`/data`), Order Book Replay (`/book`) | both |
| Execution Analysis | Runs & Forensics (`/forensics`), Comparisons (`/compare`), Risk & P&L (`/risk`) | both |
| System | System status (`/system`) | both; admin tabs need a key |

Source: `web/src/shell/workspaces.ts` (`SECTIONS`, `WORKSPACES`). The rail is labelled by default
on desktop widths, and the inspector is closed by default (`shell/settings.tsx`, settings key
`bb.settings.v2`).

## 3. Public latency experiment data flow

```mermaid
flowchart LR
  D[GET /demos] --> S[loadLatencyStory]
  R[GET /runs/:id x2] --> S
  C[GET /comparisons + /comparisons/:id] --> S
  S --> P[page: question, result, divergence]
  P -->|select order / time| M[loadMoment: /datasets/:id/versions/:v/replay + /runs/:id/timeline x2]
  P -->|select order| O[loadOrderPair: /runs/:id/orders/:order x2]
  P -->|deep links| F[/forensics/:run/orders/:order, /compare/set]
```

Every request is an anonymous `GET` that the demo snapshot or a verified bundle answers. Context
(scenario, order, time) lives in the URL query, so deep links and the back button keep it.

## 4. Replay completeness contract

`GET /runs/{id}/timeline` is cursor-paged (T2, branch `rebuild/replay-cursor`). Events are
returned in tape `seq` order, which is the stable unique key, so equal timestamps are ordered by
`seq`. The client follows `next_cursor` until the cursor is null, and shows `loaded / events_total`
while it does. Relative nanosecond times (`RelNs`, dataset-relative) stay JS numbers only while
they are below 2^53 (about 104 days of span); absolute epoch ns stay strings (`api/model.ts`).

## 5. Deferred decisions

- Analytics grid (Perspective / Glide / current TanStack): to be decided by a measured pilot (M6).
- QuantReplay: optional isolated engine; local PoC first, no hosted service (M8).
- Real-data validation: local-only, licence-gated (M7; see REUSE_MATRIX and
  `docs/research/MARKET_MICROSTRUCTURE.md` §7).
