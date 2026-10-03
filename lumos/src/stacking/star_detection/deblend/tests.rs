//! Tests of the shared component data, and comparisons of `local_maxima` with
//! `multi_threshold`.

use crate::stacking::star_detection::deblend::component::Component;
use crate::stacking::star_detection::deblend::internals::{
    TestComponent, deblend_multi_threshold_test, make_test_component, separated_pair,
};
use crate::stacking::star_detection::deblend::local_maxima::deblend_local_maxima;
use crate::testing::prelude::*;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

#[test]
fn local_vs_multi_threshold_single_star() {
    // Both algorithms should produce same result for single star
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(100, 100),
        &[SyntheticStar::new(
            Vec2::new(50.0, 50.0),
            1.0,
            StarProfile::Gaussian { sigma: 3.0 },
        )],
    );

    // Local maxima deblending (default: min_separation=3, min_prominence=0.3)
    let local_result = deblend_local_maxima(
        &Component::new(&data, &pixels, &labels),
        3,
        0.3,
        &mut Vec::new(),
    );

    // Multi-threshold deblending (default: n_thresholds=32, min_separation=3, min_contrast=0.005)
    let mt_result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(local_result.len(), 1);
    assert_eq!(mt_result.len(), 1);

    // Peak positions should be similar
    assert!((local_result[0].peak.x as i32 - mt_result[0].peak.x as i32).abs() <= 1);
    assert!((local_result[0].peak.y as i32 - mt_result[0].peak.y as i32).abs() <= 1);
}

#[test]
fn local_vs_multi_threshold_two_stars() {
    // Both algorithms should find two stars when well-separated
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(0.8);

    // Local maxima deblending
    let local_result = deblend_local_maxima(
        &Component::new(&data, &pixels, &labels),
        3,
        0.3,
        &mut Vec::new(),
    );

    // Multi-threshold deblending
    let mt_result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(local_result.len(), 2, "Local maxima should find 2 stars");
    assert_eq!(mt_result.len(), 2, "Multi-threshold should find 2 stars");
}

#[test]
fn iter_pixels_count() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(100, 100),
        &[SyntheticStar::new(
            Vec2::new(50.0, 50.0),
            1.0,
            StarProfile::Gaussian { sigma: 3.0 },
        )],
    );

    let iter_count = Component::new(&data, &pixels, &labels).pixels().count();
    assert_eq!(
        iter_count, data.area,
        "iter_pixels should yield exactly area pixels"
    );
}
