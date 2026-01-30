//! Star candidate detection using thresholding and connected components.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use super::background::BackgroundMap;
use super::common::dilate_mask;
use super::common::threshold_mask::{create_threshold_mask, create_threshold_mask_filtered};
use super::config::{DeblendConfig, StarDetectionConfig};
use super::deblend::{
    ComponentData, MultiThresholdDeblendConfig, Pixel,
    deblend_component as multi_threshold_deblend, deblend_local_maxima,
};
use crate::common::{BitBuffer2, Buffer2};

// Re-export DetectionConfig from config module
pub use super::config::DetectionConfig;

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
    let (labels, num_labels) = connected_components(&mask);

    // Extract candidate properties with deblending (always use original pixels for peak values)
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
pub(crate) fn connected_components(mask: &BitBuffer2) -> (Buffer2<u32>, usize) {
    let width = mask.width();
    let height = mask.height();

    let mut labels = Buffer2::new_filled(width, height, 0u32);
    let mut parent: Vec<u32> = Vec::new();

    // First pass: assign provisional labels with union-find
    assign_provisional_labels(mask, &mut labels, &mut parent);

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
    let labels_data = labels.pixels_mut();
    if pixel_count > 4_000_000 {
        use rayon::prelude::*;
        labels_data.par_iter_mut().for_each(|label| {
            if *label != 0 {
                *label = label_map[*label as usize];
            }
        });
    } else {
        for label in labels_data.iter_mut() {
            if *label != 0 {
                *label = label_map[*label as usize];
            }
        }
    }

    (labels, num_labels as usize)
}

/// First pass of connected components: assign provisional labels using union-find.
///
/// Scans the mask sequentially, assigning labels to foreground pixels and
/// merging components when neighbors from different components meet.
fn assign_provisional_labels(mask: &BitBuffer2, labels: &mut Buffer2<u32>, parent: &mut Vec<u32>) {
    assert_eq!(
        (mask.width(), mask.height()),
        (labels.width(), labels.height()),
        "mask and labels must have same dimensions"
    );

    let width = mask.width();
    let height = mask.height();
    let mut next_label = 1u32;

    for y in 0..height {
        let row_start = y * width;
        for x in 0..width {
            let idx = row_start + x;
            if !mask.get(idx) {
                continue;
            }

            // Check neighbors: left and top only (for forward scan)
            let left = (x > 0 && mask.get(idx - 1)).then(|| labels[idx - 1]);
            let top = (y > 0 && mask.get(idx - width)).then(|| labels[idx - width]);

            labels[idx] = match (left, top) {
                (None, None) => {
                    parent.push(next_label);
                    next_label += 1;
                    next_label - 1
                }
                (Some(l), None) => l,
                (None, Some(t)) => t,
                (Some(l), Some(t)) => {
                    if l != t {
                        union(parent, l, t);
                    }
                    l.min(t)
                }
            };
        }
    }
}

/// Find root of a label with path compression.
fn find(parent: &mut [u32], label: u32) -> u32 {
    let idx = (label - 1) as usize;
    if parent[idx] != label {
        // flatten recursion
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

/// Data for a component when using multi-threshold deblending.
/// Multi-threshold deblend needs the actual pixel data for its tree-building algorithm.
struct MultiThresholdComponentData {
    x_min: usize,
    x_max: usize,
    y_min: usize,
    y_max: usize,
    pixels: Vec<Pixel>,
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
    labels: &Buffer2<u32>,
    num_labels: usize,
    deblend_config: &DeblendConfig,
    max_area: usize,
) -> Vec<StarCandidate> {
    if num_labels == 0 {
        return Vec::new();
    }

    if deblend_config.multi_threshold {
        // Multi-threshold deblending needs pixel data for tree-building
        extract_candidates_multi_threshold(pixels, labels, num_labels, deblend_config, max_area)
    } else {
        // Local-maxima deblending uses allocation-free ComponentData
        extract_candidates_local_maxima(pixels, labels, num_labels, deblend_config, max_area)
    }
}

/// Extract candidates using allocation-free local-maxima deblending.
fn extract_candidates_local_maxima(
    pixels: &Buffer2<f32>,
    labels: &Buffer2<u32>,
    num_labels: usize,
    deblend_config: &DeblendConfig,
    max_area: usize,
) -> Vec<StarCandidate> {
    use rayon::prelude::*;
    let width = pixels.width();

    // Collect component bounding boxes and areas in single pass (no pixel allocation)
    let mut component_data: Vec<ComponentData> = Vec::with_capacity(num_labels);
    component_data.resize_with(num_labels, || ComponentData {
        x_min: usize::MAX,
        x_max: 0,
        y_min: usize::MAX,
        y_max: 0,
        label: 0,
        area: 0,
    });

    for (idx, &label) in labels.pixels().iter().enumerate() {
        if label == 0 {
            continue;
        }
        let data = &mut component_data[(label - 1) as usize];
        // Skip area counting for oversized components
        if data.area > max_area {
            continue;
        }
        let x = idx % width;
        let y = idx / width;
        data.x_min = data.x_min.min(x);
        data.x_max = data.x_max.max(x);
        data.y_min = data.y_min.min(y);
        data.y_max = data.y_max.max(y);
        data.label = label;
        data.area += 1;
    }

    // Process each component in parallel, deblending into multiple candidates
    // Skip components that are too large - they can't be stars and would be
    // expensive to process (e.g., deblending a million-pixel component)
    component_data
        .into_par_iter()
        .filter(|data| data.area > 0 && data.area <= max_area)
        .flat_map(|data| {
            let deblended = deblend_local_maxima(&data, pixels, labels, deblend_config);

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
        })
        .collect()
}

/// Extract candidates using multi-threshold deblending (requires pixel allocation).
fn extract_candidates_multi_threshold(
    pixels: &Buffer2<f32>,
    labels: &Buffer2<u32>,
    num_labels: usize,
    deblend_config: &DeblendConfig,
    max_area: usize,
) -> Vec<StarCandidate> {
    use rayon::prelude::*;
    let width = pixels.width();

    // Multi-threshold needs actual pixel data for its tree-building algorithm
    let mut component_data: Vec<MultiThresholdComponentData> = Vec::with_capacity(num_labels);
    component_data.resize_with(num_labels, || MultiThresholdComponentData {
        x_min: usize::MAX,
        x_max: 0,
        y_min: usize::MAX,
        y_max: 0,
        pixels: Vec::with_capacity(50),
    });

    for (idx, &label) in labels.pixels().iter().enumerate() {
        if label == 0 {
            continue;
        }
        let data = &mut component_data[(label - 1) as usize];
        // Skip pixel collection for oversized components
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

    // Process each component in parallel
    component_data
        .into_par_iter()
        .filter(|data| !data.pixels.is_empty() && data.pixels.len() <= max_area)
        .flat_map(|data| {
            let mt_config = MultiThresholdDeblendConfig {
                n_thresholds: deblend_config.n_thresholds,
                min_contrast: deblend_config.min_contrast,
                min_separation: deblend_config.min_separation,
            };

            // Estimate detection threshold from the minimum pixel value in component
            let detection_threshold = data.pixels.iter().map(|p| p.value).fold(f32::MAX, f32::min);

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
        })
        .collect()
}
