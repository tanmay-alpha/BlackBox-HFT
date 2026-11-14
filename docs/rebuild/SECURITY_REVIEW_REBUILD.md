# Security review of the rebuild changes

Reviewer: `security-reviewer` agent (Claude Opus 5.5), 2026-11-14. Method: read-only review of the
source on `rebuild/explore` at HEAD `a2f754e`. Nothing was executed. Fix status is tracked in
IMPLEMENTATION_TRACKER (T16).

| # | Sev | Finding | Location | Reproduction | Fix |
|---|---|---|---|---|---|
| 1 | Medium | After the anonymous egress budget is spent, responses under 64 KiB are still served (counted, never refused), so tiny pages can keep draining bandwidth, bounded only by the rate limiter (120/min/IP) | `v1/edge.rs:47` (`EGRESS_SMALL_RESPONSE`), check near `:420` | After the budget is exhausted, follow `next_cursor` of `/runs/{id}/timeline?...&max_events=50` | Refuse everything except tiny/error responses once the budget is spent, or add a per-IP daily byte budget |
| 2 | Medium | Each timeline page, cursor pages included, decodes the whole run (`run.full()`) and recounts the window; `t0/t1` are unbounded; paging with `max_events=1` costs O(N²) and holds one of 4 heavy-read permits | `artifacts.rs:537`, `v1/runs.rs` `timeline_page`, `server.rs:536` | Follow cursors with `max_events=1` | Cache the parsed run, carry `events_total` in the cursor, enforce a minimum page size |
| 3 | Low | The cursor's window binding is an unkeyed 8-byte hash of the filter string, without the run id, so cursors can be forged or reused across runs; a forged cursor skips the first-page `window_too_large` check | `v1/respond.rs:160-183` | Build a cursor for any `row` | Include the run id; key it (HMAC) or validate the row bound |
| 4 | Low | `max_events=0` returns the same `next_cursor` forever | `v1/runs.rs` `timeline_page` | Follow cursors with `max_events=0` | No cursor for count-only requests, or reject 0 on cursor pages |
| 5 | Low | The anonymous cache key includes unknown query parameters, and the whole cache is cleared at 512 entries / 32 MiB, so unique `?x=N` requests evict the demo cache | `v1/edge.rs:484`, ~`:550` | Send 512 distinct `?x=` requests | Normalize the key to known parameters; evict per entry |

Checked with no finding: ITCH decoder length checks before field reads (`itch/decode.rs:404-418`);
execute/cancel quarantine prevents underflow (`itch/book.rs:880,930`); API key kept in
`sessionStorage` and cleared on 401; no `dangerouslySetInnerHTML`/`innerHTML`/`eval` in `web/src`.

Not reviewed: the ITCH CLI path handling, `itch/l2.rs`, and the explore components in detail. The
lead should still run `git ls-files` to check that no data or secret files are tracked.
