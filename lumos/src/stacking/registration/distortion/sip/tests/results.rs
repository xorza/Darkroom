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

#[test]
fn norm_scale_stored_correctly() {
    // The norm_scale should be the average distance from ref_points to reference_point.
    let center = DVec2::new(0.0, 0.0);
    let (ref_points, target_points) = make_radial_distortion_points(center, 1e-7, 100, 1000);

    let transform = Transform::identity();
    let config = SipConfig {
        order: 2,
        reference_point: Some(center),
        ..Default::default()
    };

    let sip = fit_sip(&ref_points, &target_points, &transform, &config).polynomial;

    let expected_norm_scale = avg_distance(&ref_points, center);
    assert!(
        (sip.norm.scale - expected_norm_scale).abs() < 1e-10,
        "norm scale: got {:.6}, expected {:.6}",
        sip.norm.scale,
        expected_norm_scale
    );
}

/// One `fit_sip` run and the metrics it must produce.
#[derive(Debug)]
struct MetricsCase {
    name: &'static str,
    /// Barrel coefficient in `d·k·|d|²`. Zero is an undistorted grid.
    k: f64,
    /// Additional `d·k4·|d|⁴` term, which order 3 cannot model.
    k4: f64,
    grid_step: usize,
    /// Append [`OUTLIERS`] after the clean grid.
    outliers: bool,
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

/// Gross outliers — 20–30 px off the barrel field — for the clipping cases.
const OUTLIERS: [([f64; 2], [f64; 2]); 3] = [
    ([300.0, 300.0], [320.0, 280.0]),
    ([700.0, 200.0], [685.0, 225.0]),
    ([100.0, 800.0], [130.0, 810.0]),
];

/// The field centre every case distorts about, and the reference point every fit is given.
const CENTRE: DVec2 = DVec2::new(500.0, 500.0);

fn build_case(case: &MetricsCase) -> (Vec<DVec2>, Vec<DVec2>) {
    let mut ref_points = Vec::new();
    let mut target_points = Vec::new();
    for y in (0..=1000).step_by(case.grid_step) {
        for x in (0..=1000).step_by(case.grid_step) {
            let p = DVec2::new(x as f64, y as f64);
            let d = p - CENTRE;
            let r2 = d.length_squared();
            ref_points.push(p);
            target_points.push(p + d * case.k * r2 + d * case.k4 * r2 * r2);
        }
    }
    if case.outliers {
        for (reference, target) in OUTLIERS {
            ref_points.push(DVec2::from_array(reference));
            target_points.push(DVec2::from_array(target));
        }
    }
    (ref_points, target_points)
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
/// exactly, so the recovered correction has to match that, not merely approach it.
#[test]
fn fit_sip_metrics_match_every_fixture() {
    let corner = 25.0 * 2.0_f64.sqrt();
    let cases = [
        MetricsCase {
            name: "undistorted",
            k: 0.0,
            k4: 0.0,
            grid_step: 100,
            outliers: false,
            order: 2,
            clip_iterations: 3,
            rejected: Rejected::Exactly(0),
            rms_below: 1e-10,
            rms_above: None,
            max_residual_below: Some(1e-10),
            correction: Some(Approx {
                value: 0.0,
                tolerance: 1e-10,
            }),
        },
        MetricsCase {
            name: "barrel, order 3",
            k: 1e-7,
            k4: 0.0,
            grid_step: 100,
            outliers: false,
            order: 3,
            clip_iterations: 3,
            rejected: Rejected::Exactly(0),
            rms_below: 0.01,
            rms_above: None,
            max_residual_below: Some(0.05),
            correction: Some(Approx {
                value: corner,
                tolerance: 0.1,
            }),
        },
        MetricsCase {
            name: "barrel with outliers, clipping on",
            k: 1e-7,
            k4: 0.0,
            grid_step: 100,
            outliers: true,
            order: 3,
            clip_iterations: 3,
            rejected: Rejected::AtLeast(3),
            rms_below: 0.01,
            rms_above: None,
            max_residual_below: None,
            correction: None,
        },
        MetricsCase {
            name: "barrel with outliers, clipping off",
            k: 1e-7,
            k4: 0.0,
            grid_step: 100,
            outliers: true,
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
            k: 1e-7,
            k4: 1e-14,
            grid_step: 50,
            outliers: false,
            order: 3,
            clip_iterations: 0,
            rejected: Rejected::Exactly(0),
            rms_below: f64::INFINITY,
            rms_above: Some(1e-6),
            max_residual_below: None,
            correction: Some(Approx {
                value: corner,
                tolerance: corner * 0.2,
            }),
        },
    ];

    for case in &cases {
        let (ref_points, target_points) = build_case(case);
        let n = ref_points.len();
        let config = SipConfig {
            order: case.order,
            reference_point: Some(CENTRE),
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
                assert_eq!(result.points_rejected, expected, "{name}: rejection count")
            }
            Rejected::AtLeast(floor) => assert!(
                result.points_rejected >= floor,
                "{name}: expected at least {floor} rejections, got {}",
                result.points_rejected
            ),
        }
        assert!(
            result.rms_residual < case.rms_below,
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
                result.max_residual < ceiling,
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
        reference_point: Some(CENTRE),
        clip_iterations,
        ..Default::default()
    };

    // Order 3 captures the r² term but not r⁴; order 4 adds terms that partially model it. Clipping
    // is off so both fit the same points — otherwise order 3 rejects what it cannot model and the
    // two fits are graded on different data.
    let quartic = MetricsCase {
        name: "quartic",
        k: 1e-7,
        k4: 1e-14,
        grid_step: 50,
        outliers: false,
        order: 3,
        clip_iterations: 0,
        rejected: Rejected::Exactly(0),
        rms_below: f64::INFINITY,
        rms_above: None,
        max_residual_below: None,
        correction: None,
    };
    let (ref_points, target_points) = build_case(&quartic);
    let low = fit_sip(&ref_points, &target_points, &transform, &order(3, 0));
    let high = fit_sip(&ref_points, &target_points, &transform, &order(4, 0));

    assert!(
        high.rms_residual < low.rms_residual,
        "order 4 rms {:.6e} should beat order 3 rms {:.6e}",
        high.rms_residual,
        low.rms_residual
    );
    assert!(
        high.max_residual <= low.max_residual,
        "order 4 max {:.6e} should be no worse than order 3 max {:.6e}",
        high.max_residual,
        low.max_residual
    );

    // Same points, clipping on versus off. The clipped fit is graded on its survivors, so its RMS
    // is strictly lower than the unclipped fit that the outliers pull.
    let contaminated = MetricsCase {
        outliers: true,
        grid_step: 100,
        k4: 0.0,
        ..quartic
    };
    let (ref_points, target_points) = build_case(&contaminated);
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
