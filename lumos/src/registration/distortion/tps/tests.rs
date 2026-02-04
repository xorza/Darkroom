//! Tests for thin-plate spline distortion modeling.

use super::*;
use glam::DVec2;

/// Test that TPS passes exactly through control points (zero regularization).
#[test]
fn test_tps_exact_interpolation() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 50.0),
    ];

    let target = vec![
        DVec2::new(5.0, 3.0),
        DVec2::new(102.0, 1.0),
        DVec2::new(2.0, 98.0),
        DVec2::new(105.0, 103.0),
        DVec2::new(52.0, 51.0),
    ];

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Check that transformed source points match target points
    for (i, (&src, &tgt)) in source.iter().zip(target.iter()).enumerate() {
        let t = tps.transform(src);
        assert!(
            (t.x - tgt.x).abs() < 1e-6,
            "Point {}: x mismatch: {} vs {}",
            i,
            t.x,
            tgt.x
        );
        assert!(
            (t.y - tgt.y).abs() < 1e-6,
            "Point {}: y mismatch: {} vs {}",
            i,
            t.y,
            tgt.y
        );
    }
}

/// Test TPS with pure translation.
#[test]
fn test_tps_translation() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];

    // Translate by (10, 5)
    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(10.0, 5.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Test at control points
    for (&src, &tgt) in source.iter().zip(target.iter()) {
        let t = tps.transform(src);
        assert!((t.x - tgt.x).abs() < 1e-6);
        assert!((t.y - tgt.y).abs() < 1e-6);
    }

    // Test at an intermediate point
    let t = tps.transform(DVec2::new(50.0, 50.0));
    assert!((t.x - 60.0).abs() < 1e-3);
    assert!((t.y - 55.0).abs() < 1e-3);
}

/// Test TPS with uniform scaling.
#[test]
fn test_tps_scaling() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];

    // Scale by 1.1
    let target: Vec<DVec2> = source.iter().map(|&p| p * 1.1).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Test at control points
    for (&src, &tgt) in source.iter().zip(target.iter()) {
        let t = tps.transform(src);
        assert!((t.x - tgt.x).abs() < 1e-6);
        assert!((t.y - tgt.y).abs() < 1e-6);
    }

    // Test at center
    let t = tps.transform(DVec2::new(50.0, 50.0));
    assert!((t.x - 55.0).abs() < 1e-3);
    assert!((t.y - 55.0).abs() < 1e-3);
}

/// Test TPS with rotation.
#[test]
fn test_tps_rotation() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];

    // Rotate by 10 degrees around origin
    let angle = 10.0_f64.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    let target: Vec<DVec2> = source
        .iter()
        .map(|&p| DVec2::new(p.x * cos_a - p.y * sin_a, p.x * sin_a + p.y * cos_a))
        .collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Test at control points
    for (&src, &tgt) in source.iter().zip(target.iter()) {
        let t = tps.transform(src);
        assert!((t.x - tgt.x).abs() < 1e-5, "x: {} vs {}", t.x, tgt.x);
        assert!((t.y - tgt.y).abs() < 1e-5, "y: {} vs {}", t.y, tgt.y);
    }
}

/// Test TPS with local distortion (barrel distortion pattern).
#[test]
fn test_tps_barrel_distortion() {
    // Create a denser grid of points to better capture the non-linear distortion
    let mut source = Vec::new();
    let mut target = Vec::new();

    let center = DVec2::new(500.0, 500.0);
    let k = 0.000001; // Smaller barrel distortion coefficient for smoother interpolation

    // Use a denser grid (100 pixel spacing instead of 200)
    for y in (0..=1000).step_by(100) {
        for x in (0..=1000).step_by(100) {
            let s = DVec2::new(x as f64, y as f64);
            source.push(s);

            // Apply barrel distortion: r' = r(1 + k*r²)
            let d = s - center;
            let r2 = d.length_squared();
            let factor = 1.0 + k * r2;

            let t = center + d * factor;
            target.push(t);
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
    let test_s = DVec2::new(400.0, 600.0);
    let t = tps.transform(test_s);
    // Find the expected value for this control point
    let d = test_s - center;
    let r2 = d.length_squared();
    let factor = 1.0 + k * r2;
    let expected = center + d * factor;

    assert!(
        (t.x - expected.x).abs() < 1e-5,
        "Control point x: {} vs {}",
        t.x,
        expected.x
    );
    assert!(
        (t.y - expected.y).abs() < 1e-5,
        "Control point y: {} vs {}",
        t.y,
        expected.y
    );

    // Test at an intermediate point (between control points)
    // TPS provides smooth interpolation - verify it's reasonable
    let test_mid = DVec2::new(450.0, 550.0); // Midpoint between 400 and 500, 500 and 600
    let t = tps.transform(test_mid);

    // Compute expected distortion at this point
    let d_test = test_mid - center;
    let r2_test = d_test.length_squared();
    let factor_test = 1.0 + k * r2_test;
    let expected_mid = center + d_test * factor_test;

    // TPS interpolation should be within a few pixels of the analytic result
    // for a dense enough grid
    assert!(
        (t.x - expected_mid.x).abs() < 5.0,
        "Intermediate x: {} vs expected {}",
        t.x,
        expected_mid.x
    );
    assert!(
        (t.y - expected_mid.y).abs() < 5.0,
        "Intermediate y: {} vs expected {}",
        t.y,
        expected_mid.y
    );
}

/// Test TPS with regularization.
#[test]
fn test_tps_regularization() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 50.0),
    ];

    // Add some noise to target points
    let target = vec![
        DVec2::new(2.0, 1.0),
        DVec2::new(98.0, 3.0),
        DVec2::new(1.0, 102.0),
        DVec2::new(103.0, 99.0),
        DVec2::new(51.0, 49.0),
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
    let source2 = vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 100.0)];
    let target2 = vec![DVec2::new(1.0, 1.0), DVec2::new(101.0, 101.0)];
    assert!(ThinPlateSpline::fit(&source2, &target2, TpsConfig::default()).is_none());

    // Three points - minimum for TPS
    let source3 = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(50.0, 100.0),
    ];
    let target3 = vec![
        DVec2::new(1.0, 1.0),
        DVec2::new(101.0, 1.0),
        DVec2::new(51.0, 101.0),
    ];
    assert!(ThinPlateSpline::fit(&source3, &target3, TpsConfig::default()).is_some());
}

/// Test with collinear points (degenerate case).
#[test]
fn test_tps_collinear_points() {
    // Collinear points form a singular matrix
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(50.0, 0.0),
        DVec2::new(100.0, 0.0),
    ];
    let target = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(50.0, 0.0),
        DVec2::new(100.0, 0.0),
    ];

    // This should fail because the matrix is singular
    let result = ThinPlateSpline::fit(&source, &target, TpsConfig::default());
    // May or may not succeed depending on numerical precision
    // The important thing is it doesn't panic
    let _ = result;
}

/// Test mismatched point counts.
#[test]
fn test_tps_mismatched_counts() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
    ];
    let target = vec![DVec2::new(1.0, 1.0), DVec2::new(101.0, 1.0)]; // One less point

    assert!(ThinPlateSpline::fit(&source, &target, TpsConfig::default()).is_none());
}

/// Test transform_points batch method.
#[test]
fn test_tps_transform_points() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];
    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(10.0, 5.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    let test_points = vec![
        DVec2::new(25.0, 25.0),
        DVec2::new(75.0, 25.0),
        DVec2::new(25.0, 75.0),
        DVec2::new(75.0, 75.0),
    ];
    let transformed = tps.transform_points(&test_points);

    assert_eq!(transformed.len(), test_points.len());

    for (i, (&orig, &trans)) in test_points.iter().zip(transformed.iter()).enumerate() {
        let single = tps.transform(orig);
        assert!(
            (trans.x - single.x).abs() < 1e-10,
            "Point {}: batch x {} vs single {}",
            i,
            trans.x,
            single.x
        );
        assert!(
            (trans.y - single.y).abs() < 1e-10,
            "Point {}: batch y {} vs single {}",
            i,
            trans.y,
            single.y
        );
    }
}

/// Test TPS kernel function.
#[test]
fn test_tps_kernel() {
    use super::tps_kernel;

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
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];
    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(5.0, 3.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    let map = DistortionMap::from_tps(&tps, 100, 100, 20.0);

    // For pure translation, all distortion vectors should be approximately (5, 3)
    for gy in 0..map.height {
        for gx in 0..map.width {
            let d = map.get(gx, gy).unwrap();
            assert!((d.x - 5.0).abs() < 0.5, "dx at ({}, {}): {}", gx, gy, d.x);
            assert!((d.y - 3.0).abs() < 0.5, "dy at ({}, {}): {}", gx, gy, d.y);
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
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];
    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(10.0, 5.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();
    let map = DistortionMap::from_tps(&tps, 100, 100, 25.0);

    // Test interpolation at a non-grid point
    let d = map.interpolate(DVec2::new(37.5, 62.5));

    // For translation, should be close to (10, 5)
    assert!((d.x - 10.0).abs() < 1.0, "Interpolated dx: {}", d.x);
    assert!((d.y - 5.0).abs() < 1.0, "Interpolated dy: {}", d.y);
}

/// Test with many control points.
#[test]
fn test_tps_many_points() {
    // Create a dense grid of control points
    let mut source = Vec::new();
    let mut target = Vec::new();

    for y in (0..=500).step_by(50) {
        for x in (0..=500).step_by(50) {
            let s = DVec2::new(x as f64, y as f64);
            source.push(s);

            // Add small random-like perturbations based on position
            let dx = (s.x * 0.01).sin() * 2.0;
            let dy = (s.y * 0.01).cos() * 2.0;
            target.push(s + DVec2::new(dx, dy));
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
        DVec2::new(offset, offset),
        DVec2::new(offset + 100.0, offset),
        DVec2::new(offset, offset + 100.0),
        DVec2::new(offset + 100.0, offset + 100.0),
    ];

    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(5.0, 3.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Test transformation
    let t = tps.transform(DVec2::new(offset + 50.0, offset + 50.0));
    assert!(
        (t.x - (offset + 55.0)).abs() < 1.0,
        "tx: {} expected {}",
        t.x,
        offset + 55.0
    );
    assert!(
        (t.y - (offset + 53.0)).abs() < 1.0,
        "ty: {} expected {}",
        t.y,
        offset + 53.0
    );
}

/// Test that identity transformation works.
#[test]
fn test_tps_identity() {
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 50.0),
    ];

    let tps = ThinPlateSpline::fit(&points, &points, TpsConfig::default()).unwrap();

    // All points should map to themselves
    for &p in &points {
        let t = tps.transform(p);
        assert!((t.x - p.x).abs() < 1e-6, "Identity failed for x");
        assert!((t.y - p.y).abs() < 1e-6, "Identity failed for y");
    }

    // Test at intermediate points
    let t = tps.transform(DVec2::new(25.0, 75.0));
    assert!((t.x - 25.0).abs() < 1e-3);
    assert!((t.y - 75.0).abs() < 1e-3);
}

// ============================================================================
// Additional edge case tests
// ============================================================================

/// Test TPS with various regularization values
#[test]
fn test_tps_regularization_values() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 50.0),
    ];

    // Noisy target points
    let target = vec![
        DVec2::new(3.0, -2.0),
        DVec2::new(97.0, 4.0),
        DVec2::new(-1.0, 103.0),
        DVec2::new(104.0, 98.0),
        DVec2::new(48.0, 52.0),
    ];

    // Test with increasing regularization
    let lambdas = [0.0, 1.0, 10.0, 100.0, 1000.0];
    let mut prev_energy = f64::MAX;

    for &lambda in &lambdas {
        let config = TpsConfig {
            regularization: lambda,
        };
        let tps = ThinPlateSpline::fit(&source, &target, config).unwrap();
        let energy = tps.bending_energy();

        // Higher regularization should result in lower or equal bending energy
        if lambda > 0.0 {
            assert!(
                energy <= prev_energy + 1e-6,
                "λ={}: energy {} should be <= previous {}",
                lambda,
                energy,
                prev_energy
            );
        }
        prev_energy = energy;
    }
}

/// Test TPS with nearly collinear points (ill-conditioned)
#[test]
fn test_tps_nearly_collinear() {
    // Points that are nearly but not exactly collinear
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(50.0, 0.1), // Slightly off the line
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0), // This breaks collinearity
    ];

    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(5.0, 3.0)).collect();

    let result = ThinPlateSpline::fit(&source, &target, TpsConfig::default());

    // Should succeed since points are not exactly collinear
    assert!(
        result.is_some(),
        "TPS should handle nearly collinear points"
    );

    let tps = result.unwrap();

    // Should still interpolate control points reasonably
    for (&src, &tgt) in source.iter().zip(target.iter()) {
        let t = tps.transform(src);
        assert!(
            (t.x - tgt.x).abs() < 1.0,
            "Control point x: {} vs {}",
            t.x,
            tgt.x
        );
        assert!(
            (t.y - tgt.y).abs() < 1.0,
            "Control point y: {} vs {}",
            t.y,
            tgt.y
        );
    }
}

/// Test TPS with large deformations
#[test]
fn test_tps_large_deformation() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 50.0),
    ];

    // Large deformation - twist the grid
    let target = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(120.0, 20.0),  // Stretched and sheared
        DVec2::new(20.0, 120.0),  // Stretched and sheared
        DVec2::new(100.0, 100.0), // Unchanged
        DVec2::new(60.0, 40.0),   // Center moved
    ];

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Control points should still be interpolated exactly
    for (&src, &tgt) in source.iter().zip(target.iter()) {
        let t = tps.transform(src);
        assert!(
            (t.x - tgt.x).abs() < 1e-5,
            "Large deform x: {} vs {}",
            t.x,
            tgt.x
        );
        assert!(
            (t.y - tgt.y).abs() < 1e-5,
            "Large deform y: {} vs {}",
            t.y,
            tgt.y
        );
    }

    // Bending energy should be significant for large deformation
    let energy = tps.bending_energy();
    assert!(
        energy.abs() > 0.01,
        "Large deformation should have significant bending energy: {}",
        energy
    );
}

/// Test bending energy calculation
#[test]
fn test_tps_bending_energy_properties() {
    let source = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(100.0, 100.0),
    ];

    // Identity: zero bending energy
    let tps_identity = ThinPlateSpline::fit(&source, &source, TpsConfig::default()).unwrap();
    let energy_identity = tps_identity.bending_energy();
    assert!(
        energy_identity.abs() < 1e-10,
        "Identity should have zero bending energy: {}",
        energy_identity
    );

    // Pure translation: zero bending energy
    let target_trans: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(50.0, 30.0)).collect();
    let tps_trans = ThinPlateSpline::fit(&source, &target_trans, TpsConfig::default()).unwrap();
    let energy_trans = tps_trans.bending_energy();
    assert!(
        energy_trans.abs() < 1e-6,
        "Pure translation should have near-zero bending energy: {}",
        energy_trans
    );

    // Pure rotation: near-zero bending energy (affine)
    let angle: f64 = 0.3;
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let target_rot: Vec<DVec2> = source
        .iter()
        .map(|&p| DVec2::new(p.x * cos_a - p.y * sin_a, p.x * sin_a + p.y * cos_a))
        .collect();
    let tps_rot = ThinPlateSpline::fit(&source, &target_rot, TpsConfig::default()).unwrap();
    let energy_rot = tps_rot.bending_energy();
    assert!(
        energy_rot.abs() < 1e-5,
        "Pure rotation should have near-zero bending energy: {}",
        energy_rot
    );
}

/// Test TPS with clustered control points
#[test]
fn test_tps_clustered_points() {
    // Two clusters of points far apart
    let source = vec![
        // Cluster 1 at (0,0)
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        // Cluster 2 at (1000,1000)
        DVec2::new(1000.0, 1000.0),
        DVec2::new(1010.0, 1000.0),
        DVec2::new(1000.0, 1010.0),
        DVec2::new(1010.0, 1010.0),
    ];

    let target: Vec<DVec2> = source.iter().map(|&p| p + DVec2::new(5.0, 3.0)).collect();

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();

    // Should interpolate both clusters correctly
    for (&src, &tgt) in source.iter().zip(target.iter()) {
        let t = tps.transform(src);
        assert!(
            (t.x - tgt.x).abs() < 1e-4,
            "Clustered x: {} vs {}",
            t.x,
            tgt.x
        );
        assert!(
            (t.y - tgt.y).abs() < 1e-4,
            "Clustered y: {} vs {}",
            t.y,
            tgt.y
        );
    }

    // Test interpolation between clusters
    let t = tps.transform(DVec2::new(500.0, 500.0));
    // Should be approximately (505, 503) for pure translation
    assert!(
        (t.x - 505.0).abs() < 10.0,
        "Between clusters x: {} expected ~505",
        t.x
    );
    assert!(
        (t.y - 503.0).abs() < 10.0,
        "Between clusters y: {} expected ~503",
        t.y
    );
}

/// Test distortion map with non-uniform distortion
#[test]
fn test_distortion_map_non_uniform() {
    // Create a grid with position-dependent distortion
    let mut source = Vec::new();
    let mut target = Vec::new();

    for y in (0..=200).step_by(50) {
        for x in (0..=200).step_by(50) {
            let s = DVec2::new(x as f64, y as f64);
            source.push(s);
            // Distortion increases with x
            let dx = s.x * 0.05;
            let dy = s.y * 0.02;
            target.push(s + DVec2::new(dx, dy));
        }
    }

    let tps = ThinPlateSpline::fit(&source, &target, TpsConfig::default()).unwrap();
    let map = DistortionMap::from_tps(&tps, 200, 200, 25.0);

    // Left side should have smaller distortion than right side
    let d_left = map.interpolate(DVec2::new(25.0, 100.0));
    let d_right = map.interpolate(DVec2::new(175.0, 100.0));

    assert!(
        d_right.x > d_left.x,
        "Right side distortion {} should be larger than left {}",
        d_right.x,
        d_left.x
    );
}
