//! Tests of the shared component data, and comparisons of `local_maxima` with
//! `multi_threshold`.

use glam::Vec2;

use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::internals::{
    Ladder, TestComponent, deblend_multi_threshold_test, make_test_component, separated_pair,
};
use crate::star_detection::deblend::local_maxima::LocalMaximaParams;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::star_detection_config::extraction_config::{Deblend, ExtractionConfig};
use common::USize2;

/// [`LocalMaximaParams::deblend`] at the default config's separation and relative height, into a
/// fresh list.
fn local_maxima_regions(component: &Component<'_>) -> Vec<Region> {
    let config = ExtractionConfig::default();
    let Deblend::LocalMaxima {
        min_relative_height,
    } = config.deblend
    else {
        unreachable!("the default deblends by local maxima")
    };
    let mut regions = Vec::new();
    LocalMaximaParams {
        min_separation: config.deblend_min_separation,
        min_relative_height,
    }
    .deblend(component, &mut DeblendBuffers::default(), &mut regions);
    regions
}

#[test]
fn local_vs_multi_threshold_single_star() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        USize2::new(100, 100),
        &[SyntheticStar::new(
            Vec2::new(50.0, 50.0),
            1.0,
            StarProfile::gaussian(3.0),
        )],
    );

    let local_result = local_maxima_regions(&Component::new(&data, &pixels, &labels));

    let mt_result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), Ladder::STANDARD);

    assert_eq!(local_result.len(), 1);
    assert_eq!(mt_result.len(), 1);

    assert!((local_result[0].peak.x as i32 - mt_result[0].peak.x as i32).abs() <= 1);
    assert!((local_result[0].peak.y as i32 - mt_result[0].peak.y as i32).abs() <= 1);
}

#[test]
fn local_vs_multi_threshold_two_stars() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(0.8);

    let local_result = local_maxima_regions(&Component::new(&data, &pixels, &labels));

    let mt_result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), Ladder::STANDARD);

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
        USize2::new(100, 100),
        &[SyntheticStar::new(
            Vec2::new(50.0, 50.0),
            1.0,
            StarProfile::gaussian(3.0),
        )],
    );

    let iter_count = Component::new(&data, &pixels, &labels).pixels().count();
    assert_eq!(
        iter_count, data.area,
        "iter_pixels should yield exactly area pixels"
    );
}
