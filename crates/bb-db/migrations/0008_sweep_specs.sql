-- ---------------------------------------------------------------------------------------------
-- Sweeps as bb-core `SweepSpec`: grid, random and seed-ensemble designs with an optional
-- in-sample / out-of-sample partition
-- ---------------------------------------------------------------------------------------------

-- A sweep run is keyed by (sweep, cell index, segment): a partitioned cell runs twice (in-sample
-- and out-of-sample, each on a derived slice dataset) and a random or seed design has no 2-D
-- coordinates, so the GridSpec-only (sweep_x, sweep_y) key goes. Legacy GridSpec sweeps become
-- grid SweepSpecs with the default objective (net_pnl, maximize); their cells keep the row-major
-- index y * x_len + x (bb-core `SweepDesign::Grid`, first axis fastest) and the segment `full`.
DROP VIEW bb.sweep_cells;

ALTER TABLE bb.experiments
    ADD COLUMN sweep_cell smallint CHECK (sweep_cell BETWEEN 0 AND 99),   -- bb-core MAX_SWEEP_CELLS
    ADD COLUMN sweep_segment text CHECK (sweep_segment IN ('full', 'in_sample', 'out_of_sample'));
UPDATE bb.experiments e
    SET sweep_cell = e.sweep_y * s.x_len + e.sweep_x, sweep_segment = 'full'
    FROM bb.sweeps s
    WHERE s.id = e.sweep_id;
-- Drops the (sweep_id, sweep_x, sweep_y) UNIQUE and the coordinate CHECK with the columns.
ALTER TABLE bb.experiments DROP COLUMN sweep_x, DROP COLUMN sweep_y;
ALTER TABLE bb.experiments
    ADD CONSTRAINT experiments_sweep_membership
        CHECK ((sweep_id IS NULL) = (sweep_cell IS NULL)
               AND (sweep_id IS NULL) = (sweep_segment IS NULL)),
    ADD CONSTRAINT experiments_sweep_run UNIQUE (sweep_id, sweep_cell, sweep_segment);

CREATE VIEW bb.sweep_cells AS
    SELECT sweep_id, sweep_cell AS cell_index, sweep_segment AS segment, id AS experiment_id,
           experiment_config_id, job_id
    FROM bb.experiments
    WHERE sweep_id IS NOT NULL;

ALTER TABLE bb.sweeps
    ADD COLUMN cell_count smallint,
    ADD COLUMN run_count smallint;
UPDATE bb.sweeps
    SET cell_count = x_len * y_len,
        run_count = x_len * y_len,
        spec = jsonb_build_object(
            'base', spec -> 'base',
            'design', jsonb_build_object('type', 'grid',
                                         'axes', jsonb_build_array(spec -> 'x', spec -> 'y')),
            'objective', jsonb_build_object('metric', 'net_pnl', 'direction', 'maximize'))
    WHERE NOT spec ? 'design';
-- Drops the x_len * y_len CHECK with the columns.
ALTER TABLE bb.sweeps DROP COLUMN x_axis, DROP COLUMN y_axis, DROP COLUMN x_len, DROP COLUMN y_len;
ALTER TABLE bb.sweeps
    ALTER COLUMN cell_count SET NOT NULL,
    ALTER COLUMN run_count SET NOT NULL,
    ADD CONSTRAINT sweeps_cell_count CHECK (cell_count BETWEEN 1 AND 100),
    -- One run per cell, or an in-sample and an out-of-sample run per cell.
    ADD CONSTRAINT sweeps_run_count CHECK (run_count = cell_count OR run_count = 2 * cell_count),
    ADD COLUMN design text GENERATED ALWAYS AS (spec #>> '{design,type}') STORED;
COMMENT ON COLUMN bb.sweeps.spec IS 'bb-core SweepSpec (canonical JSON as admitted)';
