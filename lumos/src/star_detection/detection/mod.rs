//! Star candidate detection using thresholding and connected components.

#[cfg(test)]
mod tests;

use super::StarDetectionConfig;
use super::background::BackgroundMap;
use super::common::dilate_mask;
use super::common::threshold_mask::{create_threshold_mask, create_threshold_mask_filtered};
use super::deblend::{
    ComponentData, DeblendConfig, MultiThresholdDeblendConfig, Pixel,
    deblend_component as multi_threshold_deblend, deblend_local_maxima,
};
use crate::common::{BitBuffer2, Buffer2};

/// A candidate star region before centroid refinement.
#[derive(Debug)]
pub struct StarCandidate {
    /// Bounding box min X.
    pub x_min: usize,
    /// Bounding box max X.
    pub x_max: usize,
    /// Bounding box min Y.
    pub y_min: usize,
    /// Bounding box max Y.
    pub y_max: usize,
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
        self.x_max - self.x_min + 1
    }

    /// Height of bounding box.
    #[allow(dead_code)]
    pub fn height(&self) -> usize {
        self.y_max - self.y_min + 1
    }
}

/// Configuration for detection algorithm.
#[derive(Debug, Clone)]
pub struct DetectionConfig {
    /// Detection threshold in sigma above background.
    pub sigma_threshold: f32,
    /// Minimum area in pixels.
    pub min_area: usize,
    /// Maximum area in pixels.
    pub max_area: usize,
    /// Edge margin (reject candidates near edges).
    pub edge_margin: usize,
}

impl From<&StarDetectionConfig> for DetectionConfig {
    fn from(config: &StarDetectionConfig) -> Self {
        Self {
            sigma_threshold: config.background_config.detection_sigma,
            min_area: config.min_area,
            max_area: config.max_area,
            edge_margin: config.edge_margin,
        }
    }
}

/// Detect star candidates in an image (without matched filtering).
///
/// Uses connected component labeling to find regions above the detection threshold.
/// For better faint-star detection, use `find_stars()` with `expected_fwhm > 0`.
///
/// # Arguments
/// * `pixels` - Image pixel data
/// * `background` - Background map from `estimate_background`
/// * `config` - Detection configuration
pub fn detect_stars(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<StarCandidate> {
    let width = pixels.width();
    let height = pixels.height();
    let detection_config = DetectionConfig::from(config);

    // Create binary mask of above-threshold pixels
    let mut mask = BitBuffer2::new_filled(width, height, false);
    create_threshold_mask(
        pixels.pixels(),
        background.background.pixels(),
        background.noise.pixels(),
        detection_config.sigma_threshold,
        &mut mask,
    );

    // Dilate mask to connect nearby pixels that may be separated due to
    // Bayer pattern artifacts or noise. Use radius 1 (3x3 structuring element)
    // to minimize merging of close stars while still connecting fragmented detections.
    let mut dilated = BitBuffer2::new_filled(width, height, false);
    dilate_mask(&mask, 1, &mut dilated);
    std::mem::swap(&mut mask, &mut dilated);

    // Find connected components
    let (labels, num_labels) = connected_components(&mask);

    // Extract candidate properties with deblending
    let deblend_config = DeblendConfig::from(config);
    let mut candidates = extract_candidates(
        pixels,
        &labels,
        num_labels,
        &deblend_config,
        detection_config.max_area,
    );

    // Filter candidates
    candidates.retain(|c| {
        // Size filter
        c.area >= detection_config.min_area
            // Edge filter
            && c.x_min >= detection_config.edge_margin
            && c.y_min >= detection_config.edge_margin
            && c.x_max < width - detection_config.edge_margin
            && c.y_max < height - detection_config.edge_margin
    });

    candidates
}

/// Connected component labeling using union-find with parallel second pass.
///
/// Uses a two-pass algorithm:
/// 1. First pass: Sequential scan assigning provisional labels with union-find
/// 2. Second pass: Parallel label flattening using precomputed root mapping
///
/// For images >1M pixels, the second pass is parallelized using rayon.
pub(crate) fn connected_components(mask: &BitBuffer2) -> (Vec<u32>, usize) {
    let width = mask.width();
    let height = mask.height();

    let mut labels = vec![0u32; width * height];
    let mut parent: Vec<u32> = Vec::new();
    let mut next_label = 1u32;

    // First pass: assign provisional labels
    // This must be sequential due to union-find dependencies
    for y in 0..height {
        let row_start = y * width;
        for x in 0..width {
            let idx = row_start + x;
            if !mask.get(idx) {
                continue;
            }

            // Check neighbors: left and top only (for forward scan)
            let left_label = if x > 0 && mask.get(idx - 1) {
                labels[idx - 1]
            } else {
                0
            };

            let top_label = if y > 0 && mask.get(idx - width) {
                labels[idx - width]
            } else {
                0
            };

            match (left_label, top_label) {
                (0, 0) => {
                    // New component
                    labels[idx] = next_label;
                    parent.push(next_label);
                    next_label += 1;
                }
                (l, 0) => {
                    // Only left neighbor
                    labels[idx] = l;
                }
                (0, t) => {
                    // Only top neighbor
                    labels[idx] = t;
                }
                (l, t) => {
                    // Both neighbors - use minimum and union
                    let min_label = l.min(t);
                    labels[idx] = min_label;
                    if l != t {
                        union(&mut parent, l, t);
                    }
                }
            }
        }
    }

    if parent.is_empty() {
        return (labels, 0);
    }

    // Flatten all roots first (sequential, but small - just parent.len() elements)
    let mut root_to_final = vec![0u32; parent.len() + 1];
    let mut num_labels = 0u32;
    for label in 1..=parent.len() as u32 {
        let root = find(&mut parent, label);
        if root_to_final[root as usize] == 0 {
            num_labels += 1;
            root_to_final[root as usize] = num_labels;
        }
    }

    // Create direct mapping from provisional label to final label
    let label_map: Vec<u32> = (0..=parent.len() as u32)
        .map(|label| {
            if label == 0 {
                0
            } else {
                let root = find(&mut parent, label);
                root_to_final[root as usize]
            }
        })
        .collect();

    // Second pass: apply label mapping
    // Parallelize for large images (>4M pixels, i.e., 2K×2K and larger)
    let pixel_count = width * height;
    if pixel_count > 4_000_000 {
        use rayon::prelude::*;
        labels.par_iter_mut().for_each(|label| {
            if *label != 0 {
                *label = label_map[*label as usize];
            }
        });
    } else {
        for label in labels.iter_mut() {
            if *label != 0 {
                *label = label_map[*label as usize];
            }
        }
    }

    (labels, num_labels as usize)
}

/// Find root of a label with path compression.
fn find(parent: &mut [u32], label: u32) -> u32 {
    let idx = (label - 1) as usize;
    if parent[idx] != label {
        parent[idx] = find(parent, parent[idx]); // Path compression
    }
    parent[idx]
}

/// Union two labels.
fn union(parent: &mut [u32], a: u32, b: u32) {
    let root_a = find(parent, a);
    let root_b = find(parent, b);
    if root_a != root_b {
        // Union by making smaller root point to larger
        if root_a < root_b {
            parent[(root_b - 1) as usize] = root_a;
        } else {
            parent[(root_a - 1) as usize] = root_b;
        }
    }
}

/// Detect star candidates using a filtered image for thresholding.
///
/// This variant uses matched filtering (Gaussian convolution) to improve SNR
/// for faint star detection. The filtered image is used for thresholding,
/// while the original image is used for peak values and centroiding.
///
/// # Arguments
/// * `pixels` - Original image pixel data (for peak values)
/// * `filtered` - Matched-filtered image (background-subtracted and convolved)
/// * `background` - Background map (for noise estimates)
/// * `config` - Detection configuration
pub fn detect_stars_filtered(
    pixels: &Buffer2<f32>,
    filtered: &Buffer2<f32>,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<StarCandidate> {
    let width = pixels.width();
    let height = pixels.height();
    debug_assert_eq!(width, filtered.width());
    debug_assert_eq!(height, filtered.height());
    let detection_config = DetectionConfig::from(config);

    // Create binary mask from the filtered image
    // The threshold is based on noise in the filtered image
    let mut mask = BitBuffer2::new_filled(width, height, false);
    create_threshold_mask_filtered(
        filtered.pixels(),
        background.noise.pixels(),
        detection_config.sigma_threshold,
        &mut mask,
    );

    // Dilate mask with radius 1 to connect fragmented detections while
    // minimizing merging of close stars. Matched filtering already provides
    // good connectivity, so minimal dilation is needed.
    let mut dilated = BitBuffer2::new_filled(width, height, false);
    dilate_mask(&mask, 1, &mut dilated);
    std::mem::swap(&mut mask, &mut dilated);
    drop(dilated);

    // Find connected components
    let (labels, num_labels) = connected_components(&mask);

    // Extract candidate properties using ORIGINAL pixels for peak values, with deblending
    let deblend_config = DeblendConfig::from(config);
    let mut candidates = extract_candidates(
        pixels,
        &labels,
        num_labels,
        &deblend_config,
        detection_config.max_area,
    );

    // Filter candidates
    candidates.retain(|c| {
        // Size filter
        c.area >= detection_config.min_area
            // Edge filter
            && c.x_min >= detection_config.edge_margin
            && c.y_min >= detection_config.edge_margin
            && c.x_max < width - detection_config.edge_margin
            && c.y_max < height - detection_config.edge_margin
    });

    candidates
}

impl From<&StarDetectionConfig> for DeblendConfig {
    fn from(config: &StarDetectionConfig) -> Self {
        Self {
            min_separation: config.deblend_min_separation,
            min_prominence: config.deblend_min_prominence,
            multi_threshold: config.multi_threshold_deblend,
            n_thresholds: config.deblend_nthresh,
            min_contrast: config.deblend_min_contrast,
        }
    }
}

/// Extract candidate properties from labeled image with deblending.
///
/// For components with multiple local maxima (star pairs), this function
/// splits them into separate candidates based on peak positions.
/// Supports both simple local-maxima deblending and multi-threshold deblending.
///
/// Components larger than `max_area` are skipped early to avoid expensive
/// processing of pathologically large regions (e.g., when the entire image
/// is erroneously detected as one component due to bad thresholding).
pub(crate) fn extract_candidates(
    pixels: &Buffer2<f32>,
    labels: &[u32],
    num_labels: usize,
    deblend_config: &DeblendConfig,
    max_area: usize,
) -> Vec<StarCandidate> {
    if num_labels == 0 {
        return Vec::new();
    }
    use rayon::prelude::*;
    let width = pixels.width();

    // Collect component data in single pass
    // Stop collecting pixels for components that exceed max_area to avoid
    // allocating millions of pixels for pathologically large components
    let mut component_data: Vec<ComponentData> = Vec::with_capacity(num_labels);
    component_data.resize_with(num_labels, || ComponentData {
        x_min: usize::MAX,
        x_max: 0,
        y_min: usize::MAX,
        y_max: 0,
        pixels: Vec::with_capacity(50),
    });

    for (idx, &label) in labels.iter().enumerate() {
        if label == 0 {
            continue;
        }
        let data = &mut component_data[(label - 1) as usize];
        // Skip pixel collection for oversized components (already exceeded max_area)
        if data.pixels.len() > max_area {
            continue;
        }
        let x = idx % width;
        let y = idx / width;
        data.x_min = data.x_min.min(x);
        data.x_max = data.x_max.max(x);
        data.y_min = data.y_min.min(y);
        data.y_max = data.y_max.max(y);
        data.pixels.push(Pixel {
            x,
            y,
            value: pixels[idx],
        });
    }

    // Process each component in parallel, deblending into multiple candidates
    // Skip components that are too large - they can't be stars and would be
    // expensive to process (e.g., deblending a million-pixel component)
    let candidates: Vec<StarCandidate> = component_data
        .into_par_iter()
        .filter(|data| !data.pixels.is_empty() && data.pixels.len() <= max_area)
        .flat_map(|data| {
            if deblend_config.multi_threshold {
                // Use multi-threshold deblending (SExtractor-style)
                let mt_config = MultiThresholdDeblendConfig {
                    n_thresholds: deblend_config.n_thresholds,
                    min_contrast: deblend_config.min_contrast,
                    min_separation: deblend_config.min_separation,
                };

                // Estimate detection threshold from the minimum pixel value in component
                let detection_threshold =
                    data.pixels.iter().map(|p| p.value).fold(f32::MAX, f32::min);

                let deblended =
                    multi_threshold_deblend(&data.pixels, width, detection_threshold, &mt_config);

                deblended
                    .into_iter()
                    .map(|obj| StarCandidate {
                        x_min: obj.bbox.0,
                        x_max: obj.bbox.1,
                        y_min: obj.bbox.2,
                        y_max: obj.bbox.3,
                        peak_x: obj.peak_x,
                        peak_y: obj.peak_y,
                        peak_value: obj.peak_value,
                        area: obj.pixels.len(),
                    })
                    .collect::<Vec<_>>()
            } else {
                // Use simple local-maxima deblending from deblend module
                let deblended = deblend_local_maxima(&data, pixels, deblend_config);

                deblended
                    .into_iter()
                    .map(|obj| StarCandidate {
                        x_min: obj.x_min,
                        x_max: obj.x_max,
                        y_min: obj.y_min,
                        y_max: obj.y_max,
                        peak_x: obj.peak_x,
                        peak_y: obj.peak_y,
                        peak_value: obj.peak_value,
                        area: obj.area,
                    })
                    .collect::<Vec<_>>()
            }
        })
        .collect();

    candidates
}
