-- ---------------------------------------------------------------------------------------------
-- Row digests of served summaries (FINAL_AUDIT §4 item 5)
-- ---------------------------------------------------------------------------------------------

-- DB-only list paths (runs, experiments, comparisons) serve stored summaries without opening
-- artefacts. Each such row carries a digest of exactly the stored fields it serves, written once
-- when the row is inserted (the run at job completion, the comparison at creation) and checked on
-- every read by recomputing it from the row. The digest is computed by Postgres over its own
-- canonical jsonb text, so a value that jsonb normalises (e.g. -0.0 stored as 0) is hashed in its
-- stored form and never produces a false mismatch. Timestamps enter as integer microseconds so
-- the session time zone cannot change the text.
CREATE FUNCTION bb.run_summary_digest(r bb.runs) RETURNS bytea
LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog AS $$
    SELECT sha256(convert_to(jsonb_build_object(
        'experiment_config_id', r.experiment_config_id,
        'platform', r.platform,
        'is_canonical', r.is_canonical,
        'blackbox_version', r.blackbox_version,
        'build_profile', r.build_profile,
        'result_sha256', encode(r.result_sha256, 'hex'),
        'summary_sha256', encode(r.summary_sha256, 'hex'),
        'counts', jsonb_build_array(r.ticks, r.order_count, r.fill_count, r.tape_len, r.equity_len),
        'wall_time_ms', r.wall_time_ms,
        'peak_rss_bytes', r.peak_rss_bytes,
        'metrics', r.metrics,
        'engine_check', r.engine_check,
        'created_at_us', floor(extract(epoch FROM r.created_at) * 1000000)::bigint
    )::text, 'UTF8'))
$$;

CREATE FUNCTION bb.comparison_summary_digest(c bb.comparisons) RETURNS bytea
LANGUAGE sql IMMUTABLE PARALLEL SAFE SET search_path = pg_catalog AS $$
    SELECT sha256(convert_to(jsonb_build_object(
        'workspace_id', c.workspace_id,
        'run_a_id', c.run_a_id,
        'run_b_id', c.run_b_id,
        'comparator_version', c.comparator_version,
        'eligibility', c.eligibility,
        'reasons', c.reasons,
        'summary', c.summary,
        'summary_sha256', encode(c.summary_sha256, 'hex'),
        'created_at_us', floor(extract(epoch FROM c.created_at) * 1000000)::bigint
    )::text, 'UTF8'))
$$;

CREATE FUNCTION bb.set_run_summary_digest() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    NEW.summary_row_sha256 := bb.run_summary_digest(NEW);
    RETURN NEW;
END $$;

CREATE FUNCTION bb.set_comparison_summary_digest() RETURNS trigger
LANGUAGE plpgsql SET search_path = pg_catalog AS $$
BEGIN
    NEW.summary_row_sha256 := bb.comparison_summary_digest(NEW);
    RETURN NEW;
END $$;

-- Existing rows are trusted as they are now (they were verified on detail reads until now).
ALTER TABLE bb.runs ADD COLUMN summary_row_sha256 bytea
    CHECK (octet_length(summary_row_sha256) = 32);
ALTER TABLE bb.runs DISABLE TRIGGER runs_immutable;
UPDATE bb.runs r SET summary_row_sha256 = bb.run_summary_digest(r);
ALTER TABLE bb.runs ENABLE TRIGGER runs_immutable;
ALTER TABLE bb.runs ALTER COLUMN summary_row_sha256 SET NOT NULL;
CREATE TRIGGER runs_summary_row_digest BEFORE INSERT ON bb.runs
    FOR EACH ROW EXECUTE FUNCTION bb.set_run_summary_digest();

ALTER TABLE bb.comparisons ADD COLUMN summary_row_sha256 bytea
    CHECK (octet_length(summary_row_sha256) = 32);
ALTER TABLE bb.comparisons DISABLE TRIGGER comparisons_immutable;
ALTER TABLE bb.comparisons DISABLE TRIGGER comparisons_version;
UPDATE bb.comparisons c SET summary_row_sha256 = bb.comparison_summary_digest(c);
ALTER TABLE bb.comparisons ENABLE TRIGGER comparisons_version;
ALTER TABLE bb.comparisons ENABLE TRIGGER comparisons_immutable;
ALTER TABLE bb.comparisons ALTER COLUMN summary_row_sha256 SET NOT NULL;
CREATE TRIGGER comparisons_summary_row_digest BEFORE INSERT ON bb.comparisons
    FOR EACH ROW EXECUTE FUNCTION bb.set_comparison_summary_digest();

-- ---------------------------------------------------------------------------------------------
-- Hosted quotas and the per-workspace Blob upload share (FINAL_AUDIT §4 item 7, SEC-8/SEC-11)
-- ---------------------------------------------------------------------------------------------

-- At most 5 live admission units per workspace: each single experiment's job is one unit and a
-- sweep is one unit however many cells it has (bb-db `submit`). Workspaces still on the old
-- default of 50 move to 5; deliberately raised or lowered values stay.
ALTER TABLE bb.workspaces ALTER COLUMN quota_active_jobs SET DEFAULT 5;
UPDATE bb.workspaces SET quota_active_jobs = 5 WHERE quota_active_jobs = 50;

-- Each workspace may spend this many Vercel Blob upload attempts per month, next to the global
-- monthly allowance. `reserve_upload` charges one attempt (retries included) before every PUT,
-- and admission refuses new runs of a workspace whose share is spent.
ALTER TABLE bb.workspaces ADD COLUMN quota_uploads_per_month integer NOT NULL DEFAULT 300
    CHECK (quota_uploads_per_month >= 0);
CREATE TABLE bb.workspace_uploads_monthly (
    workspace_id     uuid NOT NULL REFERENCES bb.workspaces (id) ON DELETE CASCADE,
    month            date NOT NULL CHECK (month = date_trunc('month', month)::date),
    storage_backend  text NOT NULL CHECK (storage_backend IN ('fs', 'vercel_blob')),
    attempts         integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    PRIMARY KEY (workspace_id, month, storage_backend)
);
