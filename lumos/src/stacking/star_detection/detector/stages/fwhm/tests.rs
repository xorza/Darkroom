use crate::stacking::star_detection::detector::stages::fwhm::*;
use crate::testing::prelude::*;

// FWHM estimation never reads position, so every fixture below sits at the origin, and the
// remaining `Star::at` defaults already clear `filter_config()` — so each test spoils exactly
// the one property it is about.

/// Estimation settings with a 4.0 fallback, so a fallback result is distinguishable
/// from the 3.0 the star fixtures below are built around.
fn fwhm_config(min_stars: usize) -> FwhmConfig {
    FwhmConfig {
        expected: 4.0,
        min_stars,
        ..Default::default()
    }
}

/// Quality bounds loose enough that only the star each test spoils gets rejected.
fn filter_config() -> FilterConfig {
    FilterConfig {
        max_eccentricity: 0.8,
        max_sharpness: 0.7,
        ..Default::default()
    }
}

#[test]
fn fwhm_estimation_insufficient_stars() {
    // Fewer than min_stars returns default FWHM
    let stars: Vec<Star> = (0..4)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(3.0))
        .collect();

    let result = from_stars(&stars, &fwhm_config(5), &filter_config());

    // The fallback FWHM has no dependence on the 4 candidate stars, so it is `Configured` —
    // not an `Estimated` claiming 4 stars produced it.
    assert!(matches!(result, FwhmSource::Configured(fwhm) if (fwhm - 4.0).abs() < 0.01));
}

#[test]
fn fwhm_estimation_pre_rejection_median_reports_pre_rejection_count() {
    // 6 stars at FWHM=3.0 (tight cluster) + 4 outliers at FWHM=18.0. MAD-based
    // rejection drops all 4 outliers, leaving 6 < min_stars(7), so the function
    // falls back to the pre-rejection median (still 3.0, computed over all 10
    // stars). stars_used must reflect that provenance: 10 stars actually
    // contributed to the returned value (non-zero, since a pre-rejection median
    // is still a genuine auto-estimate, unlike the insufficient-stars fallback).
    let mut stars: Vec<Star> = (0..6)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(3.0))
        .collect();
    stars.extend((0..4).map(|_| Star::at(DVec2::ZERO).with_fwhm(18.0)));

    let result = from_stars(&stars, &fwhm_config(7), &filter_config());

    let fwhm = result.value().expect("an FWHM was estimated");
    assert!((fwhm - 3.0).abs() < 0.01);
    assert!(
        matches!(result, FwhmSource::Estimated { stars_used: 10, .. }),
        "the pre-rejection count that produced the median must be what is reported, got {result:?}"
    );
}

#[test]
fn fwhm_estimation_filters_saturated() {
    // Saturated stars (peak > 0.95) are excluded
    // 9 good stars at FWHM=3.0 + 1 saturated at FWHM=10.0
    let mut stars: Vec<Star> = (0..10)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(3.0))
        .collect();
    stars[0] = Star::at(DVec2::ZERO).with_fwhm(10.0).with_peak(0.98);

    let result = from_stars(&stars, &fwhm_config(5), &filter_config());

    // All 9 good stars have FWHM=3.0, so median should be exactly 3.0
    let fwhm = result
        .value()
        .expect("FWHM should be 3.0 (saturated star filtered)");
    assert!(
        (fwhm - 3.0).abs() < 0.01,
        "FWHM should be 3.0 (saturated star filtered), got {fwhm}"
    );
}

#[test]
fn fwhm_estimation_filters_high_eccentricity() {
    // High eccentricity stars (> max_eccentricity=0.8) are excluded
    let mut stars: Vec<Star> = (0..10)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(3.0))
        .collect();
    stars[0] = Star::at(DVec2::ZERO).with_fwhm(10.0).with_eccentricity(0.9);

    let result = from_stars(&stars, &fwhm_config(5), &filter_config());

    let fwhm = result
        .value()
        .expect("FWHM should be 3.0 (high-ecc star filtered)");
    assert!(
        (fwhm - 3.0).abs() < 0.01,
        "FWHM should be 3.0 (high-ecc star filtered), got {fwhm}"
    );
}

#[test]
fn fwhm_estimation_filters_cosmic_rays() {
    // High sharpness (cosmic rays, sharpness >= 0.7) are excluded
    let mut stars: Vec<Star> = (0..10)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(3.0))
        .collect();
    stars[0] = Star::at(DVec2::ZERO).with_fwhm(1.0).with_sharpness(0.9);

    let result = from_stars(&stars, &fwhm_config(5), &filter_config());

    let fwhm = result
        .value()
        .expect("FWHM should be 3.0 (cosmic ray filtered)");
    assert!(
        (fwhm - 3.0).abs() < 0.01,
        "FWHM should be 3.0 (cosmic ray filtered), got {fwhm}"
    );
}

#[test]
fn fwhm_estimation_filters_invalid_fwhm() {
    // FWHM outside valid range (0.5..20.0) are excluded
    let mut stars: Vec<Star> = (0..10)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(3.0))
        .collect();
    stars[0] = Star::at(DVec2::ZERO).with_fwhm(0.2); // Too small
    stars[1] = Star::at(DVec2::ZERO).with_fwhm(25.0); // Too large

    let result = from_stars(&stars, &fwhm_config(5), &filter_config());

    // 8 remaining stars all at FWHM=3.0
    let fwhm = result
        .value()
        .expect("FWHM should be 3.0 (invalid FWHM stars filtered)");
    assert!(
        (fwhm - 3.0).abs() < 0.01,
        "FWHM should be 3.0 (invalid FWHM stars filtered), got {fwhm}"
    );
}

#[test]
fn fwhm_estimation_rejects_outliers() {
    // 10 stars at FWHM=3.0 + 2 outliers at 12.0 and 15.0
    let mut stars: Vec<Star> = (0..10)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(3.0))
        .collect();
    stars.push(Star::at(DVec2::ZERO).with_fwhm(12.0));
    stars.push(Star::at(DVec2::ZERO).with_fwhm(15.0));

    let result = from_stars(&stars, &fwhm_config(5), &filter_config());

    // MAD-based rejection should remove the 12.0 and 15.0 outliers
    let fwhm = result
        .value()
        .expect("FWHM should be 3.0 (outliers rejected)");
    assert!(
        (fwhm - 3.0).abs() < 0.01,
        "FWHM should be 3.0 (outliers rejected), got {fwhm}"
    );
}

#[test]
fn fwhm_estimation_uniform_values() {
    // All identical FWHM values
    let stars: Vec<Star> = (0..10)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(4.5))
        .collect();

    let result = from_stars(&stars, &fwhm_config(5), &filter_config());

    let fwhm = result.value().expect("an FWHM was estimated");
    assert!((fwhm - 4.5).abs() < 0.01);
    assert!(matches!(
        result,
        FwhmSource::Estimated { stars_used: 10, .. }
    ));
}

#[test]
fn fwhm_estimation_varying_values() {
    // FWHM values: [2.8, 2.9, 2.9, 3.0, 3.0, 3.0, 3.1, 3.1, 3.2, 3.3]
    // Sorted: median is average of values at indices 4,5 = (3.0+3.0)/2 = 3.0
    // No outliers, so all 10 stars should be used
    let fwhms = [2.8, 3.0, 3.1, 3.2, 2.9, 3.3, 3.0, 3.1, 2.9, 3.0];
    let stars: Vec<Star> = fwhms
        .iter()
        .map(|&f| Star::at(DVec2::ZERO).with_fwhm(f))
        .collect();

    let result = from_stars(&stars, &fwhm_config(5), &filter_config());

    let fwhm = result.value().expect("an FWHM was estimated");
    // Median of sorted [2.8, 2.9, 2.9, 3.0, 3.0, 3.0, 3.1, 3.1, 3.2, 3.3]
    // = value at index 5 = 3.0
    assert!(
        (fwhm - 3.0).abs() < 0.05,
        "FWHM should be ~3.0 (median of varied values), got {fwhm}"
    );
    assert!(
        matches!(result, FwhmSource::Estimated { stars_used: 10, .. }),
        "all 10 stars should be used, got {result:?}"
    );
}

#[test]
fn fwhm_estimation_empty_after_filtering() {
    // All stars filtered out → returns default with 0 stars
    let stars: Vec<Star> = (0..10)
        .map(|_| Star::at(DVec2::ZERO).with_fwhm(3.0).with_peak(0.98)) // All saturated
        .collect();

    let result = from_stars(&stars, &fwhm_config(5), &filter_config());

    assert!(matches!(result, FwhmSource::Configured(fwhm) if (fwhm - 4.0).abs() < 0.01));
}
