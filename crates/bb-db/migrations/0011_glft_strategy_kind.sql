-- `glft` (StrategyKind::Glft, preset `glft_baseline`) was added to the Rust enum after 0003 and never
-- to the generated column's CHECK, so every GLFT submission failed the insert with HTTP 500.
-- Widening an IN list cannot invalidate an existing row; code built before this migration keeps
-- working against the wider constraint (rollback by redeploy needs no down-migration).
ALTER TABLE bb.experiment_configs
    DROP CONSTRAINT experiment_configs_strategy_kind_check,
    ADD CONSTRAINT experiment_configs_strategy_kind_check
        CHECK (strategy_kind IN ('touch_maker', 'avellaneda_stoikov', 'glft',
                                 'imbalance_taker', 'scripted'));
