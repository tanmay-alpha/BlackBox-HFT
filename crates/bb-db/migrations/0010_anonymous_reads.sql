-- Anonymous share of the Blob read budget (SECURITY_REVIEW §6.3, SEC-11): reads caused by
-- anonymous requests may take at most half of the daily pacing share. The counter must survive
-- restarts (a spin-down would reset an in-process count), and `storage_backend` cannot carry the
-- caller class (its CHECK allows only the backend names), so the daily row gets a column.
-- Additive: existing rows read as 0 anonymous reads for their day.
ALTER TABLE bb.storage_reads_daily
    ADD COLUMN anonymous_reads integer NOT NULL DEFAULT 0
        CHECK (anonymous_reads >= 0 AND anonymous_reads <= reads);
