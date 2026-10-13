use std::f64::consts::SQRT_2;

use glam::DVec2;

use crate::registration::point_normalization::{PointNormalization, centroid};

#[test]
fn centroid_is_the_mean_and_the_origin_for_none() {
    assert_eq!(centroid(&[]), DVec2::ZERO);
    assert_eq!(centroid(&[DVec2::new(7.0, -3.0)]), DVec2::new(7.0, -3.0));
    // ((1 + 3 + 5) / 3, (2 + 4 + 6) / 3) = (3, 4).
    let points = [
        DVec2::new(1.0, 2.0),
        DVec2::new(3.0, 4.0),
        DVec2::new(5.0, 6.0),
    ];
    assert_eq!(centroid(&points), DVec2::new(3.0, 4.0));
}

/// The square (0,0)–(10,10): centroid (5, 5), every corner 5√2 away, so the Hartley scale is
/// 5√2 / √2 = 5 and the corners normalize to (±1, ±1), average distance √2. Both transforms
/// agree with the point maps, and denormalizing undoes normalizing.
#[test]
fn hartley_puts_the_average_distance_at_root_two() {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(0.0, 10.0),
    ];
    let normalization = PointNormalization::hartley(&points);
    assert_eq!(
        normalization,
        PointNormalization::new(DVec2::new(5.0, 5.0), 5.0)
    );
    let expected = [
        DVec2::new(-1.0, -1.0),
        DVec2::new(1.0, -1.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(-1.0, 1.0),
    ];
    for (&point, &normalized) in points.iter().zip(&expected) {
        assert_eq!(normalization.normalize(point), normalized);
        assert_eq!(
            normalization.normalizing_transform().apply(point),
            normalized
        );
        assert_eq!(
            normalization.denormalizing_transform().apply(normalized),
            point
        );
        assert_eq!(normalization.denormalize(normalized), point);
    }
    let average = expected.iter().map(|p| p.length()).sum::<f64>() / expected.len() as f64;
    assert_eq!(average, SQRT_2);
}

#[test]
fn no_spread_normalizes_to_the_identity() {
    assert_eq!(
        PointNormalization::hartley(&[]),
        PointNormalization::new(DVec2::ZERO, 1.0)
    );
    let coincident = [DVec2::new(5.0, 5.0); 4];
    assert_eq!(
        PointNormalization::hartley(&coincident),
        PointNormalization::new(DVec2::ZERO, 1.0)
    );
    assert_eq!(
        PointNormalization::around(&coincident, DVec2::new(5.0, 5.0)),
        PointNormalization::new(DVec2::new(5.0, 5.0), 1.0)
    );
}

/// Unit squares at 10¹⁰: the centroid is 10¹⁰ + ½ and every corner is √2/2 from it, so the
/// corners normalize to (±1, ±1) exactly — the offset is removed before any scaling.
#[test]
fn hartley_conditions_points_far_from_the_origin() {
    let base = 1e10;
    let points = [
        DVec2::new(base, base),
        DVec2::new(base + 1.0, base),
        DVec2::new(base, base + 1.0),
        DVec2::new(base + 1.0, base + 1.0),
    ];
    let normalization = PointNormalization::hartley(&points);
    assert_eq!(normalization.normalize(points[3]), DVec2::new(1.0, 1.0));
    assert_eq!(
        centroid(&points.map(|p| normalization.normalize(p))),
        DVec2::ZERO
    );
}

/// `around` keeps the given center and scales by the mean distance from it: the points (3, 4)
/// and (−3, −4) are both 5 from the origin.
#[test]
fn around_scales_by_the_mean_distance_from_its_center() {
    let points = [DVec2::new(3.0, 4.0), DVec2::new(-3.0, -4.0)];
    let normalization = PointNormalization::around(&points, DVec2::ZERO);
    assert_eq!(normalization, PointNormalization::new(DVec2::ZERO, 5.0));
    assert_eq!(
        normalization.normalize_delta(DVec2::new(10.0, 0.0)),
        DVec2::new(2.0, 0.0)
    );
    assert_eq!(
        normalization.denormalize_delta(DVec2::new(2.0, 0.0)),
        DVec2::new(10.0, 0.0)
    );
}
