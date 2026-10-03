use crate::internals::prelude::*;
use crate::internals::synthetic::background_map;
use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::deblend::internals::{TestComponent, make_test_component};
use crate::star_detection::detector::stages::detect::internals::detect_test;
use crate::star_detection::detector::stages::detect::*;

fn local_maxima_config() -> DetectionConfig {
    DetectionConfig {
        deblend: Deblend::LocalMaxima {
            min_prominence: 0.3,
        },
        deblend_min_separation: 3,
        min_area: 1,
        max_area: usize::MAX,
        edge_margin: 0,
        ..Default::default()
    }
}

/// A flat sky noise of 0.01 over `size`.
fn flat_sky(size: Size2us) -> SkyNoise {
    background_map::uniform(size, 0.0, 0.01).sky_noise()
}

/// `values` as a detection plane under a flat noise of 0.01.
fn flat_plane(values: &Buffer2<f32>) -> DetectionPlane {
    DetectionPlane {
        values: values.clone(),
        noise: flat_sky(Size2us::new(values.width(), values.height())),
    }
}

#[test]
fn local_maxima_deblended_counts_split_components_not_extra_regions() {
    // One connected blob with three well-separated peaks. Local-maxima deblending
    // splits it into three regions, but it is ONE component that split, so
    // `deblended_components` is 1, not `regions − components` = 2.
    let TestComponent {
        pixels,
        labels: label_map,
        ..
    } = make_test_component(
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

    let result = extract_candidates(
        &flat_plane(&pixels),
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
    let TestComponent {
        pixels,
        labels: label_map,
        ..
    } = make_test_component(
        Size2us::new(32, 32),
        &[SyntheticStar::new(
            Vec2::new(16.0, 16.0),
            1.0,
            StarProfile::Gaussian { sigma: 3.0 },
        )],
    );

    let result = extract_candidates(
        &flat_plane(&pixels),
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
        let TestComponent {
            pixels,
            labels: label_map,
            ..
        } = make_test_component(
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

        let result = extract_candidates(
            &flat_plane(&pixels),
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

/// One labelled rectangle: its half-open corners and the pixels that peak at 1.0 inside it.
#[derive(Debug)]
struct Rect {
    min: (usize, usize),
    max: (usize, usize),
    peaks: &'static [(usize, usize)],
}

/// A residual of 0.1 over each rectangle with its peaks at 1.0, and the label map naming one
/// component per rectangle, in order.
#[derive(Debug)]
struct Rectangles {
    residual: Buffer2<f32>,
    labels: LabelMap,
}

fn rectangles(size: Size2us, rects: &[Rect]) -> Rectangles {
    let mut residual = Buffer2::new_filled(size.width, size.height, 0.0f32);
    let mut labels = Buffer2::new_filled(size.width, size.height, 0u32);
    for (index, rect) in rects.iter().enumerate() {
        for y in rect.min.1..rect.max.1 {
            for x in rect.min.0..rect.max.0 {
                residual[(x, y)] = 0.1;
                labels[(x, y)] = index as u32 + 1;
            }
        }
        for &(x, y) in rect.peaks {
            residual[(x, y)] = 1.0;
        }
    }
    Rectangles {
        residual,
        labels: LabelMap::from_raw(labels, rects.len()),
    }
}

/// Which regions survive the area and edge filters, each at its bound: both area bounds apply to a
/// region after deblending — so a component too large is kept as the pieces that fit, and pieces
/// too small are dropped even from a component that is not. The edge margin admits a box that
/// starts at it or ends at `size − margin` (boxes are half-open), and nothing beyond.
#[test]
fn region_filter_bounds() {
    let size = Size2us::new(64, 64);
    let config = DetectionConfig {
        min_area: 5,
        max_area: 20,
        edge_margin: 10,
        ..local_maxima_config()
    };
    let Rectangles {
        residual,
        labels: label_map,
    } = rectangles(
        size,
        &[
            // Area 4 < 5: dropped.
            Rect {
                min: (20, 12),
                max: (22, 14),
                peaks: &[(20, 12)],
            },
            // Area 5: kept.
            Rect {
                min: (30, 12),
                max: (35, 13),
                peaks: &[(32, 12)],
            },
            // Area 20 = max: kept.
            Rect {
                min: (40, 12),
                max: (44, 17),
                peaks: &[(41, 13)],
            },
            // Area 21 > max: dropped.
            Rect {
                min: (12, 20),
                max: (15, 27),
                peaks: &[(13, 22)],
            },
            // Starts at x = 10, the margin: kept.
            Rect {
                min: (10, 30),
                max: (13, 32),
                peaks: &[(11, 30)],
            },
            // Starts at x = 9: dropped.
            Rect {
                min: (9, 35),
                max: (12, 37),
                peaks: &[(10, 35)],
            },
            // Ends at x = 54 = 64 − 10: kept.
            Rect {
                min: (51, 40),
                max: (54, 42),
                peaks: &[(52, 40)],
            },
            // Ends at x = 55: dropped.
            Rect {
                min: (52, 45),
                max: (55, 47),
                peaks: &[(53, 45)],
            },
            // Area 30 > max, two peaks that split it into 15s: both kept.
            Rect {
                min: (20, 48),
                max: (30, 51),
                peaks: &[(21, 49), (28, 49)],
            },
            // Area 8, two peaks that split it into 4s, each under min: both dropped.
            Rect {
                min: (34, 48),
                max: (42, 49),
                peaks: &[(34, 48), (41, 48)],
            },
        ],
    );

    let result = extract_candidates(
        &flat_plane(&residual),
        &label_map,
        &config,
        &JobScratchPool::default(),
    );
    let mut kept: Vec<(usize, usize)> = result
        .regions
        .iter()
        .map(|region| (region.peak.x, region.peak.y))
        .collect();
    kept.sort_unstable();
    assert_eq!(
        kept,
        [(11, 30), (21, 49), (28, 49), (32, 12), (41, 13), (52, 40)]
    );
    // Both two-peak components split, though neither piece of the area-8 one survived.
    assert_eq!(result.deblended_components, 2);
}

/// The stage's counts on a residual whose answer is known: two 3×3 squares and one 10×1 bar of
/// 1.0 on zero, against a 0.04 threshold (4 · 0.01). The bar has a peak at each end, 9 px apart,
/// which the multi-threshold tree splits; the squares are flat and stay whole.
#[test]
fn detect_counts_pixels_components_and_splits() {
    let size = Size2us::new(64, 64);
    let mut residual = Buffer2::new_filled(size.width, size.height, 0.0f32);
    for (x0, y0) in [(15, 15), (40, 15)] {
        for y in y0..y0 + 3 {
            for x in x0..x0 + 3 {
                residual[(x, y)] = 1.0;
            }
        }
    }
    for x in 20..30 {
        residual[(x, 40)] = 0.5;
    }
    residual[(20, 40)] = 1.0;
    residual[(29, 40)] = 1.0;
    let config = DetectionConfig {
        deblend: Deblend::MultiThreshold {
            n_thresholds: 32,
            min_contrast: 0.005,
        },
        min_area: 1,
        ..DetectionConfig::default()
    };

    let result = detect_test(&residual, &flat_sky(size), &config);
    assert_eq!(result.pixels_above_threshold, 9 + 9 + 10);
    assert_eq!(result.connected_components, 3);
    assert_eq!(result.deblended_components, 1);
    let mut peaks: Vec<(usize, usize)> = result
        .regions
        .iter()
        .map(|region| (region.peak.x, region.peak.y))
        .collect();
    peaks.sort_unstable();
    assert_eq!(peaks, [(15, 15), (20, 40), (29, 40), (40, 15)]);
}
