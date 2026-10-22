//! Registration's handling of the star lists it is handed: too few, degenerate FWHM,
//! mismatched counts.

use glam::DVec2;

use crate::registration::error::{RegistrationCatalog, RegistrationError, StarField};
use crate::registration::register::median_fwhm;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::tuning;
use crate::star_detection::star::Star;

// Registration reads only `pos` and `fwhm`, so these fixtures set the FWHM under test and
// leave everything else — position included — at its `Star::at` default.

#[test]
fn median_fwhm_basic() {
    let ref_stars = vec![
        Star::at(DVec2::ZERO).with_fwhm(2.0),
        Star::at(DVec2::ZERO).with_fwhm(3.0),
        Star::at(DVec2::ZERO).with_fwhm(4.0),
    ];
    let target_stars = vec![
        Star::at(DVec2::ZERO).with_fwhm(2.5),
        Star::at(DVec2::ZERO).with_fwhm(3.5),
    ];
    // Combined: [2.0, 2.5, 3.0, 3.5, 4.0] -> median = 3.0
    let median = median_fwhm(&ref_stars, &target_stars);
    assert!((median - 3.0).abs() < 0.01);
}

#[test]
fn median_fwhm_even_count_averages_the_middle_pair() {
    // Four stars: [2.0, 3.0, 4.0, 5.0] -> (3.0 + 4.0) / 2 = 3.5, the middle pair averaged as the
    // detector's own median FWHM is.
    let ref_stars = vec![
        Star::at(DVec2::ZERO).with_fwhm(2.0),
        Star::at(DVec2::ZERO).with_fwhm(5.0),
    ];
    let target_stars = vec![
        Star::at(DVec2::ZERO).with_fwhm(4.0),
        Star::at(DVec2::ZERO).with_fwhm(3.0),
    ];
    let median = median_fwhm(&ref_stars, &target_stars);
    assert!((median - 3.5).abs() < 0.01, "got {median}");
}

#[test]
fn median_fwhm_single_set() {
    let ref_stars = vec![
        Star::at(DVec2::ZERO).with_fwhm(1.5),
        Star::at(DVec2::ZERO).with_fwhm(2.5),
        Star::at(DVec2::ZERO).with_fwhm(3.5),
    ];
    let target_stars = vec![];
    // Combined: [1.5, 2.5, 3.5] -> median = 2.5
    let median = median_fwhm(&ref_stars, &target_stars);
    assert!((median - 2.5).abs() < 0.01);
}

#[test]
fn max_sigma_typical_seeing() {
    // Typical ground seeing: FWHM = 2.0-4.0 pixels
    // FWHM = 2.0 -> max_sigma = 1.0 (~3px effective threshold)
    // FWHM = 4.0 -> max_sigma = 2.0 (~6px effective threshold)
    let ref_stars = vec![
        Star::at(DVec2::ZERO).with_fwhm(2.0),
        Star::at(DVec2::ZERO).with_fwhm(2.5),
        Star::at(DVec2::ZERO).with_fwhm(3.0),
    ];
    let target_stars = vec![
        Star::at(DVec2::ZERO).with_fwhm(2.2),
        Star::at(DVec2::ZERO).with_fwhm(2.8),
    ];

    // Median of [2.0, 2.2, 2.5, 2.8, 3.0] = 2.5, so max_sigma = 2.5 * 0.5 = 1.25. The formula
    // itself is pinned in `tuning`; what matters here is that `register` feeds it the median of
    // *both* catalogs.
    let median = median_fwhm(&ref_stars, &target_stars);
    assert!((median - 2.5).abs() < 0.01);
    assert!((tuning::max_sigma_from_fwhm(median) - 1.25).abs() < 0.01);
}

#[test]
fn register_rejects_non_finite_positions_in_both_catalogs() {
    for catalog in [RegistrationCatalog::Reference, RegistrationCatalog::Target] {
        let mut ref_stars = vec![Star::at(DVec2::ZERO).with_fwhm(2.0); 8];
        let mut target_stars = ref_stars.clone();
        let stars = match catalog {
            RegistrationCatalog::Reference => &mut ref_stars,
            RegistrationCatalog::Target => &mut target_stars,
        };
        stars[3].pos = DVec2::new(f64::NAN, 4.0);

        let error = RegistrationConfig::default()
            .registrar()
            .unwrap()
            .register(&ref_stars, &target_stars)
            .unwrap_err();
        match error {
            RegistrationError::InvalidStarPos {
                catalog: actual,
                idx,
                pos,
            } => {
                assert_eq!(actual, catalog);
                assert_eq!(idx, 3);
                assert!(pos.x.is_nan());
                assert_eq!(pos.y, 4.0);
            }
            other => panic!("expected invalid star position, got {other:?}"),
        }
    }
}

/// What registration reads is checked, in both catalogs: a finite FWHM and a finite, positive
/// position σ, which the final fit divides by. A measurement it does not read, such as the SNR, is
/// not checked, and a NaN there does not fail a registration.
#[test]
fn register_checks_the_fields_it_reads_in_both_catalogs() {
    for catalog in [RegistrationCatalog::Reference, RegistrationCatalog::Target] {
        let mut ref_stars = vec![Star::at(DVec2::ZERO).with_fwhm(2.0); 8];
        let mut target_stars = ref_stars.clone();
        let stars = match catalog {
            RegistrationCatalog::Reference => &mut ref_stars,
            RegistrationCatalog::Target => &mut target_stars,
        };
        stars[5].fwhm = f32::INFINITY;
        stars[6].pos_sigma = 0.0;
        stars[7].snr = f32::NAN;

        let error = RegistrationConfig::default()
            .registrar()
            .unwrap()
            .register(&ref_stars, &target_stars)
            .unwrap_err();
        match error {
            RegistrationError::InvalidStarField {
                catalog: actual,
                idx,
                field,
                value,
            } => {
                assert_eq!(actual, catalog);
                assert_eq!((idx, field), (5, StarField::Fwhm));
                assert_eq!(value, f64::INFINITY);
            }
            other => panic!("expected invalid star FWHM, got {other:?}"),
        }
        let stars = match catalog {
            RegistrationCatalog::Reference => &mut ref_stars,
            RegistrationCatalog::Target => &mut target_stars,
        };
        stars[5].fwhm = 2.0;
        let error = RegistrationConfig::default()
            .registrar()
            .unwrap()
            .register(&ref_stars, &target_stars)
            .unwrap_err();
        assert!(
            matches!(
                error,
                RegistrationError::InvalidStarField {
                    idx: 6,
                    field: StarField::PosSigma,
                    value: 0.0,
                    ..
                }
            ),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            format!("{catalog} star 6 position σ must be finite and positive, got 0")
        );
        let stars = match catalog {
            RegistrationCatalog::Reference => &mut ref_stars,
            RegistrationCatalog::Target => &mut target_stars,
        };
        stars[6].pos_sigma = 0.1;
        let error = RegistrationConfig::default()
            .registrar()
            .unwrap()
            .register(&ref_stars, &target_stars);
        assert!(
            !matches!(error, Err(RegistrationError::InvalidStarField { .. })),
            "{error:?}"
        );
    }
}

/// A catalog with too few unsaturated stars for the fit fails at once, naming the catalog, on
/// either side: the final fit reads no saturated star. Before, an all-saturated target read
/// "found 0" stars and an all-saturated reference ran on to "too few inliers".
#[test]
fn too_few_unsaturated_stars_name_their_catalog() {
    let stars: Vec<Star> = (0..30)
        .map(|i| {
            Star::at(DVec2::new(
                f64::from(i) * 37.0,
                f64::from(i * i % 23) * 41.0,
            ))
        })
        .collect();
    let config = RegistrationConfig::default();
    let required = config.matching.min_matches;
    for catalog in [RegistrationCatalog::Reference, RegistrationCatalog::Target] {
        let mut saturated = stars.clone();
        for star in &mut saturated {
            star.saturated = true;
        }
        let (reference, target) = match catalog {
            RegistrationCatalog::Reference => (&saturated, &stars),
            RegistrationCatalog::Target => (&stars, &saturated),
        };
        let error = config
            .registrar()
            .unwrap()
            .register(reference, target)
            .unwrap_err();
        assert!(
            matches!(
                error,
                RegistrationError::TooFewUnsaturatedStars { catalog: actual, found: 0, required: r }
                    if actual == catalog && r == required
            ),
            "{catalog}: {error:?}"
        );
    }
}
