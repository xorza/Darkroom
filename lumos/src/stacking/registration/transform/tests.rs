use crate::stacking::registration::transform::*;
use crate::testing::prelude::*;
use std::f64::consts::PI;

const EPSILON: f64 = 1e-10;

#[test]
fn transform_type_min_points() {
    assert_eq!(TransformType::Translation.min_points(), 1);
    assert_eq!(TransformType::Euclidean.min_points(), 2);
    assert_eq!(TransformType::Similarity.min_points(), 2);
    assert_eq!(TransformType::Affine.min_points(), 3);
    assert_eq!(TransformType::Homography.min_points(), 4);
}

#[test]
fn identity_transform() {
    let t = Transform::identity();
    let p = t.apply(DVec2::new(5.0, 7.0));
    assert_close!(p.x, 5.0, EPSILON);
    assert_close!(p.y, 7.0, EPSILON);
}

#[test]
fn translation_transform() {
    let t = Transform::translation(DVec2::new(10.0, -5.0));
    let p = t.apply(DVec2::new(3.0, 4.0));
    assert_close!(p.x, 13.0, EPSILON);
    assert_close!(p.y, -1.0, EPSILON);
}

#[test]
fn rotation_90_degrees() {
    let t = Transform::euclidean(DVec2::ZERO, PI / 2.0);
    let p = t.apply(DVec2::new(1.0, 0.0));
    assert_close!(p.x, 0.0, EPSILON);
    assert_close!(p.y, 1.0, EPSILON);
}

#[test]
fn rotation_180_degrees() {
    let t = Transform::euclidean(DVec2::ZERO, PI);
    let p = t.apply(DVec2::new(1.0, 0.0));
    assert_close!(p.x, -1.0, EPSILON);
    assert_close!(p.y, 0.0, EPSILON);
}

#[test]
fn scale_transform() {
    let t = Transform::similarity(DVec2::ZERO, 0.0, 2.0);
    let p = t.apply(DVec2::new(3.0, 4.0));
    assert_close!(p.x, 6.0, EPSILON);
    assert_close!(p.y, 8.0, EPSILON);
}

#[test]
fn similarity_with_rotation_and_scale() {
    let t = Transform::similarity(DVec2::new(5.0, 10.0), PI / 2.0, 2.0);
    let p = t.apply(DVec2::new(1.0, 0.0));
    // Rotate 90° then scale 2x: (1,0) -> (0,1) -> (0,2), then translate
    assert_close!(p.x, 5.0, EPSILON);
    assert_close!(p.y, 12.0, EPSILON);
}

#[test]
fn affine_transform() {
    // Shear transform
    let t = Transform::affine([1.0, 0.5, 0.0, 0.0, 1.0, 0.0]);
    let p = t.apply(DVec2::new(2.0, 2.0));
    assert_close!(p.x, 3.0, EPSILON); // 2 + 0.5*2
    assert_close!(p.y, 2.0, EPSILON);
}

#[test]
fn translation_components_return_the_offset_built_in() {
    let t = Transform::translation(DVec2::new(7.0, -3.0));
    let tc = t.translation_components();
    assert_close!(tc.x, 7.0, EPSILON);
    assert_close!(tc.y, -3.0, EPSILON);
}

#[test]
fn rotation_angle_recovers_the_euclidean_angle() {
    let angle = 0.5;
    let t = Transform::euclidean(DVec2::ZERO, angle);
    assert_close!(t.rotation_angle(), angle, EPSILON);
}

#[test]
fn scale_factor_recovers_the_similarity_scale() {
    let scale = 2.5;
    let t = Transform::similarity(DVec2::ZERO, 0.0, scale);
    assert_close!(t.scale_factor(), scale, EPSILON);
}

#[test]
fn is_valid_rejects_degenerate_and_singular_matrices() {
    let valid = Transform::similarity(DVec2::new(1.0, 2.0), 0.5, 1.5);
    assert!(valid.is_valid());

    // Degenerate matrix (zero scale)
    let degenerate = Transform::from_matrix(
        DMat3::from_array([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
        TransformType::Affine,
    );
    assert!(!degenerate.is_valid());

    // The upper-left 2×2 block is nonsingular, but the complete projective
    // matrix has duplicate rows and therefore no inverse.
    let singular_homography = Transform::homography([1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
    assert!(!singular_homography.is_valid());
}

#[test]
fn homography_transform() {
    // Simple homography that acts like translation
    let t = Transform::homography([1.0, 0.0, 5.0, 0.0, 1.0, 3.0, 0.0, 0.0]);
    let p = t.apply(DVec2::new(2.0, 4.0));
    assert_close!(p.x, 7.0, EPSILON);
    assert_close!(p.y, 7.0, EPSILON);
}

#[test]
fn homography_perspective() {
    // Homography with perspective component
    let t = Transform::homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.001, 0.0]);
    let p = t.apply(DVec2::new(100.0, 0.0));
    // w = 0.001 * 100 + 1 = 1.1
    // x' = 100 / 1.1 ≈ 90.9
    assert!((p.x - 90.909).abs() < 0.01);
    assert_close!(p.y, 0.0, EPSILON);
}

#[test]
fn warp_transform_new() {
    let t = Transform::translation(DVec2::new(10.0, 5.0));
    let wt = WarpTransform::new(t);
    assert!(!wt.has_sip());
    assert!(wt.is_linear());

    let p = wt.apply(DVec2::new(1.0, 2.0));
    assert_close!(p.x, 11.0, EPSILON);
    assert_close!(p.y, 7.0, EPSILON);
}

#[test]
fn warp_transform_with_sip() {
    use crate::stacking::registration::distortion::sip::{SipConfig, SipPolynomial};

    let transform = Transform::identity();

    // Create a simple SIP from synthetic points with barrel distortion
    let cx = 50.0;
    let cy = 50.0;
    let k = 1e-4;
    let mut ref_pts = Vec::new();
    let mut tgt_pts = Vec::new();
    for gy in 0..10 {
        for gx in 0..10 {
            let rx = 5.0 + gx as f64 * 10.0;
            let ry = 5.0 + gy as f64 * 10.0;
            let dx = rx - cx;
            let dy = ry - cy;
            let r2 = dx * dx + dy * dy;
            ref_pts.push(DVec2::new(rx, ry));
            tgt_pts.push(DVec2::new(rx + k * dx * r2, ry + k * dy * r2));
        }
    }
    let sip_config = SipConfig {
        order: 3,
        reference_point: Some(DVec2::new(cx, cy)),
        ..Default::default()
    };
    let sip = SipPolynomial::fit_from_transform(&ref_pts, &tgt_pts, &transform, &sip_config)
        .unwrap()
        .polynomial;

    let wt = WarpTransform::with_sip(transform, sip);
    assert!(wt.has_sip());
    assert!(!wt.is_linear());

    // Corner point should differ from identity
    let corner = DVec2::new(0.0, 0.0);
    let result = wt.apply(corner);
    let no_sip = WarpTransform::new(transform).apply(corner);
    assert!(
        (result - no_sip).length() > 0.01,
        "SIP should produce different coordinates"
    );
}

#[test]
fn warp_transform_is_linear() {
    // Translation: linear
    let wt = WarpTransform::new(Transform::translation(DVec2::new(1.0, 2.0)));
    assert!(wt.is_linear());

    // Euclidean: linear
    let wt = WarpTransform::new(Transform::euclidean(DVec2::ZERO, 0.1));
    assert!(wt.is_linear());

    // Similarity: linear
    let wt = WarpTransform::new(Transform::similarity(DVec2::ZERO, 0.1, 1.02));
    assert!(wt.is_linear());

    // Affine: linear
    let wt = WarpTransform::new(Transform::affine([1.0, 0.0, 5.0, 0.0, 1.0, 3.0]));
    assert!(wt.is_linear());

    // Homography: not linear
    let wt = WarpTransform::new(Transform::homography([
        1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.001, 0.0,
    ]));
    assert!(!wt.is_linear());
}

#[test]
fn warp_transform_apply_no_sip_matches_transform() {
    let t = Transform::similarity(DVec2::new(3.0, -2.0), 0.5, 1.1);
    let wt = WarpTransform::new(t);

    for &p in &[
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 50.0),
        DVec2::new(-10.0, 200.0),
    ] {
        let from_wt = wt.apply(p);
        let from_t = t.apply(p);
        assert_close!(from_wt.x, from_t.x, EPSILON);
        assert_close!(from_wt.y, from_t.y, EPSILON);
    }
}

#[test]
fn auto_sizes_its_gates_against_the_model_it_can_climb_to() {
    // The ladder ends at Homography, so every count `Auto` is measured by has to be Homography's
    // — anything smaller would let a pair through that the last rung cannot fit. Previously
    // `TransformType::Auto` answered Similarity's 2 here while every caller substituted
    // Homography's 4, and only the fact that nothing called it kept the two from disagreeing.
    assert_eq!(
        TransformModel::Auto.most_general(),
        TransformType::Homography
    );
    assert_eq!(TransformModel::Auto.most_general().min_points(), 4);
    // A fixed choice is its own ceiling.
    for transform_type in [
        TransformType::Translation,
        TransformType::Euclidean,
        TransformType::Similarity,
        TransformType::Affine,
        TransformType::Homography,
    ] {
        assert_eq!(
            TransformModel::Fixed(transform_type).most_general(),
            transform_type
        );
    }
}

#[test]
fn default_is_identity() {
    let t = Transform::default();
    let p = t.apply(DVec2::new(42.0, -17.0));
    // Identity: output == input
    assert_close!(p.x, 42.0, EPSILON);
    assert_close!(p.y, -17.0, EPSILON);
    assert_eq!(t.transform_type(), TransformType::Translation);
}

#[test]
fn scale_constructor() {
    // Transform::scale(DVec2::new(sx, sy)) creates a diagonal scaling matrix
    // Matrix: [sx, 0, 0, 0, sy, 0, 0, 0, 1]
    // (3.0, 4.0) -> (3.0*2.0, 4.0*0.5) = (6.0, 2.0)
    let t = Transform::scale(DVec2::new(2.0, 0.5));
    let p = t.apply(DVec2::new(3.0, 4.0));
    assert_close!(p.x, 6.0, EPSILON);
    assert_close!(p.y, 2.0, EPSILON);
    assert_eq!(t.transform_type(), TransformType::Affine);
}

#[test]
fn rotation_around_center() {
    // Rotate 90 degrees around center (100, 100)
    // Point (150, 100) is 50 units right of center
    // After 90° CCW rotation: should be 50 units above center -> (100, 150)
    //
    // Math: T(-cx,-cy) * R(90°) * T(cx,cy)
    // Relative to center: (50, 0)
    // After R(90°): (0, 50)
    // Back to absolute: (100, 150)
    let center = DVec2::new(100.0, 100.0);
    let t = Transform::rotation_around(center, PI / 2.0);

    let p = t.apply(DVec2::new(150.0, 100.0));
    assert!((p.x - 100.0).abs() < EPSILON, "Expected x=100, got {}", p.x);
    assert!((p.y - 150.0).abs() < EPSILON, "Expected y=150, got {}", p.y);

    // The center itself should remain fixed
    let center_mapped = t.apply(center);
    assert!(
        (center_mapped.x - 100.0).abs() < EPSILON,
        "Center should be fixed, got x={}",
        center_mapped.x
    );
    assert!(
        (center_mapped.y - 100.0).abs() < EPSILON,
        "Center should be fixed, got y={}",
        center_mapped.y
    );
}

#[test]
fn inverse_roundtrip_translation() {
    // T(10, -5) then T^{-1} should give back the original point
    let t = Transform::translation(DVec2::new(10.0, -5.0));
    let p = DVec2::new(3.0, 7.0);

    // apply then apply_inverse should return original
    let mapped = t.apply(p);
    let recovered = t.apply_inverse(mapped);
    assert!(
        (recovered.x - p.x).abs() < EPSILON,
        "Roundtrip x: expected {}, got {}",
        p.x,
        recovered.x
    );
    assert!(
        (recovered.y - p.y).abs() < EPSILON,
        "Roundtrip y: expected {}, got {}",
        p.y,
        recovered.y
    );
}

#[test]
fn inverse_roundtrip_similarity() {
    // Similarity with rotation and scale: roundtrip should recover original
    let t = Transform::similarity(DVec2::new(7.0, -3.0), 0.7, 1.3);
    let p = DVec2::new(100.0, 200.0);

    let mapped = t.apply(p);
    let recovered = t.apply_inverse(mapped);
    assert!(
        (recovered.x - p.x).abs() < 1e-8,
        "Roundtrip x: expected {}, got {}",
        p.x,
        recovered.x
    );
    assert!(
        (recovered.y - p.y).abs() < 1e-8,
        "Roundtrip y: expected {}, got {}",
        p.y,
        recovered.y
    );
}

#[test]
fn compose_translation_translation() {
    // Composing T(3,4) * T(5,-2) should give T(8,2)
    // compose(other) = self * other, apply other first then self
    let t1 = Transform::translation(DVec2::new(3.0, 4.0));
    let t2 = Transform::translation(DVec2::new(5.0, -2.0));

    let composed = t1.compose(&t2);
    let p = composed.apply(DVec2::new(0.0, 0.0));
    // (0,0) -> T2 -> (5,-2) -> T1 -> (5+3, -2+4) = (8, 2)
    assert_close!(p.x, 8.0, EPSILON);
    assert_close!(p.y, 2.0, EPSILON);
}

#[test]
fn compose_takes_more_complex_type() {
    // When composing Translation * Affine, result should be Affine
    let t1 = Transform::translation(DVec2::new(1.0, 2.0));
    let t2 = Transform::affine([1.0, 0.5, 0.0, 0.0, 1.0, 0.0]);

    let composed = t1.compose(&t2);
    assert_eq!(composed.transform_type(), TransformType::Affine);

    // Verify the composed transform: apply T2 first (shear), then T1 (translate)
    // T2: (2,2) -> (2 + 0.5*2, 2) = (3, 2)
    // T1: (3,2) -> (3+1, 2+2) = (4, 4)
    let p = composed.apply(DVec2::new(2.0, 2.0));
    assert_close!(p.x, 4.0, EPSILON);
    assert_close!(p.y, 4.0, EPSILON);
}

#[test]
fn compose_rotation_then_translation() {
    // Rotate 90° then translate by (10, 0)
    let rot = Transform::euclidean(DVec2::ZERO, PI / 2.0);
    let trans = Transform::translation(DVec2::new(10.0, 0.0));

    // trans.compose(rot): apply rot first, then trans
    let composed = trans.compose(&rot);
    // (1, 0) -> rot 90° -> (0, 1) -> translate -> (10, 1)
    let p = composed.apply(DVec2::new(1.0, 0.0));
    assert!((p.x - 10.0).abs() < EPSILON, "Expected x=10, got {}", p.x);
    assert!((p.y - 1.0).abs() < EPSILON, "Expected y=1, got {}", p.y);
}

#[test]
fn deviation_from_identity_is_the_frobenius_norm_of_the_difference() {
    // Identity has zero deviation
    let id = Transform::identity();
    assert_close!(id.deviation_from_identity(), 0.0, EPSILON);

    // Translation has non-zero deviation
    let t = Transform::translation(DVec2::new(3.0, 4.0));
    // Deviation is Frobenius norm of (M - I)
    // M - I = [[0,0,3],[0,0,4],[0,0,0]]
    // Frobenius = sqrt(9 + 16) = sqrt(25) = 5.0
    let dev = t.deviation_from_identity();
    assert!(
        (dev - 5.0).abs() < EPSILON,
        "Expected deviation 5.0, got {}",
        dev
    );
}

#[test]
fn homography_perspective_hand_computed() {
    // Homography: h = [1, 0, 10, 0, 1, 20, 0.002, 0.001]
    // For point (200, 100):
    // w = 0.002*200 + 0.001*100 + 1 = 0.4 + 0.1 + 1 = 1.5
    // x' = (1*200 + 0*100 + 10) / 1.5 = 210 / 1.5 = 140.0
    // y' = (0*200 + 1*100 + 20) / 1.5 = 120 / 1.5 = 80.0
    let t = Transform::homography([1.0, 0.0, 10.0, 0.0, 1.0, 20.0, 0.002, 0.001]);
    let p = t.apply(DVec2::new(200.0, 100.0));
    assert!((p.x - 140.0).abs() < EPSILON, "Expected x=140, got {}", p.x);
    assert!((p.y - 80.0).abs() < EPSILON, "Expected y=80, got {}", p.y);
}

#[test]
fn display_translation() {
    let t = Transform::translation(DVec2::new(10.5, -3.2));
    let s = format!("{}", t);
    assert_eq!(s, "Translation(dx=10.50, dy=-3.20)");
}

#[test]
fn display_euclidean() {
    let t = Transform::euclidean(DVec2::new(5.0, -2.0), 0.0);
    let s = format!("{}", t);
    // rotation_angle() = atan2(sin_a, cos_a) = atan2(0, 1) = 0
    assert_eq!(s, "Euclidean(dx=5.00, dy=-2.00, rot=0.000\u{b0})");
}

#[test]
fn display_similarity() {
    let t = Transform::similarity(DVec2::new(1.0, 2.0), 0.0, 1.5);
    let s = format!("{}", t);
    assert_eq!(
        s,
        "Similarity(dx=1.00, dy=2.00, rot=0.000\u{b0}, scale=1.5000)"
    );
}

#[test]
#[should_panic(expected = "Cannot invert singular transform matrix")]
fn inverse_singular_panics() {
    let degenerate = Transform::affine([0.0; 6]);
    let _ = degenerate.inverse();
}

#[test]
fn from_matrix_keeps_the_matrix_its_type_and_its_mapping() {
    let m = DMat3::from_array([2.0, 0.0, 5.0, 0.0, 3.0, -1.0, 0.0, 0.0, 1.0]);
    let t = Transform::from_matrix(m, TransformType::Affine);
    // (1, 1) -> (2*1 + 0*1 + 5, 0*1 + 3*1 + (-1)) = (7, 2)
    let p = t.apply(DVec2::new(1.0, 1.0));
    assert_close!(p.x, 7.0, EPSILON);
    assert_close!(p.y, 2.0, EPSILON);
    assert_eq!(t.transform_type(), TransformType::Affine);
    assert_eq!(t.matrix(), m.as_array());
}

#[test]
#[should_panic(expected = "affine-or-simpler transforms require homogeneous bottom row [0, 0, 1]")]
fn from_matrix_rejects_a_projective_bottom_row() {
    let projective = DMat3::from_array([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.01, 0.0, 1.0]);
    Transform::from_matrix(projective, TransformType::Affine);
}

#[test]
fn from_matrix_canonicalizes_affine_roundoff() {
    let rounded = DMat3::from_array([
        1.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        f64::EPSILON,
        -f64::EPSILON,
        1.0 + f64::EPSILON,
    ]);
    let transform = Transform::from_matrix(rounded, TransformType::Affine);
    assert_eq!(&transform.matrix()[6..], &[0.0, 0.0, 1.0]);
}
