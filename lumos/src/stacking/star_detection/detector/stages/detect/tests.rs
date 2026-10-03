use crate::stacking::star_detection::detector::stages::detect::*;
use crate::testing::prelude::*;
use crate::testing::synthetic::background_map;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

/// The rendered stars and a label map marking all of them as one component.
#[derive(Debug)]
struct OneComponent {
    pixels: Buffer2<f32>,
    labels: LabelMap,
}

/// Render Gaussian `stars` into a single connected component: every lit pixel gets label 1.
fn one_component(size: Size2us, stars: &[SyntheticStar]) -> OneComponent {
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
    OneComponent {
        pixels,
        labels: LabelMap::from_raw(labels, 1),
    }
}

fn local_maxima_config() -> DetectionConfig {
    DetectionConfig {
        deblend: Deblend::LocalMaxima {
            min_prominence: 0.3,
        },
        deblend_min_separation: 3,
        max_area: usize::MAX,
        ..Default::default()
    }
}

/// A flat sky noise of 0.01 over `size`.
fn flat_sky(size: Size2us) -> SkyNoise {
    background_map::uniform(size, 0.0, 0.01).sky_noise()
}

#[test]
fn local_maxima_deblended_counts_split_components_not_extra_regions() {
    // One connected blob with three well-separated peaks. Local-maxima deblending
    // splits it into three regions, but it is ONE component that split, so
    // `deblended_components` must be 1. The previous `regions - num_components`
    // formula reported 3 - 1 = 2 here, which this pins against.
    let OneComponent {
        pixels,
        labels: label_map,
    } = one_component(
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

    let sky = flat_sky(Size2us::new(pixels.width(), pixels.height()));
    let result = extract_candidates(
        &pixels,
        &sky,
        &label_map,
        &local_maxima_config(),
        &JobScratchPool::default(),
    );

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
    let OneComponent {
        pixels,
        labels: label_map,
    } = one_component(
        Size2us::new(32, 32),
        &[SyntheticStar::new(
            Vec2::new(16.0, 16.0),
            1.0,
            StarProfile::Gaussian { sigma: 3.0 },
        )],
    );

    let sky = flat_sky(Size2us::new(pixels.width(), pixels.height()));
    let result = extract_candidates(
        &pixels,
        &sky,
        &label_map,
        &local_maxima_config(),
        &JobScratchPool::default(),
    );

    assert_eq!(result.regions.len(), 1);
    assert_eq!(result.deblended_components, 0);
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
        let OneComponent {
            pixels,
            labels: label_map,
        } = one_component(
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

        let result = extract_and_filter_candidates(
            &pixels,
            &flat_sky(Size2us::new(32, 32)),
            &label_map,
            &config,
            &JobScratchPool::default(),
        );

        assert!(
            result.regions.is_empty(),
            "edge_margin {edge_margin} leaves no valid interior in 32x32, so every \
             region must be filtered out"
        );
    }
}
