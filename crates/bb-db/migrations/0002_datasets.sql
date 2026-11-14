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

