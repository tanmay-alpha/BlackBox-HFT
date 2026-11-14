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

