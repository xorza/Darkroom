//! Detection stage: threshold, label, deblend, extract regions.
//!
//! Combines matched filtering (optional), thresholding, connected component
//! labeling, and deblending into a single stage that returns detected regions.

use rayon::prelude::*;

use crate::common::Buffer2;
use crate::math::{Aabb, Vec2us};

use super::super::super::background::BackgroundEstimate;
use super::super::super::buffer_pool::BufferPool;
use super::super::super::config::Config;
use super::super::super::convolution::matched_filter;
use super::super::super::deblend::Region;
use super::super::super::deblend::{
    ComponentData, DeblendBuffers, deblend_local_maxima, deblend_multi_threshold,
};
use super::super::super::labeling::LabelMap;
use super::super::super::mask_dilation::dilate_mask;
use super::super::super::threshold_mask::{create_threshold_mask, create_threshold_mask_filtered};

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
    config: &Config,
    pool: &mut BufferPool,
) -> DetectResult {
    let width = pixels.width();
    let height = pixels.height();

    let mut scratch = pool.acquire_f32();

    // Apply matched filter if FWHM is provided
    let filtered: Option<&Buffer2<f32>> = if let Some(fwhm) = fwhm {
        tracing::debug!(
            "Applying matched filter with FWHM={:.1}, axis_ratio={:.2}, angle={:.1}°",
            fwhm,
            config.psf_axis_ratio,
            config.psf_angle.to_degrees()
        );

        let mut convolution_scratch = pool.acquire_f32();
        let mut convolution_temp = pool.acquire_f32();
        matched_filter(
            pixels,
            &stats.background,
            fwhm,
            config.psf_axis_ratio,
            config.psf_angle,
            &mut scratch,
            &mut convolution_scratch,
            &mut convolution_temp,
        );
        pool.release_f32(convolution_temp);
        pool.release_f32(convolution_scratch);

        Some(&scratch)
    } else {
        None
    };

    // Acquire mask buffer from pool
    let mut mask = pool.acquire_bit();
    mask.fill(false);

    if let Some(filtered) = filtered {
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

    // Count pixels above threshold before dilation
    let pixels_above_threshold = mask.count_ones();

    // Dilate mask to connect nearby pixels
    let mut dilated = pool.acquire_bit();
    dilated.fill(false);
    dilate_mask(&mask, 1, &mut dilated);
    std::mem::swap(&mut mask, &mut dilated);

    pool.release_bit(dilated);

    let label_map = LabelMap::from_pool(&mask, config.connectivity, pool);
    let connected_components = label_map.num_labels();

    pool.release_bit(mask);

    let extraction = extract_and_filter_candidates(pixels, &label_map, config, width, height);

    label_map.release_to_pool(pool);
    pool.release_f32(scratch);

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
    config: &Config,
    width: usize,
    height: usize,
) -> ExtractionResult {
    let mut result = extract_candidates(pixels, label_map, config);

    result.regions.retain(|c| {
        c.area >= config.min_area
            && c.bbox.min.x >= config.edge_margin
            && c.bbox.min.y >= config.edge_margin
            && c.bbox.max.x < width - config.edge_margin
            && c.bbox.max.y < height - config.edge_margin
    });

    result
}

/// Extract candidate properties from labeled image with deblending.
fn extract_candidates(
    pixels: &Buffer2<f32>,
    label_map: &LabelMap,
    config: &Config,
) -> ExtractionResult {
    if label_map.num_labels() == 0 {
        return ExtractionResult {
            regions: Vec::new(),
            deblended_components: 0,
        };
    }
    let component_data = collect_component_data(label_map, config.max_area);
    let total_components = component_data.len();

    tracing::debug!(
        total_components,
        max_area = config.max_area,
        multi_threshold = config.is_multi_threshold(),
        "Processing components for candidate extraction"
    );

    let filtered: Vec<_> = component_data
        .into_iter()
        .filter(|data| data.area > 0 && data.area <= config.max_area)
        .collect();

    let num_components = filtered.len();

    // Track (regions, deblended_count) where deblended_count is number of
    // components that produced more than one region
    let result = if config.is_multi_threshold() {
        let (regions, deblended_components) = filtered
            .into_par_iter()
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
        let regions: Vec<Region> = filtered
            .into_par_iter()
            .flat_map_iter(|data| {
                deblend_local_maxima(
                    &data,
                    pixels,
                    label_map,
                    config.deblend_min_separation,
                    config.deblend_min_prominence,
                )
            })
            .collect();
        // For local_maxima, deblended = regions - components (if more regions than components)
        let deblended_components = regions.len().saturating_sub(num_components);
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
fn collect_component_data(label_map: &LabelMap, max_area: usize) -> Vec<ComponentData> {
    use parking_lot::Mutex;
    use rayon::prelude::*;

    let num_labels = label_map.num_labels();
    let labels = label_map.labels();
    let width = label_map.width();
    let height = label_map.height();

    // Calculate optimal number of jobs for parallel processing
    let num_jobs = (rayon::current_num_threads()).min(height).max(1);
    let rows_per_job = (height / num_jobs).max(1);

    let result = Mutex::new(vec![
        ComponentData {
            bbox: Aabb::empty(),
            label: 0,
            area: 0,
        };
        num_labels
    ]);

    (0..num_jobs).into_par_iter().for_each_init(
        || {
            (
                vec![
                    ComponentData {
                        bbox: Aabb::empty(),
                        label: 0,
                        area: 0,
                    };
                    num_labels
                ],
                Vec::<usize>::with_capacity(1024),
            )
        },
        |(local_data, touched), job_idx| {
            let start_row = job_idx * rows_per_job;
            let end_row = if job_idx == num_jobs - 1 {
                height
            } else {
                ((job_idx + 1) * rows_per_job).min(height)
            };

            for &idx in touched.iter() {
                local_data[idx] = ComponentData {
                    bbox: Aabb::empty(),
                    label: 0,
                    area: 0,
                };
            }
            touched.clear();

            for y in start_row..end_row {
                let row_start = y * width;
                for x in 0..width {
                    let label = labels[row_start + x];
                    if label == 0 {
                        continue;
                    }
                    let idx = (label - 1) as usize;
                    let data = &mut local_data[idx];
                    if data.area == 0 {
                        touched.push(idx);
                    }
                    data.bbox.include(Vec2us::new(x, y));
                    data.label = label;
                    data.area += 1;
                }
            }

            let mut result = result.lock();
            for &idx in touched.iter() {
                let partial_comp = &local_data[idx];
                let data = &mut result[idx];
                data.bbox = data.bbox.merge(&partial_comp.bbox);
                data.label = partial_comp.label;
                data.area += partial_comp.area;
            }
        },
    );

    let mut component_data = result.into_inner();

    for data in &mut component_data {
        if data.area > max_area {
            data.area = max_area + 1;
        }
    }

    component_data
}

// =============================================================================
// Test helpers
// =============================================================================

/// Test utility: detect stars with automatic buffer pool management.
///
/// Creates a temporary buffer pool internally. For benchmarks, use
/// `detect` directly with a pre-allocated pool.
#[cfg(test)]
pub(crate) fn detect_stars_test(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    config: &Config,
) -> Vec<Region> {
    let mut pool = BufferPool::new(pixels.width(), pixels.height());
    detect(pixels, background, None, config, &mut pool).regions
}
