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

