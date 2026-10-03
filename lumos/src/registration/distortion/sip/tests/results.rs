use super::*;

#[test]
fn sip_config_default_values() {
    let config = SipConfig::default();
    assert_eq!(config.order, 3);
    assert!(config.reference_point.is_none());
    assert!((config.clip_sigma - 3.0).abs() < 1e-15);
    assert_eq!(config.clip_iterations, 3);
}

#[test]
fn sip_config_validate_accepts_all_valid_orders() {
    for order in 2..=5 {
        let config = SipConfig {
            order,
            ..Default::default()
        };
        config.validate().unwrap();
    }
}

/// The fit's normalization is the points' around the reference point.
#[test]
fn norm_scale_stored_correctly() {
    let field = RadialField::new(DVec2::ZERO, 1e-7);
    let RadialPairs { reference, .. } = field.pairs();
    let sip = fit_field(&field, 2, 3).polynomial;
    assert_eq!(
        sip.norm,
        PointNormalization::around(&reference, DVec2::ZERO)
    );
}

/// The point field a case fits: a radial field on the `[0, 1000]²` grid, with or without outliers.
#[derive(Debug, Clone, Copy)]
struct Distortion {
    field: RadialField,
    /// Append [`OUTLIERS`] after the clean grid.
    outliers: bool,
}

/// One `fit_sip` run and the metrics it must produce.
#[derive(Debug)]
struct MetricsCase {
    name: &'static str,
    distortion: Distortion,
    order: usize,
    clip_iterations: usize,
    rejected: Rejected,
    /// Bounds on the surviving fit's RMS residual. The lower bound is what proves a model too weak
    /// for its data — an order-3 fit of an r⁴ field cannot drive the residual to zero.
    rms_below: f64,
    /// `None` where the fit is expected to be exact; a floor only where the model is too weak for
    /// its data and driving the residual to zero would mean the fixture was wrong.
    rms_above: Option<f64>,
    /// Upper bound on `max_residual`. Not implied by `rms_below`: the invariant runs the other way,
    /// so a single bad point can sit far above a small RMS.
    max_residual_below: Option<f64>,
    /// `max_correction`, where the geometry pins it.
    correction: Option<Approx>,
}

#[derive(Debug, Clone, Copy)]
enum Rejected {
    Exactly(usize),
    /// A floor, not a count: the first iterations fit contaminated data, so clean points beside an
    /// outlier can be clipped too before the fit converges.
    AtLeast(usize),
}

#[derive(Debug, Clone, Copy)]
struct Approx {
    value: f64,
    tolerance: f64,
}

/// The barrel field with a `d·1e-14·|d|⁴` term, which order 3 cannot hold, on a 50 px grid.
fn quartic() -> RadialField {
    RadialField {
        k4: 1e-14,
        step: 50,
        ..barrel()
    }
}

/// Gross outliers — 20–30 px off the barrel field — for the clipping cases.
const OUTLIERS: [([f64; 2], [f64; 2]); 3] = [
    ([300.0, 300.0], [320.0, 280.0]),
    ([700.0, 200.0], [685.0, 225.0]),
    ([100.0, 800.0], [130.0, 810.0]),
];

fn build_case(case: &Distortion) -> RadialPairs {
    let mut pairs = case.field.pairs();
    if case.outliers {
        for (reference, target) in OUTLIERS {
            pairs.reference.push(DVec2::from_array(reference));
            pairs.target.push(DVec2::from_array(target));
        }
    }
    pairs
}

/// Every `SipFitResult` metric, across the distortion shapes and clipping settings that produce
/// them.
///
/// Two invariants hold for *any* fit and are asserted on every row rather than on whichever case
/// happens to mention them: `points_used + points_rejected` accounts for every input point, and
/// `max_residual >= rms_residual`, because the largest residual also contributes to the mean of
/// squares it is compared against.
///
/// The hand-computed figure is `max_correction` on the barrel field. Its farthest grid point from
/// the centre is a corner at `d = (-500, -500)`, so `|d|² = 500000` and the distortion there is
/// `d·k·|d|² = (-25, -25)`, of magnitude `25√2 = 35.3553`. SIP order 3 models a radial `r²` term
/// exactly, so the recovered correction and every residual are held to [`EXACT_FIT_PX`].
#[test]
fn fit_sip_metrics_match_every_fixture() {
    let corner = 25.0 * 2.0_f64.sqrt();
    let cases = [
        MetricsCase {
            name: "undistorted",
            distortion: Distortion {
                field: RadialField { k: 0.0, ..barrel() },
                outliers: false,
            },
            order: 2,
            clip_iterations: 3,
            rejected: Rejected::Exactly(0),
            rms_below: EXACT_FIT_PX,
            rms_above: None,
            max_residual_below: Some(EXACT_FIT_PX),
            correction: Some(Approx {
                value: 0.0,
                tolerance: EXACT_FIT_PX,
            }),
        },
        MetricsCase {
            name: "barrel, order 3",
            distortion: Distortion {
                field: barrel(),
                outliers: false,
            },
            order: 3,
            clip_iterations: 3,
            rejected: Rejected::Exactly(0),
            rms_below: EXACT_FIT_PX,
            rms_above: None,
            max_residual_below: Some(EXACT_FIT_PX),
            correction: Some(Approx {
                value: corner,
                tolerance: EXACT_FIT_PX,
            }),
        },
        MetricsCase {
            name: "barrel with outliers, clipping on",
            distortion: Distortion {
                field: barrel(),
                outliers: true,
            },
            order: 3,
            clip_iterations: 3,
            rejected: Rejected::AtLeast(3),
            rms_below: EXACT_FIT_PX,
            rms_above: None,
            max_residual_below: None,
            correction: None,
        },
        MetricsCase {
            name: "barrel with outliers, clipping off",
            distortion: Distortion {
                field: barrel(),
                outliers: true,
            },
            order: 3,
            clip_iterations: 0,
            rejected: Rejected::Exactly(0),
            rms_below: f64::INFINITY,
            rms_above: None,
            max_residual_below: None,
            correction: None,
        },
        MetricsCase {
            name: "quartic field, order 3 cannot model it",
            distortion: Distortion {
                field: quartic(),
                outliers: false,
            },
            order: 3,
            clip_iterations: 0,
            rejected: Rejected::Exactly(0),
            rms_below: f64::INFINITY,
            rms_above: Some(1e-6),
            max_residual_below: None,
            correction: None,
        },
    ];

    for case in &cases {
        let RadialPairs {
            reference: ref_points,
            target: target_points,
        } = build_case(&case.distortion);
        let n = ref_points.len();
        let config = SipConfig {
            order: case.order,
            reference_point: Some(case.distortion.field.centre),
            clip_iterations: case.clip_iterations,
            ..Default::default()
        };
        let result = fit_sip(&ref_points, &target_points, &Transform::identity(), &config);
        let name = case.name;

        assert_eq!(
            result.points_used + result.points_rejected,
            n,
            "{name}: {} used + {} rejected does not account for {n} points",
            result.points_used,
            result.points_rejected
        );
        assert!(
            result.max_residual >= result.rms_residual,
            "{name}: max {:.6e} must be >= rms {:.6e}",
            result.max_residual,
            result.rms_residual
        );

        match case.rejected {
            Rejected::Exactly(expected) => {
                assert_eq!(result.points_rejected, expected, "{name}: rejection count");
            }
            Rejected::AtLeast(floor) => assert!(
                result.points_rejected >= floor,
                "{name}: expected at least {floor} rejections, got {}",
                result.points_rejected
            ),
        }
        assert!(
            result.rms_residual <= case.rms_below,
            "{name}: rms {:.6e} should be under {:.6e}",
            result.rms_residual,
            case.rms_below
        );
        if let Some(floor) = case.rms_above {
            assert!(
                result.rms_residual > floor,
                "{name}: rms {:.6e} should exceed {floor:.6e}",
                result.rms_residual
            );
        }
        if let Some(ceiling) = case.max_residual_below {
            assert!(
                result.max_residual <= ceiling,
                "{name}: max_residual {:.6e} should be under {ceiling:.6e}",
                result.max_residual
            );
        }
        if let Some(Approx { value, tolerance }) = case.correction {
            assert!(
                (result.max_correction - value).abs() <= tolerance,
                "{name}: max_correction {:.6} should be {value:.6} +- {tolerance:.6}",
                result.max_correction
            );
        }
    }
}

/// The two comparisons the table cannot make, because each grades one fit against another rather
/// than against a number: a richer model fits better, and clipping outliers beats keeping them.
#[test]
fn fit_sip_quality_improves_with_order_and_with_clipping() {
    let transform = Transform::identity();
    let order = |order, clip_iterations| SipConfig {
        order,
        reference_point: Some(barrel().centre),
        clip_iterations,
        ..Default::default()
    };

    // Order 3 captures the r²·d term but not r⁴·d, a degree-5 field, which order 5 holds exactly.
    // Order 4 would not do: on a grid symmetric about the centre its added even terms are
    // orthogonal to an odd field and buy nothing. Clipping is off so both fit the same points —
    // otherwise order 3 rejects what it cannot model and the two fits are graded on different data.
    let quartic = Distortion {
        field: quartic(),
        outliers: false,
    };
    let RadialPairs {
        reference: ref_points,
        target: target_points,
    } = build_case(&quartic);
    let low = fit_sip(&ref_points, &target_points, &transform, &order(3, 0));
    let high = fit_sip(&ref_points, &target_points, &transform, &order(5, 0));

    assert!(
        high.rms_residual < low.rms_residual,
        "order 5 rms {:.6e} should beat order 3 rms {:.6e}",
        high.rms_residual,
        low.rms_residual
    );
    assert!(
        high.max_residual <= low.max_residual,
        "order 5 max {:.6e} should be no worse than order 3 max {:.6e}",
        high.max_residual,
        low.max_residual
    );

    // Same points, clipping on versus off. The clipped fit is graded on its survivors, so its RMS
    // is strictly lower than the unclipped fit that the outliers pull.
    let contaminated = Distortion {
        field: barrel(),
        outliers: true,
    };
    let RadialPairs {
        reference: ref_points,
        target: target_points,
    } = build_case(&contaminated);
    let n = ref_points.len();
    let clipped = fit_sip(&ref_points, &target_points, &transform, &order(3, 3));
    let unclipped = fit_sip(&ref_points, &target_points, &transform, &order(3, 0));

    assert_eq!(unclipped.points_used, n);
    assert!(clipped.points_used < n);
    assert!(
        clipped.rms_residual < unclipped.rms_residual,
        "clipped rms {:.6e} should beat unclipped rms {:.6e}",
        clipped.rms_residual,
        unclipped.rms_residual
    );
}
