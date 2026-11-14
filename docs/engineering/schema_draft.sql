-- BlackBox HFT: PostgreSQL schema draft (target: PostgreSQL 18, Neon free plan).
--
-- Status: DRAFT for Phase 2. It becomes migration `0001_initial.up.sql` in `crates/bb-db/migrations`.
-- The matching down migration is the single statement at the end of this file (commented out).
-- Design rationale, ERD, index-to-query mapping, retention and the validation transcript are in
-- docs/engineering/DATABASE_DESIGN.md.
--
-- Conventions
--   * Everything lives in schema `bb`. Application SQL is always schema-qualified (`bb.jobs`), so
--     nothing depends on `search_path`. Neon's pooler (PgBouncer, transaction mode) does not
--     support session `SET`.
--   * Ids are `uuid` from PostgreSQL 18's built-in `uuidv7()` (time-ordered, so B-tree inserts stay
--     append-mostly). Natural text ids are kept where the product already uses them (dataset ids).
--   * SHA-256 digests are `bytea` of exactly 32 bytes (CHECKed). Hex is produced at the API edge.
--   * Nanosecond quantities are `bigint` (i64). Wall-clock instants are `timestamptz`.
--   * Every state or kind column is `text` + CHECK, not a PostgreSQL ENUM. Adding a value is then an
--     ordinary transactional `ALTER TABLE ... DROP/ADD CONSTRAINT` in a migration, and a value can
--     also be removed. Neither is possible with `ALTER TYPE ... ADD VALUE`.
--   * Custom SQLSTATEs: BB001 = immutability violation, BB002 = illegal job state transition.
--     bb-db maps them to the API's `conflict` / `internal` errors.

BEGIN;

CREATE SCHEMA bb;
COMMENT ON SCHEMA bb IS 'BlackBox HFT application schema (see docs/engineering/DATABASE_DESIGN.md)';

-- ---------------------------------------------------------------------------------------------
-- Generic trigger functions
-- ---------------------------------------------------------------------------------------------

-- Row immutability. Attached AFTER UPDATE, so stored generated columns in NEW are already
-- computed. TG_ARGV lists the columns that may still change. Any other difference aborts the
-- statement. FK actions (ON DELETE SET NULL) are UPDATEs too and must therefore be whitelisted.
CREATE FUNCTION bb.forbid_update() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
DECLARE
    allowed text[] := coalesce(TG_ARGV::text[], '{}'::text[]);
BEGIN
    IF (to_jsonb(OLD) - allowed) IS DISTINCT FROM (to_jsonb(NEW) - allowed) THEN
        RAISE EXCEPTION 'bb.%: row is immutable (mutable columns: %)', TG_TABLE_NAME,
            coalesce(nullif(array_to_string(allowed, ', '), ''), 'none')
            USING ERRCODE = 'BB001';
    END IF;
    RETURN NULL;
END $$;

-- TRUNCATE bypasses row triggers, so the tables that hold scientific inputs and results get a
-- statement-level guard as well.
CREATE FUNCTION bb.forbid_truncate() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    RAISE EXCEPTION 'bb.%: TRUNCATE is not allowed', TG_TABLE_NAME USING ERRCODE = 'BB001';
END $$;

-- Optimistic concurrency. Writers send `WHERE id = $1 AND version = $2`. A row count of 0 means a
-- concurrent writer won, and the API answers 409 `conflict`.
CREATE FUNCTION bb.bump_version() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    NEW.version := OLD.version + 1;
    NEW.updated_at := now();
    RETURN NEW;
END $$;

-- ---------------------------------------------------------------------------------------------
-- Identity and access
-- ---------------------------------------------------------------------------------------------

CREATE TABLE bb.workspaces (
    id                         uuid PRIMARY KEY DEFAULT uuidv7(),
    slug                       text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,39}$'),
    name                       text NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    kind                       text NOT NULL CHECK (kind IN ('demo', 'research', 'system')),
    -- Anonymous requests read the (single) public workspace.
    is_public_read             boolean NOT NULL DEFAULT false,
    quota_experiments_per_day  integer NOT NULL DEFAULT 100 CHECK (quota_experiments_per_day >= 0),
    quota_sweeps_per_day       integer NOT NULL DEFAULT 5 CHECK (quota_sweeps_per_day >= 0),
    quota_active_jobs          integer NOT NULL DEFAULT 50 CHECK (quota_active_jobs >= 0),
    created_at                 timestamptz NOT NULL DEFAULT now(),
    updated_at                 timestamptz NOT NULL DEFAULT now(),
    version                    bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    CHECK (kind <> 'demo' OR is_public_read),
    CHECK (kind = 'demo' OR NOT is_public_read)
);
-- Exactly one demo workspace may exist.
CREATE UNIQUE INDEX workspaces_one_demo ON bb.workspaces ((true)) WHERE kind = 'demo';
CREATE TRIGGER workspaces_version BEFORE UPDATE ON bb.workspaces
    FOR EACH ROW EXECUTE FUNCTION bb.bump_version();

CREATE TABLE bb.api_keys (
    id              uuid PRIMARY KEY DEFAULT uuidv7(),
    workspace_id    uuid NOT NULL REFERENCES bb.workspaces (id) ON DELETE CASCADE,
    -- sha256(utf8 key) of `bbk_<43 base64url chars>` (32 random bytes). Never the key itself.
    key_sha256      bytea NOT NULL UNIQUE CHECK (octet_length(key_sha256) = 32),
    -- `bbk_` plus the first 4 characters of the secret part, shown in UIs and logs.
    display_prefix  text NOT NULL CHECK (display_prefix ~ '^bbk_[A-Za-z0-9_-]{4}$'),
    label           text NOT NULL CHECK (length(label) BETWEEN 1 AND 100),
    scopes          text[] NOT NULL
                    CHECK (cardinality(scopes) >= 1 AND scopes <@ ARRAY['read', 'run', 'admin']::text[]),
    created_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz,
    revoked_at      timestamptz,
    -- Written at most once per hour per key (the app guards with `last_used_at < now() - 1h`).
    last_used_at    timestamptz,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    version         bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    CHECK (expires_at IS NULL OR expires_at > created_at),
    CHECK (revoked_at IS NULL OR revoked_at >= created_at)
);
CREATE INDEX api_keys_workspace ON bb.api_keys (workspace_id);
CREATE TRIGGER api_keys_version BEFORE UPDATE ON bb.api_keys
    FOR EACH ROW EXECUTE FUNCTION bb.bump_version();

-- Append-only history of security-relevant actions. No foreign keys on purpose: FK actions would
-- rewrite history when a workspace or key is deleted. Retention deletes old rows.
CREATE TABLE bb.audit_events (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at            timestamptz NOT NULL DEFAULT now(),
    workspace_id  uuid,
    api_key_id    uuid,
    action        text NOT NULL CHECK (action ~ '^[a-z_]+\.[a-z_]+$'),   -- e.g. experiment.create
    outcome       text NOT NULL CHECK (outcome IN ('ok', 'denied', 'error')),
    target_type   text,
    target_id     text,
    request_id    text,
    client_ip     inet,
    detail        jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(detail) = 'object')
);
CREATE INDEX audit_events_workspace_at ON bb.audit_events (workspace_id, at DESC);
CREATE INDEX audit_events_at_brin ON bb.audit_events USING brin (at);
CREATE TRIGGER audit_events_immutable AFTER UPDATE ON bb.audit_events
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update();

-- ---------------------------------------------------------------------------------------------
-- Market data
-- ---------------------------------------------------------------------------------------------

CREATE TABLE bb.data_sources (
    id                      text PRIMARY KEY CHECK (id ~ '^[a-z0-9_-]{1,64}$'),
    vendor                  text NOT NULL,
    licence                 text NOT NULL,
    licence_url             text,
    redistribution_allowed  boolean NOT NULL,
    -- Hosted deployments refuse to register datasets whose source is not hostable (enforced by the
    -- app's hosted profile, as export-static does today; a local DB may hold vendor data).
    hostable                boolean NOT NULL,
    notes                   text NOT NULL DEFAULT '',
    created_at              timestamptz NOT NULL DEFAULT now(),
    CHECK (NOT hostable OR redistribution_allowed)
);

CREATE TABLE bb.instruments (
    id          uuid PRIMARY KEY DEFAULT uuidv7(),
    exchange    text NOT NULL CHECK (length(exchange) BETWEEN 1 AND 64),
    symbol      text NOT NULL CHECK (length(symbol) BETWEEN 1 AND 64),
    kind        text NOT NULL CHECK (kind IN ('perpetual', 'future', 'spot', 'synthetic')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (exchange, symbol)
);

CREATE TABLE bb.datasets (
    id                  text PRIMARY KEY CHECK (id ~ '^[a-z0-9_-]{1,64}$'),
    -- NULL = global dataset visible to every workspace (the hosted synthetic datasets).
    owner_workspace_id  uuid REFERENCES bb.workspaces (id) ON DELETE RESTRICT,
    title               text NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
    kind                text NOT NULL CHECK (kind IN ('synthetic', 'recorded')),
    level               text NOT NULL CHECK (level IN ('l2_mbp')),
    instrument_id       uuid NOT NULL REFERENCES bb.instruments (id) ON DELETE RESTRICT,
    data_source_id      text NOT NULL REFERENCES bb.data_sources (id) ON DELETE RESTRICT,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    version             bigint NOT NULL DEFAULT 1 CHECK (version >= 1)
);
CREATE INDEX datasets_owner ON bb.datasets (owner_workspace_id);
CREATE TRIGGER datasets_version BEFORE UPDATE ON bb.datasets
    FOR EACH ROW EXECUTE FUNCTION bb.bump_version();

-- One immutable content version of a dataset. A run binds to a version, never to a dataset id.
CREATE TABLE bb.dataset_versions (
    id                     uuid PRIMARY KEY DEFAULT uuidv7(),
    dataset_id             text NOT NULL REFERENCES bb.datasets (id) ON DELETE RESTRICT,
    version_no             integer NOT NULL CHECK (version_no >= 1),
    events_sha256          bytea NOT NULL CHECK (octet_length(events_sha256) = 32),
    normalization_version  integer NOT NULL CHECK (normalization_version >= 1),
    contract_version       integer NOT NULL CHECK (contract_version >= 1),
    tick_size              double precision NOT NULL CHECK (tick_size > 0),
    lot_size               double precision NOT NULL CHECK (lot_size > 0),
    contract_size          double precision NOT NULL CHECK (contract_size > 0),
    origin_ns              bigint NOT NULL,                  -- absolute epoch ns
    start_rel_ns           bigint NOT NULL,
    end_rel_ns             bigint NOT NULL,
    timestamp_unit         text NOT NULL CHECK (timestamp_unit IN ('ns', 'us', 'ms')),
    row_count              bigint NOT NULL CHECK (row_count >= 0),
    events_bytes           bigint NOT NULL CHECK (events_bytes >= 0),
    -- 'regenerate': synthetic, rebuilt deterministically from manifest.generator on cold start and
    -- verified against events_sha256 (no storage, no Blob operations).
    -- 'blob' / 'fs': event archive at storage_key, verified against events_sha256 on load.
    storage                text NOT NULL CHECK (storage IN ('regenerate', 'blob', 'fs')),
    storage_key            text,
    manifest               jsonb NOT NULL CHECK (jsonb_typeof(manifest) = 'object'),
    created_at             timestamptz NOT NULL DEFAULT now(),
    retired_at             timestamptz,
    UNIQUE (dataset_id, version_no),
    UNIQUE (dataset_id, events_sha256),
    UNIQUE (id, dataset_id),                                  -- target of a composite FK
    CHECK (end_rel_ns >= start_rel_ns),
    -- Precision guard (BASELINE_AUDIT AT-9): every relative ns stays an exact JS number.
    CHECK (end_rel_ns - start_rel_ns < 9007199254740992),
    CHECK ((storage = 'regenerate') = (storage_key IS NULL))
);
CREATE TRIGGER dataset_versions_immutable AFTER UPDATE ON bb.dataset_versions
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update('retired_at');
CREATE TRIGGER dataset_versions_no_truncate BEFORE TRUNCATE ON bb.dataset_versions
    FOR EACH STATEMENT EXECUTE FUNCTION bb.forbid_truncate();

-- Quality checks are re-runnable with a newer checker without touching the immutable version.
CREATE TABLE bb.dataset_quality_reports (
    dataset_version_id   uuid NOT NULL REFERENCES bb.dataset_versions (id) ON DELETE CASCADE,
    checker_version      integer NOT NULL CHECK (checker_version >= 1),
    passed               boolean NOT NULL,
    error_count          integer NOT NULL CHECK (error_count >= 0),
    warning_count        integer NOT NULL CHECK (warning_count >= 0),
    book_valid_fraction  double precision NOT NULL CHECK (book_valid_fraction BETWEEN 0 AND 1),
    report               jsonb NOT NULL CHECK (jsonb_typeof(report) = 'object'),
    report_sha256        bytea NOT NULL CHECK (octet_length(report_sha256) = 32),
    created_at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (dataset_version_id, checker_version),
    CHECK (passed = (error_count = 0))
);
CREATE TRIGGER dataset_quality_reports_immutable AFTER UPDATE ON bb.dataset_quality_reports
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update();

-- ---------------------------------------------------------------------------------------------
-- Research: scientific inputs and results (global, content-addressed, immutable)
-- ---------------------------------------------------------------------------------------------

-- The experiment identity: sha256(contract_version, simulation_revision, engine name, engine
-- version, events_sha256, config), as computed by bb-app `run_identity` today.
CREATE TABLE bb.experiment_configs (
    id                      uuid PRIMARY KEY DEFAULT uuidv7(),
    fingerprint             bytea NOT NULL UNIQUE CHECK (octet_length(fingerprint) = 32),
    -- Legacy 16-hex run id, kept as a lookup alias. UNIQUE turns a 64-bit prefix collision into a
    -- loud insert error instead of the silent overwrite FsRunStore allows (BASELINE_AUDIT §4.2).
    short_id                text GENERATED ALWAYS AS (substr(encode(fingerprint, 'hex'), 1, 16)) STORED
                            UNIQUE,
    dataset_version_id      uuid NOT NULL,
    dataset_id              text NOT NULL,
    contract_version        integer NOT NULL CHECK (contract_version >= 1),
    simulation_revision     integer NOT NULL CHECK (simulation_revision >= 1),
    engine_name             text NOT NULL,
    engine_version          text NOT NULL,
    config                  jsonb NOT NULL CHECK (jsonb_typeof(config) = 'object'),
    -- Denormalised from `config` for filtering and comparison eligibility. Generated, so they
    -- cannot disagree with the config. `#>>` yields text for both number and string encodings.
    strategy_kind           text GENERATED ALWAYS AS (config #>> '{strategy,kind,type}') STORED
                            CHECK (strategy_kind IN ('touch_maker', 'avellaneda_stoikov',
                                                     'imbalance_taker', 'scripted')),
    interval_ns             bigint GENERATED ALWAYS AS ((config #>> '{strategy,interval_ns}')::bigint) STORED
                            CHECK (interval_ns > 0),
    -- RunConfig::estimate_work(&DatasetManifest), fixed at admission (deterministic).
    est_ticks               bigint NOT NULL CHECK (est_ticks >= 0),
    est_tape_events_upper   bigint NOT NULL CHECK (est_tape_events_upper >= 0),
    est_output_bytes_upper  bigint NOT NULL CHECK (est_output_bytes_upper >= 0),
    created_at              timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (dataset_version_id, dataset_id)
        REFERENCES bb.dataset_versions (id, dataset_id) ON DELETE RESTRICT,
    UNIQUE (id, dataset_id, strategy_kind),                  -- target of a composite FK
    CHECK (config ->> 'dataset_id' = dataset_id)
);
CREATE INDEX experiment_configs_dataset_version ON bb.experiment_configs (dataset_version_id);
CREATE TRIGGER experiment_configs_immutable AFTER UPDATE ON bb.experiment_configs
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update();
CREATE TRIGGER experiment_configs_no_truncate BEFORE TRUNCATE ON bb.experiment_configs
    FOR EACH STATEMENT EXECUTE FUNCTION bb.forbid_truncate();

-- A result: one experiment executed on one platform class. Only a run with quarantined_at IS NULL
-- counts for the identity, so a run that failed an integrity check can be re-simulated.
CREATE TABLE bb.runs (
    id                    uuid PRIMARY KEY DEFAULT uuidv7(),
    experiment_config_id  uuid NOT NULL REFERENCES bb.experiment_configs (id) ON DELETE RESTRICT,
    platform              text NOT NULL CHECK (platform IN ('linux-x86_64', 'linux-aarch64',
                                                            'windows-x86_64', 'macos-x86_64',
                                                            'macos-aarch64')),
    is_canonical          boolean GENERATED ALWAYS AS (platform = 'linux-x86_64') STORED,
    blackbox_version      text NOT NULL,
    build_profile         text NOT NULL CHECK (build_profile IN ('release', 'debug')),
    worker_id             text NOT NULL,
    result_sha256         bytea NOT NULL CHECK (octet_length(result_sha256) = 32),
    -- sha256 of the canonical JSON of (Metrics, EngineStateCheck, counts). Recomputed and checked
    -- whenever `metrics` is served, so a tampered DB value is detected (AT-3).
    summary_sha256        bytea NOT NULL CHECK (octet_length(summary_sha256) = 32),
    ticks                 bigint NOT NULL CHECK (ticks >= 0),
    order_count           bigint NOT NULL CHECK (order_count >= 0),
    fill_count            bigint NOT NULL CHECK (fill_count >= 0),
    tape_len              bigint NOT NULL CHECK (tape_len >= 0),
    equity_len            bigint NOT NULL CHECK (equity_len >= 0),
    wall_time_ms          bigint NOT NULL CHECK (wall_time_ms >= 0),
    peak_rss_bytes        bigint CHECK (peak_rss_bytes >= 0),
    metrics               jsonb NOT NULL CHECK (jsonb_typeof(metrics) = 'object'),
    engine_check          jsonb NOT NULL CHECK (jsonb_typeof(engine_check) = 'object'),
    -- Promoted for list sorting/filtering; generated so they always equal the JSON.
    net_pnl               double precision GENERATED ALWAYS AS ((metrics ->> 'net_pnl')::double precision) STORED,
    fees                  double precision GENERATED ALWAYS AS ((metrics ->> 'fees')::double precision) STORED,
    max_drawdown          double precision GENERATED ALWAYS AS ((metrics ->> 'max_drawdown')::double precision) STORED,
    orders_submitted      bigint GENERATED ALWAYS AS ((metrics ->> 'orders_submitted')::bigint) STORED,
    engine_check_agrees   boolean GENERATED ALWAYS AS ((engine_check ->> 'agrees')::boolean) STORED,
    created_at            timestamptz NOT NULL DEFAULT now(),
    quarantined_at        timestamptz,
    quarantine_reason     text,
    CHECK ((quarantined_at IS NULL) = (quarantine_reason IS NULL)),
    CHECK (fill_count <= tape_len)
);
CREATE UNIQUE INDEX runs_identity ON bb.runs (experiment_config_id, platform) WHERE quarantined_at IS NULL;
CREATE TRIGGER runs_immutable AFTER UPDATE ON bb.runs
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update('quarantined_at', 'quarantine_reason');
CREATE TRIGGER runs_no_truncate BEFORE TRUNCATE ON bb.runs
    FOR EACH STATEMENT EXECUTE FUNCTION bb.forbid_truncate();

-- One artefact bundle per run (gzip NDJSON, one gzip member per page; OSS_TECHNOLOGY_EVALUATION §4).
CREATE TABLE bb.run_artifacts (
    id                  uuid PRIMARY KEY DEFAULT uuidv7(),
    run_id              uuid NOT NULL REFERENCES bb.runs (id) ON DELETE CASCADE,
    kind                text NOT NULL CHECK (kind IN ('bundle')),
    format_version      integer NOT NULL CHECK (format_version >= 1),
    storage_backend     text NOT NULL CHECK (storage_backend IN ('fs', 'vercel_blob')),
    -- runs/<fingerprint hex>/<platform>/<run uuid>.bbz : unique per run row by construction.
    storage_key         text NOT NULL CHECK (storage_key ~ '^runs/[0-9a-f]{64}/[a-z0-9_-]+/[0-9a-f-]{36}\.bbz$'),
    sha256              bytea NOT NULL CHECK (octet_length(sha256) = 32),
    size_bytes          bigint NOT NULL CHECK (size_bytes > 0),
    uncompressed_bytes  bigint NOT NULL CHECK (uncompressed_bytes > 0),
    -- Page index: per section (tape, fills, equity, orders) the list of
    -- {first_row, rows, offset, len, sha256}; range reads verify each page against it.
    page_index          jsonb NOT NULL CHECK (jsonb_typeof(page_index) = 'object'),
    page_index_sha256   bytea NOT NULL CHECK (octet_length(page_index_sha256) = 32),
    created_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (run_id, kind),
    UNIQUE (storage_backend, storage_key)
);
CREATE TRIGGER run_artifacts_immutable AFTER UPDATE ON bb.run_artifacts
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update();
CREATE TRIGGER run_artifacts_no_truncate BEFORE TRUNCATE ON bb.run_artifacts
    FOR EACH STATEMENT EXECUTE FUNCTION bb.forbid_truncate();

-- Object deletion outbox and upload intent log. A row is inserted (own transaction) *before* an
-- upload with not_before = now() + 1 hour; the completion transaction deletes it. A row that
-- survives (failed or abandoned commit) is an orphan the GC deletes after not_before. Deleting
-- a run_artifacts row enqueues its object immediately (trigger below). Deletes are free on
-- Vercel Blob; `list` is never needed.
CREATE TABLE bb.blob_gc (
    storage_backend  text NOT NULL CHECK (storage_backend IN ('fs', 'vercel_blob')),
    storage_key      text NOT NULL,
    reason           text NOT NULL CHECK (reason IN ('upload_intent', 'artifact_deleted')),
    not_before       timestamptz NOT NULL DEFAULT now(),
    attempts         integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error       text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (storage_backend, storage_key)
);
CREATE INDEX blob_gc_due ON bb.blob_gc (not_before);

CREATE FUNCTION bb.enqueue_blob_gc() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    INSERT INTO bb.blob_gc (storage_backend, storage_key, reason)
    VALUES (OLD.storage_backend, OLD.storage_key, 'artifact_deleted')
    ON CONFLICT (storage_backend, storage_key)
        DO UPDATE SET reason = 'artifact_deleted', not_before = now();
    RETURN NULL;
END $$;
CREATE TRIGGER run_artifacts_gc AFTER DELETE ON bb.run_artifacts
    FOR EACH ROW EXECUTE FUNCTION bb.enqueue_blob_gc();

-- Monthly usage counters for the Vercel Blob Hobby allowances (2,000 advanced ops, 10,000 simple
-- ops). Incremented by the app on every upload and every uncached GET (upper bound: CDN hits are
-- not observable). Admission refuses new runs before the allowance is reached.
CREATE TABLE bb.storage_usage_monthly (
    month            date NOT NULL CHECK (month = date_trunc('month', month)::date),
    storage_backend  text NOT NULL CHECK (storage_backend IN ('fs', 'vercel_blob')),
    uploads          integer NOT NULL DEFAULT 0 CHECK (uploads >= 0),
    reads            integer NOT NULL DEFAULT 0 CHECK (reads >= 0),
    bytes_uploaded   bigint NOT NULL DEFAULT 0 CHECK (bytes_uploaded >= 0),
    PRIMARY KEY (month, storage_backend)
);

-- ---------------------------------------------------------------------------------------------
-- Research: workspace-owned objects (mutable metadata over immutable science)
-- ---------------------------------------------------------------------------------------------

CREATE TABLE bb.sweeps (
    id                   uuid PRIMARY KEY DEFAULT uuidv7(),
    workspace_id         uuid NOT NULL REFERENCES bb.workspaces (id) ON DELETE CASCADE,
    spec                 jsonb NOT NULL CHECK (jsonb_typeof(spec) = 'object'),     -- GridSpec
    spec_fingerprint     bytea NOT NULL CHECK (octet_length(spec_fingerprint) = 32),
    x_axis               text GENERATED ALWAYS AS (spec #>> '{x,type}') STORED,
    y_axis               text GENERATED ALWAYS AS (spec #>> '{y,type}') STORED,
    x_len                smallint NOT NULL CHECK (x_len BETWEEN 1 AND 100),
    y_len                smallint NOT NULL CHECK (y_len BETWEEN 1 AND 100),
    label                text NOT NULL DEFAULT '' CHECK (length(label) <= 200),
    created_by_key_id    uuid REFERENCES bb.api_keys (id) ON DELETE SET NULL,
    cancel_requested_at  timestamptz,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now(),
    version              bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    CHECK (x_len * y_len <= 100),                       -- bb-core MAX_GRID_CELLS
    UNIQUE (workspace_id, spec_fingerprint)             -- idempotent sweep submission
);
CREATE INDEX sweeps_workspace_created ON bb.sweeps (workspace_id, created_at DESC, id DESC);
CREATE INDEX sweeps_created_by_key ON bb.sweeps (created_by_key_id, created_at);
CREATE TRIGGER sweeps_version BEFORE UPDATE ON bb.sweeps
    FOR EACH ROW EXECUTE FUNCTION bb.bump_version();

-- ---------------------------------------------------------------------------------------------
-- Operations: job queue
-- ---------------------------------------------------------------------------------------------

-- Legal job state transitions. The trigger below rejects anything else (SQLSTATE BB002).
-- `actors` documents who may take the edge; the app enforces actors, the DB enforces edges.
CREATE TABLE bb.job_transitions (
    from_state  text NOT NULL,
    to_state    text NOT NULL,
    actors      text[] NOT NULL,
    meaning     text NOT NULL,
    PRIMARY KEY (from_state, to_state)
);
INSERT INTO bb.job_transitions (from_state, to_state, actors, meaning) VALUES
    ('created',    'queued',     '{api}',           'prerequisites satisfied (e.g. dataset version materialised)'),
    ('created',    'cancelled',  '{api}',           'cancel before queueing: immediate'),
    ('created',    'expired',    '{reaper}',        'queue TTL passed'),
    ('queued',     'running',    '{worker}',        'claim: lease taken'),
    ('queued',     'cancelled',  '{api}',           'cancel while queued: immediate'),
    ('queued',     'expired',    '{reaper}',        'queue TTL passed'),
    ('running',    'completed',  '{worker}',        'lease holder committed run + artefact'),
    ('running',    'failed',     '{worker,reaper}', 'worker: deterministic failure / budget; reaper: lease expired with attempts exhausted'),
    ('running',    'queued',     '{worker,reaper}', 'worker: graceful release on drain; reaper: lease expired, attempts remain'),
    ('running',    'cancelling', '{api}',           'cancel requested; worker stops at next checkpoint'),
    ('running',    'expired',    '{worker}',        'per-attempt deadline passed at a checkpoint'),
    ('cancelling', 'cancelled',  '{worker,reaper}', 'worker observed cancel at a checkpoint; or lease expired while cancelling');

CREATE TABLE bb.jobs (
    id                         uuid PRIMARY KEY DEFAULT uuidv7(),
    kind                       text NOT NULL DEFAULT 'run' CHECK (kind IN ('run')),
    experiment_config_id       uuid NOT NULL REFERENCES bb.experiment_configs (id) ON DELETE RESTRICT,
    platform                   text NOT NULL CHECK (platform IN ('linux-x86_64', 'linux-aarch64',
                                                                 'windows-x86_64', 'macos-x86_64',
                                                                 'macos-aarch64')),
    state                      text NOT NULL DEFAULT 'queued'
                               CHECK (state IN ('created', 'queued', 'running', 'cancelling',
                                                'completed', 'failed', 'cancelled', 'expired')),
    -- Higher first. Interactive runs 100, sweep cells 50, demo seeding 10.
    priority                   smallint NOT NULL DEFAULT 100 CHECK (priority BETWEEN 0 AND 1000),
    submitted_by_workspace_id  uuid REFERENCES bb.workspaces (id) ON DELETE SET NULL,
    submitted_by_key_id        uuid REFERENCES bb.api_keys (id) ON DELETE SET NULL,
    run_after                  timestamptz NOT NULL DEFAULT now(),   -- backoff after a lease loss
    expires_at                 timestamptz NOT NULL DEFAULT now() + interval '24 hours',
    deadline_ms                integer NOT NULL DEFAULT 600000 CHECK (deadline_ms > 0),
    claim_count                smallint NOT NULL DEFAULT 0 CHECK (claim_count >= 0),
    lease_losses               smallint NOT NULL DEFAULT 0 CHECK (lease_losses >= 0),
    max_attempts               smallint NOT NULL DEFAULT 3 CHECK (max_attempts >= 1),
    lease_owner                text,
    lease_expires_at           timestamptz,
    heartbeat_at               timestamptz,
    ticks_done                 bigint NOT NULL DEFAULT 0,
    ticks_total                bigint NOT NULL CHECK (ticks_total >= 0),
    tape_events                bigint NOT NULL DEFAULT 0 CHECK (tape_events >= 0),
    cancel_requested_at        timestamptz,
    error_category             text CHECK (error_category IN ('invalid_config', 'budget_exceeded',
                                   'simulation_failed', 'lease_expired', 'integrity_failed',
                                   'storage_failed', 'internal')),
    error_code                 text,
    error_message              text,
    expired_reason             text,
    run_id                     uuid REFERENCES bb.runs (id) ON DELETE RESTRICT,
    created_at                 timestamptz NOT NULL DEFAULT now(),
    queued_at                  timestamptz,
    started_at                 timestamptz,
    finished_at                timestamptz,
    updated_at                 timestamptz NOT NULL DEFAULT now(),
    version                    bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    CHECK (ticks_done BETWEEN 0 AND ticks_total),
    CHECK (lease_losses <= max_attempts),
    -- Only running/cancelling jobs hold a lease.
    CHECK ((state IN ('running', 'cancelling')) = (lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)),
    CHECK (state <> 'queued' OR queued_at IS NOT NULL),
    CHECK ((state = 'completed') = (run_id IS NOT NULL)),
    CHECK ((state = 'failed') = (error_category IS NOT NULL AND error_code IS NOT NULL)),
    CHECK ((state = 'expired') = (expired_reason IS NOT NULL)),
    CHECK (state <> 'cancelling' OR cancel_requested_at IS NOT NULL),
    CHECK ((state IN ('completed', 'failed', 'cancelled', 'expired')) = (finished_at IS NOT NULL))
);
-- Single-flight: at most one live job per (experiment, platform). INSERT ... ON CONFLICT targets it.
CREATE UNIQUE INDEX jobs_single_flight ON bb.jobs (experiment_config_id, platform)
    WHERE state IN ('created', 'queued', 'running', 'cancelling');
-- Claim order and queue position. A worker claims only jobs of its own platform class, so a
-- Windows development worker can never produce a result recorded as linux-x86_64.
CREATE INDEX jobs_claim ON bb.jobs (platform, priority DESC, queued_at, id) WHERE state = 'queued';
-- Lease reaper.
CREATE INDEX jobs_lease ON bb.jobs (lease_expires_at) WHERE state IN ('running', 'cancelling');
-- Queue TTL reaper.
CREATE INDEX jobs_ttl ON bb.jobs (expires_at) WHERE state IN ('created', 'queued');
-- Retention of terminal jobs.
CREATE INDEX jobs_finished ON bb.jobs (finished_at) WHERE finished_at IS NOT NULL;
CREATE INDEX jobs_run ON bb.jobs (run_id) WHERE run_id IS NOT NULL;
CREATE INDEX jobs_submitted_by_key ON bb.jobs (submitted_by_key_id);
CREATE INDEX jobs_submitted_by_workspace ON bb.jobs (submitted_by_workspace_id);

CREATE FUNCTION bb.check_job_transition() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.state NOT IN ('created', 'queued') THEN
            RAISE EXCEPTION 'bb.jobs: a job must be inserted as created or queued, not %', NEW.state
                USING ERRCODE = 'BB002';
        END IF;
    ELSIF NOT EXISTS (SELECT 1 FROM bb.job_transitions t
                      WHERE t.from_state = OLD.state AND t.to_state = NEW.state) THEN
        RAISE EXCEPTION 'bb.jobs: illegal state transition % -> % (job %)', OLD.state, NEW.state, OLD.id
            USING ERRCODE = 'BB002';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER jobs_transition_insert BEFORE INSERT ON bb.jobs
    FOR EACH ROW EXECUTE FUNCTION bb.check_job_transition();
CREATE TRIGGER jobs_transition_update BEFORE UPDATE OF state ON bb.jobs
    FOR EACH ROW WHEN (OLD.state IS DISTINCT FROM NEW.state) EXECUTE FUNCTION bb.check_job_transition();
CREATE TRIGGER jobs_version BEFORE UPDATE ON bb.jobs
    FOR EACH ROW EXECUTE FUNCTION bb.bump_version();

CREATE TABLE bb.job_attempts (
    job_id          uuid NOT NULL REFERENCES bb.jobs (id) ON DELETE CASCADE,
    attempt_no      smallint NOT NULL CHECK (attempt_no >= 1),      -- = jobs.claim_count at claim
    worker_id       text NOT NULL,
    claimed_at      timestamptz NOT NULL DEFAULT now(),
    ended_at        timestamptz,
    outcome         text CHECK (outcome IN ('completed', 'failed', 'cancelled', 'released',
                                            'lease_expired', 'expired')),
    error_code      text,
    peak_rss_bytes  bigint CHECK (peak_rss_bytes >= 0),
    PRIMARY KEY (job_id, attempt_no),
    CHECK ((ended_at IS NULL) = (outcome IS NULL)),
    CHECK (ended_at IS NULL OR ended_at >= claimed_at)
);

-- The workspace's view of an experiment: label, sweep membership, pinning, current job.
-- A sweep cell IS an experiment row with sweep coordinates (see bb.sweep_cells view).
CREATE TABLE bb.experiments (
    id                    uuid PRIMARY KEY DEFAULT uuidv7(),
    workspace_id          uuid NOT NULL REFERENCES bb.workspaces (id) ON DELETE CASCADE,
    experiment_config_id  uuid NOT NULL,
    dataset_id            text NOT NULL,
    strategy_kind         text NOT NULL,
    -- Set by the API to the deployment's result platform (hosted: linux-x86_64); no default.
    platform              text NOT NULL
                          CHECK (platform IN ('linux-x86_64', 'linux-aarch64', 'windows-x86_64',
                                              'macos-x86_64', 'macos-aarch64')),
    label                 text NOT NULL DEFAULT '' CHECK (length(label) <= 200),
    sweep_id              uuid REFERENCES bb.sweeps (id) ON DELETE CASCADE,
    sweep_x               smallint CHECK (sweep_x >= 0),
    sweep_y               smallint CHECK (sweep_y >= 0),
    job_id                uuid REFERENCES bb.jobs (id) ON DELETE SET NULL,
    created_by_key_id     uuid REFERENCES bb.api_keys (id) ON DELETE SET NULL,
    pinned                boolean NOT NULL DEFAULT false,      -- exempt from retention (demo)
    cancel_requested_at   timestamptz,
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now(),
    version               bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    -- The composite FK guarantees the denormalised filter columns equal the config's.
    FOREIGN KEY (experiment_config_id, dataset_id, strategy_kind)
        REFERENCES bb.experiment_configs (id, dataset_id, strategy_kind) ON DELETE RESTRICT,
    CHECK ((sweep_id IS NULL) = (sweep_x IS NULL) AND (sweep_id IS NULL) = (sweep_y IS NULL)),
    -- Idempotent submission per workspace; NULLS NOT DISTINCT makes standalone duplicates collide.
    CONSTRAINT experiments_idempotency
        UNIQUE NULLS NOT DISTINCT (workspace_id, experiment_config_id, platform, sweep_id),
    UNIQUE (sweep_id, sweep_x, sweep_y)
);
CREATE INDEX experiments_workspace_created ON bb.experiments (workspace_id, created_at DESC, id DESC);
CREATE INDEX experiments_workspace_dataset_created
    ON bb.experiments (workspace_id, dataset_id, created_at DESC, id DESC);
CREATE INDEX experiments_workspace_job ON bb.experiments (workspace_id, job_id) WHERE job_id IS NOT NULL;
CREATE INDEX experiments_job ON bb.experiments (job_id) WHERE job_id IS NOT NULL;
CREATE INDEX experiments_config ON bb.experiments (experiment_config_id);
CREATE INDEX experiments_created_by_key ON bb.experiments (created_by_key_id, created_at);
CREATE TRIGGER experiments_version BEFORE UPDATE ON bb.experiments
    FOR EACH ROW EXECUTE FUNCTION bb.bump_version();

CREATE VIEW bb.sweep_cells AS
    SELECT sweep_id, sweep_x AS x_index, sweep_y AS y_index, id AS experiment_id,
           experiment_config_id, job_id
    FROM bb.experiments
    WHERE sweep_id IS NOT NULL;

CREATE TABLE bb.comparisons (
    id                  uuid PRIMARY KEY DEFAULT uuidv7(),
    workspace_id        uuid NOT NULL REFERENCES bb.workspaces (id) ON DELETE CASCADE,
    run_a_id            uuid NOT NULL REFERENCES bb.runs (id) ON DELETE RESTRICT,
    run_b_id            uuid NOT NULL REFERENCES bb.runs (id) ON DELETE RESTRICT,
    comparator_version  integer NOT NULL CHECK (comparator_version >= 1),
    -- 'aligned': per-order alignment is valid. 'metrics_only': alignment refused; `reasons`
    -- names every differing eligibility field (BASELINE_AUDIT AT-1).
    eligibility         text NOT NULL CHECK (eligibility IN ('aligned', 'metrics_only')),
    reasons             jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(reasons) = 'array'),
    summary             jsonb NOT NULL CHECK (jsonb_typeof(summary) = 'object'),
    summary_sha256      bytea NOT NULL CHECK (octet_length(summary_sha256) = 32),
    label               text NOT NULL DEFAULT '' CHECK (length(label) <= 200),
    created_by_key_id   uuid REFERENCES bb.api_keys (id) ON DELETE SET NULL,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    version             bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    CHECK (run_a_id <> run_b_id),
    CHECK ((eligibility = 'aligned') = (jsonb_array_length(reasons) = 0)),
    UNIQUE (workspace_id, run_a_id, run_b_id, comparator_version)
);
CREATE INDEX comparisons_workspace_created ON bb.comparisons (workspace_id, created_at DESC, id DESC);
CREATE INDEX comparisons_run_b ON bb.comparisons (run_b_id);
CREATE INDEX comparisons_run_a ON bb.comparisons (run_a_id);
CREATE TRIGGER comparisons_version BEFORE UPDATE ON bb.comparisons
    FOR EACH ROW EXECUTE FUNCTION bb.bump_version();
CREATE TRIGGER comparisons_immutable AFTER UPDATE ON bb.comparisons
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update('label', 'updated_at', 'version', 'created_by_key_id');

-- Saved, immutable report snapshots. Unsaved reports are generated on demand and not stored.
CREATE TABLE bb.research_reports (
    id                 uuid PRIMARY KEY DEFAULT uuidv7(),
    workspace_id       uuid NOT NULL REFERENCES bb.workspaces (id) ON DELETE CASCADE,
    kind               text NOT NULL CHECK (kind IN ('run', 'comparison', 'sweep')),
    run_id             uuid REFERENCES bb.runs (id) ON DELETE RESTRICT,
    comparison_id      uuid REFERENCES bb.comparisons (id) ON DELETE SET NULL,
    sweep_id           uuid REFERENCES bb.sweeps (id) ON DELETE SET NULL,
    title              text NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
    generator_version  integer NOT NULL CHECK (generator_version >= 1),
    format             text NOT NULL DEFAULT 'markdown' CHECK (format IN ('markdown')),
    body               text NOT NULL CHECK (octet_length(body) <= 1048576),
    body_sha256        bytea NOT NULL CHECK (octet_length(body_sha256) = 32),
    created_by_key_id  uuid REFERENCES bb.api_keys (id) ON DELETE SET NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    CHECK (num_nonnulls(run_id, comparison_id, sweep_id) <= 1),
    CHECK (kind <> 'run' OR run_id IS NOT NULL)
);
CREATE INDEX research_reports_workspace_created ON bb.research_reports (workspace_id, created_at DESC, id DESC);
CREATE INDEX research_reports_run ON bb.research_reports (run_id) WHERE run_id IS NOT NULL;
CREATE INDEX research_reports_comparison ON bb.research_reports (comparison_id) WHERE comparison_id IS NOT NULL;
CREATE INDEX research_reports_sweep ON bb.research_reports (sweep_id) WHERE sweep_id IS NOT NULL;
CREATE TRIGGER research_reports_immutable AFTER UPDATE ON bb.research_reports
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update('comparison_id', 'sweep_id', 'created_by_key_id');

-- ---------------------------------------------------------------------------------------------
-- Operations: workers and system events
-- ---------------------------------------------------------------------------------------------

CREATE TABLE bb.worker_heartbeats (
    worker_id              text PRIMARY KEY CHECK (length(worker_id) BETWEEN 1 AND 128),
    hostname               text NOT NULL,
    pid                    integer NOT NULL CHECK (pid > 0),
    blackbox_version       text NOT NULL,
    platform               text NOT NULL,
    mode                   text NOT NULL CHECK (mode IN ('in_process', 'standalone')),
    slots                  smallint NOT NULL CHECK (slots >= 1),
    state                  text NOT NULL CHECK (state IN ('starting', 'idle', 'busy', 'draining', 'stopped')),
    current_job_id         uuid REFERENCES bb.jobs (id) ON DELETE SET NULL,
    heartbeat_interval_ms  integer NOT NULL CHECK (heartbeat_interval_ms > 0),
    started_at             timestamptz NOT NULL DEFAULT now(),
    last_heartbeat_at      timestamptz NOT NULL DEFAULT now(),
    stopped_at             timestamptz,
    CHECK ((state = 'stopped') = (stopped_at IS NOT NULL)),
    CHECK (state = 'busy' OR current_job_id IS NULL)
);
CREATE INDEX worker_heartbeats_last ON bb.worker_heartbeats (last_heartbeat_at DESC);
CREATE INDEX worker_heartbeats_job ON bb.worker_heartbeats (current_job_id) WHERE current_job_id IS NOT NULL;

-- Append-only machine events (lease reclaimed, integrity failure, nondeterminism detected, ...).
CREATE TABLE bb.system_events (
    id         bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at         timestamptz NOT NULL DEFAULT now(),
    level      text NOT NULL CHECK (level IN ('debug', 'info', 'warn', 'error')),
    kind       text NOT NULL CHECK (kind ~ '^[a-z_]+$'),
    worker_id  text,
    job_id     uuid,
    run_id     uuid,
    message    text NOT NULL,
    detail     jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(detail) = 'object')
);
CREATE INDEX system_events_at ON bb.system_events (at DESC);
CREATE INDEX system_events_kind_at ON bb.system_events (kind, at DESC);
CREATE TRIGGER system_events_immutable AFTER UPDATE ON bb.system_events
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update();

-- ---------------------------------------------------------------------------------------------
-- Reference rows the code relies on
-- ---------------------------------------------------------------------------------------------

INSERT INTO bb.workspaces (slug, name, kind, is_public_read, quota_experiments_per_day,
                           quota_sweeps_per_day, quota_active_jobs)
VALUES ('system', 'System', 'system', false, 0, 0, 0),
       ('demo', 'Public demo', 'demo', true, 0, 0, 0);

INSERT INTO bb.data_sources (id, vendor, licence, licence_url, redistribution_allowed, hostable, notes)
VALUES ('blackbox-synthetic', 'BlackBox HFT synthetic generator', 'MIT',
        'https://github.com/tanmay-alpha/BlackBox-HFT/blob/main/LICENSE', true, true,
        'Generated by bb-data synthetic.rs; deterministic from the manifest generator parameters.');

COMMIT;

-- Down migration (0001_initial.down.sql):
-- DROP SCHEMA bb CASCADE;
