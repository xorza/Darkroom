use super::*;

/// The windowed centroid lands on noiseless Gaussian stars at sub-pixel offsets, each measured at
/// its own FWHM, from the nearest pixel and from 2 and 3 px off: its Newton step is exact for a
/// Gaussian, so it stops within its tolerance of 1e-5 px, held here to 1e-4. The profile fits from
/// there land on one optimum whatever they start from — the centroid or the nearest pixel, apart
/// by no more than the optimizer's own convergence (measured 3e-14 px): a Gaussian on the truth,
/// to the f32 rounding of the residual (2.1e-6 px measured), and a Moffat, a different profile,
/// within its fixed-β bias (2.6e-4 px measured).
#[test]
fn the_windowed_centroid_converges_and_the_fits_ignore_the_seed() {
    for (x, y, sigma) in [(32.3, 32.7, 2.5f32), (32.8, 32.2, 3.5), (32.1, 32.9, 1.8)] {
        let truth = DVec2::new(x, y);
        let size = Size2us::new(64, 64);
        let pixels = SyntheticStar::new(truth.as_vec2(), 0.8, StarProfile::Gaussian { sigma })
            .stamp(size, 0.1);
        let measured = Measured::flat(&pixels, 0.1, 0.01);
        let fwhm = sigma_to_fwhm(sigma);
        let start = DVec2::new(x.round(), y.round());
        let error = |p: DVec2| (p - truth).length();
        for from in [
            start,
            truth + DVec2::new(2.0, -2.0),
            truth + DVec2::new(2.1, 2.1),
        ] {
            let centre = measured
                .windowed(from, fwhm)
                .expect("the centroid converges");
            assert!(
                error(centre.pos) < 1e-4,
                "σ {sigma} from {from}: {}",
                centre.pos
            );
        }
        let windowed = measured.windowed(start, fwhm).unwrap().pos;

        let grid = StampGrid::new(MeasureGrid::stamp_radius(fwhm));
        let gaussian = |seed| {
            GaussianFit::new(&measured.residual, seed, &grid, 0.0, None)
                .expect("the Gaussian fit lands")
                .pos
        };
        let moffat = |seed| {
            MoffatFit::new(&measured.residual, seed, &grid, 0.0, None, 2.5)
                .expect("the Moffat fit lands")
                .pos
        };
        for (model, fit, bias) in [
            ("Gaussian", &gaussian as &dyn Fn(DVec2) -> DVec2, 1e-5),
            ("Moffat", &moffat, 1e-3),
        ] {
            let (from_centroid, from_pixel) = (fit(windowed), fit(start));
            assert!(
                (from_centroid - from_pixel).length() < 1e-8,
                "σ {sigma} {model}: {from_centroid} vs {from_pixel}"
            );
            assert!(
                error(from_centroid) < bias,
                "σ {sigma} {model}: {}",
                error(from_centroid)
            );
        }
    }
}

/// Review table 11.1: with the window of a FWHM-3 star, stars of FWHM 3, 4.5 and 6 started 0.4 px
/// off their centres come back within 1e-4 px, where ten fixed-point steps left −0.0035, −0.034 and
/// −0.092 px.
#[test]
fn the_windowed_centroid_converges_on_stars_wider_than_its_window() {
    for fwhm in [3.0f32, 4.5, 6.0] {
        let truth = DVec2::new(32.3, 32.6);
        let size = Size2us::new(64, 64);
        let pixels = SyntheticStar::new(
            truth.as_vec2(),
            0.8,
            StarProfile::Gaussian {
                sigma: fwhm_to_sigma(fwhm),
            },
        )
        .stamp(size, 0.1);
        let centre = Measured::flat(&pixels, 0.1, 0.01)
            .windowed(truth + DVec2::new(0.4, 0.0), 3.0)
            .expect("the centroid converges");
        assert!(
            (centre.pos - truth).length() < 1e-4,
            "FWHM {fwhm}: {}",
            centre.pos
        );
    }
}

#[test]
fn compute_stamp_radius_scales_and_clamps() {
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
            MeasureGrid::stamp_radius(fwhm),
            expected,
            "FWHM {fwhm} uses ceil(1.75 × FWHM), clamped to [4, 15]"
        );
    }
}
