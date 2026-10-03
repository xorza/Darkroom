//! Detection stage: threshold, label, deblend, extract regions.
//!
//! Combines matched filtering (optional), thresholding, connected component
//! labeling, and deblending into a single stage that returns detected regions.

use rayon::prelude::*;

use crate::concurrency::JobScratchPool;
use crate::math::size2us::Size2us;
use imaginarium::Buffer2;

use crate::star_detection::background::sky_noise::SkyNoise;
use crate::star_detection::config::detection_config::{Deblend, DetectionConfig};
use crate::star_detection::convolution::{MatchedFilterBuffers, matched_filter};
use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::local_maxima::deblend_local_maxima;
use crate::star_detection::deblend::multi_threshold::{
    MultiThresholdParams, deblend_multi_threshold,
};
use crate::star_detection::deblend::region::Region;
use crate::star_detection::labeling::LabelMap;
use crate::star_detection::resources::DetectionResources;

use crate::star_detection::threshold_mask::{ThresholdParams, create_residual_threshold_mask};

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
#[derive(Debug, Default)]
struct ExtractionResult {
    regions: Vec<Region>,
    deblended_components: usize,
}

impl DetectResult {
    /// Detect star candidate regions in the residual — the image less its sky.
    ///
    /// Applies the matched filter when `fwhm` is given, then thresholds against the sky noise,
    /// labels the connected components and deblends them, all on the residual.
    pub(crate) fn from_image(
        residual: &Buffer2<f32>,
        sky: &SkyNoise,
        fwhm: Option<f32>,
        config: &DetectionConfig,
        pool: &mut DetectionResources,
    ) -> Self {
        let filtered: Option<Buffer2<f32>> = fwhm.map(|fwhm| {
            tracing::debug!(
                "Applying matched filter with FWHM={:.1}, axis_ratio={:.2}, angle={:.1}°",
                fwhm,
                config.psf_axis_ratio,
                config.psf_angle.to_degrees()
            );
            let mut output = pool.acquire_f32();
            let mut temp = pool.acquire_f32();
            matched_filter(
                residual,
                fwhm,
                config.psf_axis_ratio,
                config.psf_angle,
                &mut MatchedFilterBuffers {
                    output: &mut output,
                    temp: &mut temp,
                },
            );
            pool.release_f32(temp);
            output
        });

        let mut mask = pool.acquire_bit();
        mask.fill(false);
        create_residual_threshold_mask(
            filtered.as_ref().unwrap_or(residual),
            &sky.noise,
            ThresholdParams {
                sigma: config.sigma_threshold,
                min_noise: sky.floor,
            },
            &mut mask,
        );
        let pixels_above_threshold = mask.count_ones();

        let label_map = LabelMap::from_pool(&mask, config.connectivity, pool);
        let connected_components = label_map.num_labels();
        pool.release_bit(mask);
        if let Some(filtered) = filtered {
            pool.release_f32(filtered);
        }

        let extraction =
            extract_and_filter_candidates(residual, sky, &label_map, config, &pool.deblend);
        label_map.release_to_pool(pool);

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
    residual: &Buffer2<f32>,
    sky: &SkyNoise,
    label_map: &LabelMap,
    config: &DetectionConfig,
    deblend_buffers: &JobScratchPool<DeblendBuffers>,
) -> ExtractionResult {
    let size = Size2us::new(residual.width(), residual.height());
    let mut result = extract_candidates(residual, sky, label_map, config, deblend_buffers);

    // `DetectionConfig::validate()` can't bound `edge_margin` against the image (it doesn't know
    // the image size), so a margin that swallows the whole image is only catchable here: the retain
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
        (config.min_area..=config.max_area).contains(&c.area)
            && c.bbox.min.x >= config.edge_margin
            && c.bbox.min.y >= config.edge_margin
            && c.bbox.max.x <= size.width.saturating_sub(config.edge_margin)
            && c.bbox.max.y <= size.height.saturating_sub(config.edge_margin)
    });

    result
}

/// Extract candidate properties from labeled image with deblending.
fn extract_candidates(
    residual: &Buffer2<f32>,
    sky: &SkyNoise,
    label_map: &LabelMap,
    config: &DetectionConfig,
    deblend_buffers: &JobScratchPool<DeblendBuffers>,
) -> ExtractionResult {
    if label_map.num_labels() == 0 {
        return ExtractionResult::default();
    }
    let total_components = label_map.num_labels();

    tracing::debug!(
        total_components,
        max_area = config.max_area,
        deblend = ?config.deblend,
        "Processing components for candidate extraction"
    );

    // One deblend buffer set per fold split, leased from the detector's pool so a frame after the
    // first reuses the last one's.
    let result = label_map
        .components()
        .par_iter()
        .filter(|data| data.area > 0)
        .fold(
            || (ExtractionResult::default(), deblend_buffers.acquire()),
            |(mut acc, mut buffers), data| {
                let component = Component::new(data, residual, label_map);
                let pushed = match config.deblend {
                    Deblend::MultiThreshold {
                        n_thresholds,
                        min_contrast,
                    } => deblend_multi_threshold(
                        &component,
                        sky.threshold_at(component.peak().pos, config.sigma_threshold),
                        MultiThresholdParams {
                            n_thresholds,
                            min_contrast,
                            min_separation: config.deblend_min_separation,
                            connectivity: config.connectivity,
                        },
                        &mut buffers,
                        &mut acc.regions,
                    ),
                    Deblend::LocalMaxima { min_prominence } => deblend_local_maxima(
                        &component,
                        config.deblend_min_separation,
                        min_prominence,
                        &mut buffers,
                        &mut acc.regions,
                    ),
                };
                acc.deblended_components += usize::from(pushed > 1);
                (acc, buffers)
            },
        )
        .map(|(acc, _)| acc)
        .reduce(ExtractionResult::default, |mut a, b| {
            a.regions.extend(b.regions);
            a.deblended_components += b.deblended_components;
            a
        });

    tracing::debug!(
        regions = result.regions.len(),
        deblended = result.deblended_components,
        "Candidate extraction complete"
    );

    result
}

/// Test and bench helpers that reach into this stage.
#[cfg(test)]
pub(crate) mod internals {
    use crate::math::size2us::Size2us;
    use crate::star_detection::background::sky_noise::SkyNoise;
    use crate::star_detection::config::detection_config::DetectionConfig;
    use crate::star_detection::deblend::region::Region;
    use crate::star_detection::detector::stages::detect::DetectResult;
    use crate::star_detection::resources::DetectionResources;
    use imaginarium::Buffer2;

    /// Detect stars in a residual with automatic buffer pool management, allocating a throwaway
    /// [`DetectionResources`] per call. Benchmarks that care about that cost drive
    /// [`DetectResult::from_image`] directly with a pre-allocated pool instead.
    pub(crate) fn detect_stars_test(
        residual: &Buffer2<f32>,
        sky: &SkyNoise,
        config: &DetectionConfig,
    ) -> Vec<Region> {
        detect_test(residual, sky, config).regions
    }

    /// [`detect_stars_test`] with the stage's whole result: the regions and the counts behind
    /// them.
    pub(crate) fn detect_test(
        residual: &Buffer2<f32>,
        sky: &SkyNoise,
        config: &DetectionConfig,
    ) -> DetectResult {
        let mut pool = DetectionResources::new(Size2us::new(residual.width(), residual.height()));
        DetectResult::from_image(residual, sky, None, config, &mut pool)
    }
}

#[cfg(test)]
mod tests;
