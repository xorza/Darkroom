use crate::internals::synthetic::star_profiles::*;
use crate::math::fwhm;
use std::f32::consts::FRAC_PI_2;

const GAUSSIAN_2: StarProfile = StarProfile::gaussian(2.0);

#[test]
fn gaussian_peaks_at_its_centre_and_vanishes_at_the_corner() {
    let mut pixels = Buffer2::new_filled(64, 64, 0.0f32);
    SyntheticStar::new(Vec2::splat(32.0), 1.0, GAUSSIAN_2).add_to(&mut pixels);

    assert!(pixels[(32, 32)] > 0.9, "peak should be near 1.0");
    assert!(pixels[(0, 0)] < 0.001, "corner should be near 0");
}

#[test]
fn gaussian_value_matches_the_closed_form() {
    let star = SyntheticStar::new(Vec2::splat(10.0), 0.8, GAUSSIAN_2);
    // At 2px off-centre with sigma 2: 0.8 * exp(-4/8) = 0.8 * exp(-0.5) = 0.485225...
    let expected = 0.8 * (-0.5f32).exp();
    assert!((star.value_at(12.0, 10.0) - expected).abs() < 1e-6);
    // Radially symmetric: the same offset along y, and along the diagonal at r² = 4.
    assert!((star.value_at(10.0, 8.0) - expected).abs() < 1e-6);
    let diagonal = star.value_at(10.0 + 2.0f32.sqrt(), 10.0 + 2.0f32.sqrt());
    assert!((diagonal - expected).abs() < 1e-6);
}

#[test]
fn truncated_and_exact_agree_inside_the_radius_and_differ_outside() {
    let size = Size2us::new(64, 64);
    let star = SyntheticStar::new(Vec2::splat(32.0), 1.0, GAUSSIAN_2);

    let mut truncated = Buffer2::new_filled(size.width, size.height, 0.0f32);
    star.add_to(&mut truncated);
    let exact = star.stamp(size, 0.0);

    // Inside the 4σ = 8px box the two modes are bit-identical.
    assert_eq!(truncated[(32, 32)], exact[(32, 32)]);
    assert_eq!(truncated[32 * 64 + 39], exact[(32, 39)]);
    // Outside it, truncation drops a small but non-zero wing that `add_exact` keeps.
    assert_eq!(truncated[32 * 64 + 45], 0.0);
    let wing = exact[(32, 45)];
    assert!(
        wing > 0.0 && wing < 1e-3,
        "wing at 13px should be tiny but present, got {wing}"
    );
}

#[test]
fn stamp_lays_the_star_over_a_flat_background() {
    let stamp =
        SyntheticStar::new(Vec2::splat(10.0), 0.5, GAUSSIAN_2).stamp(Size2us::new(21, 21), 0.1);
    // The peak pixel holds the profile's mean over it, at σ 2 m(0) = 4·√(π/2)·erf(1/(4√2)) =
    // 0.989680 per axis: 0.1 + 0.5·m(0)² = 0.589734, to the f32 rounding of the sum.
    assert!((stamp[(10, 10)] - 0.589_733_5).abs() <= 1e-7);
    // A far corner is background plus a negligible wing.
    assert!(stamp[(0, 0)] >= 0.1 && stamp[(0, 0)] < 0.1 + 1e-4);
}

#[test]
fn elliptical_is_elongated_along_its_wider_axis_and_rotates_with_angle() {
    let wide = StarProfile::Gaussian {
        sigma_x: 4.0,
        sigma_y: 2.0,
        angle: 0.0,
    };
    let star = SyntheticStar::new(Vec2::splat(32.0), 1.0, wide);
    // 6px along the wide (x) axis keeps more flux than 6px along the narrow (y) axis.
    assert!(star.value_at(38.0, 32.0) > star.value_at(32.0, 38.0));

    // Rotating by 90° swaps which direction is wide.
    let turned = SyntheticStar::new(
        Vec2::splat(32.0),
        1.0,
        StarProfile::Gaussian {
            sigma_x: 4.0,
            sigma_y: 2.0,
            angle: FRAC_PI_2,
        },
    );
    assert!(turned.value_at(32.0, 38.0) > turned.value_at(38.0, 32.0));
    // The rotation is rigid: the peak and the profile's extent are unchanged.
    assert!((turned.value_at(32.0, 38.0) - star.value_at(38.0, 32.0)).abs() < 1e-6);
}

/// A round Gaussian turned by any angle takes the quadrature, and the same Gaussian unturned
/// the closed form: their pixel means agree to 1e-12 of the peak, so their f32 values agree
/// within one rounding of each, at σ 0.6 and 2.
#[test]
fn the_quadrature_and_the_closed_form_agree() {
    for sigma in [0.6f32, 2.0] {
        let centre = Vec2::new(8.3, 7.6);
        let closed = SyntheticStar::new(centre, 1.0, StarProfile::gaussian(sigma)).pixels();
        let turned = SyntheticStar::new(
            centre,
            1.0,
            StarProfile::Gaussian {
                sigma_x: sigma,
                sigma_y: sigma,
                angle: 1.0,
            },
        )
        .pixels();
        for y in 0..16 {
            for x in 0..16 {
                let (a, b) = (closed.value(x, y), turned.value(x, y));
                assert!(
                    (a - b).abs() <= f32::EPSILON * a.max(b) + 1e-12,
                    "σ {sigma} at ({x}, {y}): {a} vs {b}"
                );
            }
        }
    }
}

#[test]
fn moffat_carries_heavier_wings_than_a_gaussian_of_equal_fwhm() {
    let beta = 2.5;
    let fwhm = 4.0;
    let moffat = SyntheticStar::new(
        Vec2::splat(32.0),
        1.0,
        StarProfile::Moffat {
            alpha: fwhm::to_alpha(fwhm, beta),
            beta,
        },
    );
    let gaussian = SyntheticStar::new(
        Vec2::splat(32.0),
        1.0,
        StarProfile::gaussian(fwhm::to_sigma(fwhm)),
    );

    // Equal FWHM means they cross at the half-maximum point...
    let half = 32.0 + fwhm / 2.0;
    assert!((moffat.value_at(half, 32.0) - 0.5).abs() < 1e-5);
    assert!((gaussian.value_at(half, 32.0) - 0.5).abs() < 1e-5);
    // ...but far out, the power law dominates the exponential.
    assert!(moffat.value_at(44.0, 32.0) > gaussian.value_at(44.0, 32.0) * 100.0);
}

#[test]
fn moffat_needs_a_wider_radius_than_a_gaussian_to_reach_the_same_floor() {
    // 8α vs 4σ: for equal FWHM the Moffat box is the larger one.
    let beta = 2.5;
    let moffat = StarProfile::Moffat {
        alpha: fwhm::to_alpha(4.0, beta),
        beta,
    };
    let gaussian = StarProfile::gaussian(fwhm::to_sigma(4.0));
    assert!(moffat.radius() > gaussian.radius());
}
