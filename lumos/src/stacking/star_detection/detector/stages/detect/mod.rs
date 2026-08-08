//! Detection stage: threshold, label, deblend, extract regions.
//!
//! Combines matched filtering (optional), thresholding, connected component
//! labeling, and deblending into a single stage that returns detected regions.

use parking_lot::Mutex;
use rayon::prelude::*;

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use imaginarium::Buffer2;

use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::detection_config::DetectionConfig;
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
    pub(crate) regions: Vec<Region>,
    /// Number of pixels above the detection threshold.
    pub(crate) pixels_above_threshold: usize,
    /// Number of connected components found.
    pub(crate) connected_components: usize,
    /// Number of components that were deblended into multiple regions.
    pub(crate) deblended_components: usize,
}

/// Result of candidate extraction (internal).
struct ExtractionResult {
    regions: Vec<Region>,
    deblended_components: usize,
}

impl DetectResult {
    /// Detect star candidate regions in the image.
    ///
    /// Applies matched filtering if FWHM is provided, then performs thresholding,
    /// connected component labeling, and deblending to extract candidate regions.
    ///
    /// All buffer management is contained within this function.
    pub(crate) fn from_image(
        pixels: &Buffer2<f32>,
        stats: &BackgroundEstimate,
        fwhm: Option<f32>,
        config: &DetectionConfig,
        pool: &mut DetectionResources,
    ) -> Self {
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
            create_threshold_mask_filtered(
                filtered,
                &stats.noise,
                config.sigma_threshold,
                &mut mask,
            );
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

        let extraction =
            extract_and_filter_candidates(pixels, &label_map, config, Size2us::new(width, height));

        label_map.release_to_pool(pool);
        if let Some(scratch) = filtered {
            pool.release_f32(scratch);
        }

        Self {
            regions: extraction.regions,
            pixels_above_threshold,
            connected_components,
            deblended_components: extraction.deblended_components,
        }
    }
}

/// Extract candidates from label map and filter by size/edge constraints.
fn extract_and_filter_candidates(
    pixels: &Buffer2<f32>,
    label_map: &LabelMap,
    config: &DetectionConfig,
    size: Size2us,
) -> ExtractionResult {
    let mut result = extract_candidates(pixels, label_map, config);

    // `DetectionConfig::validate()` can't bound `edge_margin` against the image (it doesn't know the
    // image size), so a margin that swallows the whole image is only catchable here: the retain
    // below needs `bbox.min >= edge_margin && bbox.max <= dim - edge_margin`, which no bbox can
    // satisfy once `2 * edge_margin >= dim` — every region is silently filtered out. Surface it
    // instead of leaving an empty result indistinguishable from "no stars in the image".
    if 2 * config.edge_margin >= size.width.min(size.height) {
        tracing::warn!(
            "edge_margin ({}) leaves no valid interior in a {}x{} image \
             (needs 2 * edge_margin < the smallest dimension); every detected region \
             will be filtered out",
            config.edge_margin,
            size.width,
            size.height,
        );
    }

    result.regions.retain(|c| {
        c.area >= config.min_area
            && c.bbox.min.x >= config.edge_margin
            && c.bbox.min.y >= config.edge_margin
            && c.bbox.max.x <= size.width.saturating_sub(config.edge_margin)
            && c.bbox.max.y <= size.height.saturating_sub(config.edge_margin)
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
    let height = label_map.height();
    let max_jobs = (rayon::current_num_threads()).min(height).max(1);
    let num_jobs = dense_component_jobs(num_labels, label_map.labels().len(), max_jobs);
    collect_component_data_dense(label_map, num_jobs)
}

fn dense_component_jobs(num_labels: usize, pixel_count: usize, max_jobs: usize) -> usize {
    let bytes_per_job = num_labels.saturating_mul(std::mem::size_of::<ComponentData>());
    if bytes_per_job == 0 {
        return max_jobs;
    }
    let scratch_budget = pixel_count.saturating_mul(std::mem::size_of::<u32>());
    (scratch_budget / bytes_per_job).clamp(1, max_jobs)
}

fn collect_component_data_dense(label_map: &LabelMap, num_jobs: usize) -> Vec<ComponentData> {
    let num_labels = label_map.num_labels();
    let labels = label_map.labels();
    let width = label_map.width();
    let height = label_map.height();
    if num_jobs == 1 {
        let mut result = vec![ComponentData::default(); num_labels];
        accumulate_component_rows(labels, width, 0, height, &mut result, |_| {});
        return result;
    }

    let rows_per_job = height.div_ceil(num_jobs);
    let result = Mutex::new(vec![ComponentData::default(); num_labels]);

    (0..num_jobs).into_par_iter().for_each(|job_idx| {
        let start_row = job_idx * rows_per_job;
        let end_row = (start_row + rows_per_job).min(height);
        let mut local = vec![ComponentData::default(); num_labels];
        let mut touched = Vec::with_capacity(num_labels.min(1024));

        accumulate_component_rows(labels, width, start_row, end_row, &mut local, |index| {
            touched.push(index)
        });

        let mut result = result.lock();
        for index in touched {
            merge_component_data(&mut result[index], local[index]);
        }
    });

    result.into_inner()
}

fn accumulate_component_rows(
    labels: &[u32],
    width: usize,
    start_row: usize,
    end_row: usize,
    data: &mut [ComponentData],
    mut first_seen: impl FnMut(usize),
) {
    for y in start_row..end_row {
        let row_start = y * width;
        for x in 0..width {
            let label = labels[row_start + x];
            if label == 0 {
                continue;
            }
            let index = (label - 1) as usize;
            let component = &mut data[index];
            if component.area == 0 {
                component.label = label;
                first_seen(index);
            }
            component.bbox.include(Vec2us::new(x, y));
            component.area += 1;
        }
    }
}

fn merge_component_data(target: &mut ComponentData, source: ComponentData) {
    target.bbox = target.bbox.union(source.bbox);
    target.label = source.label;
    target.area += source.area;
}

/// Reaches `collect_component_data` from the detector's benchmarks; production code and the
/// tests both go through `DetectResult::from_image`.
#[cfg(all(test, feature = "internals"))]
pub(crate) mod internals {
    use crate::stacking::star_detection::deblend::ComponentData;
    use crate::stacking::star_detection::detector::stages::detect::collect_component_data;
    use crate::stacking::star_detection::labeling::LabelMap;

    pub(crate) fn collect_components(label_map: &LabelMap) -> Vec<ComponentData> {
        collect_component_data(label_map)
    }
}

#[cfg(test)]
mod tests;
