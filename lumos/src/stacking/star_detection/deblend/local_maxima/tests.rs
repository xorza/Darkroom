//! Tests for local maxima deblending.
//!
//! The synthetic stars sit on integer centres, so a centre pixel holds exactly its amplitude plus
//! what its neighbours add there — zero beyond their labelled cutoff — and peaks compare exactly.

use crate::math::urect::URect;
use crate::stacking::star_detection::config::detection_config::{Deblend, DetectionConfig};
use crate::stacking::star_detection::deblend::internals::{TestComponent, make_test_component};
use crate::stacking::star_detection::deblend::local_maxima::*;
use crate::stacking::star_detection::labeling::LabelMap;
use crate::stacking::star_detection::labeling::component_data::ComponentData;
use crate::testing::prelude::*;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

/// The default config's separation.
fn default_separation() -> usize {
    DetectionConfig::default().deblend_min_separation
}

/// The default config's prominence.
fn default_prominence() -> f32 {
    match DetectionConfig::default().deblend {
        Deblend::LocalMaxima { min_prominence } => min_prominence,
        Deblend::MultiThreshold { .. } => unreachable!("the default deblends by local maxima"),
    }
}

/// Gaussian stars of `sigma` at `(x, y, amplitude)`, as one labelled component.
fn stars(size: Size2us, sigma: f32, stars: &[(f32, f32, f32)]) -> TestComponent {
    let stars: Vec<SyntheticStar> = stars
        .iter()
        .map(|&(x, y, amplitude)| {
            SyntheticStar::new(Vec2::new(x, y), amplitude, StarProfile::Gaussian { sigma })
        })
        .collect();
    make_test_component(size, &stars)
}

/// `(x, y)` of each peak, in the order given.
fn positions(peaks: &[Pixel]) -> Vec<(usize, usize)> {
    peaks.iter().map(|p| (p.pos.x, p.pos.y)).collect()
}

#[test]
fn single_star_is_one_whole_region() {
    let fixture = stars(Size2us::new(100, 100), 3.0, &[(50.0, 50.0, 1.0)]);
    let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);

    let peaks = find_local_maxima(
        &component,
        default_separation(),
        default_prominence(),
        &mut Vec::new(),
    );
    assert_eq!(positions(&peaks), [(50, 50)]);
    assert_eq!(peaks[0].value, 1.0);

    let regions = deblend_local_maxima(
        &component,
        default_separation(),
        default_prominence(),
        &mut Vec::new(),
    );
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].bbox, fixture.data.bbox);
    assert_eq!(regions[0].area, fixture.data.area);
    assert_eq!(regions[0].peak, Vec2us::new(50, 50));
    assert_eq!(regions[0].peak_value, fixture.pixels[(50, 50)]);
}

#[test]
fn two_separated_stars() {
    // 40 px apart, each labelled out to where it falls to 0.001: two disjoint blobs, one label.
    // The Voronoi line is x = 50, clear of both, so each region is exactly its blob.
    let fixture = stars(
        Size2us::new(100, 100),
        2.5,
        &[(30.0, 50.0, 1.0), (70.0, 50.0, 0.8)],
    );
    let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);

    let peaks = find_local_maxima(&component, 3, 0.3, &mut Vec::new());
    assert_eq!(positions(&peaks), [(30, 50), (70, 50)]);
    assert_eq!((peaks[0].value, peaks[1].value), (1.0, 0.8));

    let regions = deblend_local_maxima(&component, 3, 0.3, &mut Vec::new());
    let blob_area = |left: bool| {
        component
            .pixels()
            .filter(|p| (p.pos.x < 50) == left)
            .count()
    };
    let summary: Vec<(Vec2us, f32, usize)> = regions
        .iter()
        .map(|r| (r.peak, r.peak_value, r.area))
        .collect();
    assert_eq!(
        summary,
        [
            (Vec2us::new(30, 50), 1.0, blob_area(true)),
            (Vec2us::new(70, 50), 0.8, blob_area(false)),
        ]
    );
    assert!(regions.iter().all(|r| r.bbox.contains(r.peak)));
    assert_eq!(
        regions.iter().map(|r| r.area).sum::<usize>(),
        fixture.data.area
    );
}

#[test]
fn euclidean_separation() {
    // Two peaks √18 ≈ 4.24 apart on the diagonal: within 5 (25 > 18), clear of 4 (16 < 18).
    let fixture = stars(
        Size2us::new(100, 100),
        1.5,
        &[(50.0, 50.0, 1.0), (53.0, 53.0, 0.9)],
    );
    let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);

    let merged = find_local_maxima(&component, 5, 0.3, &mut Vec::new());
    assert_eq!(positions(&merged), [(50, 50)]);
    let separate = find_local_maxima(&component, 4, 0.3, &mut Vec::new());
    assert_eq!(positions(&separate), [(50, 50), (53, 53)]);
}

#[test]
fn prominence_filter() {
    // The faint star peaks at 0.2 of the bright one: out at prominence 0.5, in at 0.1.
    let fixture = stars(
        Size2us::new(100, 100),
        2.5,
        &[(30.0, 50.0, 1.0), (70.0, 50.0, 0.2)],
    );
    let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);

    let strict = find_local_maxima(&component, 3, 0.5, &mut Vec::new());
    assert_eq!(positions(&strict), [(30, 50)]);
    let loose = find_local_maxima(&component, 3, 0.1, &mut Vec::new());
    assert_eq!(positions(&loose), [(30, 50), (70, 50)]);
}

#[test]
fn zero_min_separation_keeps_adjacent_peaks() {
    // Stars 3 px apart, σ 1: both centres are local maxima (the faint one's 0.8 + e^−4.5 against
    // its neighbours' 0.62 and 0.49). Separation 0 suppresses nothing; 4 keeps the brighter.
    let fixture = stars(
        Size2us::new(100, 100),
        1.0,
        &[(50.0, 50.0, 1.0), (53.0, 50.0, 0.8)],
    );
    let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);

    let none = find_local_maxima(&component, 0, 0.1, &mut Vec::new());
    assert_eq!(positions(&none), [(50, 50), (53, 50)]);
    let four = find_local_maxima(&component, 4, 0.1, &mut Vec::new());
    assert_eq!(positions(&four), [(50, 50)]);
}

#[test]
fn equal_brightness_tie_breaking() {
    // Two equal stars 4 px apart, σ 1: each centre holds 1 + e^−8, bit for bit the same (the sum
    // commutes). Within a separation of 5 only one survives, and the tie goes to the first in
    // raster order; at 4 they are not too close and both stay.
    let fixture = stars(
        Size2us::new(100, 100),
        1.0,
        &[(48.0, 50.0, 1.0), (52.0, 50.0, 1.0)],
    );
    let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);
    assert_eq!(fixture.pixels[(48, 50)], fixture.pixels[(52, 50)]);

    let tied = find_local_maxima(&component, 5, 0.3, &mut Vec::new());
    assert_eq!(positions(&tied), [(48, 50)]);
    let both = find_local_maxima(&component, 4, 0.3, &mut Vec::new());
    assert_eq!(positions(&both), [(48, 50), (52, 50)]);
}

#[test]
fn peaks_sorted_by_brightness() {
    let fixture = stars(
        Size2us::new(100, 100),
        2.5,
        &[(30.0, 50.0, 0.5), (50.0, 50.0, 1.0), (70.0, 50.0, 0.7)],
    );
    let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);

    let peaks = find_local_maxima(&component, 3, 0.3, &mut Vec::new());
    assert_eq!(positions(&peaks), [(50, 50), (70, 50), (30, 50)]);

    // The component's own peak is the brightest pixel.
    assert_eq!(component.peak().pos, Vec2us::new(50, 50));
    assert_eq!(component.peak().value, fixture.pixels[(50, 50)]);
}

#[test]
fn close_peaks_keep_the_brighter() {
    // 1 px apart: one local maximum survives the sum, and it is the bright centre.
    let fixture = stars(
        Size2us::new(100, 100),
        1.5,
        &[(50.0, 50.0, 1.0), (51.0, 50.0, 0.8)],
    );
    let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);

    let peaks = find_local_maxima(&component, 5, 0.3, &mut Vec::new());
    assert_eq!(positions(&peaks), [(50, 50)]);
}

#[test]
fn many_peaks_keep_the_brightest() {
    // Twelve stars 8 px apart, brightening left to right (0.45 + 0.05·i), so raster order meets
    // the dimmest first. Neighbours add exp(−64/4.5) ≈ 7e-7 at each centre, so every centre is a
    // local maximum of its own amplitude. The kept eight must be the brightest — i = 11 down to
    // 4, at x = 10 + 8i — not the first eight scanned.
    let star_list: Vec<(f32, f32, f32)> = (0..12)
        .map(|i| ((10 + i * 8) as f32, 50.0, 0.45 + i as f32 * 0.05))
        .collect();
    let fixture = stars(Size2us::new(120, 100), 1.5, &star_list);

    let peaks = find_local_maxima(
        &Component::new(&fixture.data, &fixture.pixels, &fixture.labels),
        2,
        0.1,
        &mut Vec::new(),
    );

    let xs: Vec<usize> = peaks.iter().map(|p| p.pos.x).collect();
    assert_eq!(xs, [98, 90, 82, 74, 66, 58, 50, 42]);
    assert!(peaks.iter().all(|p| p.pos.y == 50));
}

#[test]
fn suppressed_peak_does_not_suppress_dimmer_ones() {
    // C (0.6) at x=50, B (0.8) at 53, A (1.0) at 56, σ = 1 px, min_separation = 4. Each centre is
    // a local maximum: at B, 0.8 + e^−4.5 + 0.6·e^−4.5 = 0.818 against 0.715 and 0.62 beside it.
    // Brightest first, A is kept, B (3 px from A) is suppressed, and C (6 px from A) is kept: B's
    // suppression must not carry over to C. Taken in raster order instead, B would replace C and
    // A would replace B, leaving A alone.
    let fixture = stars(
        Size2us::new(100, 100),
        1.0,
        &[(50.0, 50.0, 0.6), (53.0, 50.0, 0.8), (56.0, 50.0, 1.0)],
    );

    let peaks = find_local_maxima(
        &Component::new(&fixture.data, &fixture.pixels, &fixture.labels),
        4,
        0.1,
        &mut Vec::new(),
    );

    assert_eq!(positions(&peaks), [(56, 50), (50, 50)]);
}

#[test]
fn is_local_maximum_cases() {
    // Strictly above all eight neighbours that exist: a corner and an edge have fewer, a diagonal
    // ring counts as much as an orthogonal one, and an equal neighbour (a plateau) disqualifies.
    let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);
    pixels[(0, 0)] = 1.0;
    pixels[(1, 1)] = 0.5;
    pixels[(9, 9)] = 1.0;
    pixels[(5, 0)] = 1.0;
    pixels[(4, 0)] = 0.5;
    pixels[(5, 1)] = 0.5;
    pixels[(5, 5)] = 1.0;
    for (x, y) in [(4, 4), (6, 6), (4, 6), (6, 4)] {
        pixels[(x, y)] = 0.9;
    }
    pixels[(2, 8)] = 0.7;
    pixels[(3, 8)] = 0.7;
    pixels[(7, 2)] = 0.4;
    pixels[(8, 2)] = 0.6;

    let cases = [
        ((0, 0), true, "corner"),
        ((9, 9), true, "opposite corner"),
        ((5, 0), true, "top edge"),
        ((5, 5), true, "diagonal neighbours only, all lower"),
        ((4, 4), false, "below its diagonal neighbour"),
        ((2, 8), false, "plateau: an equal neighbour"),
        ((7, 2), false, "a brighter orthogonal neighbour"),
        ((8, 2), true, "above every neighbour"),
    ];
    for ((x, y), expected, why) in cases {
        let pixel = Pixel {
            pos: Vec2us::new(x, y),
            value: pixels[(x, y)],
        };
        assert_eq!(is_local_maximum(pixel, &pixels), expected, "{why}");
    }
}

#[test]
fn plateau_no_local_max() {
    let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(10, 10, 0u32);

    for y in 3..7 {
        for x in 3..7 {
            pixels[(x, y)] = 1.0;
            labels_buf[(x, y)] = 1;
        }
    }

    let labels = LabelMap::from_raw(labels_buf, 1);
    let data = ComponentData {
        bbox: URect::new(Vec2us::new(3, 3), Vec2us::new(7, 7)),
        label: 1,
        area: 16,
    };
    let component = Component::new(&data, &pixels, &labels);

    let peaks = find_local_maxima(&component, 1, 0.0, &mut Vec::new());
    assert!(peaks.is_empty(), "a plateau has no strict local maximum");

    // With no maximum the component stays whole, peaked at its first brightest pixel.
    let regions = deblend_local_maxima(&component, 1, 0.0, &mut Vec::new());
    assert_eq!(regions.len(), 1);
    assert_eq!((regions[0].peak, regions[0].area), (Vec2us::new(3, 3), 16));
}

#[test]
fn single_pixel_is_local_max() {
    let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(10, 10, 0u32);
    pixels[(5, 5)] = 1.0;
    labels_buf[(5, 5)] = 1;

    let labels = LabelMap::from_raw(labels_buf, 1);
    let data = ComponentData {
        bbox: URect::new(Vec2us::new(5, 5), Vec2us::new(6, 6)),
        label: 1,
        area: 1,
    };

    let peaks = find_local_maxima(
        &Component::new(&data, &pixels, &labels),
        3,
        0.3,
        &mut Vec::new(),
    );
    assert_eq!(positions(&peaks), [(5, 5)]);
}

#[test]
fn voronoi_assignment_and_its_tie() {
    // A 61-pixel line x = 20..=80 with peaks at its ends. Pixel 50 is 30 from both, and the tie
    // goes to the first peak given: 31 pixels to x = 20, 30 to x = 80. No peaks, no regions.
    let mut pixels = Buffer2::new_filled(100, 100, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(100, 100, 0u32);
    for x in 20..=80 {
        pixels[(x, 50)] = 0.5;
        labels_buf[(x, 50)] = 1;
    }
    pixels[(20, 50)] = 1.0;
    pixels[(80, 50)] = 1.0;

    let labels = LabelMap::from_raw(labels_buf, 1);
    let data = ComponentData {
        bbox: URect::new(Vec2us::new(20, 50), Vec2us::new(81, 51)),
        label: 1,
        area: 61,
    };
    let component = Component::new(&data, &pixels, &labels);
    let peak = |x: usize| Pixel {
        pos: Vec2us::new(x, 50),
        value: 1.0,
    };

    let regions = component.assign_to_nearest(&[peak(20), peak(80)]);
    let summary: Vec<(usize, URect)> = regions.iter().map(|r| (r.area, r.bbox)).collect();
    assert_eq!(
        summary,
        [
            (31, URect::new(Vec2us::new(20, 50), Vec2us::new(51, 51))),
            (30, URect::new(Vec2us::new(51, 50), Vec2us::new(81, 51))),
        ]
    );

    assert!(component.assign_to_nearest(&[]).is_empty());
}
