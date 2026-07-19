//! Integration tests for deblending algorithms.
//! These tests compare behavior between local_maxima and multi_threshold.

use crate::stacking::star_detection::deblend::local_maxima::deblend_local_maxima;
use crate::stacking::star_detection::deblend::test_support::{
    TestComponent, deblend_multi_threshold_test, make_test_component,
};

#[test]
fn test_local_vs_multi_threshold_single_star() {
    // Both algorithms should produce same result for single star
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);

    // Local maxima deblending (default: min_separation=3, min_prominence=0.3)
    let local_result = deblend_local_maxima(&data, &pixels, &labels, 3, 0.3);

    // Multi-threshold deblending (default: n_thresholds=32, min_separation=3, min_contrast=0.005)
    let mt_result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

    assert_eq!(local_result.len(), 1);
    assert_eq!(mt_result.len(), 1);

    // Peak positions should be similar
    assert!((local_result[0].peak.x as i32 - mt_result[0].peak.x as i32).abs() <= 1);
    assert!((local_result[0].peak.y as i32 - mt_result[0].peak.y as i32).abs() <= 1);
}

#[test]
fn test_local_vs_multi_threshold_two_stars() {
    // Both algorithms should find two stars when well-separated
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    // Local maxima deblending
    let local_result = deblend_local_maxima(&data, &pixels, &labels, 3, 0.3);

    // Multi-threshold deblending
    let mt_result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

    assert_eq!(local_result.len(), 2, "Local maxima should find 2 stars");
    assert_eq!(mt_result.len(), 2, "Multi-threshold should find 2 stars");
}
