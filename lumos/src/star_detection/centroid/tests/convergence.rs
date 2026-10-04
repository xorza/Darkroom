use super::*;

/// How the moments phase converges and what the profile fits make of it, on noiseless Gaussian
/// stars at sub-pixel offsets, each measured at its own FWHM and stamp.
///
/// The moments window is a Gaussian of `σ_w = 0.8σ` around the current position, and the weighted
/// mean of a Gaussian star under it lies at `(μ·σ_w² + p·σ²) / (σ² + σ_w²)`: each step shrinks the
/// error by `σ² / (σ² + σ_w²)` = 1 / 1.64. Two steps, as `measure_star` takes before a fit, leave
/// it at 0.37 of the start — and the fits must not care: from the 2-step and the 10-step seed they
/// land on the same optimum, a Gaussian on the truth (the profile is its own model) and a Moffat
/// within its model's bias.
#[test]
fn moments_contract_and_fits_ignore_the_seed() {
    const CONTRACTION: f64 = 1.0 / 1.64;
    for (x, y, sigma) in [(32.3, 32.7, 2.5f32), (32.8, 32.2, 3.5), (32.1, 32.9, 1.8)] {
        let truth = DVec2::new(x, y);
        let size = Size2us::new(64, 64);
        let pixels = SyntheticStar::new(truth.as_vec2(), 0.8, StarProfile::Gaussian { sigma })
            .stamp(size, 0.1);
        let residual = background_map::uniform(size, 0.1, 0.01).residual_of(&pixels);
        let fwhm = sigma_to_fwhm(sigma);
        let radius = compute_stamp_radius(fwhm);
        let start = DVec2::new(x.round(), y.round());
        let moments = |n| moments_centroid(&residual, start, radius, fwhm, n).unwrap();
        let error = |p: DVec2| (p - truth).length();

        // The sampled star follows the continuous contraction to within 1%, and from 2 and 3 px
        // off as from the nearest pixel: ten steps leave 1.64⁻¹⁰ = 0.7% of the start.
        for offset in [DVec2::new(2.0, -2.0), DVec2::new(2.1, 2.1)] {
            let far = truth + offset;
            let after = moments_centroid(&residual, far, radius, fwhm, 10).unwrap();
            assert!(
                error(after) < 1.05 * offset.length() * CONTRACTION.powi(10),
                "σ {sigma} from {offset}: {}",
                error(after)
            );
        }
        let (one, two, ten) = (moments(1), moments(2), moments(10));
        let ratio = error(two) / error(one);
        assert!(
            (ratio - CONTRACTION).abs() < 0.01 * CONTRACTION,
            "σ {sigma}: step ratio {ratio}"
        );
        assert!(
            error(ten) < 1.01 * error(one) * CONTRACTION.powi(9),
            "σ {sigma}: ten steps left {}",
            error(ten)
        );

        // The fits converge to one optimum from either seed: apart by no more than the optimizer's
        // stopping step, 1e-8 of a parameter (measured: 3e-14 px). The Gaussian lands on the truth
        // to the f32 rounding of the residual, ≤ 2.1e-6 px measured; the Moffat, a different
        // profile, carries its fixed-β bias, ≤ 2.6e-4 px measured on these stamps.
        let grid = StampGrid::new(radius);
        let gaussian = |seed| {
            GaussianFit::new(&residual, seed, &grid, 0.0, None)
                .expect("the Gaussian fit lands")
                .pos
        };
        let moffat = |seed| {
            let beta = 2.5;
            MoffatFit::new(&residual, seed, &grid, 0.0, None, beta)
                .expect("the Moffat fit lands")
                .pos
        };
        for (model, fit, bias) in [
            ("Gaussian", &gaussian as &dyn Fn(DVec2) -> DVec2, 1e-5),
            ("Moffat", &moffat, 1e-3),
        ] {
            let (from_two, from_ten) = (fit(two), fit(ten));
            assert!(
                (from_two - from_ten).length() < 1e-8,
                "σ {sigma} {model}: {from_two} vs {from_ten}"
            );
            assert!(
                error(from_two) < bias,
                "σ {sigma} {model}: {}",
                error(from_two)
            );
        }
    }
}

#[test]
fn compute_stamp_radius_scales_and_clamps() {
    use crate::star_detection::centroid::compute_stamp_radius;

    let cases = [
        (1.0, 4),
        (2.0, 4),
        (3.0, 6),
        (4.0, 7),
        (5.0, 9),
        (6.0, 11),
        (8.0, 14),
        (10.0, 15),
        (20.0, 15),
    ];
    for (fwhm, expected) in cases {
        assert_eq!(
            compute_stamp_radius(fwhm),
            expected,
            "FWHM {fwhm} uses ceil(1.75 × FWHM), clamped to [4, 15]"
        );
    }
}
