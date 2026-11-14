-- ---------------------------------------------------------------------------------------------
-- Research: scientific inputs and results (global, content-addressed, immutable)
-- ---------------------------------------------------------------------------------------------

-- The experiment identity: sha256(contract_version, simulation_revision, engine name, engine
-- version, events_sha256, config), as computed by bb-app `run_identity` today.
CREATE TABLE bb.experiment_configs (
    id                      uuid PRIMARY KEY DEFAULT uuidv7(),
    fingerprint             bytea NOT NULL UNIQUE CHECK (octet_length(fingerprint) = 32),
    -- Legacy 16-hex run id, kept as a lookup alias. UNIQUE turns a 64-bit prefix collision into a
    -- loud insert error instead of the silent overwrite FsRunStore allows (BASELINE_AUDIT Â§4.2).
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

-- One artefact bundle per run (gzip NDJSON, one gzip member per page; OSS_TECHNOLOGY_EVALUATION Â§4).
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

