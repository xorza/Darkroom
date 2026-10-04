//! Tests for multi-threshold deblending.

#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::internals::prelude::*;
use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::math::urect::URect;
use crate::star_detection::deblend::internals::{
    TestComponent, deblend_multi_threshold_floored, deblend_multi_threshold_test,
    make_test_component, separated_pair,
};
use crate::star_detection::deblend::local_maxima::LocalMaximaParams;
use crate::star_detection::deblend::multi_threshold::*;
use crate::star_detection::labeling::LabelMap;
use crate::star_detection::labeling::component_data::ComponentData;

#[test]
fn single_star_no_deblending() {
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
    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(result.len(), 1, "Single star should produce one object");
    assert!((result[0].peak.x as i32 - 50).abs() <= 1);
    assert!((result[0].peak.y as i32 - 50).abs() <= 1);
}

#[test]
fn two_separated_stars_deblend() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(0.8);

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(
        result.len(),
        2,
        "Two separated stars should produce two objects"
    );

    let mut peaks: Vec<_> = result.iter().map(|o| (o.peak.x, o.peak.y)).collect();
    peaks.sort_by_key(|&(x, _)| x);

    assert!(
        (peaks[0].0 as i32 - 30).abs() <= 2,
        "First peak should be near x=30"
    );
    assert!(
        (peaks[1].0 as i32 - 70).abs() <= 2,
        "Second peak should be near x=70"
    );
}

#[test]
fn late_gaussian_split_uses_full_threshold_ladder() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(100, 100),
        &[
            SyntheticStar::new(
                Vec2::new(44.0, 50.0),
                1.0,
                StarProfile::Gaussian { sigma: 4.0 },
            ),
            SyntheticStar::new(
                Vec2::new(56.0, 50.0),
                1.0,
                StarProfile::Gaussian { sigma: 4.0 },
            ),
        ],
    );

    // The ladder `deblend_multi_threshold_test` cuts at: from the component's faintest pixel to
    // its peak. The pair only separates above their saddle, late in it — so the deblender must
    // run the ladder that far, not stop after its first few levels.
    let threshold_count = 32;
    let component = Component::new(&data, &pixels, &labels);
    let ladder = ThresholdLadder {
        low: component
            .pixels()
            .map(|pixel| pixel.value)
            .fold(f32::MAX, f32::min),
        high: component.peak().value,
        n_thresholds: threshold_count,
    };
    let saddle = pixels[(50, 50)];
    let split_level = (0..=threshold_count)
        .find(|&level| ladder.level(level) > saddle)
        .unwrap();
    assert!(
        split_level > 4,
        "fixture must stay connected through the ladder's first levels, split at {split_level}"
    );

    let result = deblend_multi_threshold_test(
        &Component::new(&data, &pixels, &labels),
        threshold_count,
        3,
        0.005,
    );
    let mut peaks: Vec<_> = result.iter().map(|region| region.peak.x).collect();
    peaks.sort_unstable();

    assert_eq!(
        peaks,
        vec![44, 56],
        "the configured full ladder must reach both peaks after crossing their saddle"
    );
}

#[test]
fn faint_secondary_below_contrast() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(0.001);

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.01);

    assert_eq!(
        result.len(),
        1,
        "Faint secondary should not cause deblending"
    );
}

#[test]
fn threshold_ladder_is_exponential_from_floor_to_peak() {
    // 0.1 to 1.0 in ten steps: each level 10^(1/10) ≈ 1.2589 times the last. Level 0 is `low`
    // exactly (a zeroth power); the rest carry one powf and one multiply of f32 rounding, a few
    // ulps — 8ε relative bounds them.
    let ladder = ThresholdLadder {
        low: 0.1,
        high: 1.0,
        n_thresholds: 10,
    };
    let tolerance = |value: f32| 8.0 * f32::EPSILON * value;
    assert_eq!(ladder.level(0), 0.1);
    assert!((ladder.level(10) - 1.0).abs() <= tolerance(1.0));
    let step = 10f32.powf(0.1);
    for i in 1..=10 {
        let expected = 0.1 * step.powi(i as i32);
        assert!(
            (ladder.level(i) - expected).abs() <= tolerance(expected),
            "level {i}: {} vs {expected}",
            ladder.level(i)
        );
    }
}

#[test]
fn close_peaks_merge() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(100, 100),
        &[
            SyntheticStar::new(
                Vec2::new(48.0, 50.0),
                1.0,
                StarProfile::Gaussian { sigma: 2.0 },
            ),
            SyntheticStar::new(
                Vec2::new(52.0, 50.0),
                0.9,
                StarProfile::Gaussian { sigma: 2.0 },
            ),
        ],
    );

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 5, 0.005);

    assert_eq!(result.len(), 1, "Close peaks should not be deblended");
}

#[test]
fn deblend_disabled_with_high_contrast() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(0.8);

    let result = deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 1.0);

    assert_eq!(
        result.len(),
        1,
        "High contrast setting should disable deblending"
    );
}

#[test]
fn three_stars_deblend() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(150, 100),
        &[
            SyntheticStar::new(
                Vec2::new(30.0, 50.0),
                1.0,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(75.0, 50.0),
                0.9,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(120.0, 50.0),
                0.8,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
        ],
    );

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(
        result.len(),
        3,
        "Three separated stars should produce three objects"
    );

    let mut peaks: Vec<_> = result.iter().map(|o| o.peak.x).collect();
    peaks.sort_unstable();
    assert!((peaks[0] as i32 - 30).abs() <= 2);
    assert!((peaks[1] as i32 - 75).abs() <= 2);
    assert!((peaks[2] as i32 - 120).abs() <= 2);
}

#[test]
fn hierarchical_deblend() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(150, 100),
        &[
            SyntheticStar::new(
                Vec2::new(30.0, 50.0),
                1.0,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(100.0, 50.0),
                0.8,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(115.0, 50.0),
                0.7,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
        ],
    );

    let result = deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.1);

    assert!(
        result.len() >= 2,
        "Should find at least the isolated star and close pair"
    );
}

/// The bottom-up walk over hand-built trees, at min_contrast 0.2 of a root of 100: a bar of 20.
/// - `root → [bright(60), dim(40) → [mid(25), faint(12)]]`: the root splits in two, and inside dim
///   only mid clears the bar, so dim stays whole: {bright, dim}. A parent-relative bar, 0.2·40 = 8,
///   would split dim too.
/// - Review example 9.6, `root → [A(80) → [A1(35), A2(30)], B(5)]`: A splits into A1 and A2, which
///   marks the root split, and the root's own split has one significant son, so nothing more is
///   added: {A1, A2}. A top-down walk stops at the root, which one significant son leaves whole.
/// - `root → [A(80) → [A1(15), A2(30)], B(25)]`: A has one significant son and stays whole, and
///   the root splits into A and B: {A, B}.
#[test]
fn stars_are_found_from_the_top_level_down() {
    fn tree(objects: &[(u32, f32)]) -> TreeBuffers {
        TreeBuffers {
            objects: objects
                .iter()
                .map(|&(parent, flux_above)| TreeObject {
                    parent,
                    peak: Pixel {
                        pos: Vec2us::new(0, 0),
                        value: flux_above,
                    },
                    flux_above,
                })
                .collect(),
            ..TreeBuffers::default()
        }
    }
    for (objects, expected) in [
        (
            vec![(0, 0.0), (0, 60.0), (0, 40.0), (2, 25.0), (2, 12.0)],
            vec![1, 2],
        ),
        (
            vec![(0, 0.0), (0, 80.0), (0, 5.0), (1, 35.0), (1, 30.0)],
            vec![3, 4],
        ),
        (
            vec![(0, 0.0), (0, 80.0), (0, 25.0), (1, 15.0), (1, 30.0)],
            vec![1, 2],
        ),
    ] {
        let mut tree = tree(&objects);
        tree.find_stars(20.0);
        tree.stars.sort_unstable();
        assert_eq!(tree.stars, expected, "{objects:?}");
    }
}

#[test]
fn equal_brightness_stars() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(1.0);

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(
        result.len(),
        2,
        "Equal brightness stars should both be found"
    );

    let area_diff = (result[0].area as i32 - result[1].area as i32).abs();
    let avg_area = usize::midpoint(result[0].area, result[1].area);
    assert!(
        area_diff < (avg_area as i32 / 2),
        "Equal stars should have similar areas"
    );
}

#[test]
fn contrast_at_boundary() {
    // Two disjoint stars, σ 2.5, amplitudes 1.0 and 0.1, under one label: they split at the
    // floor, each branch the whole star. The labelled pixels stop where a star falls to 0.001,
    // at r = σ·√(2 ln(A/0.001)): the bright one keeps 1 − 0.001 of its flux, the faint one
    // 1 − 0.01. The faint branch's share is 0.1·0.99 / (0.1·0.99 + 0.999) = 0.0902: a contrast of
    // 0.08 splits it off, 0.10 does not.
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(0.1);
    let component = Component::new(&data, &pixels, &labels);
    assert_eq!(
        deblend_multi_threshold_test(&component, 32, 3, 0.08).len(),
        2
    );
    assert_eq!(
        deblend_multi_threshold_test(&component, 32, 3, 0.10).len(),
        1
    );
}

#[test]
fn pixel_assignment_conservation() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(0.8);

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(
        total_area, data.area,
        "Sum of deblended areas should equal original area"
    );
}

#[test]
fn vertical_star_pair() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(100, 100),
        &[
            SyntheticStar::new(
                Vec2::new(50.0, 30.0),
                1.0,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(50.0, 70.0),
                0.8,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
        ],
    );

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(result.len(), 2, "Vertically separated stars should deblend");

    let mut peaks: Vec<_> = result.iter().map(|o| o.peak.y).collect();
    peaks.sort_unstable();
    assert!((peaks[0] as i32 - 30).abs() <= 2);
    assert!((peaks[1] as i32 - 70).abs() <= 2);
}

#[test]
fn diagonal_star_pair() {
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(100, 100),
        &[
            SyntheticStar::new(
                Vec2::new(30.0, 30.0),
                1.0,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(70.0, 70.0),
                0.8,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
        ],
    );

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(result.len(), 2, "Diagonally separated stars should deblend");
}

#[test]
fn n_thresholds_effect() {
    // A 1.0 and a 0.9 star 8 px apart, σ 2.5: the saddle midway holds (1.0 + 0.9)·e^(−16/12.5) =
    // 0.528 and the peaks 1.006 and 0.906. The ladder runs from the faintest labelled pixel
    // (~0.001) to 1.006, a ratio of about 1000. At four levels the top two sit at 0.179 — below
    // the saddle, still one region — and at the bright peak itself, above the faint one: no level
    // falls between, so the pair never splits. At 64 levels, 1.114 apart, several do.
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(100, 100),
        &[
            SyntheticStar::new(
                Vec2::new(46.0, 50.0),
                1.0,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(54.0, 50.0),
                0.9,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
        ],
    );
    let component = Component::new(&data, &pixels, &labels);
    assert_eq!(
        deblend_multi_threshold_test(&component, 4, 3, 0.005).len(),
        1
    );
    let split = deblend_multi_threshold_test(&component, 64, 3, 0.005);
    let mut peaks: Vec<usize> = split.iter().map(|region| region.peak.x).collect();
    peaks.sort_unstable();
    assert_eq!(peaks, [46, 54]);
}

#[test]
fn single_pixel_component() {
    let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(10, 10, 0u32);

    pixels[(5, 5)] = 1.0;
    labels_buf[(5, 5)] = 1;

    let labels = LabelMap::from_raw(&labels_buf, 1);
    let data = ComponentData {
        bbox: URect::new(Vec2us::new(5, 5), Vec2us::new(6, 6)),
        label: 1,
        area: 1,
    };

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(result.len(), 1, "Single pixel should produce one object");
    assert_eq!(result[0].area, 1);
}

#[test]
fn flat_profile_no_deblend() {
    let mut pixels = Buffer2::new_filled(50, 50, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(50, 50, 0u32);

    let mut bbox = URect::empty();
    let mut area = 0;

    for y in 10..40 {
        for x in 10..40 {
            let dx = x as i32 - 25;
            let dy = y as i32 - 25;
            if dx * dx + dy * dy < 150 {
                pixels[(x, y)] = 1.0;
                labels_buf[(x, y)] = 1;
                bbox.include(Vec2us::new(x, y));
                area += 1;
            }
        }
    }

    let labels = LabelMap::from_raw(&labels_buf, 1);
    let data = ComponentData {
        bbox,
        label: 1,
        area,
    };

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    assert_eq!(
        result.len(),
        1,
        "Flat profile should not deblend into multiple objects"
    );
}

#[test]
fn zero_valued_pixels_below_the_floor_do_not_prevent_deblending() {
    // A component whose residual is exactly 0.0 over most of it (a bounding box padded well past
    // both stars' Gaussian falloff), as a matched-filter detection's wings can be. The ladder
    // starts at the detection threshold, not at the component's faintest pixel, so those pixels
    // sit below every level, join no branch, and go to their nearest peak at the end: the two
    // well-separated stars must still deblend into two objects that share the whole area.
    let width = 100;
    let height = 100;
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(width, height, 0u32);

    let mut bbox = URect::empty();
    let mut area = 0;
    // The whole rectangle is one connected component; most of it stays at 0.0.
    for y in 10..90 {
        for x in 10..90 {
            labels_buf[(x, y)] = 1;
            bbox.include(Vec2us::new(x, y));
            area += 1;
        }
    }

    for &star in &[
        SyntheticStar::new(
            Vec2::new(30.0, 50.0),
            1.0,
            StarProfile::Gaussian { sigma: 2.5 },
        ),
        SyntheticStar::new(
            Vec2::new(70.0, 50.0),
            0.8,
            StarProfile::Gaussian { sigma: 2.5 },
        ),
    ] {
        let StarProfile::Gaussian { sigma } = star.profile else {
            unreachable!("this fixture's stars are all Gaussian")
        };
        // 6σ, not the profile's own 4σ: this component is padded well past the falloff on purpose.
        let radius = (sigma * 6.0).ceil() as i32;
        let star_pixels = star.pixels();
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (star.center.x as i32 + dx) as usize;
                let y = (star.center.y as i32 + dy) as usize;
                pixels[(x, y)] += star_pixels.value(x, y);
            }
        }
    }

    let labels = LabelMap::from_raw(&labels_buf, 1);
    let data = ComponentData {
        bbox,
        label: 1,
        area,
    };

    // Sanity-check the setup actually reaches the reported bug's precondition.
    let min_value = Component::new(&data, &pixels, &labels)
        .pixels()
        .map(|p| p.value)
        .fold(f32::MAX, f32::min);
    assert_eq!(
        min_value, 0.0,
        "test setup must have an exact-zero floor pixel"
    );

    let result = deblend_multi_threshold_floored(
        &Component::new(&data, &pixels, &labels),
        0.01,
        32,
        3,
        0.005,
    );

    assert_eq!(
        result.len(),
        2,
        "Two well-separated stars in a zero-floored component should still deblend"
    );
    assert_eq!(result.iter().map(|r| r.area).sum::<usize>(), data.area);
}

#[test]
fn many_stars_are_all_kept() {
    // Twelve stars 12 px apart in one connected chain, brightening left to right (0.45 + 0.05·i),
    // so raster and tree order meet the dimmest first. Each is its own branch (its flux is several
    // percent of the chain's, far above 0.005), and every one is kept: x = 15 + 12i.
    let stars: Vec<_> = (0..12)
        .map(|i| {
            SyntheticStar::new(
                Vec2::new((15 + i * 12) as f32, 50.0),
                0.45 + i as f32 * 0.05,
                StarProfile::Gaussian { sigma: 2.0 },
            )
        })
        .collect();

    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(Size2us::new(180, 100), &stars);

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    let mut peaks: Vec<usize> = result.iter().map(|region| region.peak.x).collect();
    peaks.sort_unstable();
    assert_eq!(peaks, [15, 27, 39, 51, 63, 75, 87, 99, 111, 123, 135, 147]);
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");
}

#[test]
fn a_wide_split_keeps_every_child() {
    // Sixteen disjoint stars on a 25 px grid under one label, dimming by 0.03 in raster order: the
    // root splits sixteen ways at the floor and keeps every branch.
    let mut stars = Vec::new();
    for row in 0..4 {
        for col in 0..4 {
            stars.push(SyntheticStar::new(
                Vec2::new((20 + col * 25) as f32, (20 + row * 25) as f32),
                1.0 - (row * 4 + col) as f32 * 0.03,
                StarProfile::Gaussian { sigma: 2.0 },
            ));
        }
    }

    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(Size2us::new(150, 150), &stars);

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 64, 3, 0.005);

    let mut peaks: Vec<(usize, usize)> = result
        .iter()
        .map(|region| (region.peak.y, region.peak.x))
        .collect();
    peaks.sort_unstable();
    let expected: Vec<(usize, usize)> = (0..4)
        .flat_map(|row| (0..4).map(move |col| (20 + row * 25, 20 + col * 25)))
        .collect();
    assert_eq!(peaks, expected);
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");
}

#[test]
fn buffer_reuse_consistency() {
    // One `TreeBuffers` through A, then a different component B, then A again: every result must
    // equal the one fresh buffers give, so nothing a previous component left behind leaks in.
    let pair = make_test_component(
        Size2us::new(100, 100),
        &[
            SyntheticStar::new(
                Vec2::new(30.0, 50.0),
                1.0,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(70.0, 50.0),
                0.8,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
        ],
    );
    let close = make_test_component(
        Size2us::new(60, 60),
        &[
            SyntheticStar::new(
                Vec2::new(26.0, 30.0),
                1.0,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(34.0, 30.0),
                0.9,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
        ],
    );
    let run = |fixture: &TestComponent, buffers: &mut DeblendBuffers| {
        let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);
        let mut regions = Vec::new();
        let floor = component
            .pixels()
            .map(|p| p.value)
            .fold(f32::INFINITY, f32::min);
        MultiThresholdParams {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            min_area: 1,
            connectivity: Connectivity::Eight,
        }
        .deblend(&component, floor, buffers, &mut regions);
        regions
            .iter()
            .map(|region| (region.peak, region.bbox, region.area))
            .collect::<Vec<_>>()
    };

    let fresh_pair = run(&pair, &mut DeblendBuffers::default());
    let fresh_close = run(&close, &mut DeblendBuffers::default());
    assert_eq!(fresh_pair.len(), 2);
    assert_eq!(fresh_close.len(), 2);

    let mut shared = DeblendBuffers::default();
    assert_eq!(run(&pair, &mut shared), fresh_pair);
    assert_eq!(run(&close, &mut shared), fresh_close);
    assert_eq!(run(&pair, &mut shared), fresh_pair);
}

#[test]
fn bbox_contains_all_peaks() {
    // Verify that each deblended object's bbox contains its peak
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(150, 100),
        &[
            SyntheticStar::new(
                Vec2::new(30.0, 30.0),
                1.0,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(75.0, 50.0),
                0.9,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
            SyntheticStar::new(
                Vec2::new(120.0, 70.0),
                0.8,
                StarProfile::Gaussian { sigma: 2.5 },
            ),
        ],
    );

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    for candidate in &result {
        assert!(
            candidate.bbox.contains(candidate.peak),
            "Candidate bbox {:?} should contain peak {:?}",
            candidate.bbox,
            candidate.peak
        );
    }
}

#[test]
fn single_threshold_level() {
    // Test with n_thresholds = 1 (edge case)
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(0.8);

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 1, 3, 0.005);

    // Should still produce valid output
    assert!(!result.is_empty(), "Should produce at least one object");

    // Area conservation
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");
}

/// Review item 9.5: a region above a level is an object only with `min_area` pixels. A star of
/// σ 2 and amplitude 1 at (15, 15) with a spike of 0.3 at (19, 15) on its wing: the spike pixel
/// reads 0.3 + e^(−2) = 0.435 against its neighbours' 0.325 and below, so at the 28th level of the
/// ladder from the floor of 0.001 to 1, 0.001 · 1000^(27/32) = 0.339, it is a region of its own
/// pixel, holding 0.096 above the level against a bar of 0.001 of the star's 25. With `min_area`
/// 1 it splits off, and with 2 it is no object and the star stays whole.
#[test]
fn a_region_under_min_area_is_no_object() {
    let mut fixture = make_test_component(
        Size2us::new(31, 31),
        &[SyntheticStar::new(
            Vec2::new(15.0, 15.0),
            1.0,
            StarProfile::Gaussian { sigma: 2.0 },
        )],
    );
    fixture.pixels[(19, 15)] += 0.3;
    let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);
    let floor = component
        .pixels()
        .map(|p| p.value)
        .fold(f32::INFINITY, f32::min);
    let split = |min_area| {
        let mut regions = Vec::new();
        MultiThresholdParams {
            n_thresholds: 32,
            min_contrast: 0.001,
            min_separation: 1,
            min_area,
            connectivity: Connectivity::Eight,
        }
        .deblend(
            &component,
            floor,
            &mut DeblendBuffers::default(),
            &mut regions,
        );
        let mut peaks: Vec<(usize, usize)> = regions
            .iter()
            .map(|region| (region.peak.x, region.peak.y))
            .collect();
        peaks.sort_unstable();
        peaks
    };
    assert_eq!(split(1), [(15, 15), (19, 15)]);
    assert_eq!(split(2), [(15, 15)]);
}

/// Review item 15.8: the scratch follows the component's pixels, not its box. A diagonal line of
/// 1000 pixels has a box of 10⁶; after both deblenders the per-pixel buffers hold 1000 entries and
/// the row index 1001, where a grid over the box held 10⁶ cells each.
#[test]
fn the_scratch_follows_the_pixels_not_the_box() {
    let size = Size2us::new(1000, 1000);
    let mut pixels = Buffer2::new_filled(size.width, size.height, 0.0f32);
    let mut labels = Buffer2::new_filled(size.width, size.height, 0u32);
    let mut bbox = URect::empty();
    for i in 0..1000 {
        pixels[(i, i)] = 1.0 + (i % 7) as f32 / 10.0;
        labels[(i, i)] = 1;
        bbox.include(Vec2us::new(i, i));
    }
    let labels = LabelMap::from_raw(&labels, 1);
    let data = ComponentData {
        bbox,
        label: 1,
        area: 1000,
    };
    let component = Component::new(&data, &pixels, &labels);
    let mut buffers = DeblendBuffers::default();
    let mut regions = Vec::new();
    MultiThresholdParams {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        min_area: 1,
        connectivity: Connectivity::Eight,
    }
    .deblend(&component, 0.5, &mut buffers, &mut regions);
    LocalMaximaParams {
        min_separation: 3,
        min_prominence: 0.3,
    }
    .deblend(&component, &mut buffers, &mut regions);
    assert_eq!(buffers.pixels.pixels.len(), 1000);
    assert!(buffers.tree.object_of.capacity() < 2000);
    assert!(buffers.tree.visited.capacity() < 2000);
    assert!(buffers.occupied.capacity() < 2000);
}
