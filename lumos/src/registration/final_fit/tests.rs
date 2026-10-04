use rayon::prelude::*;

use super::*;
use crate::internals::synthetic::transforms::generate_random_positions;
use crate::internals::test_rng::TestRng;
use crate::registration::ransac::transforms::estimate_transform;

/// A `angle_deg` rotation about (1000, 1000), then `offset`.
fn rotation_about_centre(angle_deg: f64, offset: DVec2) -> Transform {
    Transform::translation(offset).compose(&Transform::rotation_around(
        DVec2::splat(1000.0),
        angle_deg.to_radians(),
    ))
}

/// Stars at `positions`, each with position σ `sigma`.
fn stars(positions: &[DVec2], sigma: f64) -> Vec<Star> {
    positions
        .iter()
        .map(|&p| Star {
            position_sigma: sigma,
            ..Star::at(p)
        })
        .collect()
}

fn indices(fit: &FinalFit) -> Vec<(usize, usize)> {
    fit.matches
        .iter()
        .map(|m| (m.indices.reference, m.indices.target))
        .collect()
}

/// A seed transform that misses some stars is improved on: 50 stars under a 1° rotation about
/// (1000, 1000) and a shift, seeded with that rotation 0.15° short and 1 px off. The seed's error
/// grows with distance from the centre — `0.0026·r` — so the first pass misses stars beyond its
/// 3 px radius and finds the rest. Refitting on those finds the truth, which reaches every star:
/// all 50 come back, and the transform is the truth to the rounding of coordinates up to 2000 px.
#[test]
fn a_seed_that_misses_stars_reaches_them_all() {
    let reference = generate_random_positions(50, 2000.0, 2000.0, 42);
    let truth = rotation_about_centre(1.0, DVec2::new(30.0, -20.0));
    let target: Vec<DVec2> = reference.iter().map(|&p| truth.apply(p)).collect();
    let seed = rotation_about_centre(0.85, DVec2::new(31.0, -20.0));
    let missed = reference
        .iter()
        .zip(&target)
        .filter(|&(&r, &t)| seed.apply(r).distance(t) > 3.0)
        .count();
    assert!(missed > 0, "the seed has to miss stars to test anything");

    let catalogs = FitCatalogs::new(&stars(&reference, 0.01), &stars(&target, 0.01)).unwrap();
    let fit = FinalFit::run(
        &catalogs,
        seed,
        FitModel {
            transform: TransformType::Euclidean,
            sip: None,
        },
        3.0,
    )
    .unwrap();
    assert_eq!(indices(&fit), (0..50).map(|i| (i, i)).collect::<Vec<_>>());
    for p in [DVec2::ZERO, DVec2::new(2000.0, 0.0), DVec2::splat(2000.0)] {
        assert!(fit.warp.transform.apply(p).distance(truth.apply(p)) < 1e-9);
    }
}

/// Each pair weighs by the inverse of its variance. Two precise stars (σ 0.01) sit `k·σ·√2` right
/// of a translation by (5, 0) and eight imprecise ones (σ 0.04) `k·σ·√2` left of it, k = ½, the
/// pair σ being `σ·√2`: every normalized residual is ½, so the Cauchy weights are equal, and the
/// weighted condition `Σ (z/σ_pair)` balances as `2/0.01 = 8/0.04`. The translation is (5, 0) to
/// rounding; the plain mean would sit `(2·0.01 − 8·0.04)·√2/(2·10)` = −0.0212 px off it.
#[test]
fn pairs_weigh_by_their_variance() {
    let half_root2 = 0.5 * std::f64::consts::SQRT_2;
    let mut reference = Vec::new();
    let mut target = Vec::new();
    for i in 0..10 {
        let r = DVec2::new(100.0 * f64::from(i), 50.0 * f64::from(i % 3));
        let (sigma, offset) = if i < 2 {
            (0.01, 0.01 * half_root2)
        } else {
            (0.04, -0.04 * half_root2)
        };
        reference.push(Star {
            position_sigma: sigma,
            ..Star::at(r)
        });
        target.push(Star {
            position_sigma: sigma,
            ..Star::at(r + DVec2::new(5.0 + offset, 0.0))
        });
    }
    let catalogs = FitCatalogs::new(&reference, &target).unwrap();
    let fit = FinalFit::run(
        &catalogs,
        Transform::translation(DVec2::new(5.0, 0.0)),
        FitModel {
            transform: TransformType::Translation,
            sip: None,
        },
        3.0,
    )
    .unwrap();
    let found = fit.warp.transform.translation_components();
    assert!((found - DVec2::new(5.0, 0.0)).length() < 1e-12, "{found}");
    assert_eq!(fit.scale, 1.0);
}

/// Saturated stars take no part, on either side, and a blend 2 px off its true place falls out at
/// the second pass, whose gate is `√χ²₀.₉₉(2)·σ` ≈ 0.04 px for exact pairs of σ 0.01: of 51 stars,
/// star 3 saturated in the reference, star 7 in the target and star 11 blended, the other 48 match
/// and the fit is exact.
#[test]
fn saturated_stars_and_a_blend_take_no_part() {
    let reference_points = generate_random_positions(51, 2000.0, 2000.0, 7);
    let truth = rotation_about_centre(0.3, DVec2::new(-12.0, 8.0));
    let mut target_points: Vec<DVec2> = reference_points.iter().map(|&p| truth.apply(p)).collect();
    target_points[11] += DVec2::new(2.0, 0.0);
    let mut reference = stars(&reference_points, 0.01);
    let mut target = stars(&target_points, 0.01);
    reference[3].saturated = true;
    target[7].saturated = true;
    let catalogs = FitCatalogs::new(&reference, &target).unwrap();
    let fit = FinalFit::run(
        &catalogs,
        truth,
        FitModel {
            transform: TransformType::Euclidean,
            sip: None,
        },
        3.0,
    )
    .unwrap();
    let expected: Vec<(usize, usize)> = (0..51)
        .filter(|i| ![3, 7, 11].contains(i))
        .map(|i| (i, i))
        .collect();
    assert_eq!(indices(&fit), expected);
    for p in [DVec2::ZERO, DVec2::splat(2000.0)] {
        assert!(fit.warp.transform.apply(p).distance(truth.apply(p)) < 1e-9);
    }
}

/// The asymptotic variance of the Cauchy-weighted fit over the least-squares one, for isotropic
/// 2-D residuals: the estimating equation `Σ w(|z|)·z = 0` with `w(r) = 1/(1 + r²/c²)` has
/// variance `(E[w²r²]/2) / (E[w] + E[r·w′]/2)²` per unit Fisher information, `r` Rayleigh of
/// σ 1. Integrated by the midpoint rule over `[0, 12]`, past which the Rayleigh density is 6e-31.
fn cauchy_variance_ratio() -> f64 {
    let c2 = CAUCHY_TUNING * CAUCHY_TUNING;
    let steps = 120_000;
    let h = 12.0 / f64::from(steps);
    let (mut w2r2, mut w, mut rw_prime) = (0.0, 0.0, 0.0);
    for i in 0..steps {
        let r = (f64::from(i) + 0.5) * h;
        let density = r * (-r * r / 2.0).exp() * h;
        let weight = 1.0 / (1.0 + r * r / c2);
        w2r2 += weight * weight * r * r * density;
        w += weight * density;
        rw_prime += r * (-2.0 * r / c2) * weight * weight * density;
    }
    (w2r2 / 2.0) / (w + rw_prime / 2.0).powi(2)
}

/// The fit is as precise as the stars allow. 2000 stars under a similarity, each with its own
/// position σ between 0.02 and 0.2 px, both catalogs perturbed by Gaussian noise of that σ in 300
/// fixed-seed draws: the scatter of the four fitted parameters matches the Cramér–Rao bound from
/// the weighted Fisher information `Σ JᵢᵀJᵢ/(s²σᵣ² + σₜ²)`, times the Cauchy weights' variance
/// ratio ([`cauchy_variance_ratio`], 1.062). A variance from 300 draws carries a standard error
/// of `√(2/299)` = 8.2% of itself; each must lie within 3 of those.
#[test]
fn the_fit_reaches_the_cramer_rao_bound() {
    const DRAWS: u64 = 300;
    let positions = generate_random_positions(2000, 2000.0, 2000.0, 11);
    let sigmas: Vec<f64> = (0..positions.len())
        .map(|i| 0.02 + 0.18 * (i % 10) as f64 / 9.0)
        .collect();
    let (shift, angle, scale) = (DVec2::new(14.0, -9.0), 0.6f64.to_radians(), 1.002);
    let truth = Transform::similarity(shift, angle, scale);
    let parameters = |transform: &Transform| {
        let m = transform.matrix();
        let scale = m[0].hypot(m[3]);
        [m[2], m[5], m[3].atan2(m[0]), scale]
    };

    // The Fisher information of (tx, ty, θ, s) for t = s·R(θ)·r + (tx, ty).
    let mut information = [[0.0f64; 4]; 4];
    let (sin, cos) = angle.sin_cos();
    for (&r, &sigma) in positions.iter().zip(&sigmas) {
        let weight = 1.0 / ((scale * scale + 1.0) * sigma * sigma);
        let rotated = DVec2::new(cos * r.x - sin * r.y, sin * r.x + cos * r.y);
        let d_angle = DVec2::new(-scale * rotated.y, scale * rotated.x);
        let columns = [DVec2::X, DVec2::Y, d_angle, rotated];
        for i in 0..4 {
            for j in 0..4 {
                information[i][j] += weight * columns[i].dot(columns[j]);
            }
        }
    }
    let bound = nalgebra::Matrix4::from_fn(|i, j| information[i][j])
        .try_inverse()
        .unwrap();

    let fits: Vec<[f64; 4]> = (0..DRAWS)
        .into_par_iter()
        .map(|seed| {
            let mut rng = TestRng::new(1000 + seed);
            let mut noisy = |p: DVec2, sigma: f64| {
                p + sigma
                    * DVec2::new(
                        f64::from(rng.next_gaussian_f32()),
                        f64::from(rng.next_gaussian_f32()),
                    )
            };
            let reference: Vec<Star> = positions
                .iter()
                .zip(&sigmas)
                .map(|(&p, &sigma)| Star {
                    position_sigma: sigma,
                    ..Star::at(noisy(p, sigma))
                })
                .collect();
            let target: Vec<Star> = positions
                .iter()
                .zip(&sigmas)
                .map(|(&p, &sigma)| Star {
                    position_sigma: sigma,
                    ..Star::at(noisy(truth.apply(p), sigma))
                })
                .collect();
            let catalogs = FitCatalogs::new(&reference, &target).unwrap();
            let fit = FinalFit::run(
                &catalogs,
                truth,
                FitModel {
                    transform: TransformType::Similarity,
                    sip: None,
                },
                3.0,
            )
            .unwrap();
            parameters(&fit.warp.transform)
        })
        .collect();

    let ratio = cauchy_variance_ratio();
    let truth_parameters = parameters(&truth);
    for (k, name) in ["tx", "ty", "θ", "s"].iter().enumerate() {
        let variance = fits
            .iter()
            .map(|fit| (fit[k] - truth_parameters[k]).powi(2))
            .sum::<f64>()
            / DRAWS as f64;
        let expected = ratio * bound[(k, k)];
        let relative = variance / expected - 1.0;
        assert!(
            relative.abs() <= 3.0 * (2.0 / (DRAWS - 1) as f64).sqrt(),
            "{name}: variance {variance:e} against {expected:e} ({relative:+.3})"
        );
    }
}

/// The homography is the least-squares one on the reprojection error, not the algebraic one. 40
/// pairs under a perspective map with 0.3 px of noise: the refined fit's `Σ|H(r) − t|²` is below
/// the DLT's, and no step of 1e-7 relative in any of its eight free entries lowers it — a minimum.
#[test]
fn the_homography_minimizes_the_reprojection_error() {
    let truth = Transform::homography([1.01, 0.02, 15.0, -0.015, 0.99, -8.0, 2e-5, -1.5e-5]);
    let reference = generate_random_positions(40, 2000.0, 2000.0, 5);
    let mut rng = TestRng::new(77);
    let mut pairs = WeightedPairs::default();
    for &r in &reference {
        let noise = 0.3
            * DVec2::new(
                f64::from(rng.next_gaussian_f32()),
                f64::from(rng.next_gaussian_f32()),
            );
        pairs.push(r, truth.apply(r) + noise, 1.0);
    }
    let chi2 = |transform: &Transform| {
        pairs
            .reference
            .iter()
            .zip(&pairs.target)
            .map(|(&r, &t)| (transform.apply(r) - t).length_squared())
            .sum::<f64>()
    };
    let refined = pairs.fit(TransformType::Homography).unwrap();
    let algebraic =
        estimate_transform(&pairs.reference, &pairs.target, TransformType::Homography).unwrap();
    assert!(
        chi2(&refined) < chi2(&algebraic),
        "{} vs {}",
        chi2(&refined),
        chi2(&algebraic)
    );
    let m = *refined.matrix();
    for i in 0..8 {
        for sign in [-1.0, 1.0] {
            let mut stepped = m;
            stepped[i] += sign * 1e-7 * m[i].abs().max(1e-12);
            let stepped = Transform::homography(std::array::from_fn(|k| stepped[k] / stepped[8]));
            assert!(
                chi2(&stepped) >= chi2(&refined) * (1.0 - 1e-12),
                "entry {i}, {sign}: {} below {}",
                chi2(&stepped),
                chi2(&refined)
            );
        }
    }
}
