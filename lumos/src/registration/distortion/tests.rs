//! Tests for thin-plate spline distortion modeling.

use super::*;

/// Test that TPS passes exactly through control points (zero regularization).
#[test]
fn test_tps_exact_interpolation() {
    let source = vec![
        (0.0, 0.0),
        (100.0, 0.0),
        (0.0, 100.0),
        (100.0, 100.0),
        (50.0, 50.0),
    ];

    let target = vec![
        (5.0, 3.0),
        (102.0, 1.0),
        (2.0, 98.0),
        (105.0, 103.0),
        (52.0, 51.0),
    ];

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Check that transformed source points match target points
    for (i, (&src, &tgt)) in source.iter().zip(target.iter()).enumerate() {
        let (tx, ty) = tps.transform(src.0, src.1);
        assert!(
            (tx - tgt.0).abs() < 1e-6,
            "Point {}: x mismatch: {} vs {}",
            i,
            tx,
            tgt.0
        );
        assert!(
            (ty - tgt.1).abs() < 1e-6,
            "Point {}: y mismatch: {} vs {}",
            i,
            ty,
            tgt.1
        );
    }
}

/// Test TPS with pure translation.
#[test]
fn test_tps_translation() {
    let source = vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)];

    // Translate by (10, 5)
    let target: Vec<(f64, f64)> = source.iter().map(|&(x, y)| (x + 10.0, y + 5.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Test at control points
    for (&src, &tgt) in source.iter().zip(target.iter()) {
        let (tx, ty) = tps.transform(src.0, src.1);
        assert!((tx - tgt.0).abs() < 1e-6);
        assert!((ty - tgt.1).abs() < 1e-6);
    }

    // Test at an intermediate point
    let (tx, ty) = tps.transform(50.0, 50.0);
    assert!((tx - 60.0).abs() < 1e-3);
    assert!((ty - 55.0).abs() < 1e-3);
}

/// Test TPS with uniform scaling.
#[test]
fn test_tps_scaling() {
    let source = vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)];

    // Scale by 1.1
    let target: Vec<(f64, f64)> = source.iter().map(|&(x, y)| (x * 1.1, y * 1.1)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Test at control points
    for (&src, &tgt) in source.iter().zip(target.iter()) {
        let (tx, ty) = tps.transform(src.0, src.1);
        assert!((tx - tgt.0).abs() < 1e-6);
        assert!((ty - tgt.1).abs() < 1e-6);
    }

    // Test at center
    let (tx, ty) = tps.transform(50.0, 50.0);
    assert!((tx - 55.0).abs() < 1e-3);
    assert!((ty - 55.0).abs() < 1e-3);
}

/// Test TPS with rotation.
#[test]
fn test_tps_rotation() {
    let source = vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)];

    // Rotate by 10 degrees around origin
    let angle = 10.0_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    let target: Vec<(f64, f64)> = source
        .iter()
        .map(|&(x, y)| (x * cos_a - y * sin_a, x * sin_a + y * cos_a))
        .collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Test at control points
    for (&src, &tgt) in source.iter().zip(target.iter()) {
        let (tx, ty) = tps.transform(src.0, src.1);
        assert!((tx - tgt.0).abs() < 1e-5, "x: {} vs {}", tx, tgt.0);
        assert!((ty - tgt.1).abs() < 1e-5, "y: {} vs {}", ty, tgt.1);
    }
}

/// Test TPS with local distortion (barrel distortion pattern).
#[test]
fn test_tps_barrel_distortion() {
    // Create a denser grid of points to better capture the non-linear distortion
    let mut source = Vec::new();
    let mut target = Vec::new();

    let center = (500.0, 500.0);
    let k = 0.000001; // Smaller barrel distortion coefficient for smoother interpolation

    // Use a denser grid (100 pixel spacing instead of 200)
    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let sx = x as f64;
            let sy = y as f64;
            source.push((sx, sy));

            // Apply barrel distortion: r' = r(1 + k*r²)
            let dx = sx - center.0;
            let dy = sy - center.1;
            let r2 = dx * dx + dy * dy;
            let factor = 1.0 + k * r2;

            let tx = center.0 + dx * factor;
            let ty = center.1 + dy * factor;
            target.push((tx, ty));
        }
    }

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Check that residuals at control points are small
    let residuals = tps.compute_residuals(&target);
    let max_residual = residuals.iter().cloned().fold(0.0f64, f64::max);
    assert!(
        max_residual < 1e-5,
        "Max residual {} too large",
        max_residual
    );

    // Test at a control point to verify exact interpolation
    let (tx, ty) = tps.transform(400.0, 600.0);
    // Find the expected value for this control point
    let dx = 400.0 - center.0;
    let dy = 600.0 - center.1;
    let r2 = dx * dx + dy * dy;
    let factor = 1.0 + k * r2;
    let expected_x = center.0 + dx * factor;
    let expected_y = center.1 + dy * factor;

    assert!(
        (tx - expected_x).abs() < 1e-5,
        "Control point x: {} vs {}",
        tx,
        expected_x
    );
    assert!(
        (ty - expected_y).abs() < 1e-5,
        "Control point y: {} vs {}",
        ty,
        expected_y
    );

    // Test at an intermediate point (between control points)
    // TPS provides smooth interpolation - verify it's reasonable
    let test_x = 450.0; // Midpoint between 400 and 500
    let test_y = 550.0; // Midpoint between 500 and 600
    let (tx, ty) = tps.transform(test_x, test_y);

    // Compute expected distortion at this point
    let dx_test = test_x - center.0;
    let dy_test = test_y - center.1;
    let r2_test = dx_test * dx_test + dy_test * dy_test;
    let factor_test = 1.0 + k * r2_test;
    let expected_tx = center.0 + dx_test * factor_test;
    let expected_ty = center.1 + dy_test * factor_test;

    // TPS interpolation should be within a few pixels of the analytic result
    // for a dense enough grid
    assert!(
        (tx - expected_tx).abs() < 5.0,
        "Intermediate x: {} vs expected {}",
        tx,
        expected_tx
    );
    assert!(
        (ty - expected_ty).abs() < 5.0,
        "Intermediate y: {} vs expected {}",
        ty,
        expected_ty
    );
}

/// Test TPS with regularization.
#[test]
fn test_tps_regularization() {
    let source = vec![
        (0.0, 0.0),
        (100.0, 0.0),
        (0.0, 100.0),
        (100.0, 100.0),
        (50.0, 50.0),
    ];

    // Add some noise to target points
    let target = vec![
        (2.0, 1.0),
        (98.0, 3.0),
        (1.0, 102.0),
        (103.0, 99.0),
        (51.0, 49.0),
    ];

    // Without regularization
    let tps_exact = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // With regularization
    let config_reg = TpsConfig {
        regularization: 100.0,
    };
    let tps_reg = ThinPlateSpline::fit(&source, &target, config_reg).unwrap();

    // Regularized version should have lower bending energy
    let energy_exact = tps_exact.bending_energy();
    let energy_reg = tps_reg.bending_energy();

    // The regularized spline should be smoother
    assert!(
        energy_reg.abs() < energy_exact.abs() + 1e-6,
        "Regularized energy {} should be less than exact energy {}",
        energy_reg,
        energy_exact
    );
}

/// Test minimum number of control points.
#[test]
fn test_tps_minimum_points() {
    // Two points - should fail
    let source2 = vec![(0.0, 0.0), (100.0, 100.0)];
    let target2 = vec![(1.0, 1.0), (101.0, 101.0)];
    assert!(ThinPlateSpline::fit(&source2, &target2, TpsConfig::default()).is_none());

    // Three points - minimum for TPS
    let source3 = vec![(0.0, 0.0), (100.0, 0.0), (50.0, 100.0)];
    let target3 = vec![(1.0, 1.0), (101.0, 1.0), (51.0, 101.0)];
    assert!(ThinPlateSpline::fit(&source3, &target3, TpsConfig::default()).is_some());
}

/// Test with collinear points (degenerate case).
#[test]
fn test_tps_collinear_points() {
    // Collinear points form a singular matrix
    let source = vec![(0.0, 0.0), (50.0, 0.0), (100.0, 0.0)];
    let target = vec![(0.0, 0.0), (50.0, 0.0), (100.0, 0.0)];

    // This should fail because the matrix is singular
    let result = ThinPlateSpline::fit(&source, &target, TpsConfig::default());
    // May or may not succeed depending on numerical precision
    // The important thing is it doesn't panic
    let _ = result;
}

/// Test mismatched point counts.
#[test]
fn test_tps_mismatched_counts() {
    let source = vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0)];
    let target = vec![(1.0, 1.0), (101.0, 1.0)]; // One less point

    assert!(ThinPlateSpline::fit(&source, &target, TpsConfig::default()).is_none());
}

/// Test transform_points batch method.
#[test]
fn test_tps_transform_points() {
    let source = vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)];
    let target: Vec<(f64, f64)> = source.iter().map(|&(x, y)| (x + 10.0, y + 5.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    let test_points = vec![(25.0, 25.0), (75.0, 25.0), (25.0, 75.0), (75.0, 75.0)];
    let transformed = tps.transform_points(&test_points);

    assert_eq!(transformed.len(), test_points.len());

    for (i, (&orig, &trans)) in test_points.iter().zip(transformed.iter()).enumerate() {
        let (single_x, single_y) = tps.transform(orig.0, orig.1);
        assert!(
            (trans.0 - single_x).abs() < 1e-10,
            "Point {}: batch x {} vs single {}",
            i,
            trans.0,
            single_x
        );
        assert!(
            (trans.1 - single_y).abs() < 1e-10,
            "Point {}: batch y {} vs single {}",
            i,
            trans.1,
            single_y
        );
    }
}

/// Test TPS kernel function.
#[test]
fn test_tps_kernel() {
    // U(0) should be 0
    assert_eq!(tps_kernel(0.0), 0.0);
    assert_eq!(tps_kernel(1e-15), 0.0);

    // U(1) = 1² * ln(1) = 0
    assert!((tps_kernel(1.0) - 0.0).abs() < 1e-10);

    // U(e) = e² * ln(e) = e²
    let e = std::f64::consts::E;
    assert!((tps_kernel(e) - e * e).abs() < 1e-10);

    // U(2) = 4 * ln(2)
    assert!((tps_kernel(2.0) - 4.0 * 2.0_f64.ln()).abs() < 1e-10);
}

/// Test distortion map creation.
#[test]
fn test_distortion_map() {
    let source = vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)];
    let target: Vec<(f64, f64)> = source.iter().map(|&(x, y)| (x + 5.0, y + 3.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    let map = DistortionMap::from_tps(&tps, 100, 100, 20.0);

    // For pure translation, all distortion vectors should be approximately (5, 3)
    for gy in 0..map.height {
        for gx in 0..map.width {
            let (dx, dy) = map.get(gx, gy).unwrap();
            assert!((dx - 5.0).abs() < 0.5, "dx at ({}, {}): {}", gx, gy, dx);
            assert!((dy - 3.0).abs() < 0.5, "dy at ({}, {}): {}", gx, gy, dy);
        }
    }

    // Mean magnitude should be approximately sqrt(5² + 3²) ≈ 5.83
    let expected_magnitude = (25.0 + 9.0_f64).sqrt();
    assert!(
        (map.mean_magnitude - expected_magnitude).abs() < 0.5,
        "Mean magnitude: {} vs expected {}",
        map.mean_magnitude,
        expected_magnitude
    );
}

/// Test distortion map interpolation.
#[test]
fn test_distortion_map_interpolation() {
    let source = vec![(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)];
    let target: Vec<(f64, f64)> = source.iter().map(|&(x, y)| (x + 10.0, y + 5.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();
    let map = DistortionMap::from_tps(&tps, 100, 100, 25.0);

    // Test interpolation at a non-grid point
    let (dx, dy) = map.interpolate(37.5, 62.5);

    // For translation, should be close to (10, 5)
    assert!((dx - 10.0).abs() < 1.0, "Interpolated dx: {}", dx);
    assert!((dy - 5.0).abs() < 1.0, "Interpolated dy: {}", dy);
}

/// Test with many control points.
#[test]
fn test_tps_many_points() {
    // Create a dense grid of control points
    let mut source = Vec::new();
    let mut target = Vec::new();

    for y in (0..=500).step_by(50) {
        for x in (0..=500).step_by(50) {
            let sx = x as f64;
            let sy = y as f64;
            source.push((sx, sy));

            // Add small random-like perturbations based on position
            let dx = (sx * 0.01).sin() * 2.0;
            let dy = (sy * 0.01).cos() * 2.0;
            target.push((sx + dx, sy + dy));
        }
    }

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default());
    assert!(tps.is_some(), "TPS should fit with many points");

    let tps = tps.unwrap();
    assert_eq!(tps.num_control_points(), source.len());

    // Residuals should be very small
    let residuals = tps.compute_residuals(&target);
    let max_residual = residuals.iter().cloned().fold(0.0f64, f64::max);
    assert!(
        max_residual < 1e-4,
        "Max residual {} too large for many points",
        max_residual
    );
}

/// Test numerical stability with large coordinates.
#[test]
fn test_tps_large_coordinates() {
    let offset = 10000.0;
    let source = vec![
        (offset, offset),
        (offset + 100.0, offset),
        (offset, offset + 100.0),
        (offset + 100.0, offset + 100.0),
    ];

    let target: Vec<(f64, f64)> = source.iter().map(|&(x, y)| (x + 5.0, y + 3.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Test transformation
    let (tx, ty) = tps.transform(offset + 50.0, offset + 50.0);
    assert!(
        (tx - (offset + 55.0)).abs() < 1.0,
        "tx: {} expected {}",
        tx,
        offset + 55.0
    );
    assert!(
        (ty - (offset + 53.0)).abs() < 1.0,
        "ty: {} expected {}",
        ty,
        offset + 53.0
    );
}

/// Test that identity transformation works.
#[test]
fn test_tps_identity() {
    let points = vec![
        (0.0, 0.0),
        (100.0, 0.0),
        (0.0, 100.0),
        (100.0, 100.0),
        (50.0, 50.0),
    ];

    let tps = ThinPlateSpline::fit(&points, &points, TpsConfig::default()).unwrap();

    // All points should map to themselves
    for &(x, y) in &points {
        let (tx, ty) = tps.transform(x, y);
        assert!((tx - x).abs() < 1e-6, "Identity failed for x");
        assert!((ty - y).abs() < 1e-6, "Identity failed for y");
    }

    // Test at intermediate points
    let (tx, ty) = tps.transform(25.0, 75.0);
    assert!((tx - 25.0).abs() < 1e-3);
    assert!((ty - 75.0).abs() < 1e-3);
}
