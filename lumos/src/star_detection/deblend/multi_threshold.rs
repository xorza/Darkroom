//! Multi-threshold deblending algorithm for separating blended sources.
//!
//! This implements a SExtractor-style deblending approach that:
//! 1. Uses multiple thresholds between detection level and peak
//! 2. Builds a tree structure tracking how regions split at higher thresholds
//! 3. Applies a contrast criterion to decide if branches are separate objects
//!
//! Reference: Bertin & Arnouts (1996), A&AS 117, 393

use std::collections::HashMap;

use super::{ComponentData, Pixel};
use crate::common::Buffer2;
use crate::math::{Aabb, Vec2us};
use crate::star_detection::detection::LabelMap;

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
    /// Pixel positions belonging to this node at its threshold level.
    pixels: Vec<Vec2us>,
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
    /// Peak position.
    pub peak: Vec2us,
    /// Peak pixel value.
    pub peak_value: f32,
    /// Total flux.
    pub flux: f32,
    /// Pixels belonging to this object.
    pub pixels: Vec<Pixel>,
    /// Bounding box.
    pub bbox: Aabb,
}

/// Multi-threshold deblending of a connected component.
///
/// Uses `ComponentData` to read pixels on-demand from the image buffer,
/// avoiding allocation of pixel vectors per component.
///
/// # Arguments
/// * `data` - Component metadata (bounding box, label, area)
/// * `pixels` - Full image pixel buffer
/// * `labels` - Label map for the image
/// * `detection_threshold` - The threshold used for initial detection
/// * `config` - Deblending configuration
///
/// # Returns
/// Vector of deblended objects, or single object if no deblending occurs.
pub fn deblend_component(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    detection_threshold: f32,
    config: &MultiThresholdDeblendConfig,
) -> Vec<DeblendedObject> {
    debug_assert_eq!(
        (pixels.width(), pixels.height()),
        (labels.width(), labels.height()),
        "pixels and labels must have same dimensions"
    );

    if data.area == 0 {
        return Vec::new();
    }

    // If min_contrast >= 1.0, deblending is effectively disabled
    // (no branch can have 100% of flux)
    if config.min_contrast >= 1.0 {
        return vec![create_single_object(data, pixels, labels)];
    }

    // Find peak value in the component
    let peak = data.find_peak(pixels, labels);
    let peak_value = peak.value;

    // If peak barely above threshold, no deblending possible
    if peak_value <= detection_threshold * 1.01 {
        return vec![create_single_object(data, pixels, labels)];
    }

    // Create threshold levels (exponentially spaced for better resolution at faint levels)
    let thresholds = create_threshold_levels(detection_threshold, peak_value, config.n_thresholds);

    // Build deblending tree by analyzing connectivity at each threshold
    let width = pixels.width();
    let tree = build_deblend_tree(
        data,
        pixels,
        labels,
        width,
        &thresholds,
        config.min_separation,
    );

    // If tree has only one leaf (no branching), return single object
    if tree.is_empty() {
        return vec![create_single_object(data, pixels, labels)];
    }

    // Find leaf nodes (objects) using contrast criterion
    let leaves = find_significant_branches(&tree, config.min_contrast);

    if leaves.len() <= 1 {
        return vec![create_single_object(data, pixels, labels)];
    }

    // Assign all pixels to nearest leaf peak
    assign_pixels_to_objects(data, pixels, labels, &tree, &leaves)
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
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    width: usize,
    thresholds: &[f32],
    min_separation: usize,
) -> Vec<DeblendNode> {
    if thresholds.is_empty() || data.area == 0 {
        return Vec::new();
    }

    let mut tree: Vec<DeblendNode> = Vec::new();

    // Collect component pixels once for the tree-building algorithm
    // (multi-threshold needs repeated access at different threshold levels)
    let component_pixels: Vec<Pixel> = data.iter_pixels(pixels, labels).collect();

    // Create a map from (x, y) to index for quick lookup
    let pixel_map: HashMap<Vec2us, usize> = component_pixels
        .iter()
        .enumerate()
        .map(|(i, p)| (p.pos, i))
        .collect();

    // Track which node each pixel belongs to at current level
    let mut pixel_to_node: HashMap<Vec2us, usize> = HashMap::with_capacity(component_pixels.len());

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
                    pixel_to_node.insert(p.pos, node_idx);
                }

                tree.push(DeblendNode {
                    pixels: region.iter().map(|p| p.pos).collect(),
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
                    if let Some(&parent_idx) = pixel_to_node.get(&p.pos) {
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
                        .filter(|&pos| {
                            component_pixels
                                .iter()
                                .find(|p| p.pos == *pos)
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
                                .filter_map(|&pos| {
                                    component_pixels.iter().find(|p| p.pos == *pos).copied()
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
                                    let dx = (child_peak.pos.x as i32 - sibling.peak.pos.x as i32)
                                        .unsigned_abs()
                                        as usize;
                                    let dy = (child_peak.pos.y as i32 - sibling.peak.pos.y as i32)
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
                                    pixel_to_node.insert(p.pos, child_idx);
                                }

                                tree.push(DeblendNode {
                                    pixels: child_region.iter().map(|p| p.pos).collect(),
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
            pos: Vec2us::new(0, 0),
            value: 0.0,
        })
}

/// Find connected regions within a set of pixels using 8-connectivity.
fn find_connected_regions(
    pixels: &[Pixel],
    _pixel_map: &HashMap<Vec2us, usize>,
    _width: usize,
) -> Vec<Vec<Pixel>> {
    if pixels.is_empty() {
        return Vec::new();
    }

    // Create a set of pixel positions for quick lookup
    let pixel_set: HashMap<Vec2us, f32> = pixels.iter().map(|p| (p.pos, p.value)).collect();

    let mut visited: HashMap<Vec2us, bool> = HashMap::new();
    let mut regions = Vec::new();

    for p in pixels {
        if visited.get(&p.pos).is_some_and(|&b| b) {
            continue;
        }

        // BFS to find connected component
        let mut region = Vec::new();
        let mut queue = vec![*p];
        visited.insert(p.pos, true);

        while let Some(current) = queue.pop() {
            region.push(current);

            // Check 8 neighbors
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }

                    let npos = Vec2us::new(
                        (current.pos.x as i32 + dx) as usize,
                        (current.pos.y as i32 + dy) as usize,
                    );

                    if visited.get(&npos).is_some_and(|&b| b) {
                        continue;
                    }

                    if let Some(&nv) = pixel_set.get(&npos) {
                        visited.insert(npos, true);
                        queue.push(Pixel {
                            pos: npos,
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
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    tree: &[DeblendNode],
    leaf_indices: &[usize],
) -> Vec<DeblendedObject> {
    if leaf_indices.is_empty() {
        return vec![create_single_object(data, pixels, labels)];
    }

    // Get peak positions for each leaf
    let peaks: Vec<Pixel> = leaf_indices.iter().map(|&i| tree[i].peak).collect();

    // Initialize objects
    let mut objects: Vec<DeblendedObject> = peaks
        .iter()
        .map(|p| DeblendedObject {
            peak: p.pos,
            peak_value: p.value,
            flux: 0.0,
            pixels: Vec::new(),
            bbox: Aabb::empty(),
        })
        .collect();

    // Assign each pixel to nearest peak
    for p in data.iter_pixels(pixels, labels) {
        let mut min_dist_sq = usize::MAX;
        let mut nearest = 0;

        for (i, peak) in peaks.iter().enumerate() {
            let dx = (p.pos.x as i32 - peak.pos.x as i32).unsigned_abs() as usize;
            let dy = (p.pos.y as i32 - peak.pos.y as i32).unsigned_abs() as usize;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq < min_dist_sq {
                min_dist_sq = dist_sq;
                nearest = i;
            }
        }

        let obj = &mut objects[nearest];
        obj.pixels.push(p);
        obj.flux += p.value;
        obj.bbox.include(p.pos);
    }

    // Filter out objects with no pixels
    objects.retain(|o| !o.pixels.is_empty());

    objects
}

/// Create a single object from all pixels (no deblending).
fn create_single_object(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
) -> DeblendedObject {
    let peak = data.find_peak(pixels, labels);

    let mut flux = 0.0f32;
    let mut pixel_list = Vec::with_capacity(data.area);

    for p in data.iter_pixels(pixels, labels) {
        flux += p.value;
        pixel_list.push(p);
    }

    DeblendedObject {
        peak: peak.pos,
        peak_value: peak.value,
        flux,
        pixels: pixel_list,
        bbox: data.bbox,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test image with Gaussian stars and return pixels, labels, and component data.
    fn make_test_component(
        width: usize,
        height: usize,
        stars: &[(usize, usize, f32, f32)], // (cx, cy, amplitude, sigma)
    ) -> (Buffer2<f32>, LabelMap, ComponentData) {
        let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
        let mut labels = Buffer2::new_filled(width, height, 0u32);

        let mut bbox = Aabb::empty();
        let mut area = 0;

        for (cx, cy, amplitude, sigma) in stars {
            let radius = (sigma * 4.0).ceil() as i32;

            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let x = (*cx as i32 + dx) as usize;
                    let y = (*cy as i32 + dy) as usize;

                    if x < width && y < height {
                        let r2 = (dx * dx + dy * dy) as f32;
                        let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
                        if value > 0.001 {
                            pixels[(x, y)] += value;
                            if labels[(x, y)] == 0 {
                                labels[(x, y)] = 1;
                                bbox.include(Vec2us::new(x, y));
                                area += 1;
                            }
                        }
                    }
                }
            }
        }

        let label_map = LabelMap::from_raw(labels, 1);
        let component = ComponentData {
            bbox,
            label: 1,
            area,
        };

        (pixels, label_map, component)
    }

    #[test]
    fn test_single_star_no_deblending() {
        let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);
        let config = MultiThresholdDeblendConfig::default();

        let result = deblend_component(&data, &pixels, &labels, 0.01, &config);

        assert_eq!(result.len(), 1, "Single star should produce one object");
        assert!((result[0].peak.x as i32 - 50).abs() <= 1);
        assert!((result[0].peak.y as i32 - 50).abs() <= 1);
    }

    #[test]
    fn test_two_separated_stars_deblend() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = MultiThresholdDeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
        };

        let result = deblend_component(&data, &pixels, &labels, 0.01, &config);

        // Should deblend into 2 separate objects
        assert_eq!(
            result.len(),
            2,
            "Two separated stars should produce two objects"
        );

        // Check peak positions
        let mut peaks: Vec<_> = result.iter().map(|o| (o.peak.x, o.peak.y)).collect();
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
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.001, 2.5)]);

        let config = MultiThresholdDeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.01, // 1% contrast required
            min_separation: 3,
        };

        let result = deblend_component(&data, &pixels, &labels, 0.01, &config);

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
        // Stars only 4 pixels apart (with min_separation=5)
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(48, 50, 1.0, 2.0), (52, 50, 0.9, 2.0)]);

        let config = MultiThresholdDeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 5, // Larger than 4 pixel separation
        };

        let result = deblend_component(&data, &pixels, &labels, 0.01, &config);

        // Should NOT deblend - peaks too close
        assert_eq!(result.len(), 1, "Close peaks should not be deblended");
    }

    #[test]
    fn test_empty_component() {
        let pixels = Buffer2::new_filled(10, 10, 0.0f32);
        let labels_buf = Buffer2::new_filled(10, 10, 0u32);
        let labels = LabelMap::from_raw(labels_buf, 0);
        let data = ComponentData {
            bbox: Aabb::default(),
            label: 1,
            area: 0,
        };
        let config = MultiThresholdDeblendConfig::default();

        let result = deblend_component(&data, &pixels, &labels, 0.01, &config);

        assert!(result.is_empty());
    }

    #[test]
    fn test_deblend_disabled_with_high_contrast() {
        // Setting min_contrast to 1.0 should disable deblending
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = MultiThresholdDeblendConfig {
            n_thresholds: 32,
            min_contrast: 1.0, // Require 100% contrast = disabled
            min_separation: 3,
        };

        let result = deblend_component(&data, &pixels, &labels, 0.01, &config);

        assert_eq!(
            result.len(),
            1,
            "High contrast setting should disable deblending"
        );
    }
}
