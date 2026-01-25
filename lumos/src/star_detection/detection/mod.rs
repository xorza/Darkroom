//! Star candidate detection using thresholding and connected components.

#[cfg(test)]
mod tests;

use super::StarDetectionConfig;
use super::background::BackgroundMap;

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
            sigma_threshold: config.detection_sigma,
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
/// * `width` - Image width
/// * `height` - Image height
/// * `background` - Background map from `estimate_background`
/// * `config` - Detection configuration
pub fn detect_stars(
    pixels: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<StarCandidate> {
    let detection_config = DetectionConfig::from(config);

    // Create binary mask of above-threshold pixels
    let mask = create_threshold_mask(pixels, background, detection_config.sigma_threshold);

    // Dilate mask to connect nearby pixels that may be separated due to
    // Bayer pattern artifacts or noise. Use radius 1 (3x3 structuring element)
    // to minimize merging of close stars while still connecting fragmented detections.
    let mask = dilate_mask(&mask, width, height, 1);

    // Find connected components
    let (labels, num_labels) = connected_components(&mask, width, height);

    // Extract candidate properties with deblending
    let deblend_config = DeblendConfig::from(config);
    let mut candidates =
        extract_candidates(pixels, &labels, num_labels, width, height, &deblend_config);

    // Filter candidates
    candidates.retain(|c| {
        // Size filter
        c.area >= detection_config.min_area
            && c.area <= detection_config.max_area
            // Edge filter
            && c.x_min >= detection_config.edge_margin
            && c.y_min >= detection_config.edge_margin
            && c.x_max < width - detection_config.edge_margin
            && c.y_max < height - detection_config.edge_margin
    });

    candidates
}

/// Dilate a binary mask by the given radius (morphological dilation).
///
/// This connects nearby pixels that might be separated due to variable threshold.
pub fn dilate_mask(mask: &[bool], width: usize, height: usize, radius: usize) -> Vec<bool> {
    let mut dilated = vec![false; width * height];

    for y in 0..height {
        for x in 0..width {
            if mask[y * width + x] {
                // Set all pixels within radius
                let y_min = y.saturating_sub(radius);
                let y_max = (y + radius).min(height - 1);
                let x_min = x.saturating_sub(radius);
                let x_max = (x + radius).min(width - 1);

                for dy in y_min..=y_max {
                    for dx in x_min..=x_max {
                        dilated[dy * width + dx] = true;
                    }
                }
            }
        }
    }

    dilated
}

/// Create binary mask of pixels above threshold.
pub(crate) fn create_threshold_mask(
    pixels: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
) -> Vec<bool> {
    pixels
        .iter()
        .zip(background.background.iter())
        .zip(background.noise.iter())
        .map(|((&px, &bg), &noise)| {
            let threshold = bg + sigma_threshold * noise.max(1e-6);
            px > threshold
        })
        .collect()
}

/// Connected component labeling using union-find.
pub(crate) fn connected_components(
    mask: &[bool],
    width: usize,
    height: usize,
) -> (Vec<u32>, usize) {
    let mut labels = vec![0u32; width * height];
    let mut parent: Vec<u32> = Vec::new();
    let mut next_label = 1u32;

    // First pass: assign provisional labels
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if !mask[idx] {
                continue;
            }

            // Use fixed-size array instead of Vec to avoid allocation in hot loop
            let mut neighbors = [0u32; 2];
            let mut neighbor_count = 0;

            // Check left neighbor
            if x > 0 && mask[idx - 1] {
                neighbors[neighbor_count] = labels[idx - 1];
                neighbor_count += 1;
            }

            // Check top neighbor
            if y > 0 && mask[idx - width] {
                neighbors[neighbor_count] = labels[idx - width];
                neighbor_count += 1;
            }

            if neighbor_count == 0 {
                // New label
                labels[idx] = next_label;
                parent.push(next_label); // parent[label-1] = label (self-reference)
                next_label += 1;
            } else {
                // Use minimum neighbor label
                let min_label = neighbors[..neighbor_count].iter().copied().min().unwrap();
                labels[idx] = min_label;

                // Union all neighbor labels
                for &label in &neighbors[..neighbor_count] {
                    union(&mut parent, min_label, label);
                }
            }
        }
    }

    // Second pass: flatten labels using path compression
    let mut label_map = vec![0u32; parent.len() + 1];
    let mut num_labels = 0u32;

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if labels[idx] == 0 {
                continue;
            }

            let root = find(&mut parent, labels[idx]);
            if label_map[root as usize] == 0 {
                num_labels += 1;
                label_map[root as usize] = num_labels;
            }
            labels[idx] = label_map[root as usize];
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
/// * `width` - Image width
/// * `height` - Image height
/// * `background` - Background map (for noise estimates)
/// * `config` - Detection configuration
pub fn detect_stars_filtered(
    pixels: &[f32],
    filtered: &[f32],
    width: usize,
    height: usize,
    background: &BackgroundMap,
    config: &StarDetectionConfig,
) -> Vec<StarCandidate> {
    let detection_config = DetectionConfig::from(config);

    // Create binary mask from the filtered image
    // The threshold is based on noise in the filtered image
    let mask =
        create_threshold_mask_filtered(filtered, background, detection_config.sigma_threshold);

    // Dilate mask with radius 1 to connect fragmented detections while
    // minimizing merging of close stars. Matched filtering already provides
    // good connectivity, so minimal dilation is needed.
    let mask = dilate_mask(&mask, width, height, 1);

    // Find connected components
    let (labels, num_labels) = connected_components(&mask, width, height);

    // Extract candidate properties using ORIGINAL pixels for peak values, with deblending
    let deblend_config = DeblendConfig::from(config);
    let mut candidates =
        extract_candidates(pixels, &labels, num_labels, width, height, &deblend_config);

    // Filter candidates
    candidates.retain(|c| {
        // Size filter
        c.area >= detection_config.min_area
            && c.area <= detection_config.max_area
            // Edge filter
            && c.x_min >= detection_config.edge_margin
            && c.y_min >= detection_config.edge_margin
            && c.x_max < width - detection_config.edge_margin
            && c.y_max < height - detection_config.edge_margin
    });

    candidates
}

/// Create binary mask from a filtered (convolved) image.
///
/// For matched-filtered images, the noise is reduced by a factor related to
/// the kernel size. We use a simpler threshold based on the filtered values
/// being positive and above a noise-scaled threshold.
fn create_threshold_mask_filtered(
    filtered: &[f32],
    background: &BackgroundMap,
    sigma_threshold: f32,
) -> Vec<bool> {
    // For a matched filter with Gaussian kernel of width σ_k, the noise
    // in the convolved image is reduced by approximately 1/sqrt(2πσ_k²)
    // We use the background noise estimate scaled appropriately.
    filtered
        .iter()
        .zip(background.noise.iter())
        .map(|(&px, &noise)| {
            // The filtered image is already background-subtracted
            // Threshold is sigma_threshold times the (reduced) noise
            let threshold = sigma_threshold * noise.max(1e-6);
            px > threshold
        })
        .collect()
}

/// Configuration for deblending algorithm.
#[derive(Debug, Clone, Copy)]
pub struct DeblendConfig {
    /// Minimum separation between peaks for deblending (in pixels).
    pub min_separation: usize,
    /// Minimum peak prominence as fraction of primary peak for deblending.
    pub min_prominence: f32,
}

impl Default for DeblendConfig {
    fn default() -> Self {
        Self {
            min_separation: 3,
            min_prominence: 0.3,
        }
    }
}

impl From<&StarDetectionConfig> for DeblendConfig {
    fn from(config: &StarDetectionConfig) -> Self {
        Self {
            min_separation: config.deblend_min_separation,
            min_prominence: config.deblend_min_prominence,
        }
    }
}

/// Extract candidate properties from labeled image with simple deblending.
///
/// For components with multiple local maxima (star pairs), this function
/// splits them into separate candidates based on peak positions.
pub(crate) fn extract_candidates(
    pixels: &[f32],
    labels: &[u32],
    num_labels: usize,
    width: usize,
    height: usize,
    deblend_config: &DeblendConfig,
) -> Vec<StarCandidate> {
    if num_labels == 0 {
        return Vec::new();
    }

    // Collect component data in first pass
    let mut component_data: Vec<ComponentData> = (0..num_labels)
        .map(|_| ComponentData {
            x_min: usize::MAX,
            x_max: 0,
            y_min: usize::MAX,
            y_max: 0,
            pixels: Vec::new(),
        })
        .collect();

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let label = labels[idx];
            if label == 0 {
                continue;
            }

            let i = (label - 1) as usize;
            let data = &mut component_data[i];

            data.x_min = data.x_min.min(x);
            data.x_max = data.x_max.max(x);
            data.y_min = data.y_min.min(y);
            data.y_max = data.y_max.max(y);
            data.pixels.push((x, y, pixels[idx]));
        }
    }

    // Process each component, potentially deblending into multiple candidates
    let mut candidates = Vec::with_capacity(num_labels);

    for data in component_data {
        if data.pixels.is_empty() {
            continue;
        }

        // Find local maxima for deblending
        let peaks = find_local_maxima(&data, pixels, width, deblend_config);

        if peaks.len() <= 1 {
            // Single peak - create one candidate
            let (peak_x, peak_y, peak_value) = if peaks.is_empty() {
                // Fallback: use global maximum
                data.pixels
                    .iter()
                    .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
                    .map(|&(x, y, v)| (x, y, v))
                    .unwrap()
            } else {
                peaks[0]
            };

            candidates.push(StarCandidate {
                x_min: data.x_min,
                x_max: data.x_max,
                y_min: data.y_min,
                y_max: data.y_max,
                peak_x,
                peak_y,
                peak_value,
                area: data.pixels.len(),
            });
        } else {
            // Multiple peaks - deblend by assigning pixels to nearest peak
            let deblended = deblend_component(&data, &peaks);
            candidates.extend(deblended);
        }
    }

    candidates
}

/// Temporary data for a connected component.
struct ComponentData {
    x_min: usize,
    x_max: usize,
    y_min: usize,
    y_max: usize,
    pixels: Vec<(usize, usize, f32)>, // (x, y, value)
}

/// Find local maxima within a component for deblending.
///
/// A pixel is a local maximum if it's greater than all 8 neighbors.
/// Only returns peaks that are sufficiently separated and prominent.
fn find_local_maxima(
    data: &ComponentData,
    pixels: &[f32],
    width: usize,
    config: &DeblendConfig,
) -> Vec<(usize, usize, f32)> {
    let mut peaks: Vec<(usize, usize, f32)> = Vec::new();

    // Find global maximum first
    let global_max = data
        .pixels
        .iter()
        .map(|&(_, _, v)| v)
        .fold(f32::MIN, f32::max);

    let min_peak_value = global_max * config.min_prominence;

    // Check each pixel for local maximum
    for &(x, y, value) in &data.pixels {
        if value < min_peak_value {
            continue;
        }

        // Check if this is a local maximum (greater than all 8 neighbors)
        let mut is_maximum = true;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }

                let nx = x as i32 + dx;
                let ny = y as i32 + dy;

                if nx >= 0 && ny >= 0 {
                    let nx = nx as usize;
                    let ny = ny as usize;
                    let neighbor_idx = ny * width + nx;

                    if neighbor_idx < pixels.len() && pixels[neighbor_idx] >= value {
                        is_maximum = false;
                        break;
                    }
                }
            }
            if !is_maximum {
                break;
            }
        }

        if is_maximum {
            // Check separation from existing peaks
            let min_sep = config.min_separation;
            let well_separated = peaks.iter().all(|&(px, py, _)| {
                let dx = (x as i32 - px as i32).unsigned_abs() as usize;
                let dy = (y as i32 - py as i32).unsigned_abs() as usize;
                dx >= min_sep || dy >= min_sep
            });

            if well_separated {
                peaks.push((x, y, value));
            } else {
                // Keep the brighter peak
                for peak in &mut peaks {
                    let dx = (x as i32 - peak.0 as i32).unsigned_abs() as usize;
                    let dy = (y as i32 - peak.1 as i32).unsigned_abs() as usize;
                    if dx < min_sep && dy < min_sep && value > peak.2 {
                        *peak = (x, y, value);
                        break;
                    }
                }
            }
        }
    }

    // Sort by brightness (brightest first)
    peaks.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    peaks
}

/// Deblend a component into multiple candidates based on peak positions.
///
/// Each pixel is assigned to the nearest peak, creating separate candidates.
fn deblend_component(data: &ComponentData, peaks: &[(usize, usize, f32)]) -> Vec<StarCandidate> {
    // Initialize per-peak data
    let mut peak_data: Vec<(usize, usize, usize, usize, usize)> = peaks
        .iter()
        .map(|_| (usize::MAX, 0, usize::MAX, 0, 0)) // (x_min, x_max, y_min, y_max, area)
        .collect();

    // Assign each pixel to nearest peak
    for &(x, y, _) in &data.pixels {
        let mut min_dist_sq = usize::MAX;
        let mut nearest_peak = 0;

        for (i, &(px, py, _)) in peaks.iter().enumerate() {
            let dx = (x as i32 - px as i32).unsigned_abs() as usize;
            let dy = (y as i32 - py as i32).unsigned_abs() as usize;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq < min_dist_sq {
                min_dist_sq = dist_sq;
                nearest_peak = i;
            }
        }

        let pd = &mut peak_data[nearest_peak];
        pd.0 = pd.0.min(x);
        pd.1 = pd.1.max(x);
        pd.2 = pd.2.min(y);
        pd.3 = pd.3.max(y);
        pd.4 += 1;
    }

    // Build candidates
    peaks
        .iter()
        .zip(peak_data.iter())
        .filter(|(_, pd)| pd.4 > 0) // Only include peaks with assigned pixels
        .map(
            |(&(peak_x, peak_y, peak_value), &(x_min, x_max, y_min, y_max, area))| StarCandidate {
                x_min,
                x_max,
                y_min,
                y_max,
                peak_x,
                peak_y,
                peak_value,
                area,
            },
        )
        .collect()
}
