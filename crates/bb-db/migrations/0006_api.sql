-- ---------------------------------------------------------------------------------------------
-- /api/v1 integration (additive): platform classes, run and dataset analytics, read indexes
-- ---------------------------------------------------------------------------------------------

-- Platform classes come from bb-core `platform_class()`: `<os>-<arch>` plus a target-env suffix
-- for non-default C runtimes (`windows-x86_64-gnu`, `linux-x86_64-musl`, ...). The fixed list of
-- 0003/0004 rejected every Windows development build, so the constraint becomes a shape check.
-- `runs.is_canonical` (platform = 'linux-x86_64') is unchanged.
ALTER TABLE bb.runs DROP CONSTRAINT runs_platform_check;
ALTER TABLE bb.runs ADD CONSTRAINT runs_platform_check
    CHECK (platform ~ '^[a-z0-9]+-[a-z0-9_]+(-[a-z0-9_]+)*$' AND length(platform) <= 64);
ALTER TABLE bb.jobs DROP CONSTRAINT jobs_platform_check;
ALTER TABLE bb.jobs ADD CONSTRAINT jobs_platform_check
    CHECK (platform ~ '^[a-z0-9]+-[a-z0-9_]+(-[a-z0-9_]+)*$' AND length(platform) <= 64);
ALTER TABLE bb.experiments DROP CONSTRAINT experiments_platform_check;
ALTER TABLE bb.experiments ADD CONSTRAINT experiments_platform_check
    CHECK (platform ~ '^[a-z0-9]+-[a-z0-9_]+(-[a-z0-9_]+)*$' AND length(platform) <= 64);

-- Execution (E1-E10) and sampled-risk analytics of one run (bb-analytics), computed by the worker
-- at completion and also embedded in the bundle trailer. `report_json` keeps the exact bytes that
-- `report_sha256` covers (jsonb would renormalise numbers such as -0.0 or 1e17).
CREATE TABLE bb.run_analytics (
    run_id             uuid PRIMARY KEY REFERENCES bb.runs (id) ON DELETE CASCADE,
    analytics_version  text NOT NULL CHECK (length(analytics_version) BETWEEN 1 AND 32),
    report_json        text NOT NULL CHECK (length(report_json) BETWEEN 2 AND 16777216),
    report_sha256      bytea NOT NULL CHECK (octet_length(report_sha256) = 32),
    created_at         timestamptz NOT NULL DEFAULT now()
);
CREATE TRIGGER run_analytics_immutable AFTER UPDATE ON bb.run_analytics
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update();

-- Market microstructure analytics (M1-M10) of one dataset version, computed once per
-- (version, analytics version) on first request or at `demo seed`.
CREATE TABLE bb.dataset_analytics (
    dataset_version_id  uuid NOT NULL REFERENCES bb.dataset_versions (id) ON DELETE CASCADE,
    analytics_version   text NOT NULL CHECK (length(analytics_version) BETWEEN 1 AND 32),
    report_json         text NOT NULL CHECK (length(report_json) BETWEEN 2 AND 16777216),
    report_sha256       bytea NOT NULL CHECK (octet_length(report_sha256) = 32),
    created_at          timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (dataset_version_id, analytics_version)
);
CREATE TRIGGER dataset_analytics_immutable AFTER UPDATE ON bb.dataset_analytics
    FOR EACH ROW EXECUTE FUNCTION bb.forbid_update();

-- Read paths of /api/v1 (api_reads.rs).
CREATE INDEX IF NOT EXISTS runs_created ON bb.runs (created_at DESC, id DESC) WHERE quarantined_at IS NULL;
CREATE INDEX IF NOT EXISTS runs_net_pnl ON bb.runs (net_pnl, id) WHERE quarantined_at IS NULL;
CREATE INDEX IF NOT EXISTS runs_max_drawdown ON bb.runs (max_drawdown, id) WHERE quarantined_at IS NULL;
CREATE INDEX IF NOT EXISTS system_events_lease_reclaimed ON bb.system_events (at) WHERE kind = 'lease_reclaimed';

-- Blob read budget (SECURITY_REVIEW §6.3, SEC-11): `charge_storage_read` reserves one simple op and
-- the expected bytes before every GET/HEAD attempt and refuses (fail closed) at the monthly caps or
-- the daily pacing share. The daily rows make the pacing restart-proof.
ALTER TABLE bb.storage_usage_monthly
    ADD COLUMN read_bytes bigint NOT NULL DEFAULT 0 CHECK (read_bytes >= 0);
CREATE TABLE bb.storage_reads_daily (
    day              date NOT NULL,
    storage_backend  text NOT NULL CHECK (storage_backend IN ('fs', 'vercel_blob')),
    reads            integer NOT NULL DEFAULT 0 CHECK (reads >= 0),
    read_bytes       bigint NOT NULL DEFAULT 0 CHECK (read_bytes >= 0),
    PRIMARY KEY (day, storage_backend)
);

-- Render egress budget per UTC day and caller class (SECURITY_REVIEW §6.2, SEC-12), flushed by the
-- app every few MB so a spin-down cannot reset it.
CREATE TABLE bb.egress_usage_daily (
    day    date NOT NULL,
    class  text NOT NULL CHECK (class IN ('anonymous', 'keyed')),
    bytes  bigint NOT NULL DEFAULT 0 CHECK (bytes >= 0),
    PRIMARY KEY (day, class)
);

-- Fair claim (SEC-8) ranks queued jobs by their workspace's live jobs and latest claim.
CREATE INDEX IF NOT EXISTS jobs_live_by_workspace ON bb.jobs (submitted_by_workspace_id)
    WHERE state IN ('running', 'cancelling');
