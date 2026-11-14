use bb_core::{
    compare::GridAxis,
    rng::Rng,
    sweep::{Partition, SweepDesign},
};

use super::*;
use crate::testkit::{MS, config};

const S: i64 = 1_000_000_000;

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// A grid of `n` latencies, optionally partitioned 80 s / 40 s.
fn spec(n: i64, partitioned: bool, direction: Direction, cscv_blocks: Option<u32>) -> SweepSpec {
    SweepSpec {
        base: config(),
        design: SweepDesign::Grid {
            axes: vec![GridAxis::OrderLatencyNs {
                values: (1..=n).map(|i| i * MS).collect(),
            }],
        },
        partition: partitioned.then_some(Partition {
            in_sample: TimeWindow {
                start: 0,
                end: 80 * S,
            },
            out_of_sample: TimeWindow {
                start: 80 * S,
                end: 120 * S,
            },
        }),
        objective: Objective {
            metric: ObjectiveMetric::NetPnl,
            direction,
        },
        cscv_blocks,
    }
}

fn outcome(cell: u32, is: Option<f64>, oos: Option<f64>) -> SweepCellOutcome {
    SweepCellOutcome {
        cell,
        in_sample: is,
        out_of_sample: oos,
        blocks: None,
        error: None,
    }
}

/// Five cells; cell 1 and cell 2 tie in sample, cell 4 failed.
fn fixture() -> Vec<SweepCellOutcome> {
    vec![
        outcome(0, Some(1.0), Some(2.0)),
        outcome(1, Some(4.0), Some(-1.0)),
        outcome(2, Some(4.0), Some(3.0)),
        outcome(3, Some(2.0), Some(0.5)),
        SweepCellOutcome {
            error: Some("budget_exceeded".into()),
            ..outcome(4, None, None)
        },
    ]
}

fn caveat<'a>(r: &'a SweepReport, code: &str) -> Option<&'a str> {
    r.caveats
        .iter()
        .find(|c| c.code == code)
        .map(|c| c.text.as_str())
}

#[test]
fn spearman_matches_hand_computation() {
    // OOS ranks with the tie at 7: [1, 2, 3.5, 5, 3.5]. Deviations from 3: x = [-2,-1,0,1,2],
    // y = [-2,-1,0.5,2,0.5]; Σxy = 8, Σx² = 10, Σy² = 9.5, rho = 8/√95.
    let rho = spearman(&[1.0, 2.0, 3.0, 4.0, 5.0], &[5.0, 6.0, 7.0, 8.0, 7.0]).unwrap();
    assert!(close(rho, 8.0 / 95f64.sqrt(), 1e-15), "{rho}");
    assert!(close(rho, 0.820_782_681_668_123_3, 1e-12));
    let inverse = spearman(&[1.0, 2.0, 3.0], &[30.0, 20.0, 10.0]).unwrap();
    assert!(close(inverse, -1.0, 1e-15));
    assert_eq!(spearman(&[1.0, 1.0, 1.0], &[1.0, 2.0, 3.0]), None);
}

#[test]
fn selection_degradation_and_dispersion_match_hand_computation() {
    let spec = spec(5, true, Direction::Maximize, None);
    let r = analyze_sweep(&spec, &fixture()).unwrap();
    assert_eq!(r.fingerprint, spec.fingerprint());
    assert_eq!(
        r.configurations,
        ConfigurationCounts {
            tried: 5,
            completed_in_sample: 4,
            completed_out_of_sample: 4,
            failed: 1,
            missing: 0,
        }
    );
    let s = r.selected.unwrap();
    // Cells 1 and 2 tie at 4; the lower index is selected.
    assert_eq!((s.cell, s.in_sample, s.tied_with), (1, 4.0, 1));
    // OOS values [2, -1, 3, 0.5]: three are better than -1.
    assert_eq!(s.out_of_sample, Some(-1.0));
    assert_eq!((s.out_of_sample_rank, s.out_of_sample_of), (Some(4), 4));
    // 4 - (-1) = 5, relative 5/4.
    assert_eq!(s.degradation, Some(5.0));
    assert_eq!(s.relative_degradation, Some(1.25));
    // 4 over 80 s, -1 over 40 s.
    assert_eq!(s.in_sample_per_second, Some(0.05));
    assert_eq!(s.out_of_sample_per_second, Some(-0.025));
    assert!(close(s.degradation_per_second.unwrap(), 0.075, 1e-15));

    // IS ranks [1, 3.5, 3.5, 2], OOS ranks [3, 1, 4, 2]: Σxy = -0.5, Σx² = 4.5, Σy² = 5.
    let rc = r.rank_correlation.available().unwrap();
    assert_eq!(rc.cells, 4);
    assert!(close(rc.spearman_rho, -0.5 / 22.5f64.sqrt(), 1e-15));

    // IS [1, 2, 4, 4]: type-7 quartiles 1.75, 3, 4; mean 2.75; s.d. √(6.75/3) = 1.5.
    let d = r.dispersion.in_sample.unwrap();
    assert_eq!(
        (d.n, d.min, d.p25, d.median, d.p75, d.max, d.mean),
        (4, 1.0, 1.75, 3.0, 4.0, 4.0, 2.75)
    );
    assert_eq!(d.std_dev, Some(1.5));
    // (4 - 2.75) / 1.5.
    assert!(close(
        r.selection_bias.selected_z.unwrap(),
        1.25 / 1.5,
        1e-15
    ));
    assert_eq!(r.selection_bias.trials, 5);

    let bias = caveat(&r, "selection_bias").unwrap();
    assert!(bias.contains("rank 4 of 4"), "{bias}");
    assert!(caveat(&r, "incomplete_cells").unwrap().contains("1 of 5"));
    assert!(caveat(&r, "window_lengths").unwrap().contains("per-second"));
    assert!(caveat(&r, "no_annualised_sharpe").is_some());
    assert!(matches!(r.pbo, Availability::Unavailable { .. }));
}

#[test]
fn minimising_reverses_selection_rank_and_degradation_sign() {
    let r = analyze_sweep(&spec(5, true, Direction::Minimize, None), &fixture()).unwrap();
    let s = r.selected.unwrap();
    assert_eq!((s.cell, s.in_sample, s.tied_with), (0, 1.0, 0));
    // Lower is better: -1 and 0.5 beat 2.
    assert_eq!(s.out_of_sample_rank, Some(3));
    // In the objective's direction: -1 - (-2) = 1, i.e. worse out of sample.
    assert_eq!(s.degradation, Some(1.0));
}

#[test]
fn caveat_wording_depends_on_trials_and_holdout() {
    let one = analyze_sweep(
        &spec(1, false, Direction::Maximize, None),
        &[outcome(0, Some(1.0), None)],
    )
    .unwrap();
    let text = caveat(&one, "selection_bias").unwrap();
    assert!(text.contains("nothing was selected"), "{text}");
    assert_eq!(one.selection_bias.expected_max_z_under_noise, None);

    let five: Vec<SweepCellOutcome> = (0..5)
        .map(|i| outcome(i, Some(f64::from(i)), None))
        .collect();
    let no_holdout = analyze_sweep(&spec(5, false, Direction::Maximize, None), &five).unwrap();
    let text = caveat(&no_holdout, "selection_bias").unwrap();
    assert!(text.contains("best of 5 noisy estimates"), "{text}");
    assert!(text.contains("No out-of-sample window"), "{text}");
    assert!(!text.contains("pure-noise"), "{text}");
    assert!(matches!(
        no_holdout.rank_correlation,
        Availability::Unavailable { .. }
    ));
    assert!(caveat(&no_holdout, "window_lengths").is_none());

    let with: Vec<SweepCellOutcome> = (0..5)
        .map(|i| outcome(i, Some(f64::from(i)), Some(1.0)))
        .collect();
    let holdout = analyze_sweep(&spec(5, true, Direction::Maximize, None), &with).unwrap();
    let text = caveat(&holdout, "selection_bias").unwrap();
    assert!(text.contains("Out of sample"), "{text}");
    assert!(text.contains("rank 1 of 5"), "{text}");

    let missing_oos = analyze_sweep(&spec(5, true, Direction::Maximize, None), &five).unwrap();
    assert!(
        caveat(&missing_oos, "selection_bias")
            .unwrap()
            .contains("out-of-sample value is missing")
    );

    let many: Vec<SweepCellOutcome> = (0..25)
        .map(|i| outcome(i, Some(f64::from(i % 7)), None))
        .collect();
    let r = analyze_sweep(&spec(25, false, Direction::Maximize, None), &many).unwrap();
    let text = caveat(&r, "selection_bias").unwrap();
    assert!(text.contains("25 independent pure-noise trials"), "{text}");
    assert!(text.contains("the selected cell lies"), "{text}");

    // Cells never reported count as missing trials.
    let partial = analyze_sweep(&spec(5, false, Direction::Maximize, None), &five[..2]).unwrap();
    assert_eq!(partial.configurations.missing, 3);
    assert_eq!(partial.selection_bias.trials, 5);
    assert!(caveat(&partial, "few_cells").is_some());
}

#[test]
fn inverse_normal_and_expected_maximum_match_reference_values() {
    // Reference quantiles of N(0,1).
    for (p, z) in [
        (0.5, 0.0),
        (0.975, 1.959_963_984_540_054),
        (0.01, -2.326_347_874_040_841),
        (0.999, 3.090_232_306_167_813),
    ] {
        let got = inverse_normal_cdf(p);
        assert!(close(got, z, 1e-8), "{p}: {got} vs {z}");
    }
    // Exact expected maxima of 10 and 100 standard normals: 1.538753 and 2.507594. The
    // approximation is asymptotic; at these sizes it is within 0.04.
    assert!(close(expected_max_of_normals(10).unwrap(), 1.538_753, 0.04));
    assert!(close(
        expected_max_of_normals(100).unwrap(),
        2.507_594,
        0.04
    ));
    assert_eq!(expected_max_of_normals(1), None);
}

/// `rows[cell][block]`.
fn matrix(rows: &[[f64; 4]]) -> Vec<Vec<f64>> {
    rows.iter().map(|r| r.to_vec()).collect()
}

#[test]
fn pbo_on_constructed_matrices_with_known_answers() {
    // Perfectly persistent ranking: cell i earns i in every block, so the in-sample best is
    // always the out-of-sample best, ω = 4/5 and λ = ln 4 in all C(4,2) = 6 splits.
    let persistent = cscv_pbo(&matrix(&[[0.0; 4], [1.0; 4], [2.0; 4], [3.0; 4]])).unwrap();
    assert_eq!(persistent.pbo, 0.0);
    assert_eq!(persistent.combinations, 6);
    assert!(close(persistent.median_logit, 4f64.ln(), 1e-15));

    // Anti-persistent: cell i earns only in block i. The training half's two earners tie and
    // the lower index is chosen; out of sample it earns 0, tied with the other training cell
    // at the bottom: average rank 1.5, ω = 0.3, λ = ln(3/7) < 0 in every split.
    let anti = cscv_pbo(&matrix(&[
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]))
    .unwrap();
    assert_eq!(anti.pbo, 1.0);
    assert!(close(anti.median_logit, (3.0f64 / 7.0).ln(), 1e-15));

    // Mixed, worked by hand. Training halves (blocks) → selected cell → its OOS rank:
    //   {0,1}: c0 (6)            OOS [0,6,2,0]: c0 ties c3 at the bottom, r=1.5, λ<0
    //   {0,2}: c0 (3, ties c1)   OOS [3,3,2,0]: c0 ties c1 at the top,   r=3.5, λ>0
    //   {0,3}: c0 (3, ties c1)   OOS [3,3,2,0]: λ>0
    //   {1,2}: c0 (3, ties c1)   OOS [3,3,2,0]: λ>0
    //   {1,3}: c0 (3, ties c1)   OOS [3,3,2,0]: λ>0
    //   {2,3}: c1 (6)            OOS [6,0,2,0]: c1 ties c3 at the bottom, λ<0
    // PBO = 2/6; logits ±ln(7/3), median (type 7) between the 3rd and 4th: +ln(7/3).
    let mixed = cscv_pbo(&matrix(&[
        [3.0, 3.0, 0.0, 0.0],
        [0.0, 0.0, 3.0, 3.0],
        [1.0, 1.0, 1.0, 1.0],
        [0.0, 0.0, 0.0, 0.0],
    ]))
    .unwrap();
    assert_eq!(mixed.pbo, 2.0 / 6.0);
    assert_eq!(mixed.logits_at_or_below_zero, 2);
    assert!(close(mixed.median_logit, (7.0f64 / 3.0).ln(), 1e-15));

    assert!(cscv_pbo(&[vec![1.0, 2.0, 3.0]]).is_none());
}

#[test]
fn pbo_of_pure_noise_is_one_half_and_of_strong_signal_is_zero() {
    // Under pure noise the training and testing halves are independent, so the selected cell's
    // test rank is uniform on 1..=N and, for even N, P(λ ≤ 0) = 1/2 exactly. Averaged over 400
    // fixed-seed matrices (N = 10 cells, S = 8 blocks, 70 splits each).
    let mut rng = Rng::for_stream(2017, "pbo-noise");
    let mut total = 0.0;
    let trials = 400;
    for _ in 0..trials {
        let m: Vec<Vec<f64>> = (0..10)
            .map(|_| (0..8).map(|_| rng.approx_normal()).collect())
            .collect();
        let p = cscv_pbo(&m).unwrap();
        assert_eq!(p.combinations, 70);
        total += p.pbo;
    }
    let mean = total / f64::from(trials);
    assert!((0.45..=0.55).contains(&mean), "{mean}");

    // Cell i earns i per block plus noise of s.d. 0.1: rankings persist in every split.
    let mut rng = Rng::for_stream(2017, "pbo-signal");
    let m: Vec<Vec<f64>> = (0..10)
        .map(|i| {
            (0..8)
                .map(|_| f64::from(i) + 0.1 * rng.approx_normal())
                .collect()
        })
        .collect();
    assert_eq!(cscv_pbo(&m).unwrap().pbo, 0.0);
}

#[test]
fn pbo_in_the_report_requires_declared_blocks_and_every_cell() {
    let with_blocks = |cell: u32, b: [f64; 4]| SweepCellOutcome {
        blocks: Some(b.to_vec()),
        ..outcome(cell, Some(b.iter().sum()), None)
    };
    let cells = vec![
        with_blocks(0, [0.0; 4]),
        with_blocks(1, [1.0; 4]),
        with_blocks(2, [2.0; 4]),
        with_blocks(3, [3.0; 4]),
    ];
    let r = analyze_sweep(&spec(4, false, Direction::Maximize, Some(4)), &cells).unwrap();
    let p = r.pbo.available().unwrap();
    assert_eq!((p.pbo, p.cells, p.blocks), (0.0, 4, 4));
    assert!(caveat(&r, "cscv_assumptions").unwrap().contains("6 ways"));

    // Minimising flips the ranking: the in-sample best (lowest) is now cell 0 everywhere.
    let r = analyze_sweep(&spec(4, false, Direction::Minimize, Some(4)), &cells).unwrap();
    assert_eq!(r.pbo.available().unwrap().pbo, 0.0);
    assert_eq!(r.selected.unwrap().cell, 0);

    let not_declared = analyze_sweep(&spec(4, false, Direction::Maximize, None), &[]).unwrap();
    assert!(
        matches!(not_declared.pbo, Availability::Unavailable { reason } if reason.contains("cscv_blocks"))
    );

    let mut gap = cells.clone();
    gap[2] = SweepCellOutcome {
        error: Some("failed".into()),
        ..outcome(2, None, None)
    };
    let r = analyze_sweep(&spec(4, false, Direction::Maximize, Some(4)), &gap).unwrap();
    match &r.pbo {
        Availability::Unavailable { reason } => assert!(reason.contains("1 of 4"), "{reason}"),
        other => panic!("{other:?}"),
    }

    let few = analyze_sweep(&spec(3, false, Direction::Maximize, Some(4)), &cells[..3]).unwrap();
    match &few.pbo {
        Availability::Unavailable { reason } => assert!(reason.contains("at least 4"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn malformed_outcomes_are_rejected() {
    let s = spec(3, false, Direction::Maximize, Some(4));
    let cases: Vec<Vec<SweepCellOutcome>> = vec![
        vec![outcome(3, Some(1.0), None)],
        vec![outcome(0, Some(1.0), None), outcome(0, Some(2.0), None)],
        vec![outcome(0, Some(f64::NAN), None)],
        vec![outcome(0, Some(1.0), Some(1.0))],
        vec![SweepCellOutcome {
            blocks: Some(vec![1.0; 3]),
            ..outcome(0, Some(1.0), None)
        }],
    ];
    for case in cases {
        assert!(analyze_sweep(&s, &case).is_err(), "{case:?}");
    }
}

fn sample(ts: i64, equity: f64) -> EquitySample {
    EquitySample {
        ts,
        tick_no: 0,
        mid_tick: None,
        position: 0.0,
        known_position: 0.0,
        cash: 0.0,
        fees: 0.0,
        realized_pnl: 0.0,
        unrealized_pnl: 0.0,
        equity,
        open_orders: 0,
    }
}

#[test]
fn objective_blocks_partition_the_running_total() {
    // Samples at 0, 10, 20, 30 and the closing sample at 40; blocks of [0, 40) are 10 ns long.
    // A sample on a boundary opens the next block: block 0 = total before 10 (0) − 0 = 0,
    // block 1 = 1 − 0, block 2 = 3 − 1, block 3 (to the closing sample) = 5 − 3.
    let equity: Vec<EquitySample> = [(0, 0.0), (10, 1.0), (20, 3.0), (30, 2.0), (40, 5.0)]
        .iter()
        .map(|&(t, e)| sample(t, e))
        .collect();
    let w = TimeWindow { start: 0, end: 40 };
    let b = objective_blocks(ObjectiveMetric::NetPnl, &equity, w, 4).unwrap();
    assert_eq!(b, vec![0.0, 1.0, 2.0, 2.0]);
    assert_eq!(b.iter().sum::<f64>(), 5.0);
    assert!(objective_blocks(ObjectiveMetric::MaxDrawdown, &equity, w, 4).is_err());
    assert!(objective_blocks(ObjectiveMetric::NetPnl, &[], w, 4).is_err());
}

#[test]
fn report_json_shape_is_stable() {
    let spec = spec(5, true, Direction::Maximize, None);
    let r = analyze_sweep(&spec, &fixture()).unwrap();
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["rank_correlation"]["status"], "available");
    assert_eq!(json["rank_correlation"]["cells"], 4);
    assert_eq!(json["pbo"]["status"], "unavailable");
    assert!(json["pbo"]["reason"].is_string());
    assert_eq!(json["objective"]["metric"], "net_pnl");
    assert_eq!(json["selected"]["out_of_sample_rank"], 4);
    let back: SweepReport = serde_json::from_value(json).unwrap();
    assert_eq!(back, r);
}
