# Keyed research journey (2026-11-14)

Walked in Chromium against the Vite dev server (http://localhost:5173) and the local API, signed in with a research key through the top-bar access badge. Dataset `synthetic-demo` (synthetic, not market data); all results are simulated on historical replay with no market impact.

| # | Step | Outcome before fixes | Fix | Evidence |
|---|------|----------------------|-----|----------|
| 1 | Sign in (access badge → API key → Use key) | Works; badge shows RESEARCH. | — | — |
| 2 | Choose dataset (Datasets → Open in Strategy Lab) | Works. Dataset shows "Quality 1 failed" (not in this scope). | — | — |
| 3 | Strategy Lab: preset, edit 3 parameters, validate | Works, but "Changed vs preset" listed raw config paths with stored values (`execution.fees.maker 0.0002 → -0.00005`, `entry_ns 1000000`); preset table showed `touch_maker`; validation messages repeated raw ids. No simplified form. | Diff tables show the field name and the value in the form's unit (`Maker fee 2 bp → -0.5 bp`, `Entry latency 1 ms → 5 ms`), path kept as secondary text. Preset/run lists use plain strategy names. Added a **Guided / Advanced** toggle (Guided by default, choice remembered): Guided shows strategy, order quantity, entry/response latency and maker/taker fees with one-line explanations; Advanced is the full tabbed form. Both use the same field descriptors on the same `RunConfig`. | `img/rj-lab-guided.webp` |
| 4 | Submit (touch maker baseline, 1,201 ticks) → Jobs & Sweeps | Works: queued → running with progress → completed in < 1 s. | — | job `01a1da8d…`, run `acd4f46f93653e29` |
| 5 | Open result (run page) | Works. | — | — |
| 6 | Replay in Order Book Replay, jump to order 257 | Works. | — | — |
| 7 | One order's forensics ("Full forensics") | Works; plain-language explanation of the fill. | — | — |
| 8 | Compare with demo run `253bbc23b342f373` | **Blocked**: with a key the run pickers list only this workspace's runs, so the public demo run could not be chosen (only reachable by editing the URL). | Pickers are now comboboxes: pick from the list or paste any run id; hint explains the 16-hex id. Comparison created from the picker. Also removed a duplicated "UTC UTC". | `img/rj-compare.webp` |
| 9 | Bounded sweep from the run (2 latencies × 2 queue models = 4 cells) | Works; report and caveats shown. "Created … UTC UTC" duplicated. | Duplicate removed. | sweep `01a1da8e-b8ef…`, `img/rj-sweep.webp` |
| 10 | Download run report (.md) | Works (`run-acd4f46f93653e29-report.md`, 25 KB). | — | — |
| 11 | Reload | Key kept (sessionStorage), private run still readable. | — | — |
| 12 | Sign out, private run must not be visible | **Defect**: the open run page kept showing the private run's data (stale client state; the API correctly answers 404 anonymously). | `AccessProvider` remounts the workspaces whenever the key is set, changed or removed, so every page refetches under the new identity. After sign out the page shows "404 run not found". | `img/rj-signed-out.webp` |

Re-walk after fixes: signed in, Guided mode with Touch maker baseline + entry 5 ms, maker −0.5 bp, taker 3.5 bp → Advanced showed the same values → submitted → run `2a998e66a4bdea7d` → comparison with `253bbc23b342f373` created via the picker → reload kept RESEARCH → sign out showed 404 instead of the run.

## Fixed (follow-up)

- Jobs & Sweeps: the "SUBMITTED" banner is replaced by "Job … is completed/failed/…" once the job reaches a terminal state; a completed job no longer shows "tape events 0"; the queue caption uses the plain worker wording ("Idle — waiting for jobs (in-process worker)", "Busy — N of M job slots in use", "Not responding …").
- Run, run list, compare, sweep, grid and forensics pages show plain model names ("No partial fills", "Power probability 3 (n = 3)", "Touch maker") from the single describer in `web/src/components/describe.ts` (the duplicate in `features/forensics/labels.ts` is removed). Sweep caveat text from the server is unchanged.
- A 404 on a run without research access says the run may be private to a workspace and offers "Enter API key".

Checks: `cd web && npx tsc --noEmit -p tsconfig.app.json` (pass), `npx vitest run` (10 files, 110 tests pass). `web/e2e/research.spec.ts` updated for the keyed journey (Guided mode, comparison with the demo run, sign-out visibility); not run here.
