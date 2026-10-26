//! Registration's handling of the star lists it is handed: too few, degenerate FWHM,
//! mismatched counts.

use glam::DVec2;

use crate::registration::error::{RegistrationCatalog, RegistrationError, StarField};
use crate::registration::register::median_fwhm;
use crate::registration::registration_config::RegistrationConfig;
use crate::star_detection::star::Star;

// Registration reads only `pos` and `fwhm`, so these fixtures set the FWHM under test and
// leave everything else — position included — at its `Star::at` default.

/// The noise scale reads the median FWHM of both catalogs together, the middle pair averaged as
/// the detector's own median is: of 2, 3, 4 and 2.5, 3.5, 3; of 2, 5 and 4, 3, (3 + 4)/2 = 3.5;
/// of 1.5, 2.5, 3.5 and none, 2.5. Every value is exact in `f32`.
#[test]
fn the_median_fwhm_is_of_both_catalogs() {
    let stars = |fwhms: &[f32]| -> Vec<Star> {
        fwhms
            .iter()
            .map(|&fwhm| Star::at(DVec2::ZERO).with_fwhm(fwhm))
            .collect()
    };
    for (reference, target, median) in [
        (&[2.0, 3.0, 4.0][..], &[2.5, 3.5][..], 3.0),
        (&[2.0, 5.0], &[4.0, 3.0], 3.5),
        (&[1.5, 2.5, 3.5], &[], 2.5),
    ] {
        assert_eq!(
            median_fwhm(&stars(reference), &stars(target)),
            median,
            "{reference:?} and {target:?}"
        );
    }
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
