//! Tests for thin-plate spline distortion modeling.

use super::*;
use glam::DVec2;

// ============================================================================
// Helpers
// ============================================================================

/// Standard 4-corner + center source grid on [0,100]x[0,100].
fn square_source_5() -> Vec<DVec2> {
    vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 50.0),
    ]
}

/// Standard 4-corner source grid on [0,100]x[0,100].
fn square_source_4() -> Vec<DVec2> {
    vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ]
}

/// Assert that point `a` is within `tol` of point `b` (per-component).
fn assert_dvec2_near(a: DVec2, b: DVec2, tol: f64, msg: &str) {
    assert!(
        (a.x - b.x).abs() < tol,
        "{}: x mismatch: got {} expected {} (diff {})",
        msg,
        a.x,
        b.x,
        (a.x - b.x).abs()
    );
    assert!(
        (a.y - b.y).abs() < tol,
        "{}: y mismatch: got {} expected {} (diff {})",
        msg,
        a.y,
        b.y,
        (a.y - b.y).abs()
    );
}

/// Fit a TPS with default config and assert it succeeds.
fn fit_default(source: &[DVec2], target: &[DVec2]) -> ThinPlateSpline {
    ThinPlateSpline::fit(source, target, TpsConfig::default()).unwrap()
}

/// Verify that all control points are interpolated exactly (within tol).
fn assert_control_points_exact(
    tps: &ThinPlateSpline,
    source: &[DVec2],
    target: &[DVec2],
    tol: f64,
) {
    for (i, (&src, &tgt)) in source.iter().zip(target.iter()).enumerate() {
        let t = tps.transform(src);
        assert_dvec2_near(t, tgt, tol, &format!("Control point {i}"));
    }
}

// ============================================================================
// tps_kernel tests
// ============================================================================

/// Test TPS kernel U(r) = r^2 * ln(r) at key values with hand-computed results.
#[test]
fn test_tps_kernel_known_values() {
    // U(0) = 0 by convention (limit as r -> 0)
    assert_eq!(tps_kernel(0.0), 0.0);

    // Very small r below threshold 1e-10 should return 0
    assert_eq!(tps_kernel(1e-15), 0.0);
    assert_eq!(tps_kernel(1e-11), 0.0);

    // At the threshold boundary: 1e-10 should return ~0 (may have tiny FP noise)
    assert!(tps_kernel(1e-10).abs() < 1e-15);

    // U(1) = 1^2 * ln(1) = 1 * 0 = 0
    assert!((tps_kernel(1.0)).abs() < 1e-15);

    // U(e) = e^2 * ln(e) = e^2 * 1 = e^2 = 7.389056098...
    let e = std::f64::consts::E;
    let expected = e * e; // 7.38905609893065
    assert!((tps_kernel(e) - expected).abs() < 1e-12);

    // U(2) = 2^2 * ln(2) = 4 * 0.693147... = 2.772588...
    let expected_2 = 4.0 * 2.0_f64.ln(); // 2.772588722239781
    assert!((tps_kernel(2.0) - expected_2).abs() < 1e-12);

    // U(0.5) = 0.25 * ln(0.5) = 0.25 * (-0.693147...) = -0.173286...
    // Note: U is negative for r < 1 because ln(r) < 0
    let expected_half = 0.25 * 0.5_f64.ln(); // -0.17328679513998632
    assert!((tps_kernel(0.5) - expected_half).abs() < 1e-12);

    // U(10) = 100 * ln(10) = 100 * 2.302585... = 230.2585...
    let expected_10 = 100.0 * 10.0_f64.ln(); // 230.25850929940458
    assert!((tps_kernel(10.0) - expected_10).abs() < 1e-10);
}

/// Verify U(r) is negative for 0 < r < 1 and positive for r > 1.
#[test]
fn test_tps_kernel_sign() {
    // For r in (0, 1): r^2 > 0 and ln(r) < 0, so U(r) < 0
    assert!(tps_kernel(0.01) < 0.0);
    assert!(tps_kernel(0.5) < 0.0);
    assert!(tps_kernel(0.999) < 0.0);

    // For r > 1: r^2 > 0 and ln(r) > 0, so U(r) > 0
    assert!(tps_kernel(1.001) > 0.0);
    assert!(tps_kernel(2.0) > 0.0);
    assert!(tps_kernel(100.0) > 0.0);
}

// ============================================================================
// compute_normalization tests
// ============================================================================

/// Test normalization with a simple square bounding box.
#[test]
fn test_compute_normalization_square() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];
    let (center, scale) = compute_normalization(&points);
    // Bounding box: [0,100] x [0,100]
    // center = (50, 50), scale = max(100, 100) / 2 = 50
    assert_dvec2_near(center, DVec2::new(50.0, 50.0), 1e-12, "center");
    assert!((scale - 50.0).abs() < 1e-12, "scale: {scale}");
}

/// Test normalization with a rectangular bounding box (wider than tall).
#[test]
fn test_compute_normalization_rectangle() {
    let points = vec![
        DVec2::new(10.0, 20.0),
        DVec2::new(210.0, 20.0),
        DVec2::new(10.0, 80.0),
        DVec2::new(210.0, 80.0),
    ];
    let (center, scale) = compute_normalization(&points);
    // Bounding box: [10,210] x [20,80]
    // center = ((10+210)/2, (20+80)/2) = (110, 50)
    // range = (200, 60), max = 200, scale = 100
    assert_dvec2_near(center, DVec2::new(110.0, 50.0), 1e-12, "center");
    assert!((scale - 100.0).abs() < 1e-12, "scale: {scale}");
}

/// Test normalization with coincident points (degenerate case).
#[test]
fn test_compute_normalization_coincident() {
    let points = vec![
        DVec2::new(42.0, 17.0),
        DVec2::new(42.0, 17.0),
        DVec2::new(42.0, 17.0),
    ];
    let (center, scale) = compute_normalization(&points);
    // All points identical, range = (0, 0), scale falls back to 1.0
    assert_dvec2_near(center, DVec2::new(42.0, 17.0), 1e-12, "center");
    assert!(
        (scale - 1.0).abs() < 1e-12,
        "degenerate scale should be 1.0"
    );
}

// ============================================================================
// solve_linear_system tests
// ============================================================================

/// Test solve_linear_system with a simple 2x2 system.
#[test]
fn test_solve_linear_system_2x2() {
    // System: 2x + y = 5, x + 3y = 7
    // Solution: x = 8/5 = 1.6, y = 9/5 = 1.8
    // By Cramer's rule: det = 2*3 - 1*1 = 5
    // x = (5*3 - 1*7) / 5 = 8/5 = 1.6
    // y = (2*7 - 5*1) / 5 = 9/5 = 1.8
    let a = vec![vec![2.0, 1.0], vec![1.0, 3.0]];
    let b = vec![5.0, 7.0];
    let x = solve_linear_system(&a, &b).unwrap();
    assert!((x[0] - 1.6).abs() < 1e-12, "x[0] = {}", x[0]);
    assert!((x[1] - 1.8).abs() < 1e-12, "x[1] = {}", x[1]);
}

/// Test solve_linear_system with a singular matrix returns None.
#[test]
fn test_solve_linear_system_singular() {
    // Rows are linearly dependent: row 1 = 2 * row 0
    let a = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
    let b = vec![3.0, 6.0];
    assert!(solve_linear_system(&a, &b).is_none());
}

/// Test solve_linear_system with a 3x3 identity matrix.
#[test]
fn test_solve_linear_system_identity() {
    // I * x = b => x = b
    let a = vec![
        vec![1.0, 0.0, 0.0],
        vec![0.0, 1.0, 0.0],
        vec![0.0, 0.0, 1.0],
    ];
    let b = vec![7.0, -3.0, 11.0];
    let x = solve_linear_system(&a, &b).unwrap();
    assert!((x[0] - 7.0).abs() < 1e-12);
    assert!((x[1] - (-3.0)).abs() < 1e-12);
    assert!((x[2] - 11.0).abs() < 1e-12);
}

/// Test solve_linear_system with mismatched dimensions returns None.
#[test]
fn test_solve_linear_system_dimension_mismatch() {
    let a = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
    let b = vec![1.0, 2.0, 3.0]; // 3 elements but matrix is 2x2
    assert!(solve_linear_system(&a, &b).is_none());
}

// ============================================================================
// ThinPlateSpline::fit rejection tests
// ============================================================================

/// Empty input returns None.
#[test]
fn test_tps_fit_empty() {
    assert!(ThinPlateSpline::fit(&[], &[], TpsConfig::default()).is_none());
}

/// One point returns None (need >= 3).
#[test]
fn test_tps_fit_one_point() {
    let s = vec![DVec2::new(5.0, 5.0)];
    let t = vec![DVec2::new(6.0, 6.0)];
    assert!(ThinPlateSpline::fit(&s, &t, TpsConfig::default()).is_none());
}

/// Two points returns None (need >= 3).
#[test]
fn test_tps_fit_two_points() {
    let s = vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 100.0)];
    let t = vec![DVec2::new(1.0, 1.0), DVec2::new(101.0, 101.0)];
    assert!(ThinPlateSpline::fit(&s, &t, TpsConfig::default()).is_none());
}

/// Mismatched source/target lengths returns None.
#[test]
fn test_tps_fit_mismatched_counts() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
    ];
    let target = vec![DVec2::new(1.0, 1.0), DVec2::new(101.0, 1.0)];
    assert!(ThinPlateSpline::fit(&source, &target, TpsConfig::default()).is_none());
}

/// Collinear points produce a singular TPS matrix and return None.
#[test]
fn test_tps_fit_collinear_returns_none() {
    // All points on the x-axis: the P matrix has all y_i = 0,
    // making the system singular (P^T column 3 is all zeros).
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(50.0, 0.0),
        DVec2::new(100.0, 0.0),
    ];
    let target = vec![
        DVec2::new(1.0, 0.0),
        DVec2::new(51.0, 0.0),
        DVec2::new(101.0, 0.0),
    ];
    let result = ThinPlateSpline::fit(&source, &target, TpsConfig::default());
    // The matrix is singular because the 3 points are collinear, so P has rank 2
    // (the y column is all zero). This should return None.
    assert!(
        result.is_none(),
        "Collinear points should produce singular matrix"
    );
}

// ============================================================================
// ThinPlateSpline::fit + transform: exact interpolation
// ============================================================================

/// With zero regularization, transformed source points must exactly match targets.
#[test]
fn test_tps_exact_interpolation() {
    let source = square_source_5();
    let target = vec![
        DVec2::new(5.0, 3.0),
        DVec2::new(102.0, 1.0),
        DVec2::new(2.0, 98.0),
        DVec2::new(105.0, 103.0),
        DVec2::new(52.0, 51.0),
    ];

    let tps = fit_default(&source, &target);
    assert_control_points_exact(&tps, &source, &target, 1e-6);
}

/// Three points (minimum) should be interpolated exactly with a simple translation.
#[test]
fn test_tps_three_points_exact() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(50.0, 100.0),
    ];
    // Translate by (3, 7)
    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(3.0, 7.0)).collect();

    let tps = fit_default(&source, &target);
    assert_control_points_exact(&tps, &source, &target, 1e-6);
    assert_eq!(tps.num_control_points(), 3);
}

// ============================================================================
// Affine transformations (identity, translation, scaling, rotation)
// ============================================================================

/// Identity: source == target. Control points and interior points map to themselves.
#[test]
fn test_tps_identity() {
    let points = square_source_5();
    let tps = fit_default(&points, &points);

    // Control points
    assert_control_points_exact(&tps, &points, &points, 1e-6);

    // Interior point: (25, 75) -> (25, 75)
    let t = tps.transform(DVec2::new(25.0, 75.0));
    assert_dvec2_near(t, DVec2::new(25.0, 75.0), 1e-3, "interior identity");

    // Bending energy should be zero for identity
    assert!(
        tps.bending_energy().abs() < 1e-10,
        "Identity bending energy should be ~0"
    );
}

/// Pure translation by (10, 5). Both control points and interior must shift.
#[test]
fn test_tps_translation() {
    let source = square_source_4();
    let shift = DVec2::new(10.0, 5.0);
    let target: Vec<DVec2> = source.iter().map(|&p| p + shift).collect();

    let tps = fit_default(&source, &target);

    // Control points
    assert_control_points_exact(&tps, &source, &target, 1e-6);

    // Interior: (50,50) -> (60,55). Translation is affine, so TPS reproduces exactly.
    let t = tps.transform(DVec2::new(50.0, 50.0));
    assert_dvec2_near(t, DVec2::new(60.0, 55.0), 1e-3, "interior translation");

    // Bending energy should be ~0 for affine transforms
    assert!(
        tps.bending_energy().abs() < 1e-6,
        "Translation bending energy: {}",
        tps.bending_energy()
    );
}

/// Uniform scaling by 1.1. Affine, so TPS reproduces exactly.
#[test]
fn test_tps_scaling() {
    let source = square_source_4();
    let target: Vec<DVec2> = source.iter().map(|&p| p * 1.1).collect();

    let tps = fit_default(&source, &target);

    // Control points
    assert_control_points_exact(&tps, &source, &target, 1e-6);

    // Interior: (50,50)*1.1 = (55,55)
    let t = tps.transform(DVec2::new(50.0, 50.0));
    assert_dvec2_near(t, DVec2::new(55.0, 55.0), 1e-3, "interior scaling");

    // Bending energy should be ~0 for affine
    assert!(
        tps.bending_energy().abs() < 1e-6,
        "Scaling bending energy: {}",
        tps.bending_energy()
    );
}

/// Rotation by 10 degrees around origin. Affine, so TPS reproduces exactly.
#[test]
fn test_tps_rotation() {
    let source = square_source_4();
    let angle = 10.0_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    let target: Vec<DVec2> = source
        .iter()
        .map(|&p| DVec2::new(p.x * cos_a - p.y * sin_a, p.x * sin_a + p.y * cos_a))
        .collect();

    let tps = fit_default(&source, &target);
    assert_control_points_exact(&tps, &source, &target, 1e-5);

    // Interior: rotate (50, 50) by 10 degrees
    // cos(10deg) = 0.98481, sin(10deg) = 0.17365
    // x' = 50*0.98481 - 50*0.17365 = 50*(0.98481 - 0.17365) = 50*0.81116 = 40.558
    // y' = 50*0.17365 + 50*0.98481 = 50*(0.17365 + 0.98481) = 50*1.15846 = 57.923
    let expected = DVec2::new(50.0 * cos_a - 50.0 * sin_a, 50.0 * sin_a + 50.0 * cos_a);
    let t = tps.transform(DVec2::new(50.0, 50.0));
    assert_dvec2_near(t, expected, 1e-3, "interior rotation");
}

// ============================================================================
// Non-affine distortion
// ============================================================================

/// Barrel distortion on a dense grid: control points exact, midpoints close.
#[test]
fn test_tps_barrel_distortion() {
    let mut source = Vec::new();
    let mut target = Vec::new();
    let center = DVec2::new(500.0, 500.0);
    let k = 0.000001; // barrel coefficient

    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let s = DVec2::new(x as f64, y as f64);
            source.push(s);
            // r' = r(1 + k*r^2)
            let d = s - center;
            let r2 = d.length_squared();
            let t = center + d * (1.0 + k * r2);
            target.push(t);
        }
    }

    let tps = fit_default(&source, &target);

    // All control-point residuals should be < 1e-5
    let residuals = tps.compute_residuals(&target);
    let max_residual = residuals.iter().cloned().fold(0.0f64, f64::max);
    assert!(max_residual < 1e-5, "Max residual: {max_residual}");

    // Specific control point (400, 600):
    // d = (-100, 100), r^2 = 20000, factor = 1.02
    // expected = (500,500) + (-100,100)*1.02 = (398, 602)
    let test_s = DVec2::new(400.0, 600.0);
    let d = test_s - center;
    let r2 = d.length_squared(); // (-100)^2 + 100^2 = 20000
    let expected = center + d * (1.0 + k * r2);
    // expected = (500 + (-100)*1.02, 500 + 100*1.02) = (398.0, 602.0)
    assert!((expected.x - 398.0).abs() < 1e-6, "hand-check x");
    assert!((expected.y - 602.0).abs() < 1e-6, "hand-check y");
    let t = tps.transform(test_s);
    assert_dvec2_near(t, expected, 1e-5, "barrel control point");

    // Interior point (450, 550) -- between grid nodes at 100px spacing.
    // d = (-50, 50), r^2 = 5000, factor = 1.005
    // expected = (500 + (-50)*1.005, 500 + 50*1.005) = (449.75, 550.25)
    let test_mid = DVec2::new(450.0, 550.0);
    let d_mid = test_mid - center;
    let r2_mid = d_mid.length_squared(); // 2500+2500 = 5000
    let expected_mid = center + d_mid * (1.0 + k * r2_mid);
    assert!((expected_mid.x - 449.75).abs() < 1e-6, "hand-check mid x");
    assert!((expected_mid.y - 550.25).abs() < 1e-6, "hand-check mid y");
    // TPS interpolation at midpoint with 100px grid should be accurate within ~0.1px
    let t_mid = tps.transform(test_mid);
    assert_dvec2_near(t_mid, expected_mid, 0.5, "barrel midpoint");
}

/// Large deformation: control points are still interpolated exactly.
#[test]
fn test_tps_large_deformation() {
    let source = square_source_5();
    let target = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(120.0, 20.0),
        DVec2::new(20.0, 120.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(60.0, 40.0),
    ];

    let tps = fit_default(&source, &target);
    assert_control_points_exact(&tps, &source, &target, 1e-5);

    // Non-affine deformation must produce nonzero bending energy.
    // Compare with identity (zero energy) to ensure it's actually different.
    let tps_identity = fit_default(&source, &source);
    let energy_deformed = tps.bending_energy().abs();
    let energy_identity = tps_identity.bending_energy().abs();
    assert!(
        energy_deformed > energy_identity + 0.001,
        "Deformed energy ({energy_deformed}) should exceed identity ({energy_identity})"
    );
}

// ============================================================================
// Regularization
// ============================================================================

/// Regularized TPS has lower bending energy but higher residuals than exact.
#[test]
fn test_tps_regularization_energy_vs_residuals() {
    let source = square_source_5();
    let target = vec![
        DVec2::new(2.0, 1.0),
        DVec2::new(98.0, 3.0),
        DVec2::new(1.0, 102.0),
        DVec2::new(103.0, 99.0),
        DVec2::new(51.0, 49.0),
    ];

    // Exact interpolation (lambda=0)
    let tps_exact = fit_default(&source, &target);
    let residuals_exact = tps_exact.compute_residuals(&target);
    let max_res_exact = residuals_exact.iter().cloned().fold(0.0f64, f64::max);

    // Regularized (lambda=100)
    let config_reg = TpsConfig {
        regularization: 100.0,
    };
    let tps_reg = ThinPlateSpline::fit(&source, &target, config_reg).unwrap();
    let residuals_reg = tps_reg.compute_residuals(&target);
    let max_res_reg = residuals_reg.iter().cloned().fold(0.0f64, f64::max);

    // Exact should have near-zero residuals
    assert!(max_res_exact < 1e-6, "Exact max residual: {max_res_exact}");

    // Regularized should have larger residuals (it doesn't pass through points)
    assert!(
        max_res_reg > max_res_exact,
        "Regularized residuals ({max_res_reg}) should exceed exact ({max_res_exact})"
    );

    // Regularized should have lower bending energy
    let energy_exact = tps_exact.bending_energy().abs();
    let energy_reg = tps_reg.bending_energy().abs();
    assert!(
        energy_reg < energy_exact,
        "Regularized energy ({energy_reg}) should be less than exact ({energy_exact})"
    );
}

/// Increasing regularization monotonically decreases bending energy.
#[test]
fn test_tps_regularization_monotonic_energy() {
    let source = square_source_5();
    let target = vec![
        DVec2::new(3.0, -2.0),
        DVec2::new(97.0, 4.0),
        DVec2::new(-1.0, 103.0),
        DVec2::new(104.0, 98.0),
        DVec2::new(48.0, 52.0),
    ];

    let lambdas = [0.0, 1.0, 10.0, 100.0, 1000.0];
    let energies: Vec<f64> = lambdas
        .iter()
        .map(|&lambda| {
            let config = TpsConfig {
                regularization: lambda,
            };
            let tps = ThinPlateSpline::fit(&source, &target, config).unwrap();
            tps.bending_energy().abs()
        })
        .collect();

    // Each successive energy should be <= the previous
    for i in 1..energies.len() {
        assert!(
            energies[i] <= energies[i - 1] + 1e-10,
            "Energy at lambda={} ({}) should be <= energy at lambda={} ({})",
            lambdas[i],
            energies[i],
            lambdas[i - 1],
            energies[i - 1]
        );
    }

    // The highest regularization should have noticeably less energy than lambda=0
    assert!(
        energies[4] < energies[0] * 0.5,
        "Lambda=1000 energy ({}) should be much less than lambda=0 ({})",
        energies[4],
        energies[0]
    );
}

// ============================================================================
// Bending energy properties
// ============================================================================

/// Identity, translation, and rotation are all affine => zero bending energy.
/// Non-affine deformation produces nonzero bending energy.
#[test]
fn test_tps_bending_energy_affine_vs_nonaffine() {
    let source = square_source_4();

    // Identity
    let tps_id = fit_default(&source, &source);
    assert!(
        tps_id.bending_energy().abs() < 1e-10,
        "Identity energy: {}",
        tps_id.bending_energy()
    );

    // Translation by (50, 30)
    let target_trans: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(50.0, 30.0)).collect();
    let tps_trans = fit_default(&source, &target_trans);
    assert!(
        tps_trans.bending_energy().abs() < 1e-6,
        "Translation energy: {}",
        tps_trans.bending_energy()
    );

    // Rotation by 0.3 radians
    let angle: f64 = 0.3;
    let (sin_a, cos_a) = angle.sin_cos();
    let target_rot: Vec<DVec2> = source
        .iter()
        .map(|&p| DVec2::new(p.x * cos_a - p.y * sin_a, p.x * sin_a + p.y * cos_a))
        .collect();
    let tps_rot = fit_default(&source, &target_rot);
    assert!(
        tps_rot.bending_energy().abs() < 1e-5,
        "Rotation energy: {}",
        tps_rot.bending_energy()
    );

    // Non-affine (5 points with center moved disproportionately)
    let source5 = square_source_5();
    let target_nonaffine = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(60.0, 40.0), // Center pushed to (60,40) -- not affine
    ];
    let tps_na = fit_default(&source5, &target_nonaffine);
    let energy_na = tps_na.bending_energy().abs();
    assert!(
        energy_na > 1e-4,
        "Non-affine should have positive bending energy: {energy_na}"
    );
}

// ============================================================================
// Numerical stability
// ============================================================================

/// Large coordinate offset: normalization ensures correct interpolation.
#[test]
fn test_tps_large_coordinates() {
    let offset = 10_000.0;
    let source = vec![
        DVec2::new(offset, offset),
        DVec2::new(offset + 100.0, offset),
        DVec2::new(offset, offset + 100.0),
        DVec2::new(offset + 100.0, offset + 100.0),
    ];
    let shift = DVec2::new(5.0, 3.0);
    let target: Vec<DVec2> = source.iter().map(|&p| p + shift).collect();

    let tps = fit_default(&source, &target);
    assert_control_points_exact(&tps, &source, &target, 1e-6);

    // Interior: translation is affine, so midpoint should be exact
    // (10050, 10050) + (5, 3) = (10055, 10053)
    let t = tps.transform(DVec2::new(offset + 50.0, offset + 50.0));
    assert_dvec2_near(
        t,
        DVec2::new(offset + 55.0, offset + 53.0),
        1e-3,
        "large offset interior",
    );
}

/// Extreme coordinate offset (100k): still works thanks to normalization.
#[test]
fn test_tps_extreme_coordinates() {
    let offset = 100_000.0;
    let source = vec![
        DVec2::new(offset, offset),
        DVec2::new(offset + 100.0, offset),
        DVec2::new(offset, offset + 100.0),
        DVec2::new(offset + 100.0, offset + 100.0),
        DVec2::new(offset + 50.0, offset + 50.0),
    ];

    // Non-trivial distortion: barrel + translation
    let center = DVec2::new(offset + 50.0, offset + 50.0);
    let k = 0.00001;
    let shift = DVec2::new(3.0, -2.0);
    let target: Vec<DVec2> = source
        .iter()
        .map(|&p| {
            let d = p - center;
            let r2 = d.length_squared();
            p + d * (k * r2) + shift
        })
        .collect();

    let tps = fit_default(&source, &target);

    // Control points should be exact
    let residuals = tps.compute_residuals(&target);
    let max_residual = residuals.iter().cloned().fold(0.0f64, f64::max);
    assert!(max_residual < 1e-5, "Max residual: {max_residual}");

    // Verify a specific control point by hand:
    // source[0] = (100000, 100000), d = (-50, -50), r2 = 5000
    // target[0] = (100000, 100000) + (-50, -50)*0.05 + (3, -2) = (99997.5 + 3, 99997.5 - 2) = (100000.5, 99995.5)
    // Wait, d*(k*r2) = (-50,-50)*(0.00001*5000) = (-50,-50)*0.05 = (-2.5, -2.5)
    // target[0] = (100000 - 2.5 + 3, 100000 - 2.5 - 2) = (100000.5, 99995.5)
    let expected_0 = DVec2::new(100000.5, 99995.5);
    assert_dvec2_near(target[0], expected_0, 1e-6, "hand-check target[0]");
    let t0 = tps.transform(source[0]);
    assert_dvec2_near(t0, expected_0, 1e-4, "extreme coord point 0");
}

/// Clustered points: two groups far apart, pure translation.
#[test]
fn test_tps_clustered_points() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(1000.0, 1000.0),
        DVec2::new(1010.0, 1000.0),
        DVec2::new(1000.0, 1010.0),
        DVec2::new(1010.0, 1010.0),
    ];
    let shift = DVec2::new(5.0, 3.0);
    let target: Vec<DVec2> = source.iter().map(|&p| p + shift).collect();

    let tps = fit_default(&source, &target);
    assert_control_points_exact(&tps, &source, &target, 1e-4);

    // Between clusters: pure translation is affine, so (500,500) -> (505,503).
    let t = tps.transform(DVec2::new(500.0, 500.0));
    assert_dvec2_near(
        t,
        DVec2::new(505.0, 503.0),
        1.0, // TPS extrapolation between distant clusters may be slightly off
        "between clusters",
    );
}

// ============================================================================
// transform_points (batch)
// ============================================================================

/// Batch transform must produce identical results to single transform, and
/// both must produce correct absolute values.
#[test]
fn test_tps_transform_points_consistency_and_correctness() {
    let source = square_source_4();
    let shift = DVec2::new(10.0, 5.0);
    let target: Vec<DVec2> = source.iter().map(|&p| p + shift).collect();

    let tps = fit_default(&source, &target);

    let test_points = vec![
        DVec2::new(25.0, 25.0),
        DVec2::new(75.0, 25.0),
        DVec2::new(25.0, 75.0),
        DVec2::new(75.0, 75.0),
    ];
    let transformed = tps.transform_points(&test_points);

    assert_eq!(transformed.len(), 4);

    // Expected values for translation by (10, 5):
    let expected = [
        DVec2::new(35.0, 30.0),
        DVec2::new(85.0, 30.0),
        DVec2::new(35.0, 80.0),
        DVec2::new(85.0, 80.0),
    ];

    for (i, ((&orig, &trans), &exp)) in test_points
        .iter()
        .zip(transformed.iter())
        .zip(expected.iter())
        .enumerate()
    {
        // Batch vs single: must be exactly identical
        let single = tps.transform(orig);
        assert_dvec2_near(trans, single, 1e-10, &format!("batch vs single {i}"));

        // Absolute correctness
        assert_dvec2_near(trans, exp, 1e-3, &format!("absolute value {i}"));
    }
}

/// transform_points on empty slice returns empty vec.
#[test]
fn test_tps_transform_points_empty() {
    let source = square_source_4();
    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(1.0, 1.0)).collect();
    let tps = fit_default(&source, &target);

    let result = tps.transform_points(&[]);
    assert!(result.is_empty());
}

// ============================================================================
// compute_residuals
// ============================================================================

/// With exact interpolation (lambda=0), all residuals should be near zero.
#[test]
fn test_tps_compute_residuals_exact() {
    let source = square_source_5();
    let target = vec![
        DVec2::new(5.0, 3.0),
        DVec2::new(102.0, 1.0),
        DVec2::new(2.0, 98.0),
        DVec2::new(105.0, 103.0),
        DVec2::new(52.0, 51.0),
    ];

    let tps = fit_default(&source, &target);
    let residuals = tps.compute_residuals(&target);

    assert_eq!(residuals.len(), 5);
    for (i, &r) in residuals.iter().enumerate() {
        assert!(r < 1e-6, "Residual {i}: {r}");
    }
}

// ============================================================================
// num_control_points and control_points accessors
// ============================================================================

#[test]
fn test_tps_accessors() {
    let source = square_source_5();
    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(1.0, 2.0)).collect();
    let tps = fit_default(&source, &target);

    assert_eq!(tps.num_control_points(), 5);
    assert_eq!(tps.control_points().len(), 5);
}

// ============================================================================
// Many control points
// ============================================================================

/// Dense grid with deterministic perturbation: verify exact interpolation.
#[test]
fn test_tps_many_points() {
    let mut source = Vec::new();
    let mut target = Vec::new();

    for y in (0..=500).step_by(50) {
        for x in (0..=500).step_by(50) {
            let s = DVec2::new(x as f64, y as f64);
            source.push(s);
            // Deterministic perturbation: dx = 2*sin(x/100), dy = 2*cos(y/100)
            let dx = (s.x * 0.01).sin() * 2.0;
            let dy = (s.y * 0.01).cos() * 2.0;
            target.push(s + DVec2::new(dx, dy));
        }
    }

    let tps = fit_default(&source, &target);
    assert_eq!(tps.num_control_points(), source.len());

    // All residuals should be small
    let residuals = tps.compute_residuals(&target);
    let max_residual = residuals.iter().cloned().fold(0.0f64, f64::max);
    assert!(max_residual < 1e-4, "Max residual: {max_residual}");

    // Spot-check a specific control point: (200, 300)
    // dx = 2*sin(2.0) = 2*0.909297... = 1.81859...
    // dy = 2*cos(3.0) = 2*(-0.98999...) = -1.97998...
    let idx = source
        .iter()
        .position(|&p| p == DVec2::new(200.0, 300.0))
        .unwrap();
    let expected_dx = 2.0 * (2.0_f64).sin(); // 1.8185948536...
    let expected_dy = 2.0 * (3.0_f64).cos(); // -1.9799849932...
    let t = tps.transform(DVec2::new(200.0, 300.0));
    assert!(
        (t.x - (200.0 + expected_dx)).abs() < 1e-4,
        "Spot x: {} expected {}",
        t.x,
        200.0 + expected_dx
    );
    assert!(
        (t.y - (300.0 + expected_dy)).abs() < 1e-4,
        "Spot y: {} expected {}",
        t.y,
        300.0 + expected_dy
    );
    let _ = idx; // used for documentation
}

// ============================================================================
// DistortionMap tests
// ============================================================================

/// DistortionMap from pure translation: grid dimensions, vectors, and statistics.
#[test]
fn test_distortion_map_translation() {
    let source = square_source_4();
    let shift = DVec2::new(5.0, 3.0);
    let target: Vec<DVec2> = source.iter().map(|&p| p + shift).collect();

    let tps = fit_default(&source, &target);
    let map = DistortionMap::from_tps(&tps, 100, 100, 20.0);

    // Grid dimensions: ceil(100/20) + 1 = 6
    assert_eq!(map.width, 6);
    assert_eq!(map.height, 6);
    assert!((map.spacing - 20.0).abs() < 1e-12);

    // For pure translation, every grid point should have distortion ~(5, 3)
    for gy in 0..map.height {
        for gx in 0..map.width {
            let d = map.get(gx, gy).unwrap();
            assert_dvec2_near(d, shift, 0.1, &format!("grid ({gx},{gy})"));
        }
    }

    // Mean magnitude should be sqrt(5^2 + 3^2) = sqrt(34) = 5.83095...
    let expected_mag = 34.0_f64.sqrt();
    assert!(
        (map.mean_magnitude - expected_mag).abs() < 0.1,
        "Mean magnitude: {} expected {}",
        map.mean_magnitude,
        expected_mag
    );

    // Max magnitude should also be ~sqrt(34) for uniform translation
    assert!(
        (map.max_magnitude - expected_mag).abs() < 0.1,
        "Max magnitude: {} expected {}",
        map.max_magnitude,
        expected_mag
    );
}

/// DistortionMap::get returns None for out-of-bounds indices.
#[test]
fn test_distortion_map_get_out_of_bounds() {
    let source = square_source_4();
    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(1.0, 1.0)).collect();
    let tps = fit_default(&source, &target);
    let map = DistortionMap::from_tps(&tps, 100, 100, 50.0);

    // Valid: within grid
    assert!(map.get(0, 0).is_some());

    // Invalid: beyond grid dimensions
    assert!(map.get(map.width, 0).is_none());
    assert!(map.get(0, map.height).is_none());
    assert!(map.get(map.width, map.height).is_none());
    assert!(map.get(1000, 1000).is_none());
}

/// DistortionMap::interpolate with bilinear on translation: exact at grid and mid-points.
#[test]
fn test_distortion_map_interpolation() {
    let source = square_source_4();
    let shift = DVec2::new(10.0, 5.0);
    let target: Vec<DVec2> = source.iter().map(|&p| p + shift).collect();

    let tps = fit_default(&source, &target);
    let map = DistortionMap::from_tps(&tps, 100, 100, 25.0);

    // At grid point (0, 0): should be ~(10, 5)
    let d0 = map.interpolate(DVec2::new(0.0, 0.0));
    assert_dvec2_near(d0, shift, 0.5, "interp at grid point");

    // At midpoint between grid nodes (37.5, 62.5):
    // For uniform translation, bilinear interpolation of constant field = constant
    let d_mid = map.interpolate(DVec2::new(37.5, 62.5));
    assert_dvec2_near(d_mid, shift, 0.5, "interp at midpoint");
}

/// DistortionMap with non-uniform distortion: verify gradient.
#[test]
fn test_distortion_map_non_uniform_gradient() {
    let mut source = Vec::new();
    let mut target = Vec::new();

    for y in (0..=200).step_by(50) {
        for x in (0..=200).step_by(50) {
            let s = DVec2::new(x as f64, y as f64);
            source.push(s);
            // Distortion: dx = 0.05*x, dy = 0.02*y
            // At x=0: dx=0. At x=200: dx=10.
            // At y=0: dy=0. At y=200: dy=4.
            let dx = s.x * 0.05;
            let dy = s.y * 0.02;
            target.push(s + DVec2::new(dx, dy));
        }
    }

    let tps = fit_default(&source, &target);
    let map = DistortionMap::from_tps(&tps, 200, 200, 25.0);

    // At (25, 100): dx = 25*0.05 = 1.25, dy = 100*0.02 = 2.0
    let d_left = map.interpolate(DVec2::new(25.0, 100.0));
    // At (175, 100): dx = 175*0.05 = 8.75, dy = 100*0.02 = 2.0
    let d_right = map.interpolate(DVec2::new(175.0, 100.0));

    assert!(
        d_right.x > d_left.x,
        "Right side dx ({}) should exceed left ({}) because distortion grows with x",
        d_right.x,
        d_left.x
    );

    // Check approximate magnitudes
    assert!(
        (d_left.x - 1.25).abs() < 0.5,
        "Left dx: {} expected ~1.25",
        d_left.x
    );
    assert!(
        (d_right.x - 8.75).abs() < 0.5,
        "Right dx: {} expected ~8.75",
        d_right.x
    );
}

// ============================================================================
// Parameter sensitivity
// ============================================================================

/// Different translations produce different transforms. Verifies that the
/// TPS model actually uses the target points and doesn't produce a fixed output.
#[test]
fn test_tps_different_translations_produce_different_results() {
    let source = square_source_4();

    let target_a: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(10.0, 0.0)).collect();
    let target_b: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(0.0, 10.0)).collect();

    let tps_a = fit_default(&source, &target_a);
    let tps_b = fit_default(&source, &target_b);

    let test_pt = DVec2::new(50.0, 50.0);
    let result_a = tps_a.transform(test_pt);
    let result_b = tps_b.transform(test_pt);

    // A translates in x: (50,50) -> (60,50)
    assert_dvec2_near(result_a, DVec2::new(60.0, 50.0), 1e-3, "translation A");
    // B translates in y: (50,50) -> (50,60)
    assert_dvec2_near(result_b, DVec2::new(50.0, 60.0), 1e-3, "translation B");

    // Results must differ
    assert!(
        (result_a.x - result_b.x).abs() > 1.0,
        "Different translations should produce different x: A={}, B={}",
        result_a.x,
        result_b.x
    );
}

/// Adding a 5th control point that breaks affinity changes interpolation at intermediate points.
#[test]
fn test_tps_extra_control_point_changes_behavior() {
    let source_4 = square_source_4();
    let target_4: Vec<DVec2> = source_4.iter().map(|&p| p + DVec2::new(5.0, 3.0)).collect();
    let tps_4 = fit_default(&source_4, &target_4);

    // Same 4 corners but add center with a different displacement
    let mut source_5 = source_4.clone();
    source_5.push(DVec2::new(50.0, 50.0));
    let mut target_5 = target_4.clone();
    target_5.push(DVec2::new(65.0, 63.0)); // (50,50) -> (65,63), not (55,53)

    let tps_5 = fit_default(&source_5, &target_5);

    // At an interior point near the center, the 5-point model should differ from the 4-point model
    let test_pt = DVec2::new(45.0, 55.0);
    let r4 = tps_4.transform(test_pt);
    let r5 = tps_5.transform(test_pt);

    // 4-point model is pure translation: (45,55) -> (50,58)
    assert_dvec2_near(r4, DVec2::new(50.0, 58.0), 0.5, "4-point affine");

    // 5-point model pushes toward the extra control point's displacement
    // So x should be > 50 and the results should differ
    assert!(
        (r5.x - r4.x).abs() > 1.0 || (r5.y - r4.y).abs() > 1.0,
        "Extra control point should change behavior: 4pt=({}, {}), 5pt=({}, {})",
        r4.x,
        r4.y,
        r5.x,
        r5.y,
    );
}

// ============================================================================
// TpsConfig
// ============================================================================

/// Default config has zero regularization.
#[test]
fn test_tps_config_default() {
    let config = TpsConfig::default();
    assert_eq!(config.regularization, 0.0);
}
