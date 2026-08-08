use crate::math::rect::URect;
use crate::stacking::star_detection::detector::stages::detect::*;
use crate::stacking::star_detection::labeling::LabelMap;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};
use glam::Vec2;

/// Render Gaussian `stars` into a single connected component: every lit pixel gets label 1.
fn one_component(size: Size2us, stars: &[SyntheticStar]) -> (Buffer2<f32>, LabelMap) {
    let mut pixels = Buffer2::new_filled(size.width, size.height, 0.0f32);
    let mut labels = Buffer2::new_filled(size.width, size.height, 0u32);
    for &star in stars {
        let radius = star.radius();
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (star.center.x as i32 + dx) as usize;
                let y = (star.center.y as i32 + dy) as usize;
                if size.contains(Vec2us::new(x, y)) {
                    let v = star.value_at(x as f32, y as f32);
                    if v > 0.001 {
                        pixels[(x, y)] += v;
                        labels[(x, y)] = 1;
                    }
                }
            }
        }
    }
    (pixels, LabelMap::from_raw(labels, 1))
}

fn local_maxima_config() -> DetectionConfig {
    DetectionConfig {
        deblend_n_thresholds: 0, // 0 selects the local-maxima deblend path
        deblend_min_separation: 3,
        deblend_min_prominence: 0.3,
        max_area: usize::MAX,
        ..Default::default()
    }
}

#[test]
fn local_maxima_deblended_counts_split_components_not_extra_regions() {
    // One connected blob with three well-separated peaks. Local-maxima deblending
    // splits it into three regions, but it is ONE component that split, so
    // `deblended_components` must be 1. The previous `regions - num_components`
    // formula reported 3 - 1 = 2 here, which this pins against.
    let (pixels, label_map) = one_component(
        Size2us::new(48, 24),
        &[
            SyntheticStar::new(
                Vec2::new(12.0, 12.0),
                1.0,
                StarProfile::Gaussian { sigma: 3.0 },
            ),
            SyntheticStar::new(
                Vec2::new(24.0, 12.0),
                1.0,
                StarProfile::Gaussian { sigma: 3.0 },
            ),
            SyntheticStar::new(
                Vec2::new(36.0, 12.0),
                1.0,
                StarProfile::Gaussian { sigma: 3.0 },
            ),
        ],
    );

    let result = extract_candidates(&pixels, &label_map, &local_maxima_config());

    assert_eq!(
        result.regions.len(),
        3,
        "three resolved peaks should yield three regions"
    );
    assert_eq!(
        result.deblended_components, 1,
        "one component split into >1 region counts once, not `regions - components`"
    );
}

#[test]
fn local_maxima_single_peak_reports_zero_deblended() {
    // A lone star: one region from one component — nothing was split.
    let (pixels, label_map) = one_component(
        Size2us::new(32, 32),
        &[SyntheticStar::new(
            Vec2::new(16.0, 16.0),
            1.0,
            StarProfile::Gaussian { sigma: 3.0 },
        )],
    );

    let result = extract_candidates(&pixels, &label_map, &local_maxima_config());

    assert_eq!(result.regions.len(), 1);
    assert_eq!(result.deblended_components, 0);
}

#[test]
fn component_collection_merges_cross_job_metadata_exactly() {
    let width = 5;
    let height = 6;
    let mut labels = Buffer2::new_filled(width, height, 0u32);
    for (x, y, label) in [
        (0, 0, 1),
        (1, 0, 1),
        (0, 3, 1),
        (4, 1, 2),
        (3, 4, 2),
        (2, 5, 3),
    ] {
        labels[(x, y)] = label;
    }
    let label_map = LabelMap::from_raw(labels, 3);

    let components = collect_component_data(&label_map);
    let parallel = collect_component_data_dense(&label_map, 3);
    let sequential = collect_component_data_dense(&label_map, 1);

    assert_eq!(components.len(), 3);
    assert_eq!(components[0].label, 1);
    assert_eq!(components[0].area, 3);
    assert_eq!(
        components[0].bbox,
        URect::new(Vec2us::new(0, 0), Vec2us::new(2, 4))
    );
    assert_eq!(components[1].label, 2);
    assert_eq!(components[1].area, 2);
    assert_eq!(
        components[1].bbox,
        URect::new(Vec2us::new(3, 1), Vec2us::new(5, 5))
    );
    assert_eq!(components[2].label, 3);
    assert_eq!(components[2].area, 1);
    assert_eq!(
        components[2].bbox,
        URect::new(Vec2us::new(2, 5), Vec2us::new(3, 6))
    );
    for alternative in [parallel, sequential] {
        assert_eq!(alternative.len(), components.len());
        for (actual, expected) in alternative.iter().zip(&components) {
            assert_eq!(actual.label, expected.label);
            assert_eq!(actual.area, expected.area);
            assert_eq!(actual.bbox, expected.bbox);
        }
    }

    assert_eq!(
        dense_component_jobs(100_000, 2048 * 2048, 8),
        3,
        "three 4.8 MB dense jobs fit in one 16 MiB label plane"
    );
    assert_eq!(
        dense_component_jobs(2048 * 2048, 2048 * 2048, 8),
        1,
        "an oversized dense scratch falls back to the scratch-free sequential scan"
    );
}

#[test]
fn edge_margin_swallowing_image_yields_no_regions_without_panicking() {
    // Once 2 * edge_margin >= the smallest dimension, the retain predicate
    // `bbox.min >= margin && bbox.max <= dim - margin` is unsatisfiable, so every region
    // is filtered out. This must degrade gracefully (empty result, no panic/overflow)
    // rather than crash, since detect() runs once per frame in a batch and one
    // oddly-sized frame shouldn't abort the whole run. Covers both the exact boundary
    // (2 * 16 == 32) and a margin past the dimension itself (saturating_sub floors at 0).
    for edge_margin in [16, 32] {
        let (pixels, label_map) = one_component(
            Size2us::new(32, 32),
            &[SyntheticStar::new(
                Vec2::new(16.0, 16.0),
                1.0,
                StarProfile::Gaussian { sigma: 3.0 },
            )],
        );
        let config = DetectionConfig {
            edge_margin,
            ..local_maxima_config()
        };

        let result =
            extract_and_filter_candidates(&pixels, &label_map, &config, Size2us::new(32, 32));

        assert!(
            result.regions.is_empty(),
            "edge_margin {edge_margin} leaves no valid interior in 32x32, so every \
             region must be filtered out"
        );
    }
}
