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
  Codes: `not_found` (404), `invalid_request` (400), `invalid_config` (422, message lists every
  problem), `simulation_failed` (500), `internal` (500).
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
