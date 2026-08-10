//! Registration's handling of the star lists it is handed: too few, degenerate FWHM,
//! mismatched counts.

use crate::stacking::registration::*;

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
    // Four stars: [2.0, 3.0, 4.0, 5.0] -> (3.0 + 4.0) / 2 = 3.5. The old full sort read the
    // upper middle (4.0); quickselect averages, matching how the detector's own median FWHM
    // is computed.
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

        let error = register(&ref_stars, &target_stars, &Config::default()).unwrap_err();
        match error {
            RegistrationError::InvalidStarPosition {
                catalog: actual,
                index,
                position,
            } => {
                assert_eq!(actual, catalog);
                assert_eq!(index, 3);
                assert!(position.x.is_nan());
                assert_eq!(position.y, 4.0);
            }
            other => panic!("expected invalid star position, got {other:?}"),
        }
    }
}

#[test]
fn register_rejects_non_finite_fwhm_in_both_catalogs() {
    for catalog in [RegistrationCatalog::Reference, RegistrationCatalog::Target] {
        let mut ref_stars = vec![Star::at(DVec2::ZERO).with_fwhm(2.0); 8];
        let mut target_stars = ref_stars.clone();
        let stars = match catalog {
            RegistrationCatalog::Reference => &mut ref_stars,
            RegistrationCatalog::Target => &mut target_stars,
        };
        stars[5].fwhm = f32::INFINITY;

        let error = register(&ref_stars, &target_stars, &Config::default()).unwrap_err();
        match error {
            RegistrationError::InvalidStarFwhm {
                catalog: actual,
                index,
                value,
            } => {
                assert_eq!(actual, catalog);
                assert_eq!(index, 5);
                assert_eq!(value, f32::INFINITY);
            }
            other => panic!("expected invalid star FWHM, got {other:?}"),
        }
    }
}
