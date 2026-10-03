//! Detection stage: threshold, label, deblend, extract regions.
//!
//! Thresholds the detection plane, labels its connected components and deblends them into the
//! regions measurement reads.

use rayon::prelude::*;

use crate::concurrency::JobScratchPool;
use crate::math::size2us::Size2us;

use crate::bit_buffer2::BitBuffer2;
use crate::star_detection::config::detection_config::{Deblend, DetectionConfig};
use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::local_maxima::deblend_local_maxima;
use crate::star_detection::deblend::multi_threshold::{
    MultiThresholdParams, deblend_multi_threshold,
};
use crate::star_detection::deblend::region::Region;
use crate::star_detection::detection_plane::DetectionPlane;
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
    /// Detect star candidate regions on `plane`: threshold it against its own noise, leaving out
    /// the pixels with no data, label the connected components, deblend them, and keep the regions
    /// whose size and place the configuration allows.
    pub(crate) fn from_plane(
        plane: &DetectionPlane,
        no_data: Option<&BitBuffer2>,
        config: &DetectionConfig,
        pool: &mut DetectionResources,
    ) -> Self {
        let mut mask = pool.acquire_bit();
        mask.fill(false);
        create_residual_threshold_mask(
            &plane.values,
            &plane.noise.noise,
            ThresholdParams {
                sigma: config.sigma_threshold,
                min_noise: plane.noise.floor,
            },
            &mut mask,
        );
        // A pixel with no data holds a fill, not a measurement, and must not join a component.
        if let Some(no_data) = no_data {
            mask.and_not(no_data);
        }
        let pixels_above_threshold = mask.count_ones();

        let label_map = LabelMap::from_pool(&mask, config.connectivity, pool);
        let connected_components = label_map.num_labels();
        pool.release_bit(mask);

        let extraction = extract_candidates(plane, &label_map, config, &pool.deblend);
        label_map.release_to_pool(pool);

        Self {
            regions: extraction.regions,
            pixels_above_threshold,
            connected_components,
            deblended_components: extraction.deblended_components,
        }
    }
}

/// Deblend every labelled component on `plane`, then keep the regions within the configured area
/// and clear of the edge margin.
fn extract_candidates(
    plane: &DetectionPlane,
    label_map: &LabelMap,
    config: &DetectionConfig,
    deblend_buffers: &JobScratchPool<DeblendBuffers>,
) -> ExtractionResult {
    let values = &plane.values;
    let size = Size2us::new(values.width(), values.height());
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
    if label_map.num_labels() == 0 {
        return ExtractionResult::default();
    }

    tracing::debug!(
        total_components = label_map.num_labels(),
        max_area = config.max_area,
        deblend = ?config.deblend,
        "Processing components for candidate extraction"
    );

    // One deblend buffer set per fold split, leased from the detector's pool so a frame after the
    // first reuses the last one's.
    let mut result = label_map
        .components()
        .par_iter()
        .filter(|data| data.area > 0)
        .fold(
            || (ExtractionResult::default(), deblend_buffers.acquire()),
            |(mut acc, mut buffers), data| {
                let component = Component::new(data, values, label_map);
                let pushed = match config.deblend {
                    Deblend::MultiThreshold {
                        n_thresholds,
                        min_contrast,
                    } => deblend_multi_threshold(
                        &component,
                        plane
                            .noise
                            .threshold_at(component.peak().pos, config.sigma_threshold),
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

    result.regions.retain(|c| {
        (config.min_area..=config.max_area).contains(&c.area)
            && c.bbox.min.x >= config.edge_margin
            && c.bbox.min.y >= config.edge_margin
            && c.bbox.max.x <= size.width.saturating_sub(config.edge_margin)
            && c.bbox.max.y <= size.height.saturating_sub(config.edge_margin)
    });
    result
}

/// Test and bench helpers that reach into this stage.
#[cfg(test)]
pub(crate) mod internals {
    use crate::math::size2us::Size2us;
    use crate::star_detection::background::sky_noise::SkyNoise;
    use crate::star_detection::config::detection_config::DetectionConfig;
    use crate::star_detection::deblend::region::Region;
    use crate::star_detection::detection_plane::DetectionPlane;
    use crate::star_detection::detector::stages::detect::DetectResult;
    use crate::star_detection::resources::DetectionResources;
    use imaginarium::Buffer2;

    /// Detect stars in `residual` taken as the detection plane, its noise `sky`, with a throwaway
    /// [`DetectionResources`] per call.
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
        let plane = DetectionPlane {
            values: residual.clone(),
            noise: SkyNoise {
                noise: sky.noise.clone(),
                floor: sky.floor,
            },
        };
        DetectResult::from_plane(&plane, None, config, &mut pool)
    }
}

#[cfg(test)]
mod tests;
