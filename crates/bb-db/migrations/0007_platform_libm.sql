-- ---------------------------------------------------------------------------------------------
-- Libm-fingerprinted platform classes
-- ---------------------------------------------------------------------------------------------

-- bb-core `platform_class()` is `<os>-<arch>[-<env>]+libm-<8 hex>`: whether two builds produce
-- bit-identical results depends on the math library, not on the operating system. Classes
-- recorded before fingerprints existed (no `+libm-` suffix) stay valid for reading old rows.
ALTER TABLE bb.runs DROP CONSTRAINT runs_platform_check;
ALTER TABLE bb.runs ADD CONSTRAINT runs_platform_check
    CHECK (platform ~ '^[a-z0-9]+-[a-z0-9_]+(-[a-z0-9_]+)*(\+libm-[0-9a-f]{8})?$'
           AND length(platform) <= 64);
ALTER TABLE bb.jobs DROP CONSTRAINT jobs_platform_check;
ALTER TABLE bb.jobs ADD CONSTRAINT jobs_platform_check
    CHECK (platform ~ '^[a-z0-9]+-[a-z0-9_]+(-[a-z0-9_]+)*(\+libm-[0-9a-f]{8})?$'
           AND length(platform) <= 64);
ALTER TABLE bb.experiments DROP CONSTRAINT experiments_platform_check;
ALTER TABLE bb.experiments ADD CONSTRAINT experiments_platform_check
    CHECK (platform ~ '^[a-z0-9]+-[a-z0-9_]+(-[a-z0-9_]+)*(\+libm-[0-9a-f]{8})?$'
           AND length(platform) <= 64);

-- Canonical is a property of the math library (bb-core `platform::is_canonical`), which SQL
-- cannot evaluate from a constant list of operating systems, so the generated column becomes a
-- plain column written by the application at insert. Existing rows are backfilled with the same
-- rule in SQL: the platform ends with `+libm-` and bb-core's CANONICAL_LIBM (53154a4e). Legacy
-- classes without a fingerprint are not canonical.
ALTER TABLE bb.runs ALTER COLUMN is_canonical DROP EXPRESSION;
ALTER TABLE bb.runs DISABLE TRIGGER runs_immutable;
UPDATE bb.runs SET is_canonical = (platform LIKE '%+libm-53154a4e');
ALTER TABLE bb.runs ENABLE TRIGGER runs_immutable;
ALTER TABLE bb.runs ALTER COLUMN is_canonical SET NOT NULL;

-- Storage keys carry a key-safe platform segment (bb-core `platform::storage_segment`: `+`
-- becomes `.`), e.g. runs/<fp>/linux-x86_64.libm-53154a4e/<uuid>.bbz.
ALTER TABLE bb.run_artifacts DROP CONSTRAINT run_artifacts_storage_key_check;
ALTER TABLE bb.run_artifacts ADD CONSTRAINT run_artifacts_storage_key_check
    CHECK (storage_key ~ '^runs/[0-9a-f]{64}/[a-z0-9_-]+(\.libm-[0-9a-f]{8})?/[0-9a-f-]{36}\.bbz$');
