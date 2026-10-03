//! Tests for multi-threshold deblending.

use crate::stacking::star_detection::deblend::internals::{
    TestComponent, deblend_multi_threshold_floored, deblend_multi_threshold_test,
    make_test_component, separated_pair,
};
use crate::stacking::star_detection::deblend::multi_threshold::*;
use crate::stacking::star_detection::labeling::LabelMap;
use crate::stacking::star_detection::labeling::component_data::ComponentData;
use crate::testing::prelude::*;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};
use std::collections::HashSet;

/// Build a `RegionSet` from separate regions, as a BFS would have appended them.
fn region_set(regions: &[&[Pixel]]) -> RegionSet {
    let mut set = RegionSet::default();
    for region in regions {
        set.pixels.extend_from_slice(region);
        set.close_region();
    }
    set
}

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

#[test]
fn deblend_contrast_bar_is_root_flux_not_parent() {
    // Hand-built tree:
    //   0 root(100) → [1 bright(60), 2 dim_branch(40) → [3 mid(25), 4 faint(12)]]
    // With min_contrast = 0.2 the global bar is 0.2·root = 20:
    //   - bright(60) and dim_branch(40) clear 20 → the root splits in two;
    //   - inside dim_branch, mid(25) clears 20 but faint(12) does not, so only
    //     one child clears → dim_branch stays a single object.
    // Result: {bright, dim_branch} = 2 objects.
    //
    // A *parent*-relative bar would let faint(12) clear 0.2·40 = 8, over-splitting
    // dim_branch into {mid, faint} → 3 objects. This pins SExtractor's root/total-flux
    // criterion.
    fn node(flux: f32, children: &[usize]) -> DeblendNode {
        DeblendNode {
            peak: Pixel {
                pos: Vec2us::new(0, 0),
                value: flux,
            },
            flux,
            children: children.iter().map(|&c| c as u32).collect(),
        }
    }

    let tree = vec![
        node(100.0, &[1, 2]),
        node(60.0, &[]),
        node(40.0, &[3, 4]),
        node(25.0, &[]),
        node(12.0, &[]),
    ];

    let mut leaves = Vec::new();
    find_significant_branches(&tree, 0.2, &mut leaves);
    leaves.sort_unstable();
    assert_eq!(
        leaves,
        vec![1, 2],
        "root-relative contrast must keep the dim branch whole (parent-relative would split it into 3)"
    );
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
fn create_child_nodes_diagonal_uses_euclidean_not_chebyshev() {
    // dx=dy=3, min_separation=4: Chebyshev distance is max(3,3)=3 (< 4, "too
    // close"), but squared Euclidean is 3²+3²=18 (>= 4²=16, "well separated").
    // create_child_nodes must agree with the squared-Euclidean metric that
    // local_maxima::find_local_maxima and the shared nearest_peak_index /
    // Component::assign_to_nearest Voronoi step use everywhere else in this module —
    // not Chebyshev, which would wrongly merge these two.
    let mut tree = vec![DeblendNode {
        peak: Pixel {
            pos: Vec2us::new(50, 50),
            value: 1.0,
        },
        flux: 10.0,
        children: ArrayVec::new(),
    }];
    let parent_idx = 0;
    let mut pixel_to_node = NodeGrid::default();

    let child_regions = region_set(&[
        &[Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        }],
        &[Pixel {
            pos: Vec2us::new(3, 3),
            value: 0.9,
        }],
    ]);

    create_child_nodes(
        &mut tree,
        &mut pixel_to_node,
        parent_idx,
        &child_regions,
        &mut Vec::new(),
        4,
    );

    assert_eq!(
        tree[parent_idx].children.len(),
        2,
        "diagonal peaks 3px apart with min_separation=4 must both become children \
         (Euclidean distance >= min_separation even though Chebyshev distance < min_separation)"
    );
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

    let labels = LabelMap::from_raw(labels_buf, 1);
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

    let labels = LabelMap::from_raw(labels_buf, 1);
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
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (star.center.x as i32 + dx) as usize;
                let y = (star.center.y as i32 + dy) as usize;
                pixels[(x, y)] += star.value_at(x as f32, y as f32);
            }
        }
    }

    let labels = LabelMap::from_raw(labels_buf, 1);
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
fn many_stars_keep_the_brightest_peaks() {
    // Twelve stars 12 px apart in one connected chain, brightening left to right (0.45 + 0.05·i),
    // so raster and tree order meet the dimmest first. Each is its own branch (its flux is several
    // percent of the chain's, far above 0.005), and only MAX_PEAKS survive: the eight brightest,
    // i = 4..11 at x = 15 + 12i.
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
    assert_eq!(peaks, [63, 75, 87, 99, 111, 123, 135, 147]);
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");
}

#[test]
fn a_wide_split_keeps_its_brightest_children() {
    // Sixteen disjoint stars on a 25 px grid under one label, dimming by 0.03 in raster order:
    // the root splits sixteen ways at the floor and keeps MAX_CHILDREN = 8, the brightest — the
    // first two rows. The eight it did not keep stay part of the root: a later level must not
    // split them off again in place of the first eight.
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
    let expected: Vec<(usize, usize)> = (0..2)
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
    let run = |fixture: &TestComponent, buffers: &mut TreeBuffers| {
        let component = Component::new(&fixture.data, &fixture.pixels, &fixture.labels);
        let floor = component
            .pixels()
            .map(|p| p.value)
            .fold(f32::INFINITY, f32::min);
        deblend_multi_threshold(
            &component,
            floor,
            MultiThresholdParams {
                n_thresholds: 32,
                min_contrast: 0.005,
                min_separation: 3,
                connectivity: Connectivity::Eight,
            },
            buffers,
        )
        .iter()
        .map(|region| (region.peak, region.bbox, region.area))
        .collect::<Vec<_>>()
    };

    let fresh_pair = run(&pair, &mut TreeBuffers::default());
    let fresh_close = run(&close, &mut TreeBuffers::default());
    assert_eq!(fresh_pair.len(), 2);
    assert_eq!(fresh_close.len(), 2);

    let mut shared = TreeBuffers::default();
    assert_eq!(run(&pair, &mut shared), fresh_pair);
    assert_eq!(run(&close, &mut shared), fresh_close);
    assert_eq!(run(&pair, &mut shared), fresh_pair);
}

#[test]
fn connected_regions_complex_shape() {
    // A dumbbell: blobs at x = 20 and 80 (σ 3, 1.0 and 0.9) joined by a thin bridge along y = 25
    // (σ 15 × 0.6, amplitude 0.05), so the component is connected through it. The bridge's crest
    // splits off as a branch of its own, about 1% of the flux, which a 5% contrast discards; the
    // blobs, near half each, stay.
    let TestComponent {
        pixels,
        labels,
        data,
    } = make_test_component(
        Size2us::new(100, 50),
        &[
            SyntheticStar::new(
                Vec2::new(20.0, 25.0),
                1.0,
                StarProfile::Gaussian { sigma: 3.0 },
            ),
            SyntheticStar::new(
                Vec2::new(80.0, 25.0),
                0.9,
                StarProfile::Gaussian { sigma: 3.0 },
            ),
            SyntheticStar::new(
                Vec2::new(50.0, 25.0),
                0.05,
                StarProfile::Elliptical {
                    sigma_x: 15.0,
                    sigma_y: 0.6,
                    angle: 0.0,
                },
            ),
        ],
    );
    let component = Component::new(&data, &pixels, &labels);
    // One component under 8-connectivity: the bridge row is lit end to end.
    assert!((20..=80).all(|x| labels[25 * 100 + x] == 1));

    let result = deblend_multi_threshold_test(&component, 32, 3, 0.05);
    let mut peaks: Vec<(usize, usize)> = result.iter().map(|c| (c.peak.x, c.peak.y)).collect();
    peaks.sort_unstable();
    assert_eq!(peaks, [(20, 25), (80, 25)]);
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");
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
fn peak_values_match_image() {
    // Verify that peak_value matches the actual pixel value
    let TestComponent {
        pixels,
        labels,
        data,
    } = separated_pair(0.8);

    let result =
        deblend_multi_threshold_test(&Component::new(&data, &pixels, &labels), 32, 3, 0.005);

    for candidate in &result {
        let actual_value = pixels[(candidate.peak.x, candidate.peak.y)];
        assert!(
            (candidate.peak_value - actual_value).abs() < 1e-6,
            "peak_value {} should match pixel value {}",
            candidate.peak_value,
            actual_value
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

#[test]
fn pixel_grid_connected_regions() {
    // Test that pixel values are stored correctly by using find_connected_regions_grid
    // which exercises the actual code path including value lookups
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(11, 10),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(10, 11),
            value: 3.0,
        },
    ];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

    // All 3 pixels should be in one connected region (they're adjacent)
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].len(), 3);

    // Verify the values were preserved
    let values: HashSet<_> = regions[0].iter().map(|p| p.value as i32).collect();
    assert!(values.contains(&1));
    assert!(values.contains(&2));
    assert!(values.contains(&3));
}

#[test]
fn pixel_grid_reuse() {
    let mut regions = RegionSet::default();

    // First use
    let pixels1 = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(15, 15),
            value: 2.0,
        },
    ];
    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels1, Connectivity::Eight, &mut regions, &mut scratch);
    assert_eq!(regions.len(), 2);

    // Reuse with different pixels — grid state should be properly reset
    let pixels2 = vec![
        Pixel {
            pos: Vec2us::new(20, 20),
            value: 3.0,
        },
        Pixel {
            pos: Vec2us::new(25, 25),
            value: 4.0,
        },
    ];
    find_connected_regions_grid(&pixels2, Connectivity::Eight, &mut regions, &mut scratch);

    // Two separate pixels should form two regions (not adjacent)
    assert_eq!(regions.len(), 2);
}

#[test]
fn pixel_grid_single_pixel() {
    let pixels = vec![Pixel {
        pos: Vec2us::new(50, 50),
        value: 42.0,
    }];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].len(), 1);
    assert_eq!(regions[0][0].value, 42.0);
}

#[test]
fn node_grid_empty() {
    let grid = NodeGrid::default();
    assert_eq!(grid.size, Size2us::default());
    assert!(grid.get(Vec2us::new(0, 0)).is_none());
}

#[test]
fn node_grid_basic_operations() {
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(11, 10),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(10, 11),
            value: 3.0,
        },
    ];

    let mut grid = NodeGrid::default();
    grid.reset_with_pixels(&pixels);

    // Initially all positions should be unassigned
    assert!(grid.get(Vec2us::new(10, 10)).is_none());
    assert!(grid.get(Vec2us::new(11, 10)).is_none());

    // Set node indices
    grid.set(Vec2us::new(10, 10), 0);
    grid.set(Vec2us::new(11, 10), 1);
    grid.set(Vec2us::new(10, 11), 0);

    // Verify
    assert_eq!(grid.get(Vec2us::new(10, 10)), Some(0));
    assert_eq!(grid.get(Vec2us::new(11, 10)), Some(1));
    assert_eq!(grid.get(Vec2us::new(10, 11)), Some(0));

    // Out of bounds should return None
    assert!(grid.get(Vec2us::new(100, 100)).is_none());
}

#[test]
fn node_grid_overwrite() {
    let pixels = vec![Pixel {
        pos: Vec2us::new(5, 5),
        value: 1.0,
    }];

    let mut grid = NodeGrid::default();
    grid.reset_with_pixels(&pixels);

    grid.set(Vec2us::new(5, 5), 10);
    assert_eq!(grid.get(Vec2us::new(5, 5)), Some(10));

    // Overwrite with new value
    grid.set(Vec2us::new(5, 5), 20);
    assert_eq!(grid.get(Vec2us::new(5, 5)), Some(20));
}

#[test]
fn node_grid_reuse() {
    let mut grid = NodeGrid::default();

    // First use
    let pixels1 = vec![Pixel {
        pos: Vec2us::new(10, 10),
        value: 1.0,
    }];
    grid.reset_with_pixels(&pixels1);
    grid.set(Vec2us::new(10, 10), 5);
    assert_eq!(grid.get(Vec2us::new(10, 10)), Some(5));

    // Reuse with different pixels
    let pixels2 = vec![Pixel {
        pos: Vec2us::new(20, 20),
        value: 2.0,
    }];
    grid.reset_with_pixels(&pixels2);

    // Old position should no longer be valid
    assert!(grid.get(Vec2us::new(10, 10)).is_none());

    // New position should be unassigned
    assert!(grid.get(Vec2us::new(20, 20)).is_none());
}

#[test]
fn node_grid_large_indices() {
    let pixels = vec![Pixel {
        pos: Vec2us::new(100, 100),
        value: 1.0,
    }];

    let mut grid = NodeGrid::default();
    grid.reset_with_pixels(&pixels);

    // Test with large node index (but within u32 range)
    let large_idx = 1_000_000;
    grid.set(Vec2us::new(100, 100), large_idx);
    assert_eq!(grid.get(Vec2us::new(100, 100)), Some(large_idx));
}

#[test]
fn node_grid_boundary() {
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(99, 99),
            value: 2.0,
        },
    ];

    let mut grid = NodeGrid::default();
    grid.reset_with_pixels(&pixels);

    grid.set(Vec2us::new(0, 0), 1);
    grid.set(Vec2us::new(99, 99), 2);

    assert_eq!(grid.get(Vec2us::new(0, 0)), Some(1));
    assert_eq!(grid.get(Vec2us::new(99, 99)), Some(2));

    // Just outside the grid
    assert!(grid.get(Vec2us::new(100, 100)).is_none());
}

#[test]
fn find_connected_regions_grid_single_region() {
    // Create a small connected region
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(5, 6),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 6),
            value: 1.0,
        },
    ];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

    assert_eq!(regions.len(), 1, "Should find one connected region");
    assert_eq!(regions[0].len(), 4, "Region should contain all 4 pixels");
}

#[test]
fn find_connected_regions_grid_two_regions() {
    // Create two separate regions
    let pixels = vec![
        // Region 1
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 5),
            value: 1.0,
        },
        // Region 2 (far away)
        Pixel {
            pos: Vec2us::new(50, 50),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(51, 50),
            value: 2.0,
        },
    ];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

    assert_eq!(regions.len(), 2, "Should find two separate regions");
    assert_eq!(
        regions[0].len() + regions[1].len(),
        4,
        "Total pixels should be 4"
    );
}

#[test]
fn find_connected_regions_grid_diagonal_connectivity() {
    // Test 8-connectivity (diagonals should connect)
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 6),
            value: 1.0,
        }, // Diagonal neighbor
    ];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

    assert_eq!(
        regions.len(),
        1,
        "Diagonal neighbors should be connected (8-connectivity)"
    );
    assert_eq!(regions[0].len(), 2);

    // Under 4-connectivity the same pair is two regions: the configured rule, not a fixed 8.
    find_connected_regions_grid(&pixels, Connectivity::Four, &mut regions, &mut scratch);
    assert_eq!(regions.len(), 2);
    assert_eq!((regions[0].len(), regions[1].len()), (1, 1));
}

#[test]
fn find_significant_branches_small_tree() {
    // Build a simple tree: root with 2 children
    let tree = vec![
        DeblendNode {
            peak: Pixel {
                pos: Vec2us::new(10, 10),
                value: 1.0,
            },
            flux: 100.0,
            children: [1, 2].into_iter().collect(),
        },
        DeblendNode {
            peak: Pixel {
                pos: Vec2us::new(5, 5),
                value: 0.8,
            },
            flux: 40.0,
            children: ArrayVec::new(),
        },
        DeblendNode {
            peak: Pixel {
                pos: Vec2us::new(15, 15),
                value: 0.7,
            },
            flux: 35.0,
            children: ArrayVec::new(),
        },
    ];

    // Bar 0.1·100 = 10: both children (40, 35) clear it, so each is its own leaf.
    let mut leaves = Vec::new();
    find_significant_branches(&tree, 0.1, &mut leaves);
    assert_eq!(leaves, [1, 2]);

    // Bar 0.9·100 = 90: neither clears it, so the root stays one object.
    find_significant_branches(&tree, 0.9, &mut leaves);
    assert_eq!(leaves, [0]);
}

#[test]
fn visit_neighbors_grid_all_directions() {
    // Create a cross pattern and verify all neighbors are visited
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        }, // Center
        Pixel {
            pos: Vec2us::new(9, 9),
            value: 1.0,
        }, // Top-left
        Pixel {
            pos: Vec2us::new(10, 9),
            value: 1.0,
        }, // Top
        Pixel {
            pos: Vec2us::new(11, 9),
            value: 1.0,
        }, // Top-right
        Pixel {
            pos: Vec2us::new(9, 10),
            value: 1.0,
        }, // Left
        Pixel {
            pos: Vec2us::new(11, 10),
            value: 1.0,
        }, // Right
        Pixel {
            pos: Vec2us::new(9, 11),
            value: 1.0,
        }, // Bottom-left
        Pixel {
            pos: Vec2us::new(10, 11),
            value: 1.0,
        }, // Bottom
        Pixel {
            pos: Vec2us::new(11, 11),
            value: 1.0,
        }, // Bottom-right
    ];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

    assert_eq!(regions.len(), 1, "All pixels should be in one region");
    assert_eq!(regions[0].len(), 9, "All 9 pixels should be found");
}

#[test]
fn pixel_grid_values_generation_isolation() {
    // Verify that generation-counter-based value storage correctly isolates
    // values between successive reset_with_pixels calls. Stale values from
    // a previous population must not be visible after reset.

    // First population: large grid with many pixels
    let pixels1: Vec<Pixel> = (0..100)
        .map(|i| Pixel {
            pos: Vec2us::new(10 + i, 10),
            value: 42.0,
        })
        .collect();
    let mut scratch = RegionScratch::default();
    scratch.grid.reset_with_pixels(&pixels1);

    // Second population: small grid with only 2 pixels
    let pixels2 = vec![
        Pixel {
            pos: Vec2us::new(50, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(51, 10),
            value: 2.0,
        },
    ];
    scratch.grid.reset_with_pixels(&pixels2);

    // BFS should only find the 2 pixels from the second population,
    // not the stale 100 pixels from the first.
    let mut regions = RegionSet::default();
    find_connected_regions_grid(&pixels2, Connectivity::Eight, &mut regions, &mut scratch);

    assert_eq!(regions.len(), 1);
    assert_eq!(
        regions[0].len(),
        2,
        "Should find exactly 2 pixels, not stale values from previous population"
    );
}

#[test]
fn pixel_grid_repeated_resets_same_positions() {
    // Verify correctness when the same positions are repopulated with
    // different values across multiple resets.
    let mut regions = RegionSet::default();

    for round in 0..10 {
        let pixels = vec![
            Pixel {
                pos: Vec2us::new(5, 5),
                value: round as f32,
            },
            Pixel {
                pos: Vec2us::new(6, 5),
                value: round as f32 + 0.5,
            },
        ];

        let mut scratch = RegionScratch::default();
        find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

        assert_eq!(regions.len(), 1, "Round {round}: should find 1 region");
        assert_eq!(
            regions[0].len(),
            2,
            "Round {round}: should find exactly 2 pixels"
        );

        // Verify values match current round, not stale from previous
        let mut values: Vec<f32> = regions[0].iter().map(|p| p.value).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(values[0], round as f32, "Round {round}: wrong value");
        assert_eq!(values[1], round as f32 + 0.5, "Round {round}: wrong value");
    }
}

#[test]
fn connected_regions_pixels_at_coordinate_zero() {
    // Regression test: pixels at coordinate (0, 0) caused segfault when
    // the grid border was computed with saturating_sub instead of wrapping_sub.
    // The border must always be guaranteed even at the image edge.
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(1, 0),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(0, 1),
            value: 3.0,
        },
    ];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

    assert_eq!(regions.len(), 1, "All 3 pixels should form one region");
    assert_eq!(regions[0].len(), 3);

    // Verify absolute coordinates are preserved correctly
    let mut positions: Vec<(usize, usize)> =
        regions[0].iter().map(|p| (p.pos.x, p.pos.y)).collect();
    positions.sort_unstable();
    assert_eq!(positions, vec![(0, 0), (0, 1), (1, 0)]);
}

#[test]
fn connected_regions_two_groups_near_zero() {
    // Two disconnected groups near coordinate 0
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 2.0,
        },
    ];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

    assert_eq!(regions.len(), 2, "Should find 2 separate regions");
    assert_eq!(regions[0].len(), 1);
    assert_eq!(regions[1].len(), 1);
}

#[test]
fn connected_regions_grid_basic() {
    // Three separate regions, no limit — all should be found
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(20, 20),
            value: 3.0,
        },
    ];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);

    assert_eq!(regions.len(), 3, "Should find all 3 separate regions");
    for region in regions.iter() {
        assert_eq!(region.len(), 1);
    }
}

#[test]
fn connected_regions_grid_replaces_previous_contents() {
    // Verify that a second search replaces the first's regions rather than appending to them
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(15, 15),
            value: 2.0,
        },
    ];

    let mut regions = RegionSet::default();

    let mut scratch = RegionScratch::default();
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);
    assert_eq!(regions.len(), 2);
    assert_eq!(regions.pixels.len(), 2, "one pixel per region");

    // Second call over the same input: two regions again, and the flat buffer holds two pixels
    // rather than four, which is what proves it was truncated and not appended to.
    find_connected_regions_grid(&pixels, Connectivity::Eight, &mut regions, &mut scratch);
    assert_eq!(regions.len(), 2);
    assert_eq!(
        regions.pixels.len(),
        2,
        "second search must replace, not append"
    );
}

#[test]
fn pixel_grid_generation_wrap_to_zero_guard() {
    // Verify that wrapping generation counter from u32::MAX to 0 is handled
    // correctly — generation 0 is skipped because generation arrays are
    // initialized to 0, so wrapping to 0 would make all cells appear valid.

    // First population to set up grid dimensions
    let pixels_large: Vec<Pixel> = (0..20)
        .map(|i| Pixel {
            pos: Vec2us::new(i, 0),
            value: 99.0,
        })
        .collect();
    let mut scratch = RegionScratch::default();
    scratch.grid.reset_with_pixels(&pixels_large);

    // Force generation counter to u32::MAX so next reset wraps
    scratch.grid.current_generation = u32::MAX;

    // Small population — reset should wrap past 0 to 1
    let pixels_small = vec![
        Pixel {
            pos: Vec2us::new(5, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 0),
            value: 2.0,
        },
    ];
    scratch.grid.reset_with_pixels(&pixels_small);

    assert_ne!(
        scratch.grid.current_generation, 0,
        "Generation 0 must be skipped on wrap"
    );

    // BFS should find exactly the 2 new pixels, not stale data
    let mut regions = RegionSet::default();
    find_connected_regions_grid(
        &pixels_small,
        Connectivity::Eight,
        &mut regions,
        &mut scratch,
    );

    assert_eq!(regions.len(), 1);
    assert_eq!(
        regions[0].len(),
        2,
        "Should find exactly 2 pixels after generation wrap, not stale values"
    );
}
