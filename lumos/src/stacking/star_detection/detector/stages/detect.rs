//! Detection stage: threshold, label, deblend, extract regions.
//!
//! Combines matched filtering (optional), thresholding, connected component
//! labeling, and deblending into a single stage that returns detected regions.

use std::collections::HashMap;

use rayon::prelude::*;

use crate::math::rect::URect;
use common::Vec2us;
use imaginarium::Buffer2;

use crate::stacking::star_detection::background::estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::DetectionConfig;
use crate::stacking::star_detection::convolution::{MatchedFilterBuffers, matched_filter};
use crate::stacking::star_detection::deblend::ComponentData;
use crate::stacking::star_detection::deblend::local_maxima::deblend_local_maxima;
use crate::stacking::star_detection::deblend::multi_threshold::{
    DeblendBuffers, deblend_multi_threshold,
};
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::labeling::LabelMap;
use crate::stacking::star_detection::resources::DetectionResources;

use crate::stacking::star_detection::threshold_mask::{
    create_threshold_mask, create_threshold_mask_filtered,
};

/// Result of detection stage with diagnostic statistics.
#[derive(Debug)]
pub(crate) struct DetectResult {
    /// Detected regions after filtering.
    pub regions: Vec<Region>,
    /// Number of pixels above the detection threshold.
    pub pixels_above_threshold: usize,
    /// Number of connected components found.
    pub connected_components: usize,
    /// Number of components that were deblended into multiple regions.
    pub deblended_components: usize,
}

/// Result of candidate extraction (internal).
struct ExtractionResult {
    regions: Vec<Region>,
    deblended_components: usize,
}

/// Detect star candidate regions in the image.
///
/// Applies matched filtering if FWHM is provided, then performs thresholding,
/// connected component labeling, and deblending to extract candidate regions.
///
/// All buffer management is contained within this function.
pub(crate) fn detect(
    pixels: &Buffer2<f32>,
    stats: &BackgroundEstimate,
    fwhm: Option<f32>,
    config: &DetectionConfig,
    pool: &mut DetectionResources,
) -> DetectResult {
    let width = pixels.width();
    let height = pixels.height();

    // Apply matched filter if FWHM is provided; its output buffer is acquired only then.
    let filtered: Option<Buffer2<f32>> = if let Some(fwhm) = fwhm {
        tracing::debug!(
            "Applying matched filter with FWHM={:.1}, axis_ratio={:.2}, angle={:.1}°",
            fwhm,
            config.psf_axis_ratio,
            config.psf_angle.to_degrees()
        );

        let mut output = pool.acquire_f32();
        let mut convolution_scratch = pool.acquire_f32();
        let mut convolution_temp = pool.acquire_f32();
        matched_filter(
            pixels,
            &stats.background,
            fwhm,
            config.psf_axis_ratio,
            config.psf_angle,
            &mut MatchedFilterBuffers {
                output: &mut output,
                subtraction_scratch: &mut convolution_scratch,
                temp: &mut convolution_temp,
            },
        );
        pool.release_f32(convolution_temp);
        pool.release_f32(convolution_scratch);

        Some(output)
    } else {
        None
    };

    // Acquire mask buffer from pool
    let mut mask = pool.acquire_bit();
    mask.fill(false);

    if let Some(filtered) = &filtered {
        debug_assert_eq!(width, filtered.width());
        debug_assert_eq!(height, filtered.height());
        create_threshold_mask_filtered(filtered, &stats.noise, config.sigma_threshold, &mut mask);
    } else {
        create_threshold_mask(
            pixels,
            &stats.background,
            &stats.noise,
            config.sigma_threshold,
            &mut mask,
        );
    }

    let pixels_above_threshold = mask.count_ones();

    let label_map = LabelMap::from_pool(&mask, config.connectivity, pool);
    let connected_components = label_map.num_labels();

    pool.release_bit(mask);

    let extraction = extract_and_filter_candidates(pixels, &label_map, config, width, height);

    label_map.release_to_pool(pool);
    if let Some(scratch) = filtered {
        pool.release_f32(scratch);
    }

    DetectResult {
        regions: extraction.regions,
        pixels_above_threshold,
        connected_components,
        deblended_components: extraction.deblended_components,
    }
}

/// Extract candidates from label map and filter by size/edge constraints.
fn extract_and_filter_candidates(
    pixels: &Buffer2<f32>,
    label_map: &LabelMap,
    config: &DetectionConfig,
    width: usize,
    height: usize,
) -> ExtractionResult {
    let mut result = extract_candidates(pixels, label_map, config);

    // `DetectionConfig::validate()` can't bound `edge_margin` against the image (it doesn't know the
    // image size), so a margin that swallows the whole image is only catchable here: the retain
    // below needs `bbox.min >= edge_margin && bbox.max <= dim - edge_margin`, which no bbox can
    // satisfy once `2 * edge_margin >= dim` — every region is silently filtered out. Surface it
    // instead of leaving an empty result indistinguishable from "no stars in the image".
    if 2 * config.edge_margin >= width.min(height) {
        tracing::warn!(
            "edge_margin ({}) leaves no valid interior in a {width}x{height} image \
             (needs 2 * edge_margin < the smallest dimension); every detected region \
             will be filtered out",
            config.edge_margin,
        );
    }

    result.regions.retain(|c| {
        c.area >= config.min_area
            && c.bbox.min.x >= config.edge_margin
            && c.bbox.min.y >= config.edge_margin
            && c.bbox.max.x <= width.saturating_sub(config.edge_margin)
            && c.bbox.max.y <= height.saturating_sub(config.edge_margin)
    });

    result
}

/// Extract candidate properties from labeled image with deblending.
fn extract_candidates(
    pixels: &Buffer2<f32>,
    label_map: &LabelMap,
    config: &DetectionConfig,
) -> ExtractionResult {
    if label_map.num_labels() == 0 {
        return ExtractionResult {
            regions: Vec::new(),
            deblended_components: 0,
        };
    }
    let component_data = collect_component_data(label_map);
    let total_components = component_data.len();

    tracing::debug!(
        total_components,
        max_area = config.max_area,
        multi_threshold = config.is_multi_threshold(),
        "Processing components for candidate extraction"
    );

    // Track (regions, deblended_count) where deblended_count is the number of
    // components that produced more than one region.
    let result = if config.is_multi_threshold() {
        let (regions, deblended_components) = component_data
            .into_par_iter()
            .filter(|data| data.area > 0 && data.area <= config.max_area)
            .fold(
                || (Vec::new(), 0usize, DeblendBuffers::new()),
                |(mut regions, mut deblended, mut buffers), data| {
                    let deblend_result = deblend_multi_threshold(
                        &data,
                        pixels,
                        label_map,
                        config.deblend_n_thresholds,
                        config.deblend_min_separation,
                        config.deblend_min_contrast,
                        &mut buffers,
                    );
                    if deblend_result.len() > 1 {
                        deblended += 1;
                    }
                    regions.extend(deblend_result);
                    (regions, deblended, buffers)
                },
            )
            .map(|(regions, deblended, _)| (regions, deblended))
            .reduce(
                || (Vec::new(), 0),
                |(mut a, da), (b, db)| {
                    a.extend(b);
                    (a, da + db)
                },
            );
        ExtractionResult {
            regions,
            deblended_components,
        }
    } else {
        let (regions, deblended_components) = component_data
            .into_par_iter()
            .filter(|data| data.area > 0 && data.area <= config.max_area)
            .fold(
                || (Vec::new(), 0usize),
                |(mut regions, mut deblended), data| {
                    let deblend_result = deblend_local_maxima(
                        &data,
                        pixels,
                        label_map,
                        config.deblend_min_separation,
                        config.deblend_min_prominence,
                    );
                    if deblend_result.len() > 1 {
                        deblended += 1;
                    }
                    regions.extend(deblend_result);
                    (regions, deblended)
                },
            )
            .reduce(
                || (Vec::new(), 0),
                |(mut a, da), (b, db)| {
                    a.extend(b);
                    (a, da + db)
                },
            );
        ExtractionResult {
            regions,
            deblended_components,
        }
    };

    tracing::debug!(
        regions = result.regions.len(),
        deblended = result.deblended_components,
        "Candidate extraction complete"
    );

    result
}

/// Collect component metadata (bounding boxes and areas) from label map.
fn collect_component_data(label_map: &LabelMap) -> Vec<ComponentData> {
    let num_labels = label_map.num_labels();
    let labels = label_map.labels();
    let width = label_map.width();
    let height = label_map.height();

    // Calculate optimal number of jobs for parallel processing
    let num_jobs = (rayon::current_num_threads()).min(height).max(1);
    let rows_per_job = height.div_ceil(num_jobs);

    let partial = (0..num_jobs)
        .into_par_iter()
        .map(|job_idx| {
            let start_row = job_idx * rows_per_job;
            let end_row = (start_row + rows_per_job).min(height);
            let mut local = HashMap::new();

            for y in start_row..end_row {
                let row_start = y * width;
                for x in 0..width {
                    let label = labels[row_start + x];
                    if label == 0 {
                        continue;
                    }
                    let data = local.entry(label).or_insert_with(|| ComponentData {
                        bbox: URect::empty(),
                        label,
                        area: 0,
                    });
                    data.bbox.include(Vec2us::new(x, y));
                    data.area += 1;
                }
            }
            local
        })
        .reduce(HashMap::new, |mut left, mut right| {
            if left.len() < right.len() {
                std::mem::swap(&mut left, &mut right);
            }
            for (label, partial_comp) in right {
                let data = left.entry(label).or_default();
                merge_component_data(data, partial_comp);
            }
            left
        });

    let mut result = vec![ComponentData::default(); num_labels];
    for (label, partial_comp) in partial {
        merge_component_data(&mut result[(label - 1) as usize], partial_comp);
    }
    result
}

fn merge_component_data(target: &mut ComponentData, source: ComponentData) {
    target.bbox = target.bbox.union(source.bbox);
    target.label = source.label;
    target.area += source.area;
}

#[cfg(test)]
mod tests {
    use crate::stacking::star_detection::detector::stages::detect::*;
    use crate::stacking::star_detection::labeling::test_utils::label_map_from_raw;

    /// Render Gaussian `stars` (cx, cy, amplitude, sigma) into a single connected
    /// component: every lit pixel gets label 1.
    fn one_component(
        width: usize,
        height: usize,
        stars: &[(usize, usize, f32, f32)],
    ) -> (Buffer2<f32>, LabelMap) {
        let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
        let mut labels = Buffer2::new_filled(width, height, 0u32);
        for &(cx, cy, amplitude, sigma) in stars {
            let radius = (sigma * 4.0).ceil() as i32;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let x = (cx as i32 + dx) as usize;
                    let y = (cy as i32 + dy) as usize;
                    if x < width && y < height {
                        let r2 = (dx * dx + dy * dy) as f32;
                        let v = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
                        if v > 0.001 {
                            pixels[(x, y)] += v;
                            labels[(x, y)] = 1;
                        }
                    }
                }
            }
        }
        (pixels, label_map_from_raw(labels, 1))
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
            48,
            24,
            &[(12, 12, 1.0, 3.0), (24, 12, 1.0, 3.0), (36, 12, 1.0, 3.0)],
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
        let (pixels, label_map) = one_component(32, 32, &[(16, 16, 1.0, 3.0)]);

        let result = extract_candidates(&pixels, &label_map, &local_maxima_config());

        assert_eq!(result.regions.len(), 1);
        assert_eq!(result.deblended_components, 0);
    }

    #[test]
    fn component_collection_merges_sparse_cross_job_metadata_exactly() {
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
        let label_map = label_map_from_raw(labels, 3);

        let components = collect_component_data(&label_map);

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
            let (pixels, label_map) = one_component(32, 32, &[(16, 16, 1.0, 3.0)]);
            let config = DetectionConfig {
                edge_margin,
                ..local_maxima_config()
            };

            let result = extract_and_filter_candidates(&pixels, &label_map, &config, 32, 32);

            assert!(
                result.regions.is_empty(),
                "edge_margin {edge_margin} leaves no valid interior in 32x32, so every \
                 region must be filtered out"
            );
        }
    }
}
