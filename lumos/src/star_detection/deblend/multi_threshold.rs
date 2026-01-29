//! Multi-threshold deblending algorithm for separating blended sources.
//!
//! This implements a SExtractor-style deblending approach that:
//! 1. Uses multiple thresholds between detection level and peak
//! 2. Builds a tree structure tracking how regions split at higher thresholds
//! 3. Applies a contrast criterion to decide if branches are separate objects
//!
//! Reference: Bertin & Arnouts (1996), A&AS 117, 393

use std::collections::HashMap;

use super::local_maxima::Pixel;

/// Configuration for multi-threshold deblending.
#[derive(Debug, Clone, Copy)]
pub struct MultiThresholdDeblendConfig {
    /// Number of deblending sub-thresholds between detection and peak.
    /// SExtractor default is 32. Higher values give finer deblending
    /// but use more memory and CPU. Typical range: 16-64.
    pub n_thresholds: usize,
    /// Minimum contrast parameter (0.0-1.0).
    /// A branch is considered a separate object only if its flux is
    /// at least min_contrast × total_flux of the parent.
    /// SExtractor default: 0.005. Lower values deblend more aggressively.
    /// Set to 1.0 to disable deblending entirely.
    pub min_contrast: f32,
    /// Minimum separation between peaks in pixels.
    /// Peaks closer than this are merged even if they pass contrast criterion.
    pub min_separation: usize,
}

impl Default for MultiThresholdDeblendConfig {
    fn default() -> Self {
        Self {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
        }
    }
}

/// A node in the deblending tree.
#[derive(Debug, Clone)]
struct DeblendNode {
    /// Pixel positions (x, y) belonging to this node at its threshold level.
    pixels: Vec<(usize, usize)>,
    /// Peak position and value.
    peak: Pixel,
    /// Total flux in this branch.
    flux: f32,
    /// Child nodes (branches that split from this node at higher threshold).
    children: Vec<usize>,
    /// Threshold level where this node exists (used for debugging).
    #[allow(dead_code)]
    threshold_level: usize,
}

/// Result of deblending a single connected component.
#[derive(Debug)]
pub struct DeblendedObject {
    /// X coordinate of peak.
    pub peak_x: usize,
    /// Y coordinate of peak.
    pub peak_y: usize,
    /// Peak pixel value.
    pub peak_value: f32,
    /// Total flux.
    pub flux: f32,
    /// Pixels belonging to this object.
    pub pixels: Vec<Pixel>,
    /// Bounding box (x_min, x_max, y_min, y_max).
    pub bbox: (usize, usize, usize, usize),
}

/// Multi-threshold deblending of a connected component.
///
/// Takes a list of pixels belonging to a single connected component and
/// attempts to split it into multiple objects using the multi-threshold approach.
///
/// # Arguments
/// * `pixels` - Pixel data for the entire image
/// * `component_pixels` - List of pixels in this component
/// * `width` - Image width
/// * `detection_threshold` - The threshold used for initial detection
/// * `config` - Deblending configuration
///
/// # Returns
/// Vector of deblended objects, or single object if no deblending occurs.
pub fn deblend_component(
    component_pixels: &[Pixel],
    width: usize,
    detection_threshold: f32,
    config: &MultiThresholdDeblendConfig,
) -> Vec<DeblendedObject> {
    if component_pixels.is_empty() {
        return Vec::new();
    }

    // If min_contrast >= 1.0, deblending is effectively disabled
    // (no branch can have 100% of flux)
    if config.min_contrast >= 1.0 {
        return vec![create_single_object(component_pixels)];
    }

    // Find peak value in the component
    let peak_value = component_pixels
        .iter()
        .map(|p| p.value)
        .fold(f32::MIN, f32::max);

    // If peak barely above threshold, no deblending possible
    if peak_value <= detection_threshold * 1.01 {
        return vec![create_single_object(component_pixels)];
    }

    // Create threshold levels (exponentially spaced for better resolution at faint levels)
    let thresholds = create_threshold_levels(detection_threshold, peak_value, config.n_thresholds);

    // Build deblending tree by analyzing connectivity at each threshold
    let tree = build_deblend_tree(component_pixels, width, &thresholds, config.min_separation);

    // If tree has only one leaf (no branching), return single object
    if tree.is_empty() {
        return vec![create_single_object(component_pixels)];
    }

    // Find leaf nodes (objects) using contrast criterion
    let leaves = find_significant_branches(&tree, config.min_contrast);

    if leaves.len() <= 1 {
        return vec![create_single_object(component_pixels)];
    }

    // Assign all pixels to nearest leaf peak
    assign_pixels_to_objects(component_pixels, &tree, &leaves)
}

/// Create exponentially spaced threshold levels.
fn create_threshold_levels(low: f32, high: f32, n: usize) -> Vec<f32> {
    if n == 0 {
        return vec![low];
    }

    // Use exponential spacing: threshold[i] = low * (high/low)^(i/n)
    // This gives finer resolution near the detection threshold where
    // blended objects are more likely to separate.
    let ratio = (high / low).max(1.0);

    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            low * ratio.powf(t)
        })
        .collect()
}

/// Build the deblending tree by tracking connectivity at each threshold level.
fn build_deblend_tree(
    component_pixels: &[Pixel],
    width: usize,
    thresholds: &[f32],
    min_separation: usize,
) -> Vec<DeblendNode> {
    if thresholds.is_empty() || component_pixels.is_empty() {
        return Vec::new();
    }

    let mut tree: Vec<DeblendNode> = Vec::new();

    // Create a map from (x, y) to index for quick lookup
    let pixel_map: HashMap<(usize, usize), usize> = component_pixels
        .iter()
        .enumerate()
        .map(|(i, p)| ((p.x, p.y), i))
        .collect();

    // Track which node each pixel belongs to at current level
    let mut pixel_to_node: HashMap<(usize, usize), usize> =
        HashMap::with_capacity(component_pixels.len());

    // Process each threshold level from low to high
    for (level, &threshold) in thresholds.iter().enumerate() {
        // Find pixels above this threshold
        let above_threshold: Vec<Pixel> = component_pixels
            .iter()
            .filter(|p| p.value >= threshold)
            .copied()
            .collect();

        if above_threshold.is_empty() {
            continue;
        }

        // Find connected components at this threshold level
        let regions = find_connected_regions(&above_threshold, &pixel_map, width);

        if level == 0 {
            // First level: create root node(s)
            for region in regions {
                let node_idx = tree.len();
                let peak = find_region_peak(&region);
                let flux = region.iter().map(|p| p.value).sum();

                for p in &region {
                    pixel_to_node.insert((p.x, p.y), node_idx);
                }

                tree.push(DeblendNode {
                    pixels: region.iter().map(|p| (p.x, p.y)).collect(),
                    peak,
                    flux,
                    children: Vec::new(),
                    threshold_level: level,
                });
            }
        } else {
            // Higher levels: check for splits
            for region in regions {
                // Find which parent node(s) this region comes from
                let mut parent_nodes: HashMap<usize, Vec<Pixel>> = HashMap::new();

                for &p in &region {
                    if let Some(&parent_idx) = pixel_to_node.get(&(p.x, p.y)) {
                        parent_nodes.entry(parent_idx).or_default().push(p);
                    }
                }

                if parent_nodes.len() > 1 {
                    // This region spans multiple parent nodes - shouldn't happen
                    // but handle gracefully by keeping pixels with their parents
                    continue;
                }

                if let Some((&parent_idx, _)) = parent_nodes.iter().next() {
                    // Check if this is the same region or a split occurred
                    let parent = &tree[parent_idx];
                    let parent_pixels_above: Vec<_> = parent
                        .pixels
                        .iter()
                        .filter(|&&(x, y)| {
                            component_pixels
                                .iter()
                                .find(|p| p.x == x && p.y == y)
                                .is_some_and(|p| p.value >= threshold)
                        })
                        .collect();

                    // If number of pixels above threshold is less than parent's pixels,
                    // check if multiple distinct regions formed
                    if region.len() < parent_pixels_above.len() {
                        // Check if other regions formed from same parent
                        let other_regions: Vec<Vec<Pixel>> = find_connected_regions(
                            &parent_pixels_above
                                .iter()
                                .filter_map(|&&(x, y)| {
                                    component_pixels
                                        .iter()
                                        .find(|p| p.x == x && p.y == y)
                                        .copied()
                                })
                                .collect::<Vec<_>>(),
                            &pixel_map,
                            width,
                        );

                        if other_regions.len() > 1 {
                            // Split occurred! Create child nodes
                            let mut child_indices = Vec::new();

                            for child_region in other_regions {
                                let child_peak = find_region_peak(&child_region);

                                // Check minimum separation from existing children
                                let too_close = child_indices.iter().any(|&idx: &usize| {
                                    let sibling = &tree[idx];
                                    let dx = (child_peak.x as i32 - sibling.peak.x as i32)
                                        .unsigned_abs()
                                        as usize;
                                    let dy = (child_peak.y as i32 - sibling.peak.y as i32)
                                        .unsigned_abs()
                                        as usize;
                                    dx < min_separation && dy < min_separation
                                });

                                if too_close {
                                    continue;
                                }

                                let child_idx = tree.len();
                                let child_flux = child_region.iter().map(|p| p.value).sum();

                                for p in &child_region {
                                    pixel_to_node.insert((p.x, p.y), child_idx);
                                }

                                tree.push(DeblendNode {
                                    pixels: child_region.iter().map(|p| (p.x, p.y)).collect(),
                                    peak: child_peak,
                                    flux: child_flux,
                                    children: Vec::new(),
                                    threshold_level: level,
                                });

                                child_indices.push(child_idx);
                            }

                            // Update parent's children
                            tree[parent_idx].children = child_indices;
                        }
                    }
                }
            }
        }
    }

    tree
}

/// Find the peak (brightest pixel) in a region.
fn find_region_peak(region: &[Pixel]) -> Pixel {
    region
        .iter()
        .max_by(|a, b| {
            a.value
                .partial_cmp(&b.value)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .unwrap_or(Pixel {
            x: 0,
            y: 0,
            value: 0.0,
        })
}

/// Find connected regions within a set of pixels using 8-connectivity.
fn find_connected_regions(
    pixels: &[Pixel],
    _pixel_map: &HashMap<(usize, usize), usize>,
    _width: usize,
) -> Vec<Vec<Pixel>> {
    if pixels.is_empty() {
        return Vec::new();
    }

    // Create a set of pixel positions for quick lookup
    let pixel_set: HashMap<(usize, usize), f32> =
        pixels.iter().map(|p| ((p.x, p.y), p.value)).collect();

    let mut visited: HashMap<(usize, usize), bool> = HashMap::new();
    let mut regions = Vec::new();

    for p in pixels {
        if visited.get(&(p.x, p.y)).is_some_and(|&b| b) {
            continue;
        }

        // BFS to find connected component
        let mut region = Vec::new();
        let mut queue = vec![*p];
        visited.insert((p.x, p.y), true);

        while let Some(current) = queue.pop() {
            region.push(current);

            // Check 8 neighbors
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }

                    let nx = (current.x as i32 + dx) as usize;
                    let ny = (current.y as i32 + dy) as usize;

                    if visited.get(&(nx, ny)).is_some_and(|&b| b) {
                        continue;
                    }

                    if let Some(&nv) = pixel_set.get(&(nx, ny)) {
                        visited.insert((nx, ny), true);
                        queue.push(Pixel {
                            x: nx,
                            y: ny,
                            value: nv,
                        });
                    }
                }
            }
        }

        regions.push(region);
    }

    regions
}

/// Find significant branches (leaves) that pass the contrast criterion.
///
/// Returns indices of nodes that should be treated as separate objects.
fn find_significant_branches(tree: &[DeblendNode], min_contrast: f32) -> Vec<usize> {
    if tree.is_empty() {
        return Vec::new();
    }

    // Find root nodes (nodes that are not children of any other node)
    let all_children: Vec<usize> = tree
        .iter()
        .flat_map(|n| n.children.iter().copied())
        .collect();
    let roots: Vec<usize> = (0..tree.len())
        .filter(|&i| !all_children.contains(&i))
        .collect();

    let mut leaves = Vec::new();

    // Process each root
    for &root_idx in &roots {
        let root_flux = tree[root_idx].flux;
        collect_significant_leaves(tree, root_idx, root_flux, min_contrast, &mut leaves);
    }

    // If no leaves found (all contrast criteria failed), return roots
    if leaves.is_empty() {
        return roots;
    }

    leaves
}

/// Recursively collect leaf nodes that pass contrast criterion.
fn collect_significant_leaves(
    tree: &[DeblendNode],
    node_idx: usize,
    root_flux: f32,
    min_contrast: f32,
    leaves: &mut Vec<usize>,
) {
    let node = &tree[node_idx];

    if node.children.is_empty() {
        // This is a leaf - add it
        leaves.push(node_idx);
        return;
    }

    // Check if children pass contrast criterion
    let children_pass_contrast: Vec<bool> = node
        .children
        .iter()
        .map(|&child_idx| {
            let child_flux = tree[child_idx].flux;
            child_flux >= min_contrast * root_flux
        })
        .collect();

    let num_pass = children_pass_contrast.iter().filter(|&&b| b).count();

    if num_pass <= 1 {
        // Not enough children pass contrast - treat this node as a leaf
        leaves.push(node_idx);
    } else {
        // Multiple children pass - recurse into each
        for (i, &child_idx) in node.children.iter().enumerate() {
            if children_pass_contrast[i] {
                collect_significant_leaves(tree, child_idx, root_flux, min_contrast, leaves);
            }
        }
    }
}

/// Assign pixels to their nearest object (based on peak positions).
fn assign_pixels_to_objects(
    component_pixels: &[Pixel],
    tree: &[DeblendNode],
    leaf_indices: &[usize],
) -> Vec<DeblendedObject> {
    if leaf_indices.is_empty() {
        return vec![create_single_object(component_pixels)];
    }

    // Get peak positions for each leaf
    let peaks: Vec<Pixel> = leaf_indices.iter().map(|&i| tree[i].peak).collect();

    // Initialize objects
    let mut objects: Vec<DeblendedObject> = peaks
        .iter()
        .map(|p| DeblendedObject {
            peak_x: p.x,
            peak_y: p.y,
            peak_value: p.value,
            flux: 0.0,
            pixels: Vec::new(),
            bbox: (usize::MAX, 0, usize::MAX, 0),
        })
        .collect();

    // Assign each pixel to nearest peak
    for &p in component_pixels {
        let mut min_dist_sq = usize::MAX;
        let mut nearest = 0;

        for (i, peak) in peaks.iter().enumerate() {
            let dx = (p.x as i32 - peak.x as i32).unsigned_abs() as usize;
            let dy = (p.y as i32 - peak.y as i32).unsigned_abs() as usize;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq < min_dist_sq {
                min_dist_sq = dist_sq;
                nearest = i;
            }
        }

        let obj = &mut objects[nearest];
        obj.pixels.push(p);
        obj.flux += p.value;
        obj.bbox.0 = obj.bbox.0.min(p.x);
        obj.bbox.1 = obj.bbox.1.max(p.x);
        obj.bbox.2 = obj.bbox.2.min(p.y);
        obj.bbox.3 = obj.bbox.3.max(p.y);
    }

    // Filter out objects with no pixels
    objects.retain(|o| !o.pixels.is_empty());

    objects
}

/// Create a single object from all pixels (no deblending).
fn create_single_object(pixels: &[Pixel]) -> DeblendedObject {
    let peak = pixels
        .iter()
        .max_by(|a, b| {
            a.value
                .partial_cmp(&b.value)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .unwrap_or(Pixel {
            x: 0,
            y: 0,
            value: 0.0,
        });

    let flux: f32 = pixels.iter().map(|p| p.value).sum();

    let x_min = pixels.iter().map(|p| p.x).min().unwrap_or(0);
    let x_max = pixels.iter().map(|p| p.x).max().unwrap_or(0);
    let y_min = pixels.iter().map(|p| p.y).min().unwrap_or(0);
    let y_max = pixels.iter().map(|p| p.y).max().unwrap_or(0);

    DeblendedObject {
        peak_x: peak.x,
        peak_y: peak.y,
        peak_value: peak.value,
        flux,
        pixels: pixels.to_vec(),
        bbox: (x_min, x_max, y_min, y_max),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gaussian_star(cx: usize, cy: usize, amplitude: f32, sigma: f32) -> Vec<Pixel> {
        let mut pixels = Vec::new();
        let radius = (sigma * 4.0).ceil() as i32;

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                let r2 = (dx * dx + dy * dy) as f32;
                let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
                if value > 0.001 {
                    pixels.push(Pixel { x, y, value });
                }
            }
        }

        pixels
    }

    #[test]
    fn test_single_star_no_deblending() {
        // Single isolated star should not be deblended
        let star = make_gaussian_star(50, 50, 1.0, 3.0);
        let config = MultiThresholdDeblendConfig::default();

        // Create full image
        let width = 100;
        let height = 100;
        let mut pixels = vec![0.0f32; width * height];
        for p in &star {
            if p.x < width && p.y < height {
                pixels[p.y * width + p.x] = p.value;
            }
        }

        let result = deblend_component(&star, width, 0.01, &config);

        assert_eq!(result.len(), 1, "Single star should produce one object");
        assert!((result[0].peak_x as i32 - 50).abs() <= 1);
        assert!((result[0].peak_y as i32 - 50).abs() <= 1);
    }

    #[test]
    fn test_two_separated_stars_deblend() {
        // Two well-separated stars should be deblended
        let width = 100;
        let height = 100;

        let star1 = make_gaussian_star(30, 50, 1.0, 2.5);
        let star2 = make_gaussian_star(70, 50, 0.8, 2.5);

        // Combine stars into one "component" (simulating blended detection)
        let mut component: Vec<Pixel> = Vec::new();
        let mut pixels = vec![0.0f32; width * height];

        for p in star1.iter().chain(star2.iter()) {
            if p.x < width && p.y < height {
                pixels[p.y * width + p.x] += p.value;
                component.push(Pixel {
                    x: p.x,
                    y: p.y,
                    value: pixels[p.y * width + p.x],
                });
            }
        }

        // Deduplicate component pixels
        let mut seen: HashMap<(usize, usize), f32> = HashMap::new();
        for p in component {
            seen.insert((p.x, p.y), p.value);
        }
        let component: Vec<_> = seen
            .into_iter()
            .map(|((x, y), value)| Pixel { x, y, value })
            .collect();

        let config = MultiThresholdDeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
        };

        let result = deblend_component(&component, width, 0.01, &config);

        // Should deblend into 2 separate objects
        assert_eq!(
            result.len(),
            2,
            "Two separated stars should produce two objects"
        );

        // Check peak positions
        let mut peaks: Vec<_> = result.iter().map(|o| (o.peak_x, o.peak_y)).collect();
        peaks.sort_by_key(|&(x, _)| x);

        assert!(
            (peaks[0].0 as i32 - 30).abs() <= 2,
            "First peak should be near x=30"
        );
        assert!(
            (peaks[1].0 as i32 - 70).abs() <= 2,
            "Second peak should be near x=70"
        );
    }

    #[test]
    fn test_faint_secondary_below_contrast() {
        // Very faint secondary should NOT be deblended (below contrast threshold)
        let width = 100;
        let height = 100;

        let star1 = make_gaussian_star(30, 50, 1.0, 2.5);
        let star2 = make_gaussian_star(70, 50, 0.001, 2.5); // Very faint

        let mut component: Vec<Pixel> = Vec::new();
        let mut pixels = vec![0.0f32; width * height];

        for p in star1.iter().chain(star2.iter()) {
            if p.x < width && p.y < height {
                pixels[p.y * width + p.x] += p.value;
                component.push(Pixel {
                    x: p.x,
                    y: p.y,
                    value: pixels[p.y * width + p.x],
                });
            }
        }

        let mut seen: HashMap<(usize, usize), f32> = HashMap::new();
        for p in component {
            seen.insert((p.x, p.y), p.value);
        }
        let component: Vec<_> = seen
            .into_iter()
            .map(|((x, y), value)| Pixel { x, y, value })
            .collect();

        let config = MultiThresholdDeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.01, // 1% contrast required
            min_separation: 3,
        };

        let result = deblend_component(&component, width, 0.01, &config);

        // Faint star should be absorbed - contrast too low
        assert_eq!(
            result.len(),
            1,
            "Faint secondary should not cause deblending"
        );
    }

    #[test]
    fn test_threshold_levels_exponential() {
        let thresholds = create_threshold_levels(0.1, 1.0, 10);

        assert_eq!(thresholds.len(), 11); // 0..=10
        assert!((thresholds[0] - 0.1).abs() < 1e-6);
        assert!((thresholds[10] - 1.0).abs() < 1e-6);

        // Check exponential spacing (ratio should be constant)
        for i in 1..thresholds.len() {
            let ratio = thresholds[i] / thresholds[i - 1];
            let expected_ratio = (1.0f32 / 0.1).powf(1.0 / 10.0);
            assert!(
                (ratio - expected_ratio).abs() < 0.01,
                "Threshold spacing should be exponential"
            );
        }
    }

    #[test]
    fn test_close_peaks_merge() {
        // Two peaks closer than min_separation should be merged
        let width = 100;
        let height = 100;

        // Stars only 4 pixels apart (with min_separation=5)
        let star1 = make_gaussian_star(48, 50, 1.0, 2.0);
        let star2 = make_gaussian_star(52, 50, 0.9, 2.0);

        let mut component: Vec<Pixel> = Vec::new();
        let mut pixels = vec![0.0f32; width * height];

        for p in star1.iter().chain(star2.iter()) {
            if p.x < width && p.y < height {
                pixels[p.y * width + p.x] += p.value;
                component.push(Pixel {
                    x: p.x,
                    y: p.y,
                    value: pixels[p.y * width + p.x],
                });
            }
        }

        let mut seen: HashMap<(usize, usize), f32> = HashMap::new();
        for p in component {
            seen.insert((p.x, p.y), p.value);
        }
        let component: Vec<_> = seen
            .into_iter()
            .map(|((x, y), value)| Pixel { x, y, value })
            .collect();

        let config = MultiThresholdDeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 5, // Larger than 4 pixel separation
        };

        let result = deblend_component(&component, width, 0.01, &config);

        // Should NOT deblend - peaks too close
        assert_eq!(result.len(), 1, "Close peaks should not be deblended");
    }

    #[test]
    fn test_empty_component() {
        let pixels = vec![0.0f32; 100];
        let component: Vec<Pixel> = Vec::new();
        let config = MultiThresholdDeblendConfig::default();

        let result = deblend_component(&component, 10, 0.01, &config);

        assert!(result.is_empty());
    }

    #[test]
    fn test_deblend_disabled_with_high_contrast() {
        // Setting min_contrast to 1.0 should disable deblending
        let width = 100;
        let height = 100;

        let star1 = make_gaussian_star(30, 50, 1.0, 2.5);
        let star2 = make_gaussian_star(70, 50, 0.8, 2.5);

        let mut component: Vec<Pixel> = Vec::new();
        let mut pixels = vec![0.0f32; width * height];

        for p in star1.iter().chain(star2.iter()) {
            if p.x < width && p.y < height {
                pixels[p.y * width + p.x] += p.value;
                component.push(Pixel {
                    x: p.x,
                    y: p.y,
                    value: pixels[p.y * width + p.x],
                });
            }
        }

        let mut seen: HashMap<(usize, usize), f32> = HashMap::new();
        for p in component {
            seen.insert((p.x, p.y), p.value);
        }
        let component: Vec<_> = seen
            .into_iter()
            .map(|((x, y), value)| Pixel { x, y, value })
            .collect();

        let config = MultiThresholdDeblendConfig {
            n_thresholds: 32,
            min_contrast: 1.0, // Require 100% contrast = disabled
            min_separation: 3,
        };

        let result = deblend_component(&component, width, 0.01, &config);

        assert_eq!(
            result.len(),
            1,
            "High contrast setting should disable deblending"
        );
    }
}
