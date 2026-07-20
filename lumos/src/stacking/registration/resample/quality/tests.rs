use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::resample::kernel;
use crate::stacking::registration::resample::quality;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use common::Vec2us;
use glam::{DVec2, Vec2};

const TOL: f32 = 1e-5;
const INTERPOLATION_METHODS: [InterpolationMethod; 6] = [
    InterpolationMethod::Nearest,
    InterpolationMethod::Bilinear,
    InterpolationMethod::Bicubic,
    InterpolationMethod::Lanczos2,
    InterpolationMethod::Lanczos3,
    InterpolationMethod::Lanczos4,
];

#[test]
fn warp_coverage_nearest_identity_is_all_ones() {
    let (w, h) = (8, 8);
    let wt = WarpTransform::new(Transform::identity());
    let cov = quality::maps(Vec2us::new(w, h), &wt, InterpolationMethod::Nearest).coverage;
    for &c in cov.pixels() {
        assert!(
            (c - 1.0).abs() < TOL,
            "nearest identity coverage should be 1.0, got {c}"
        );
    }
}

#[test]
fn warp_coverage_fully_outside_is_zero() {
    let (w, h) = (8, 8);
    // Source translated far outside the image: every kernel tap is out of bounds.
    let wt = WarpTransform::new(Transform::translation(DVec2::new(1000.0, 1000.0)));
    let cov = quality::maps(Vec2us::new(w, h), &wt, InterpolationMethod::Bilinear).coverage;
    for &c in cov.pixels() {
        assert_eq!(c, 0.0, "fully-outside coverage must be 0, got {c}");
    }
}

#[test]
fn warp_coverage_bilinear_edge_is_partial() {
    let (w, h) = (8, 8);
    // Output (0,4) maps to src (-0.5, 4.0): the 2×2 bilinear footprint straddles the left
    // edge — taps at x=-1 (out, weight 0.5) and x=0 (in, weight 0.5) → coverage 0.5.
    let wt = WarpTransform::new(Transform::translation(DVec2::new(-0.5, 0.0)));
    let cov = quality::maps(Vec2us::new(w, h), &wt, InterpolationMethod::Bilinear).coverage;
    let edge = cov.pixels()[4 * w];
    assert!(
        (edge - 0.5).abs() < TOL,
        "left-edge bilinear coverage should be 0.5, got {edge}"
    );
    // An interior output pixel maps fully in bounds → coverage 1.0.
    let interior = cov.pixels()[4 * w + 4];
    assert!(
        (interior - 1.0).abs() < TOL,
        "interior coverage should be 1.0, got {interior}"
    );
}

#[test]
fn bilinear_quality_has_hand_computed_support_and_confidence() {
    let dims = Vec2us::new(8, 8);
    let interior = quality::quality_at(Vec2::new(0.5, 4.0), dims, InterpolationMethod::Bilinear);
    assert!((interior.coverage - 1.0).abs() < TOL);
    // Coefficients [0.5, 0.5] have variance gain 0.5, so inverse variance is 2.
    assert!((interior.confidence - 2.0).abs() < TOL);

    let edge = quality::quality_at(Vec2::new(-0.5, 4.0), dims, InterpolationMethod::Bilinear);
    assert!((edge.coverage - 0.5).abs() < TOL);
    // Renormalization leaves the sole in-bounds coefficient equal to one.
    assert!((edge.confidence - 1.0).abs() < TOL);
}

#[test]
fn source_footprint_boundary_is_inclusive() {
    let dims = Vec2us::new(8, 6);
    for position in [
        Vec2::new(-0.5, 2.0),
        Vec2::new(7.5, 2.0),
        Vec2::new(3.0, -0.5),
        Vec2::new(3.0, 5.5),
    ] {
        assert!(
            kernel::source_footprint_contains(position, dims),
            "{position:?}"
        );
    }
    for position in [
        Vec2::new(-0.5001, 2.0),
        Vec2::new(7.5001, 2.0),
        Vec2::new(3.0, -0.5001),
        Vec2::new(3.0, 5.5001),
    ] {
        assert!(
            !kernel::source_footprint_contains(position, dims),
            "{position:?}"
        );
        for method in INTERPOLATION_METHODS {
            let quality = quality::quality_at(position, dims, method);
            assert_eq!(quality.coverage, 0.0, "{method:?} at {position:?}");
            assert_eq!(quality.confidence, 0.0, "{method:?} at {position:?}");
        }
    }
}

#[test]
fn coverage_is_continuous_and_monotonic_across_left_border() {
    let dims = Vec2us::new(32, 32);
    for method in INTERPOLATION_METHODS {
        let radius = method.kernel_radius() as i32;
        let mut previous = 0.0;
        for integer in -radius..=radius {
            let coverage =
                quality::quality_at(Vec2::new(integer as f32 + 0.37, 16.0), dims, method).coverage;
            assert!(
                coverage + 1e-6 >= previous,
                "{method:?}: coverage decreased from {previous} to {coverage} at x={integer}"
            );
            previous = coverage;
        }
        assert!((previous - 1.0).abs() < TOL, "{method:?}: {previous}");

        if method != InterpolationMethod::Nearest {
            let left = quality::quality_at(Vec2::new(-1e-4, 16.0), dims, method).coverage;
            let right = quality::quality_at(Vec2::new(1e-4, 16.0), dims, method).coverage;
            assert!(
                (left - right).abs() < 1e-3,
                "{method:?}: discontinuity across x=0: {left} vs {right}"
            );
        }
    }
}
