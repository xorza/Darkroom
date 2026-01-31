//! Star candidate detection using thresholding and connected components.

#[cfg(test)]
mod bench;
mod labels;
#[cfg(test)]
mod tests;

use super::background::BackgroundMap;
use super::common::dilate_mask;
use super::common::threshold_mask::{create_threshold_mask, create_threshold_mask_filtered};
use super::config::{DeblendConfig, StarDetectionConfig};
use super::deblend::{
    ComponentData, DeblendedCandidate, deblend_local_maxima, deblend_multi_threshold,
};
use crate::common::{BitBuffer2, Buffer2};
use crate::math::{Aabb, Vec2us};

pub use labels::LabelMap;

// Re-export DetectionConfig from config module
pub use super::config::DetectionConfig;

/// A candidate star region before centroid refinement.
#[derive(Debug)]
pub struct StarCandidate {
    /// Bounding box.
    pub bbox: Aabb,
    /// Peak pixel X coordinate.
    pub peak_x: usize,
    /// Peak pixel Y coordinate.
    pub peak_y: usize,
    /// Peak pixel value.
    pub peak_value: f32,
    /// Number of pixels in the region.
    pub area: usize,
}

impl StarCandidate {
    /// Width of bounding box.
    #[allow(dead_code)]
    pub fn width(&self) -> usize {
        self.bbox.width()
    }

    /// Height of bounding box.
    #[allow(dead_code)]
    pub fn height(&self) -> usize {
        self.bbox.height()
    }
}

/// Detect star candidates in an image.
///
/// Uses connected component labeling to find regions above the detection threshold.
///
/// # Arguments
/// * `pixels` - Image pixel data (used for peak values and centroiding)
/// * `filtered` - Optional matched-filtered image for thresholding (background-subtracted).
///   If None, thresholds against `pixels` using background map.
/// * `background` - Background map from `estimate_background`
/// * `config` - Detection configuration
pub fn detect_stars(
    pixels: &Buffer2<f32>,
    filtered: Option<&Buffer2<f32>>,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<StarCandidate> {
    let width = pixels.width();
    let height = pixels.height();
    let detection_config = DetectionConfig::from(config);
    let deblend_config = DeblendConfig::from(config);

    // Create binary mask of above-threshold pixels
    let mut mask = BitBuffer2::new_filled(width, height, false);
    if let Some(filtered) = filtered {
        debug_assert_eq!(width, filtered.width());
        debug_assert_eq!(height, filtered.height());
        // Filtered image is background-subtracted, threshold = sigma * noise
        create_threshold_mask_filtered(
            filtered.pixels(),
            background.noise.pixels(),
            detection_config.sigma_threshold,
            &mut mask,
        );
    } else {
        // No filtering, threshold = background + sigma * noise
        create_threshold_mask(
            pixels.pixels(),
            background.background.pixels(),
            background.noise.pixels(),
            detection_config.sigma_threshold,
            &mut mask,
        );
    }

    // Dilate mask to connect nearby pixels that may be separated due to
    // Bayer pattern artifacts or noise. Use radius 1 (3x3 structuring element)
    // to minimize merging of close stars while still connecting fragmented detections.
    let mut dilated = BitBuffer2::new_filled(width, height, false);
    dilate_mask(&mask, 1, &mut dilated);
    std::mem::swap(&mut mask, &mut dilated);

    // Find connected components
    let label_map = LabelMap::from_mask(&mask);

    // Extract candidate properties with deblending (always use original pixels for peak values)
    let mut candidates = extract_candidates(pixels, &label_map, &deblend_config);

    // Filter candidates
    candidates.retain(|c| {
        // Size filter
        c.area >= detection_config.min_area
            // Edge filter
            && c.bbox.min.x >= detection_config.edge_margin
            && c.bbox.min.y >= detection_config.edge_margin
            && c.bbox.max.x < width - detection_config.edge_margin
            && c.bbox.max.y < height - detection_config.edge_margin
    });

    candidates
}

/// Extract candidate properties from labeled image with deblending.
///
/// For components with multiple local maxima (star pairs), this function
/// splits them into separate candidates based on peak positions.
/// Supports both simple local-maxima deblending and multi-threshold deblending.
///
/// Components larger than `deblend_config.max_area` are skipped early to avoid
/// expensive processing of pathologically large regions (e.g., when the entire
/// image is erroneously detected as one component due to bad thresholding).
pub(crate) fn extract_candidates(
    pixels: &Buffer2<f32>,
    label_map: &LabelMap,
    deblend_config: &DeblendConfig,
) -> Vec<StarCandidate> {
    use rayon::prelude::*;

    if label_map.num_labels() == 0 {
        return Vec::new();
    }

    let max_area = deblend_config.max_area;
    let component_data = collect_component_data(label_map, pixels.width(), max_area);

    // Process each component in parallel, deblending into multiple candidates.
    // Skip components that are too large - they can't be stars and would be
    // expensive to process (e.g., deblending a million-pixel component).
    component_data
        .into_par_iter()
        .filter(|data| data.area > 0 && data.area <= max_area)
        .flat_map_iter(|data| {
            let map_to_candidate = |obj: DeblendedCandidate| StarCandidate {
                bbox: obj.bbox,
                peak_x: obj.peak.x,
                peak_y: obj.peak.y,
                peak_value: obj.peak_value,
                area: obj.area,
            };

            // Both deblend functions return stack-allocated collections (ArrayVec/SmallVec),
            // so we can iterate directly without heap allocation.
            if deblend_config.is_multi_threshold() {
                rayon::iter::Either::Left(
                    deblend_multi_threshold(&data, pixels, label_map, deblend_config)
                        .into_iter()
                        .map(map_to_candidate),
                )
            } else {
                rayon::iter::Either::Right(
                    deblend_local_maxima(&data, pixels, label_map, deblend_config)
                        .into_iter()
                        .map(map_to_candidate),
                )
            }
        })
        .collect()
}

/// Collect component metadata (bounding boxes and areas) from label map.
fn collect_component_data(
    label_map: &LabelMap,
    width: usize,
    max_area: usize,
) -> Vec<ComponentData> {
    let num_labels = label_map.num_labels();
    let mut component_data: Vec<ComponentData> = Vec::with_capacity(num_labels);
    component_data.resize_with(num_labels, || ComponentData {
        bbox: Aabb::empty(),
        label: 0,
        area: 0,
    });

    for (idx, &label) in label_map.iter().enumerate() {
        if label == 0 {
            continue;
        }
        let data = &mut component_data[(label - 1) as usize];
        // Skip area counting for oversized components (early termination)
        if data.area > max_area {
            continue;
        }
        let x = idx % width;
        let y = idx / width;
        data.bbox.include(Vec2us::new(x, y));
        data.label = label;
        data.area += 1;
    }

    component_data
}
