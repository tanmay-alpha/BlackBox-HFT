# Phase 11: final audit

Seven independent audits of everything shipped in Phases 6–10 (production `3ed53cd`), run on
branches from `main` `80340e6` and integrated on `p11/int`. Each confirmed defect has a regression
test that failed before its fix unless noted.

| Area | Defect (severity) | Fix |
|---|---|---|
| API cache | Anonymous cache hits ignored `If-None-Match` (200 instead of 304) (P1) | Conditional handling incl. weak and wildcard validators (`5580c6f`) |
| API cache | A cached anonymous response hid a due demo-snapshot recheck: an externally added dataset stayed invisible (failed on untouched `80340e6`) (P1) | Cache lookup misses when the recheck is due (`b602bfc`, `tests/snapshot_cache_recheck.rs`) |
| API | Oversized or malformed query values returned Axum plain-text errors instead of the error envelope (P1) | Shared envelope-preserving query extraction (`5580c6f`) |
| Worker | Cancellation and lease loss could continue persistence or delay the cancel acknowledgement (P1) | Upload and readback race fixes (`5580c6f`) |
| Reports | Exchange window and slice provenance misstated (P2) | Report text corrected; run identities and result hashes unchanged (`5580c6f`) |
| Artifacts | A verified download rejected at cache admission stayed on disk outside the budget (medium) | Temp-path ownership through admission (`97a8faa`) |
| Artifacts | Cancelling every whole-run decode reader left its single-flight entry forever (low) | Cancellation-safe flight ownership (`97a8faa`) |
| UI | Compare Set showed raw `metrics_only`; sweep form showed raw queue-model codes (low) | Readable labels (`eb7ad9d`, `bb08e36`) |
| UI a11y | Study cell detail lost focus on open (phone), Close and Escape; workbench Previous/Next lost focus; L2 ladder cells read as bare numbers; loading counter re-announced every second; touch targets 18–20 px (high to medium) | Focus return, `aria-disabled`, hidden text, `aria-hidden` counter, 44 px coarse-pointer targets (`916a80c`, `web/e2e/a11y-phase11.spec.ts`) |
| Research | `/queue-lab` quoted the superseded single-day queue conclusion (medium) | Three-day outcomes with non-replication caveat (`f32cd76`); 549 doc rows / 15,170 values re-checked against artefacts, no mismatch |
| Deploy | `render.yaml` did not declare `BB_API_KEY_PEPPER`; a fresh Blueprint would not start (medium) | Declared `sync: false` (`3dbb3e7`) |
| Docs | Tracker, release record, deployment env table, rollback target, README stale (medium to low) | Aligned with code and production (`6e43114`) |
| Prod spec | Keyed production journey used a pre-redesign preset selector | Selector fixed; passed on production `3ed53cd` (job `01a1f231-a309-7585-8556-e45a2d92b240`, run `c6d47c551e17c5af`) |

Checked without a defect: route walk (156 visits, 39 states × desktop/phone × anonymous/keyed: 0
console, request or overflow errors; 255 links valid), snapshot trust and visibility, trailer
authentication, fingerprint coverage, SQLx release hook, log redaction, page structure, focus
visibility and badge contrast (lowest 5.23:1). Performance: no confirmed regression; mobile
Explore/Replay figures were taken under concurrent build load ([PHASE11_PERF_CHECK.md](PHASE11_PERF_CHECK.md)).

Not fixed (unconfirmed or suggestions): workbench step does not announce the landed event; desktop
cell detail sits after the page in tab order; research home and workbench mobile CLS 0.086–0.087
(below 0.1, cause unattributed); consecutive backticks in source-input names may break report
Markdown fencing; external database quarantine stays visible to anonymous readers until the next
snapshot recheck (documented window).
