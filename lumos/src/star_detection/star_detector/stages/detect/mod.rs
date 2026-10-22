//! Detection stage: threshold, label, deblend, extract regions.
//!
//! Thresholds the detection plane, labels its connected components and deblends them into the
//! regions measurement reads.

use std::mem;

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::scratch_pool::ScratchPool;
use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::local_maxima::LocalMaximaParams;
use crate::star_detection::deblend::multi_threshold::MultiThresholdParams;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::detection_plane::DetectionPlane;
use crate::star_detection::labeling::LabelMap;
use crate::star_detection::resources::DetectionResources;
use crate::star_detection::star_detection_config::extraction_config::{Deblend, ExtractionConfig};

/// The candidate regions a detection plane holds, and what detecting them counted.
///
/// The regions' buffer is the detector's, taken for the detection and handed back with
/// [`Self::release`], so a frame after the first allocates none.
#[derive(Debug)]
pub(crate) struct Candidates {
    /// Detected regions after filtering.
    pub(crate) regions: Vec<Region>,
    /// Number of pixels above the detection threshold.
    pub(crate) pixels_above_threshold: usize,
    /// Number of connected components found.
    pub(crate) connected_components: usize,
    /// Number of components that were deblended into multiple regions.
    pub(crate) deblended_components: usize,
}

/// The plane a detection runs on, and its pixels with no data, which join no component.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DetectionInput<'a> {
    pub(crate) plane: &'a DetectionPlane,
    pub(crate) no_data: Option<&'a BitBuffer2>,
}

/// A detection plane, and the connected components labelled on its threshold mask.
#[derive(Debug, Clone, Copy)]
struct LabelledPlane<'a> {
    plane: &'a DetectionPlane,
    labels: &'a LabelMap,
}

impl Candidates {
    /// Detect star candidate regions on `plane`: threshold it against its own noise, leaving out
    /// the pixels with no data, label the connected components, deblend them, and keep the regions
    /// whose size and place the configuration allows.
    pub(crate) fn detect(
        input: DetectionInput<'_>,
        config: &ExtractionConfig,
        resources: &mut DetectionResources,
    ) -> Self {
        let DetectionInput { plane, no_data } = input;
        let mut mask = resources.acquire_bit();
        plane.threshold(config.sigma_threshold, &mut mask);
        // A pixel with no data holds a fill, not a measurement, and must not join a component.
        if let Some(no_data) = no_data {
            mask.and_not(no_data);
        }
        let pixels_above_threshold = mask.count_ones();

        let mut regions = mem::take(&mut resources.regions);
        regions.clear();
        let label_map = resources.labeler.label(&mask, config.connectivity);
        let connected_components = label_map.label_count();
        let labelled = LabelledPlane {
            plane,
            labels: label_map,
        };
        let deblended_components =
            extract_candidates(labelled, config, &resources.deblend, &mut regions);

        Self {
            regions,
            pixels_above_threshold,
            connected_components,
            deblended_components,
        }
    }

    /// Hand the regions' buffer back to `resources` for the next detection.
    pub(crate) fn release(self, resources: &mut DetectionResources) {
        resources.regions = self.regions;
    }
}

/// Deblend every labelled component on `plane` into `regions`, then keep the regions within the
/// configured area and clear of the edge margin; the count of components split into more than
/// one.
fn extract_candidates(
    labelled: LabelledPlane<'_>,
    config: &ExtractionConfig,
    deblend_buffers: &ScratchPool<DeblendBuffers>,
    regions: &mut Vec<Region>,
) -> usize {
    let LabelledPlane {
        plane,
        labels: label_map,
    } = labelled;
    let values = &plane.values;
    let size = Size2us::new(values.width(), values.height());
    // `ExtractionConfig::validate()` can't bound `edge_margin` against the image (it doesn't know
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
    if label_map.label_count() == 0 {
        return 0;
    }

    tracing::debug!(
        total_components = label_map.label_count(),
        max_area = config.max_area,
        deblend = ?config.deblend,
        "Processing components for candidate extraction"
    );

    // One deblend buffer set per fold split, leased from the detector's pool so a frame after the
    // first reuses the last one's, each split's regions gathered in its set: the reduce appends a
    // later set's to an earlier one's, which keeps the components' order.
    let (deblended_components, mut gathered, _) = label_map
        .components()
        .par_iter()
        // A component smaller than `min_area` holds no region that large. One larger than
        // `max_area` is still deblended: a crowded group splits into regions within it.
        .filter(|data| data.area >= config.min_area)
        .fold(
            || (0, deblend_buffers.acquire(), plane.noise.noise.reader()),
            |(mut deblended, mut buffers, mut noise), data| {
                let component = Component::new(data, values, label_map);
                let DeblendBuffers { regions: found, .. } = &mut *buffers;
                let mut split = mem::take(found);
                let pushed = match config.deblend {
                    Deblend::MultiThreshold {
                        threshold_count,
                        min_contrast,
                    } => MultiThresholdParams {
                        threshold_count,
                        min_contrast,
                        min_separation: config.deblend_min_separation,
                        min_area: config.min_area,
                        connectivity: config.connectivity,
                        floor: {
                            let peak = component.peak().pos();
                            plane
                                .threshold_params(config.sigma_threshold)
                                .level(noise.at(peak))
                        },
                    }
                    .deblend(&component, &mut buffers, &mut split),
                    Deblend::LocalMaxima {
                        min_relative_height,
                    } => LocalMaximaParams {
                        min_separation: config.deblend_min_separation,
                        min_relative_height,
                    }
                    .deblend(&component, &mut buffers, &mut split),
                };
                buffers.regions = split;
                if data.area > DeblendBuffers::KEPT_PIXELS {
                    buffers.release();
                }
                deblended += usize::from(pushed > 1);
                (deblended, buffers, noise)
            },
        )
        .reduce(
            || (0, deblend_buffers.acquire(), plane.noise.noise.reader()),
            |(a, mut first, noise), (b, mut second, _)| {
                first.regions.append(&mut second.regions);
                (a + b, first, noise)
            },
        );
    regions.append(&mut gathered.regions);

    tracing::debug!(
        regions = regions.len(),
        deblended = deblended_components,
        "Candidate extraction complete"
    );

    regions.retain(|c| {
        (config.min_area..=config.max_area).contains(&c.area)
            && c.bbox.min.x >= config.edge_margin
            && c.bbox.min.y >= config.edge_margin
            && c.bbox.max.x <= size.width.saturating_sub(config.edge_margin)
            && c.bbox.max.y <= size.height.saturating_sub(config.edge_margin)
    });
    deblended_components
}

/// Test and bench helpers that reach into this stage.
#[cfg(test)]
pub(crate) mod internals {
    use crate::math::size2us::Size2us;
    use crate::scratch_pool::Pooled;
    use crate::star_detection::bg::sky_noise::SkyNoise;
    use crate::star_detection::deblend::region::Region;
    use crate::star_detection::detection_plane::DetectionPlane;
    use crate::star_detection::resources::DetectionResources;
    use crate::star_detection::star_detection_config::extraction_config::ExtractionConfig;
    use crate::star_detection::star_detector::stages::detect::{Candidates, DetectionInput};
    use imaginarium::Buffer2;

    /// Detect stars in `residual` taken as the detection plane, its noise `sky`, with a throwaway
    /// [`DetectionResources`] per call.
    pub(crate) fn detect_stars_test(
        residual: &Buffer2<f32>,
        sky: &SkyNoise,
        config: &ExtractionConfig,
    ) -> Vec<Region> {
        detect_test(residual, sky, config).regions
    }

    /// [`detect_stars_test`] with the stage's whole result: the regions and the counts behind
    /// them.
    pub(crate) fn detect_test(
        residual: &Buffer2<f32>,
        sky: &SkyNoise,
        config: &ExtractionConfig,
    ) -> Candidates {
        let size = Size2us::new(residual.width(), residual.height());
        let mut resources = DetectionResources::new(size);
        let plane = DetectionPlane {
            values: Pooled::loose(residual.clone(), size),
            noise: SkyNoise {
                noise: Pooled::loose(sky.noise.duplicate(), ()),
                floor: sky.floor,
            },
        };
        Candidates::detect(
            DetectionInput {
                plane: &plane,
                no_data: None,
            },
            config,
            &mut resources,
        )
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
