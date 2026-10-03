use crate::stacking::star_detection::detector::stages::fwhm::*;
use crate::stacking::star_detection::roundness::Roundness;
use crate::testing::prelude::*;

// FWHM estimation never reads position, so every fixture below sits at the origin, and the
// remaining `Star::at` defaults clear `filter_config()`.

/// The fallback every case passes, distinguishable from the 3.0 the star fixtures below are
/// built around.
const FALLBACK: f32 = 4.0;

/// Estimation settings that fall back to [`FALLBACK`].
fn fwhm_config(min_stars: usize) -> FwhmConfig {
    FwhmConfig {
        mode: Some(FwhmMode::Auto { fallback: FALLBACK }),
        min_stars,
        ..Default::default()
    }
}

/// Quality bounds: the defaults, with eccentricity widened to 0.8.
fn filter_config() -> FilterConfig {
    FilterConfig {
        max_eccentricity: 0.8,
        ..Default::default()
    }
}

/// `n` clean stars of FWHM `fwhm`.
fn clean(n: usize, fwhm: f32) -> Vec<Star> {
    (0..n)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(fwhm))
        .collect()
}

/// Nine clean 3.0 stars and `spoiled`.
fn nine_and(spoiled: Star) -> Vec<Star> {
    let mut stars = clean(9, 3.0);
    stars.push(spoiled);
    stars
}

/// Every outcome of `from_stars`, as the whole `FwhmSource` it returns.
///
/// The quality cases spoil a star whose FWHM is the same 3.0 as the rest, so only the quality
/// filter can remove it: the outcome is `Estimated { 3.0, 9 }` with the filter and would be
/// `{ 3.0, 10 }` without. Medians of ten take the mean of the middle two; the MAD floor is
/// `0.1 · median`, so a 3.0 cluster with MAD 0 keeps `|f − 3| ≤ 3 · 0.3 = 0.9`.
#[test]
fn from_stars_over_every_case() {
    let base = Star::at(DVec2::ZERO);
    let cases: Vec<(&str, Vec<Star>, usize, FwhmSource)> = vec![
        // 4 < min_stars: the fallback, which no star produced.
        (
            "insufficient",
            clean(4, 3.0),
            5,
            FwhmSource::Configured(FALLBACK),
        ),
        // Median 3.0 over ten; MAD 0 → threshold 0.9 drops the four 18.0s, leaving 6 < 7: the
        // pre-rejection median, credited to the ten that made it.
        (
            "pre-rejection median",
            {
                let mut stars = clean(6, 3.0);
                stars.extend(clean(4, 18.0));
                stars
            },
            7,
            FwhmSource::Estimated {
                fwhm: 3.0,
                stars_used: 10,
            },
        ),
        (
            "saturated",
            nine_and(base.with_saturated(true)),
            5,
            FwhmSource::Estimated {
                fwhm: 3.0,
                stars_used: 9,
            },
        ),
        (
            "low snr",
            nine_and(base.with_snr(5.0)),
            5,
            FwhmSource::Estimated {
                fwhm: 3.0,
                stars_used: 9,
            },
        ),
        (
            "eccentric",
            nine_and(base.with_eccentricity(0.9)),
            5,
            FwhmSource::Estimated {
                fwhm: 3.0,
                stars_used: 9,
            },
        ),
        (
            "cosmic ray",
            nine_and(base.with_sharpness(0.9)),
            5,
            FwhmSource::Estimated {
                fwhm: 3.0,
                stars_used: 9,
            },
        ),
        (
            "not round",
            nine_and(base.with_roundness(Roundness {
                ground: 0.6,
                sround: 0.0,
            })),
            5,
            FwhmSource::Estimated {
                fwhm: 3.0,
                stars_used: 9,
            },
        ),
        // On each quality bound exactly: kept, as the filter stage keeps it.
        (
            "on every bound",
            nine_and(
                base.with_snr(10.0)
                    .with_eccentricity(0.8)
                    .with_sharpness(0.7)
                    .with_roundness(Roundness {
                        ground: 0.5,
                        sround: -0.5,
                    }),
            ),
            5,
            FwhmSource::Estimated {
                fwhm: 3.0,
                stars_used: 10,
            },
        ),
        // 0.2 and 25.0 lie outside [0.5, 20): with the range check 8 < 9 stars remain and the
        // fallback stands. Without it, ten would pass and MAD would leave the pre-rejection
        // `{ 3.0, 10 }`.
        (
            "implausible widths",
            {
                let mut stars = clean(8, 3.0);
                stars.push(base.with_fwhm(0.2));
                stars.push(base.with_fwhm(25.0));
                stars
            },
            9,
            FwhmSource::Configured(FALLBACK),
        ),
        // Twelve stars, median 3.0, MAD 0: the 12.0 and 15.0 go and ten remain.
        (
            "outliers",
            {
                let mut stars = clean(10, 3.0);
                stars.push(base.with_fwhm(12.0));
                stars.push(base.with_fwhm(15.0));
                stars
            },
            5,
            FwhmSource::Estimated {
                fwhm: 3.0,
                stars_used: 10,
            },
        ),
        (
            "uniform",
            clean(10, 4.5),
            5,
            FwhmSource::Estimated {
                fwhm: 4.5,
                stars_used: 10,
            },
        ),
        // Sorted 2.8 2.9 2.9 3.0 3.0 | 3.0 3.1 3.1 3.2 3.3: median 3.0. Deviations sort to
        // 0 0 0 0.1 0.1 | 0.1 0.1 0.2 0.2 0.3, MAD 0.1 against the 0.3 floor: threshold 0.9
        // keeps all ten.
        (
            "spread",
            [2.8, 3.0, 3.1, 3.2, 2.9, 3.3, 3.0, 3.1, 2.9, 3.0]
                .into_iter()
                .map(|fwhm| base.with_fwhm(fwhm))
                .collect(),
            5,
            FwhmSource::Estimated {
                fwhm: 3.0,
                stars_used: 10,
            },
        ),
        (
            "all rejected",
            (0..10).map(|_| base.with_saturated(true)).collect(),
            5,
            FwhmSource::Configured(FALLBACK),
        ),
    ];

    for (name, stars, min_stars, expected) in cases {
        assert_eq!(
            from_stars(&stars, &fwhm_config(min_stars), FALLBACK, &filter_config()),
            expected,
            "{name}"
        );
    }
}
