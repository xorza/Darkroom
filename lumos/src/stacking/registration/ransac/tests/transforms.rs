use crate::stacking::registration::ransac::tests::*;

#[test]
fn test_estimate_translation_hand_computed() {
    // Translation is the average displacement.
    // ref: (0,0), (10,0), (0,10), (10,10)
    // target: (5,-3), (15,-3), (5,7), (15,7)  (offset +5, -3)
    // avg displacement = ((5+5+5+5)/4, (-3-3-3-3)/4) = (5, -3)
    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];
    let target_points: Vec<DVec2> = ref_points
        .iter()
        .map(|p| *p + DVec2::new(5.0, -3.0))
        .collect();

    let t = estimate_transform(&ref_points, &target_points, TransformType::Translation).unwrap();
    let d = t.translation_components();

    assert!((d.x - 5.0).abs() < TOL);
    assert!((d.y - (-3.0)).abs() < TOL);
}

#[test]
fn test_estimate_translation_insufficient_points() {
    let result = estimate_transform(&[], &[], TransformType::Translation);
    assert!(result.is_none());
}

#[test]
fn test_estimate_euclidean_hand_computed() {
    // Euclidean: rotation + translation, scale = 1
    let angle = PI / 12.0; // 15 degrees
    let t = DVec2::new(5.0, -3.0);

    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let known = Transform::euclidean(t, angle);
    let target_points = apply_all(&known, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Euclidean).unwrap();

    assert!(
        (estimated.rotation_angle() - angle).abs() < 1e-10,
        "angle: expected {}, got {}",
        angle,
        estimated.rotation_angle()
    );
    assert!(
        (estimated.scale_factor() - 1.0).abs() < 1e-10,
        "scale: expected 1.0, got {}",
        estimated.scale_factor()
    );

    let est_t = estimated.translation_components();
    assert!((est_t.x - t.x).abs() < 1e-10);
    assert!((est_t.y - t.y).abs() < 1e-10);
}

#[test]
fn test_estimate_euclidean_insufficient_points() {
    let result = estimate_transform(
        &[DVec2::new(0.0, 0.0)],
        &[DVec2::new(1.0, 1.0)],
        TransformType::Euclidean,
    );
    assert!(result.is_none());
}

#[test]
fn test_estimate_euclidean_ignores_scale() {
    // When data has inherent scale != 1, Euclidean estimator should still produce scale=1.
    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let angle = PI / 6.0; // 30 degrees
    let sim = Transform::similarity(DVec2::new(3.0, -2.0), angle, 1.1);
    let target_points = apply_all(&sim, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Euclidean).unwrap();

    // Scale must be exactly 1.0 (Euclidean constraint)
    assert!(
        (estimated.scale_factor() - 1.0).abs() < 1e-10,
        "Euclidean scale must be 1.0, got {}",
        estimated.scale_factor()
    );

    // Rotation should still be close to the true angle
    assert!(
        (estimated.rotation_angle() - angle).abs() < 0.05,
        "Rotation: expected ~{}, got {}",
        angle,
        estimated.rotation_angle()
    );
}

#[test]
fn test_estimate_similarity_hand_computed() {
    let angle = PI / 6.0; // 30 degrees
    let scale = 1.5;
    let t = DVec2::new(20.0, -10.0);

    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let known = Transform::similarity(t, angle, scale);
    let target_points = apply_all(&known, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Similarity).unwrap();

    assert!(
        (estimated.rotation_angle() - angle).abs() < 1e-10,
        "angle: expected {}, got {}",
        angle,
        estimated.rotation_angle()
    );
    assert!(
        (estimated.scale_factor() - scale).abs() < 1e-10,
        "scale: expected {}, got {}",
        scale,
        estimated.scale_factor()
    );

    let est_t = estimated.translation_components();
    assert!((est_t.x - t.x).abs() < 1e-9);
    assert!((est_t.y - t.y).abs() < 1e-9);
}

#[test]
fn test_estimate_similarity_insufficient_points() {
    let result = estimate_transform(
        &[DVec2::new(0.0, 0.0)],
        &[DVec2::new(1.0, 1.0)],
        TransformType::Similarity,
    );
    assert!(result.is_none());
}

#[test]
fn test_estimate_affine_hand_computed() {
    // Affine: [a,b,tx,c,d,ty] → x' = a*x + b*y + tx, y' = c*x + d*y + ty
    // [1.2, 0.3, 5.0, -0.1, 0.9, -3.0]
    let params = [1.2, 0.3, 5.0, -0.1, 0.9, -3.0];
    let known = Transform::affine(params);

    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
    ];

    let target_points = apply_all(&known, &ref_points);

    let estimated = estimate_transform(&ref_points, &target_points, TransformType::Affine).unwrap();

    // Verify each point maps correctly
    // (0,0) → (1.2*0+0.3*0+5, -0.1*0+0.9*0-3) = (5, -3)
    // (10,0) → (12+0+5, -1+0-3) = (17, -4)
    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(
            (pp.x - tp.x).abs() < 1e-8 && (pp.y - tp.y).abs() < 1e-8,
            "At ({},{}): expected ({},{}), got ({},{})",
            rp.x,
            rp.y,
            tp.x,
            tp.y,
            pp.x,
            pp.y
        );
    }
}

#[test]
fn test_estimate_affine_insufficient_points() {
    let ref_pts = [DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    let tar_pts = [DVec2::new(1.0, 1.0), DVec2::new(2.0, 1.0)];
    let result = estimate_transform(&ref_pts, &tar_pts, TransformType::Affine);
    assert!(result.is_none());
}

#[test]
fn test_estimate_affine_ill_conditioned() {
    // Points spanning a large range -- stresses numerical stability.
    // Without Hartley normalization, normal equations A^T A become ill-conditioned
    // (condition number κ(A^T A) ≈ κ(A)²) and precision degrades.
    let ref_points = [
        DVec2::new(0.01, 0.02),
        DVec2::new(5000.0, 0.01),
        DVec2::new(5000.0, 4000.0),
        DVec2::new(0.01, 4000.0),
        DVec2::new(2500.0, 2000.0),
        DVec2::new(1000.0, 3000.0),
        DVec2::new(4000.0, 1000.0),
        DVec2::new(100.0, 100.0),
    ];

    // Affine: x' = 1.05*x + 0.02*y + 10.0, y' = -0.01*x + 0.98*y + 5.0
    let known = Transform::affine([1.05, 0.02, 10.0, -0.01, 0.98, 5.0]);
    let target_points = apply_all(&known, &ref_points);

    let estimated = estimate_transform(&ref_points, &target_points, TransformType::Affine).unwrap();

    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(
            (pp.x - tp.x).abs() < 1e-6 && (pp.y - tp.y).abs() < 1e-6,
            "At ({},{:.1}): expected ({:.4},{:.4}), got ({:.4},{:.4})",
            rp.x,
            rp.y,
            tp.x,
            tp.y,
            pp.x,
            pp.y,
        );
    }
}

#[test]
fn test_estimate_homography_hand_computed() {
    let ref_points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(25.0, 75.0),
    ];

    let known = Transform::homography([1.1, 0.1, 5.0, -0.05, 1.0, 3.0, 0.0001, 0.00005]);
    let target_points = apply_all(&known, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Homography).unwrap();

    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(
            (pp.x - tp.x).abs() < 0.5 && (pp.y - tp.y).abs() < 0.5,
            "At ({},{}): expected ({:.4},{:.4}), got ({:.4},{:.4})",
            rp.x,
            rp.y,
            tp.x,
            tp.y,
            pp.x,
            pp.y
        );
    }
}

#[test]
fn test_estimate_homography_insufficient_points() {
    let ref_pts = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
    ];
    let tar_pts = [
        DVec2::new(1.0, 1.0),
        DVec2::new(2.0, 1.0),
        DVec2::new(1.0, 2.0),
    ];
    let result = estimate_transform(&ref_pts, &tar_pts, TransformType::Homography);
    assert!(result.is_none());
}

#[test]
fn test_estimate_homography_ill_conditioned() {
    // Points spanning a large range -- stresses numerical stability
    let ref_points = [
        DVec2::new(0.01, 0.02),
        DVec2::new(5000.0, 0.01),
        DVec2::new(5000.0, 4000.0),
        DVec2::new(0.01, 4000.0),
        DVec2::new(2500.0, 2000.0),
        DVec2::new(1000.0, 3000.0),
        DVec2::new(4000.0, 1000.0),
        DVec2::new(100.0, 100.0),
    ];

    let known = Transform::homography([1.05, 0.02, 10.0, -0.01, 0.98, 5.0, 1e-5, -2e-5]);
    let target_points = apply_all(&known, &ref_points);

    let estimated =
        estimate_transform(&ref_points, &target_points, TransformType::Homography).unwrap();

    for (&rp, &tp) in ref_points.iter().zip(target_points.iter()) {
        let pp = estimated.apply(rp);
        assert!(
            (pp.x - tp.x).abs() < 0.5 && (pp.y - tp.y).abs() < 0.5,
            "At ({},{:.1}): expected ({:.4},{:.4}), got ({:.4},{:.4})",
            rp.x,
            rp.y,
            tp.x,
            tp.y,
            pp.x,
            pp.y,
        );
    }
}
