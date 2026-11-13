# BlackBox HFT HTTP API

Base path `/api`. JSON in, JSON out. Every payload type named below is the serde JSON form of the
Rust type with the same name in `crates/bb-core/src` — that crate is the source of truth.

## Conventions

- **Timestamps** are integers: nanoseconds relative to the dataset's `origin_ns`. `origin_ns`
  itself is a decimal string (absolute epoch nanoseconds do not fit a JavaScript number).
- **Prices** are integer ticks; multiply by `instrument.tick_size` to display.
- **Enums** serialize as `snake_case` strings. Tagged enums carry a `type` field.
- `Option<T>` serializes as `T` or `null`. Tuples such as `(i64, f64)` serialize as arrays.
- **Errors**: non-2xx with body `{"error": {"code": "<snake_case>", "message": "<text>"}}`.
  Codes: `not_found` (404), `invalid_request` (400: malformed path, query or body, including
  unknown JSON fields and bodies over the 2 MiB limit), `forbidden` (403: request addressed to a
  host name or coming from an origin the server does not allow; see SECURITY.md),
  `invalid_config` (422, message lists every problem), `simulation_failed` (500), `internal` (500;
  the response carries a generic message, details go to the server log).
- Run labels are at most 200 characters.
- Everything returned is simulated. Nothing here is an exchange execution.

## Endpoints

| Method | Path | Request | Response |
|---|---|---|---|
| GET | `/api/health` | – | `Health` |
| GET | `/api/datasets` | – | `DatasetManifest[]` |
| GET | `/api/datasets/{id}` | – | `DatasetManifest` |
| GET | `/api/datasets/{id}/replay` | query `ReplayQuery` | `ReplayWindow` |
| GET | `/api/presets` | query `dataset_id` (optional; default: first dataset) | `Preset[]` |
| POST | `/api/runs` | `RunRequest` | `RunSummary` |
| GET | `/api/runs` | – | `RunSummary[]` (newest first) |
| GET | `/api/runs/{run_id}` | – | `RunSummary` |
| DELETE | `/api/runs/{run_id}` | – | `204` |
| GET | `/api/runs/{run_id}/engine-check` | – | `EngineStateCheck` |
| GET | `/api/runs/{run_id}/orders` | query `OrdersQuery` | `OrdersPage` |
| GET | `/api/runs/{run_id}/orders/{order_id}` | – | `OrderDetail` |
| GET | `/api/runs/{run_id}/fills` | – | `FillRecord[]` |
| GET | `/api/runs/{run_id}/equity` | – | `EquitySample[]` |
| GET | `/api/runs/{run_id}/tape` | query `TapeQuery` | `TapePage` |
| POST | `/api/compare` | `CompareRequest` | `Comparison` |
| POST | `/api/grids` | `GridSpec` | `GridResult` |
| GET | `/api/runs/{run_id}/timeline` | query `TimelineQuery` | `RunTimeline` |
| GET | `/api/runs/{run_id}/report` | – | `text/markdown` attachment (research report) |
| GET | `/api/runs/{run_id}/export` | – | `RunResult` JSON attachment (complete run) |
| GET | `/api/demos` | query `dataset_id` (optional; default: first dataset) | `Demo[]` |

`POST /api/runs` is synchronous and idempotent: a configuration that was already simulated on the
same dataset returns the stored run (same `run_id`, same `result_sha256`). `POST /api/grids` is
synchronous; it runs every cell as a real simulation (at most `MAX_GRID_CELLS = 100`) and stores
each one as a normal run, so any cell can be opened in the forensics view.

## API-only types

These live in `crates/bb-app/src/api.rs`.

```rust
struct Health {
    status: String,            // "ok"
    blackbox_version: String,
    contract_version: u32,
    engine: EngineInfo,
    disclaimer: String,        // research-only / simulated-results statement
}

struct ReplayQuery {           // all optional
    t0: Option<i64>,           // default: dataset start
    t1: Option<i64>,           // default: min(dataset end, t0 + 60 s)
    step_ns: Option<i64>,      // default: (t1 - t0) / 300, at least 1 ms
    levels: Option<u32>,       // default 25, max 100
    max_trades: Option<u32>,   // default 5000, max 50000
}

struct Preset {
    id: String,
    title: String,
    description: String,
    /// What the preset is meant to demonstrate, in one sentence.
    demonstrates: String,
    config: RunConfig,
}

struct RunRequest {
    config: RunConfig,
    label: Option<String>,
}

struct OrdersQuery {           // all optional
    outcome: Option<OrderOutcome>,
    side: Option<Side>,
    offset: Option<u64>,       // default 0
    limit: Option<u64>,        // default 200, max 2000
}

struct OrdersPage {
    total: u64,                // after filtering
    offset: u64,
    orders: Vec<OrderRecord>,
}

struct OrderDetail {
    order: OrderRecord,
    /// Every tape event of this order, in `seq` order.
    events: Vec<FlightEvent>,
    fills: Vec<FillRecord>,
    explanation: Explanation,
    instrument: InstrumentMeta,
}

struct TapeQuery {             // all optional
    from_seq: Option<u64>,     // default 0
    limit: Option<u64>,        // default 500, max 5000
    order_id: Option<u64>,
}

struct TapePage {
    total: u64,                // tape length (after order_id filtering)
    events: Vec<FlightEvent>,
}

struct CompareRequest {
    a: String,                 // run_id
    b: String,                 // run_id
}
```

## Comparison semantics

`POST /api/compare` returns `Comparison` (`crates/bb-core/src/compare.rs`, computed by
`bb_forensics::compare_runs`). A comparison is a pure function of the two stored runs and the
instruments of their datasets; nothing is re-simulated.

```rust
struct Comparison {
    run_a: String, run_b: String, label_a: String, label_b: String,
    eligibility: Eligibility,
    config_diffs: Vec<ConfigDiff>,   // every dotted RunConfig path that differs
    metrics: Vec<MetricRow>,         // always returned; not like-for-like unless eligibility says so
    alignment: Option<Alignment>,    // null when eligibility.aligned is false
    interpretation: Vec<InterpretationNote>,
}

struct Eligibility {
    aligned: bool,                   // no reason has effect "no_alignment"
    like_for_like: bool,             // reasons is empty
    reasons: Vec<EligibilityReason>,
}

struct EligibilityReason {
    field: String,                   // see the table below
    a: Value, b: Value,              // null when unknown
    effect: EligibilityEffect,       // "no_alignment" | "not_like_for_like"
    explanation: String,
}

struct Alignment {
    identical: u64,
    same_fills_different_history: u64,
    same_fills_different_fees: u64,
    same_outcome_different_timing: u64,
    same_decision_different_outcome: u64,
    different_decision: u64,
    only_a: u64,
    only_b: u64,
    first_divergence: Option<OrderPair>,          // first pair in decision order that is not identical
    first_outcome_divergence: Option<OrderPair>,  // first pair whose decision or fills differ
    timing: TimingSummary,
    pairs: Vec<OrderPair>,           // non-identical pairs in decision order, at most pairs_cap
    pairs_cap: u64,                  // 500
    pairs_truncated: bool,
}

struct OrderPair {
    order_id: u64, tick_no: u64,
    decision_ts: i64,                // A's decision time (B's for only_b)
    class: PairClass,
    differences: Vec<String>,        // every differing field, most significant group first
    timing: Option<PairTiming>,      // null unless the order exists in both runs
    a: Option<DecisionOutcome>, b: Option<DecisionOutcome>,
}

struct PairTiming {                  // B minus A in ns; null unless both have the timestamp
    decision_ns, exch_arrival_ns, first_response_ns, first_fill_ns, completion_ns: Option<i64>,
}

struct TimingSummary { decision, exch_arrival, first_response, first_fill, completion: DeltaStats }
struct DeltaStats { count: u64, nonzero: u64, min: Option<i64>, median: Option<f64>, max: Option<i64> }

struct DecisionOutcome {             // one run's order record, as compared
    side, price_tick, qty, tif, ord_type, decision_ts, outcome, final_status, filled_qty,
    avg_fill_tick, maker_qty, taker_qty, fees, fill_count, queue_ahead_at_entry,
    exch_arrival_ts, first_response_ts, first_fill_ts, last_fill_ts, cancel_sent_ts, terminal_ts,
}
```

**Eligibility** is checked before any per-order alignment. Order ids are `(tick_no << 8) | slot`,
so the same id names the same decision only on the same market and decision grid.

| `field` | Effect | Why |
|---|---|---|
| `dataset.sha256` (events hash) | `no_alignment` | Different market data |
| `instrument.tick_size`, `instrument.lot_size` | `no_alignment` | Equal ticks or lots are different prices or sizes |
| `instrument.contract_size` | `not_like_for_like` | Quote values scale differently |
| `instrument` (unverifiable) | `not_like_for_like` | A run's dataset manifest is unavailable or its events changed; sizes were not compared |
| `strategy.kind` (the `type` tag) | `no_alignment` | Unrelated decisions share ids |
| `strategy.interval_ns` | `no_alignment` | Tick n falls at a different time in each run |
| `simulation_revision` | `no_alignment` | The simulator itself may decide or record differently |
| `engine` (`"<name> <version>"`) | `not_like_for_like` | The matching engine may fill differently |
| `record.sample_every_ticks` | `not_like_for_like` | Sampled metrics (drawdown, mean absolute position) are marked at different times |
| `record.max_queue_updates_per_order` | `not_like_for_like` | Recorded lifecycles can differ by retention alone |

Strategy parameters, risk limits, seed and execution assumptions are what scenarios vary; they are
listed in `config_diffs` and never block alignment.

**Pair classes.** Orders are merged by id in decision order (tick, then slot). For an order present
in both runs the groups below are checked in order; the first group with a difference decides the
class, and `differences` lists every differing field of every group:

| Class | Group that differs (fields named in `differences`) |
|---|---|
| `different_decision` | `side`, `price_tick`, `qty`, `tif`, `ord_type` |
| `same_decision_different_outcome` | `outcome`, `final_status`, `filled_qty`, `avg_fill_tick`, `fill_count`, `fills` (a fill's price, quantity or maker/taker side) |
| `same_outcome_different_timing` | `decision_ts`, `exch_arrival_ts`, `first_response_ts`, `fill_ts` (any fill's time), `cancel_sent_ts`, `terminal_ts` (a value present in one run only also differs) |
| `same_fills_different_fees` | `fees` (any fill's fee) |
| `same_fills_different_history` | `queue_ahead_at_entry`, `cancel_rejected`, `lifecycle_events` (the order's tape events, compared by time, clock, source and content, ignoring `seq`): identical fills under distinct causal histories |
| `identical` | nothing |

`only_a`/`only_b` mark an id decided in one run only. Timestamps compare exactly; quantities, prices
averaged over fills and fees compare within 1e-9 relative. `first_divergence` is therefore exact:
every pair before it is identical including timing, fees and lifecycle. `first_outcome_divergence`
is the first `different_decision`, `same_decision_different_outcome`, `only_a` or `only_b` pair;
pairs before it may differ only in timing, fees or history, and an interpretation note counts them.
`timing` summarises every pair present in both runs except `different_decision`, identical pairs
included.

Example (synthetic demo, `risk_adverse` against `power_prob3`, BASELINE_AUDIT §5.1): order 9472
fills at the same price at t = 3.831084401 s in one run and 3.859654055 s in the other; it is
`same_outcome_different_timing` with `first_fill_ns = 28569654`, and is one of the 5 timing-only
pairs the note reports before the first outcome divergence at strategy tick 671.

## Replay terminal, demos and exports

```rust
struct TimelineQuery {         // all optional
    t0: Option<i64>,           // default: run start (first equity sample ts, else 0)
    t1: Option<i64>,           // default: min(run end, t0 + 60 s)
    max_events: Option<u32>,   // default 5000, max 50000
}

/// Everything the run-level replay terminal overlays on the dataset's ReplayWindow for [t0, t1].
struct RunTimeline {
    run_id: String,
    dataset_id: String,
    t0: i64,
    t1: i64,
    /// Tape events whose `ts` is in [t0, t1] on either clock, in `seq` order, capped at
    /// max_events (earliest kept).
    events: Vec<FlightEvent>,
    events_total: u64,         // in the window before capping
    events_truncated: bool,
    /// Fills with exchange ts in [t0, t1].
    fills: Vec<FillRecord>,
    /// Equity samples with ts in [t0, t1], plus the last sample before t0 when one exists
    /// (so the terminal knows the state in force at t0).
    equity: Vec<EquitySample>,
    /// Orders alive at some point in [t0, t1]: decision_ts <= t1 and (terminal_ts, else the
    /// ts of the order's last event, else run end) >= t0. Ascending order_id.
    orders: Vec<OrderRecord>,
    /// Run span: first and last equity sample ts.
    run_start: i64,
    run_end: i64,
}

/// A canonical, reproducible experiment: a set of run configurations on one dataset that differ
/// only in the listed variables.
struct Demo {
    id: String,                // "cost_of_latency" | "queue_position" | "gross_vs_net"
    title: String,
    question: String,          // the research question in one sentence
    /// Dotted RunConfig paths that differ between the runs.
    varied: Vec<String>,
    /// Plain-language list of what is held identical (dataset hash, strategy params, seed, ...).
    controlled: Vec<String>,
    /// How to read the result, without asserting what it will be.
    how_to_read: String,
    runs: Vec<DemoRun>,
    /// CLI commands that reproduce every run of the demo.
    reproduce: Vec<String>,
}

struct DemoRun {
    label: String,             // e.g. "1 ms order latency"
    config: RunConfig,
}
```

`TimelineQuery` bounds are inclusive; `t0 == t1` is valid and `t1 < t0` returns
`invalid_request` (400). Explicit bounds are not clamped to the run span. The empty-equity
span is `[0, 0]`. The tape is selected by each event's own clock timestamp, not sorted by
timestamp; sequence order is retained. `max_events=0` returns only counts and other overlays.

Demo variants use the preset touch maker unchanged: latency is 1 ms versus 50 ms each way;
queue models are `risk_adverse`, `log_prob` and `power_prob3` (`n=3`); fee schedules are
maker/taker fractions `0.0002/0.0004`, `0/0`, and `-0.0001/0.0004` (the rebate variant
retains the baseline taker fee). `varied` is the sorted union of dotted JSON differences from
the first config, including missing object fields; arrays are compared as whole values.
`controlled` lists only identical settings, plus dataset id and events SHA-256.
With no datasets, `/api/demos` returns `[]`; an unknown explicit dataset id returns 404.
Each `reproduce` entry is a POSIX-shell recipe that writes a complete config JSON file, then
invokes `blackbox run --config <file> --dataset <id>`. Save the JSON manually when using a
different shell; add `--data-dir <directory>` for nondefault local storage.
`blackbox demos [--dataset <id>]` lists the experiments and recipes; `--run` executes every
scenario idempotently and prints comparisons of alternatives against the first scenario.

Demo runs are created through the ordinary idempotent `POST /api/runs`, so a demo that was run
before returns the stored runs instantly and the same run ids every time.

The research report (`/report`) contains: dataset provenance (id, kind, source, hashes, local
timestamp provenance, worst quality severity), engine name and version, run and config hashes,
the full configuration as JSON, every assumption in plain language, all metrics, the engine
cross-check, the CLI command that reproduces the run, and the standard limitations.

Reports have `Content-Type: text/markdown; charset=utf-8` and
`Content-Disposition: attachment; filename="blackbox-run-<run_id>.md"`. They include a
config-file creation recipe and the same CLI invocation described above. Provenance comes
from the local dataset manifest: a missing dataset returns 404; a manifest whose events hash
no longer matches the run returns `invalid_request` (400), rather than attaching provenance
for replacement data. Full JSON exports have `Content-Type: application/json` and
`Content-Disposition: attachment; filename="blackbox-run-<run_id>.json"` and need only the
stored run, not the dataset files.
