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

/// Detect star candidates in an image.
///
/// Uses connected component labeling to find regions above the detection threshold.
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
    // Bayer pattern artifacts (alternating row sensitivities) or background
    // estimation variance across a single star.
    // Use radius 2 (5x5 structuring element) to bridge Bayer gaps.
    let mask = dilate_mask(&mask, width, height, 2);

    // Find connected components
    let (labels, num_labels) = connected_components(&mask, width, height);

    // Extract candidate properties
    let mut candidates = extract_candidates(pixels, &labels, num_labels, width, height);

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
fn create_threshold_mask(
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
fn connected_components(mask: &[bool], width: usize, height: usize) -> (Vec<u32>, usize) {
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

/// Extract candidate properties from labeled image.
fn extract_candidates(
    pixels: &[f32],
    labels: &[u32],
    num_labels: usize,
    width: usize,
    height: usize,
) -> Vec<StarCandidate> {
    if num_labels == 0 {
        return Vec::new();
    }

    // Initialize candidate data
    let mut x_min = vec![usize::MAX; num_labels];
    let mut x_max = vec![0usize; num_labels];
    let mut y_min = vec![usize::MAX; num_labels];
    let mut y_max = vec![0usize; num_labels];
    let mut peak_x = vec![0usize; num_labels];
    let mut peak_y = vec![0usize; num_labels];
    let mut peak_value = vec![f32::MIN; num_labels];
    let mut area = vec![0usize; num_labels];

    // Single pass to collect all properties
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let label = labels[idx];
            if label == 0 {
                continue;
            }

            let i = (label - 1) as usize;
            area[i] += 1;

            x_min[i] = x_min[i].min(x);
            x_max[i] = x_max[i].max(x);
            y_min[i] = y_min[i].min(y);
            y_max[i] = y_max[i].max(y);

            let value = pixels[idx];
            if value > peak_value[i] {
                peak_value[i] = value;
                peak_x[i] = x;
                peak_y[i] = y;
            }
        }
    }

    // Build candidates
    (0..num_labels)
        .map(|i| StarCandidate {
            x_min: x_min[i],
            x_max: x_max[i],
            y_min: y_min[i],
            y_max: y_max[i],
            peak_x: peak_x[i],
            peak_y: peak_y[i],
            peak_value: peak_value[i],
            area: area[i],
        })
        .collect()
}
