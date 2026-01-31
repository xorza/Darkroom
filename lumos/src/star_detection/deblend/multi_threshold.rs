//! Multi-threshold deblending algorithm for separating blended sources.
//!
//! This implements a SExtractor-style deblending approach that:
//! 1. Uses multiple thresholds between detection level and peak
//! 2. Builds a tree structure tracking how regions split at higher thresholds
//! 3. Applies a contrast criterion to decide if branches are separate objects
//!
//! Reference: Bertin & Arnouts (1996), A&AS 117, 393

use arrayvec::ArrayVec;
use hashbrown::{HashMap, HashSet};
use smallvec::SmallVec;

use super::{ComponentData, DeblendedCandidate, MAX_PEAKS, Pixel};
use crate::common::Buffer2;
use crate::math::{Aabb, Vec2us};
use crate::star_detection::config::DeblendConfig;
use crate::star_detection::detection::LabelMap;

// ============================================================================
// Constants
// ============================================================================

/// Maximum children per node (same as MAX_PEAKS since each child becomes a candidate).
const MAX_CHILDREN: usize = MAX_PEAKS;

// ============================================================================
// Types
// ============================================================================

/// A node in the deblending tree.
#[derive(Debug, Clone)]
struct DeblendNode {
    /// Peak position and value.
    peak: Pixel,
    /// Total flux in this branch.
    flux: f32,
    /// Child nodes (branches that split from this node at higher threshold).
    /// Uses SmallVec to avoid heap allocation for common case (0-2 children).
    children: SmallVec<[usize; MAX_CHILDREN]>,
}

/// Reusable buffers for tree building to avoid repeated allocations.
struct TreeBuildBuffers {
    /// Pixels above current threshold.
    above_threshold: Vec<Pixel>,
    /// Pixels belonging to a parent that are above threshold.
    parent_pixels_above: Vec<Pixel>,
    /// BFS queue for connected component finding.
    bfs_queue: Vec<Pixel>,
    /// Temporary storage for regions.
    regions: Vec<Vec<Pixel>>,
}

impl TreeBuildBuffers {
    fn new(capacity: usize) -> Self {
        Self {
            above_threshold: Vec::with_capacity(capacity),
            parent_pixels_above: Vec::with_capacity(capacity),
            bfs_queue: Vec::with_capacity(capacity.min(1024)),
            regions: Vec::with_capacity(MAX_PEAKS),
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Multi-threshold deblending of a connected component.
///
/// Uses `ComponentData` to read pixels on-demand from the image buffer,
/// avoiding allocation of pixel vectors per component.
///
/// # Arguments
/// * `data` - Component metadata (bounding box, label, area)
/// * `pixels` - Full image pixel buffer
/// * `labels` - Label map for the image
/// * `config` - Deblending configuration
///
/// # Returns
/// Vector of deblended objects, or single object if no deblending occurs.
pub fn deblend_multi_threshold(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    config: &DeblendConfig,
) -> SmallVec<[DeblendedCandidate; MAX_PEAKS]> {
    debug_assert_eq!(
        (pixels.width(), pixels.height()),
        (labels.width(), labels.height()),
        "pixels and labels must have same dimensions"
    );

    // Early exit for empty components
    if data.area == 0 {
        return SmallVec::new();
    }

    // If min_contrast >= 1.0, deblending is effectively disabled
    if config.min_contrast >= 1.0 {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    // Find peak value and detection threshold
    let peak = data.find_peak(pixels, labels);
    let peak_value = peak.value;
    let detection_threshold = data
        .iter_pixels(pixels, labels)
        .map(|p| p.value)
        .fold(f32::MAX, f32::min);

    // If peak barely above threshold, no deblending possible
    if peak_value <= detection_threshold * 1.01 {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    // Build deblending tree by analyzing connectivity at each threshold
    let tree = build_deblend_tree(
        data,
        pixels,
        labels,
        detection_threshold,
        peak_value,
        config.n_thresholds,
        config.min_separation,
    );

    // If tree has only one leaf (no branching), return single object
    if tree.is_empty() {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    // Find leaf nodes (objects) using contrast criterion
    let leaves = find_significant_branches(&tree, config.min_contrast);

    if leaves.len() <= 1 {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    // Assign all pixels to nearest leaf peak
    assign_pixels_to_objects(data, pixels, labels, &tree, &leaves)
}

// ============================================================================
// Tree Building
// ============================================================================

/// Build the deblending tree by tracking connectivity at each threshold level.
///
/// Uses exponentially spaced thresholds for better resolution at faint levels.
fn build_deblend_tree(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    low: f32,
    high: f32,
    n_thresholds: usize,
    min_separation: usize,
) -> Vec<DeblendNode> {
    if data.area == 0 {
        return Vec::new();
    }

    // Use exponential spacing: threshold[i] = low * (high/low)^(i/n)
    let ratio = (high / low).max(1.0);

    let mut tree: Vec<DeblendNode> = Vec::new();

    // Collect component pixels once (multi-threshold needs repeated access)
    let component_pixels: Vec<Pixel> = data.iter_pixels(pixels, labels).collect();

    // Track which node each pixel belongs to at current level
    let mut pixel_to_node: HashMap<Vec2us, usize> = HashMap::with_capacity(component_pixels.len());

    // Reusable buffers to avoid allocations per threshold level
    let mut buffers = TreeBuildBuffers::new(component_pixels.len());

    // Visited set for connected component finding (reused across calls)
    let mut visited: HashSet<Vec2us> = HashSet::with_capacity(component_pixels.len());

    // Process each threshold level from low to high
    for level in 0..=n_thresholds {
        let t = level as f32 / n_thresholds.max(1) as f32;
        let threshold = low * ratio.powf(t);

        // Filter pixels above threshold (reuse buffer)
        buffers.above_threshold.clear();
        buffers
            .above_threshold
            .extend(component_pixels.iter().filter(|p| p.value >= threshold));

        if buffers.above_threshold.is_empty() {
            continue;
        }

        // Find connected regions (reuses visited set and queue)
        find_connected_regions_reuse(
            &buffers.above_threshold,
            &mut buffers.regions,
            &mut visited,
            &mut buffers.bfs_queue,
        );

        if level == 0 {
            process_root_level(&mut tree, &mut pixel_to_node, &buffers.regions);
        } else {
            process_higher_level(
                &mut tree,
                &mut pixel_to_node,
                &component_pixels,
                &buffers.regions,
                threshold,
                min_separation,
                &mut buffers.parent_pixels_above,
                &mut visited,
                &mut buffers.bfs_queue,
            );
        }
    }

    tree
}

/// Process the first threshold level - create root nodes.
fn process_root_level(
    tree: &mut Vec<DeblendNode>,
    pixel_to_node: &mut HashMap<Vec2us, usize>,
    regions: &[Vec<Pixel>],
) {
    for region in regions {
        let node_idx = tree.len();
        let peak = find_region_peak(region);
        let flux = region.iter().map(|p| p.value).sum();

        for p in region {
            pixel_to_node.insert(p.pos, node_idx);
        }

        tree.push(DeblendNode {
            peak,
            flux,
            children: SmallVec::new(),
        });
    }
}

/// Process higher threshold levels - check for region splits.
#[allow(clippy::too_many_arguments)]
fn process_higher_level(
    tree: &mut Vec<DeblendNode>,
    pixel_to_node: &mut HashMap<Vec2us, usize>,
    component_pixels: &[Pixel],
    regions: &[Vec<Pixel>],
    threshold: f32,
    min_separation: usize,
    parent_pixels_buf: &mut Vec<Pixel>,
    visited: &mut HashSet<Vec2us>,
    bfs_queue: &mut Vec<Pixel>,
) {
    for region in regions {
        // Find the single parent node for this region
        // (all pixels in a connected region should come from same parent)
        let parent_idx = match find_single_parent(region, pixel_to_node) {
            Some(idx) => idx,
            None => continue, // Skip if no parent or multiple parents
        };

        // Count pixels belonging to this parent that are above threshold
        // (avoid allocation by counting first)
        let parent_above_count = component_pixels
            .iter()
            .filter(|p| pixel_to_node.get(&p.pos) == Some(&parent_idx) && p.value >= threshold)
            .count();

        // Check if multiple distinct regions formed from same parent
        if region.len() < parent_above_count {
            // Collect parent pixels above threshold (reuse buffer)
            parent_pixels_buf.clear();
            parent_pixels_buf.extend(
                component_pixels
                    .iter()
                    .filter(|p| {
                        pixel_to_node.get(&p.pos) == Some(&parent_idx) && p.value >= threshold
                    })
                    .copied(),
            );

            // Find child regions (reuse buffers)
            let mut child_regions: ArrayVec<Vec<Pixel>, MAX_CHILDREN> = ArrayVec::new();
            find_connected_regions_into(parent_pixels_buf, &mut child_regions, visited, bfs_queue);

            if child_regions.len() > 1 {
                create_child_nodes(
                    tree,
                    pixel_to_node,
                    parent_idx,
                    &child_regions,
                    min_separation,
                );
            }
        }
    }
}

/// Find the single parent node for a region, or None if multiple/no parents.
#[inline]
fn find_single_parent(region: &[Pixel], pixel_to_node: &HashMap<Vec2us, usize>) -> Option<usize> {
    let mut parent: Option<usize> = None;

    for p in region {
        if let Some(&idx) = pixel_to_node.get(&p.pos) {
            match parent {
                None => parent = Some(idx),
                Some(existing) if existing != idx => return None, // Multiple parents
                _ => {}
            }
        }
    }

    parent
}

/// Create child nodes when a split is detected.
fn create_child_nodes(
    tree: &mut Vec<DeblendNode>,
    pixel_to_node: &mut HashMap<Vec2us, usize>,
    parent_idx: usize,
    child_regions: &[Vec<Pixel>],
    min_separation: usize,
) {
    let mut child_indices: ArrayVec<usize, MAX_CHILDREN> = ArrayVec::new();

    for child_region in child_regions {
        if child_indices.is_full() {
            break;
        }

        let child_peak = find_region_peak(child_region);

        // Check minimum separation from existing children
        let too_close = child_indices.iter().any(|&idx| {
            let sibling = &tree[idx];
            let dx = (child_peak.pos.x as i32 - sibling.peak.pos.x as i32).unsigned_abs() as usize;
            let dy = (child_peak.pos.y as i32 - sibling.peak.pos.y as i32).unsigned_abs() as usize;
            dx < min_separation && dy < min_separation
        });

        if too_close {
            continue;
        }

        let child_idx = tree.len();
        let child_flux = child_region.iter().map(|p| p.value).sum();

        for p in child_region {
            pixel_to_node.insert(p.pos, child_idx);
        }

        tree.push(DeblendNode {
            peak: child_peak,
            flux: child_flux,
            children: SmallVec::new(),
        });

        child_indices.push(child_idx);
    }

    // Update parent's children
    tree[parent_idx].children = child_indices.into_iter().collect();
}

// ============================================================================
// Tree Analysis
// ============================================================================

/// Find significant branches (leaves) that pass the contrast criterion.
///
/// Returns indices of nodes that should be treated as separate objects.
fn find_significant_branches(
    tree: &[DeblendNode],
    min_contrast: f32,
) -> SmallVec<[usize; MAX_PEAKS]> {
    if tree.is_empty() {
        return SmallVec::new();
    }

    // Find root nodes (nodes that are not children of any other node)
    // Use a fixed-size set for small trees
    let mut is_child = [false; 64]; // Supports trees up to 64 nodes without allocation
    let use_array = tree.len() <= 64;

    let mut child_set: HashSet<usize> = HashSet::new();

    for node in tree {
        for &child_idx in &node.children {
            if use_array {
                is_child[child_idx] = true;
            } else {
                child_set.insert(child_idx);
            }
        }
    }

    let mut leaves: SmallVec<[usize; MAX_PEAKS]> = SmallVec::new();

    // Process each root
    for i in 0..tree.len() {
        let is_root = if use_array {
            !is_child.get(i).copied().unwrap_or(false)
        } else {
            !child_set.contains(&i)
        };

        if is_root {
            collect_significant_leaves(tree, i, min_contrast, &mut leaves);
        }
    }

    // If no leaves found (all contrast criteria failed), return roots
    if leaves.is_empty() {
        for i in 0..tree.len() {
            let is_root = if use_array {
                !is_child.get(i).copied().unwrap_or(false)
            } else {
                !child_set.contains(&i)
            };
            if is_root {
                leaves.push(i);
            }
        }
    }

    leaves
}

/// Recursively collect leaf nodes that pass contrast criterion.
///
/// Per SExtractor algorithm: a branch is considered a separate object if its
/// flux is at least `min_contrast` fraction of the **parent's** flux (not root).
fn collect_significant_leaves(
    tree: &[DeblendNode],
    node_idx: usize,
    min_contrast: f32,
    leaves: &mut SmallVec<[usize; MAX_PEAKS]>,
) {
    let node = &tree[node_idx];

    if node.children.is_empty() {
        if leaves.len() < MAX_PEAKS {
            leaves.push(node_idx);
        }
        return;
    }

    // Check if children pass contrast criterion relative to THIS node's flux
    let parent_flux = node.flux;
    let min_flux = min_contrast * parent_flux;

    let mut num_pass = 0;
    for &child_idx in &node.children {
        if tree[child_idx].flux >= min_flux {
            num_pass += 1;
        }
    }

    if num_pass <= 1 {
        // Not enough children pass contrast - treat this node as a leaf
        if leaves.len() < MAX_PEAKS {
            leaves.push(node_idx);
        }
    } else {
        // Multiple children pass - recurse into each
        for &child_idx in &node.children {
            if tree[child_idx].flux >= min_flux {
                collect_significant_leaves(tree, child_idx, min_contrast, leaves);
            }
        }
    }
}

// ============================================================================
// Pixel Assignment
// ============================================================================

/// Assign pixels to their nearest object (based on peak positions).
fn assign_pixels_to_objects(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    tree: &[DeblendNode],
    leaf_indices: &[usize],
) -> SmallVec<[DeblendedCandidate; MAX_PEAKS]> {
    if leaf_indices.is_empty() {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    // Get peak positions for each leaf (stack allocated)
    let peaks: ArrayVec<Pixel, MAX_PEAKS> = leaf_indices
        .iter()
        .take(MAX_PEAKS)
        .map(|&i| tree[i].peak)
        .collect();

    // Initialize objects (stack allocated)
    let mut objects: SmallVec<[DeblendedCandidate; MAX_PEAKS]> = peaks
        .iter()
        .map(|p| DeblendedCandidate {
            bbox: Aabb::empty(),
            peak: p.pos,
            peak_value: p.value,
            area: 0,
        })
        .collect();

    // Assign each pixel to nearest peak
    for p in data.iter_pixels(pixels, labels) {
        let nearest = find_nearest_peak_index(p.pos, &peaks);
        let obj = &mut objects[nearest];
        obj.area += 1;
        obj.bbox.include(p.pos);
    }

    // Filter out objects with no pixels
    objects.retain(|o| o.area > 0);

    objects
}

/// Find the index of the nearest peak to a position.
#[inline]
fn find_nearest_peak_index(pos: Vec2us, peaks: &[Pixel]) -> usize {
    let mut min_dist_sq = usize::MAX;
    let mut nearest = 0;

    for (i, peak) in peaks.iter().enumerate() {
        let dx = (pos.x as i32 - peak.pos.x as i32).unsigned_abs() as usize;
        let dy = (pos.y as i32 - peak.pos.y as i32).unsigned_abs() as usize;
        let dist_sq = dx * dx + dy * dy;

        if dist_sq < min_dist_sq {
            min_dist_sq = dist_sq;
            nearest = i;
        }
    }

    nearest
}

// ============================================================================
// Connected Component Finding
// ============================================================================

/// Find connected regions, reusing provided buffers.
fn find_connected_regions_reuse(
    pixels: &[Pixel],
    regions: &mut Vec<Vec<Pixel>>,
    visited: &mut HashSet<Vec2us>,
    queue: &mut Vec<Pixel>,
) {
    regions.clear();
    visited.clear();

    if pixels.is_empty() {
        return;
    }

    // Build pixel lookup set
    let pixel_set: HashMap<Vec2us, f32> = pixels.iter().map(|p| (p.pos, p.value)).collect();

    for p in pixels {
        if visited.contains(&p.pos) {
            continue;
        }

        // BFS to find connected component
        let mut region = Vec::new();
        queue.clear();
        queue.push(*p);
        visited.insert(p.pos);

        while let Some(current) = queue.pop() {
            region.push(current);
            visit_neighbors(current.pos, &pixel_set, visited, queue);
        }

        regions.push(region);
    }
}

/// Find connected regions into an ArrayVec (limited capacity).
fn find_connected_regions_into<const N: usize>(
    pixels: &[Pixel],
    regions: &mut ArrayVec<Vec<Pixel>, N>,
    visited: &mut HashSet<Vec2us>,
    queue: &mut Vec<Pixel>,
) {
    regions.clear();
    visited.clear();

    if pixels.is_empty() {
        return;
    }

    let pixel_set: HashMap<Vec2us, f32> = pixels.iter().map(|p| (p.pos, p.value)).collect();

    for p in pixels {
        if regions.is_full() {
            break;
        }

        if visited.contains(&p.pos) {
            continue;
        }

        let mut region = Vec::new();
        queue.clear();
        queue.push(*p);
        visited.insert(p.pos);

        while let Some(current) = queue.pop() {
            region.push(current);
            visit_neighbors(current.pos, &pixel_set, visited, queue);
        }

        regions.push(region);
    }
}

/// Visit 8-connected neighbors and add unvisited ones to the queue.
#[inline]
fn visit_neighbors(
    pos: Vec2us,
    pixel_set: &HashMap<Vec2us, f32>,
    visited: &mut HashSet<Vec2us>,
    queue: &mut Vec<Pixel>,
) {
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }

            let nx = pos.x as i32 + dx;
            let ny = pos.y as i32 + dy;

            // Skip negative coordinates
            if nx < 0 || ny < 0 {
                continue;
            }

            let npos = Vec2us::new(nx as usize, ny as usize);

            if visited.contains(&npos) {
                continue;
            }

            if let Some(&nv) = pixel_set.get(&npos) {
                visited.insert(npos);
                queue.push(Pixel {
                    pos: npos,
                    value: nv,
                });
            }
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Find the peak (brightest pixel) in a region.
#[inline]
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

/// Create a single object from all pixels (no deblending).
#[inline]
fn create_single_object(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
) -> DeblendedCandidate {
    let peak = data.find_peak(pixels, labels);

    DeblendedCandidate {
        bbox: data.bbox,
        peak: peak.pos,
        peak_value: peak.value,
        area: data.area,
    }
}

// ============================================================================
// Tests
// ============================================================================

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
        let config = DeblendConfig::default();

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(result.len(), 1, "Single star should produce one object");
        assert!((result[0].peak.x as i32 - 50).abs() <= 1);
        assert!((result[0].peak.y as i32 - 50).abs() <= 1);
    }

    #[test]
    fn test_two_separated_stars_deblend() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(
            result.len(),
            2,
            "Two separated stars should produce two objects"
        );

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
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.001, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.01,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(
            result.len(),
            1,
            "Faint secondary should not cause deblending"
        );
    }

    #[test]
    fn test_threshold_levels_exponential() {
        let low = 0.1f32;
        let high = 1.0f32;
        let n = 10usize;
        let ratio = (high / low).max(1.0);
        let thresholds: Vec<f32> = (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                low * ratio.powf(t)
            })
            .collect();

        assert_eq!(thresholds.len(), 11);
        assert!((thresholds[0] - 0.1).abs() < 1e-6);
        assert!((thresholds[10] - 1.0).abs() < 1e-6);

        for i in 1..thresholds.len() {
            let step_ratio = thresholds[i] / thresholds[i - 1];
            let expected_ratio = (1.0f32 / 0.1).powf(1.0 / 10.0);
            assert!(
                (step_ratio - expected_ratio).abs() < 0.01,
                "Threshold spacing should be exponential"
            );
        }
    }

    #[test]
    fn test_close_peaks_merge() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(48, 50, 1.0, 2.0), (52, 50, 0.9, 2.0)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 5,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

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
        let config = DeblendConfig::default();

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert!(result.is_empty());
    }

    #[test]
    fn test_deblend_disabled_with_high_contrast() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 1.0,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(
            result.len(),
            1,
            "High contrast setting should disable deblending"
        );
    }

    #[test]
    fn test_three_stars_deblend() {
        let (pixels, labels, data) = make_test_component(
            150,
            100,
            &[(30, 50, 1.0, 2.5), (75, 50, 0.9, 2.5), (120, 50, 0.8, 2.5)],
        );

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(
            result.len(),
            3,
            "Three separated stars should produce three objects"
        );

        let mut peaks: Vec<_> = result.iter().map(|o| o.peak.x).collect();
        peaks.sort();
        assert!((peaks[0] as i32 - 30).abs() <= 2);
        assert!((peaks[1] as i32 - 75).abs() <= 2);
        assert!((peaks[2] as i32 - 120).abs() <= 2);
    }

    #[test]
    fn test_hierarchical_deblend() {
        let (pixels, labels, data) = make_test_component(
            150,
            100,
            &[(30, 50, 1.0, 2.5), (100, 50, 0.8, 2.5), (115, 50, 0.7, 2.5)],
        );

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.1,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert!(
            result.len() >= 2,
            "Should find at least the isolated star and close pair"
        );
    }

    #[test]
    fn test_equal_brightness_stars() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 1.0, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(
            result.len(),
            2,
            "Equal brightness stars should both be found"
        );

        let area_diff = (result[0].area as i32 - result[1].area as i32).abs();
        let avg_area = (result[0].area + result[1].area) / 2;
        assert!(
            area_diff < (avg_area as i32 / 2),
            "Equal stars should have similar areas"
        );
    }

    #[test]
    fn test_contrast_at_boundary() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.1, 2.5)]);

        let config_pass = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.09,
            min_separation: 3,
            ..Default::default()
        };
        let result_pass = deblend_multi_threshold(&data, &pixels, &labels, &config_pass);

        let config_fail = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.15,
            min_separation: 3,
            ..Default::default()
        };
        let result_fail = deblend_multi_threshold(&data, &pixels, &labels, &config_fail);

        assert!(
            result_pass.len() >= result_fail.len(),
            "Lower contrast threshold should find more or equal objects"
        );
    }

    #[test]
    fn test_pixel_assignment_conservation() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        let total_area: usize = result.iter().map(|o| o.area).sum();
        assert_eq!(
            total_area, data.area,
            "Sum of deblended areas should equal original area"
        );
    }

    #[test]
    fn test_vertical_star_pair() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(50, 30, 1.0, 2.5), (50, 70, 0.8, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(result.len(), 2, "Vertically separated stars should deblend");

        let mut peaks: Vec<_> = result.iter().map(|o| o.peak.y).collect();
        peaks.sort();
        assert!((peaks[0] as i32 - 30).abs() <= 2);
        assert!((peaks[1] as i32 - 70).abs() <= 2);
    }

    #[test]
    fn test_diagonal_star_pair() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 30, 1.0, 2.5), (70, 70, 0.8, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(result.len(), 2, "Diagonally separated stars should deblend");
    }

    #[test]
    fn test_n_thresholds_effect() {
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(35, 50, 1.0, 2.5), (65, 50, 0.9, 2.5)]);

        let config_few = DeblendConfig {
            n_thresholds: 4,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let config_many = DeblendConfig {
            n_thresholds: 64,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result_few = deblend_multi_threshold(&data, &pixels, &labels, &config_few);
        let result_many = deblend_multi_threshold(&data, &pixels, &labels, &config_many);

        assert!(
            result_many.len() >= result_few.len(),
            "More thresholds should find >= objects"
        );
    }

    #[test]
    fn test_single_pixel_component() {
        let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);
        let mut labels_buf = Buffer2::new_filled(10, 10, 0u32);

        pixels[(5, 5)] = 1.0;
        labels_buf[(5, 5)] = 1;

        let labels = LabelMap::from_raw(labels_buf, 1);
        let data = ComponentData {
            bbox: Aabb::new(Vec2us::new(5, 5), Vec2us::new(5, 5)),
            label: 1,
            area: 1,
        };

        let config = DeblendConfig::default();
        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(result.len(), 1, "Single pixel should produce one object");
        assert_eq!(result[0].area, 1);
    }

    #[test]
    fn test_flat_profile_no_deblend() {
        let mut pixels = Buffer2::new_filled(50, 50, 0.0f32);
        let mut labels_buf = Buffer2::new_filled(50, 50, 0u32);

        let mut bbox = Aabb::empty();
        let mut area = 0;

        for y in 10..40 {
            for x in 10..40 {
                let dx = x as i32 - 25;
                let dy = y as i32 - 25;
                if dx * dx + dy * dy < 150 {
                    pixels[(x, y)] = 1.0;
                    labels_buf[(x, y)] = 1;
                    bbox.include(Vec2us::new(x, y));
                    area += 1;
                }
            }
        }

        let labels = LabelMap::from_raw(labels_buf, 1);
        let data = ComponentData {
            bbox,
            label: 1,
            area,
        };

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        assert_eq!(
            result.len(),
            1,
            "Flat profile should not deblend into multiple objects"
        );
    }

    // ========================================================================
    // Additional tests for optimized code paths
    // ========================================================================

    #[test]
    fn test_many_stars_max_peaks_limit() {
        // Create more stars than MAX_PEAKS to test limiting behavior
        let stars: Vec<_> = (0..12)
            .map(|i| (15 + i * 12, 50usize, 1.0 - i as f32 * 0.05, 2.0f32))
            .collect();

        let (pixels, labels, data) = make_test_component(180, 100, &stars);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        // Should not exceed MAX_PEAKS
        assert!(
            result.len() <= MAX_PEAKS,
            "Should not exceed MAX_PEAKS ({}), got {}",
            MAX_PEAKS,
            result.len()
        );

        // Area should still be conserved
        let total_area: usize = result.iter().map(|o| o.area).sum();
        assert_eq!(total_area, data.area, "Area should be conserved");
    }

    #[test]
    fn test_large_tree_over_64_nodes() {
        // Create a scenario that might produce > 64 nodes to test HashSet fallback
        // Use many small stars that will create many tree nodes
        let mut stars = Vec::new();
        for row in 0..4 {
            for col in 0..4 {
                let x = 20 + col * 25;
                let y = 20 + row * 25;
                let amp = 1.0 - (row * 4 + col) as f32 * 0.03;
                stars.push((x, y, amp, 2.0f32));
            }
        }

        let (pixels, labels, data) = make_test_component(150, 150, &stars);

        let config = DeblendConfig {
            n_thresholds: 64, // More thresholds = more potential nodes
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        // Should find multiple objects
        assert!(result.len() >= 2, "Should find multiple objects");

        // Area conservation
        let total_area: usize = result.iter().map(|o| o.area).sum();
        assert_eq!(total_area, data.area, "Area should be conserved");
    }

    #[test]
    fn test_buffer_reuse_consistency() {
        // Run the same deblending multiple times to ensure buffer reuse doesn't cause issues
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result1 = deblend_multi_threshold(&data, &pixels, &labels, &config);
        let result2 = deblend_multi_threshold(&data, &pixels, &labels, &config);
        let result3 = deblend_multi_threshold(&data, &pixels, &labels, &config);

        // All runs should produce identical results
        assert_eq!(result1.len(), result2.len());
        assert_eq!(result2.len(), result3.len());

        for i in 0..result1.len() {
            assert_eq!(result1[i].peak, result2[i].peak);
            assert_eq!(result2[i].peak, result3[i].peak);
            assert_eq!(result1[i].area, result2[i].area);
            assert_eq!(result2[i].area, result3[i].area);
        }
    }

    #[test]
    fn test_connected_regions_complex_shape() {
        // Test with a dumbbell-shaped component (two blobs connected by thin bridge)
        // The two blobs have distinct peaks that should be deblended
        let (pixels, labels, data) = make_test_component(
            100,
            50,
            &[
                (20, 25, 1.0, 3.0), // Left blob
                (80, 25, 0.9, 3.0), // Right blob
            ],
        );

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        // Should find both peaks
        assert_eq!(result.len(), 2, "Should find both peaks");

        // Area conservation
        let total_area: usize = result.iter().map(|o| o.area).sum();
        assert_eq!(total_area, data.area, "Area should be conserved");

        // Peaks should be at expected positions
        let mut peak_xs: Vec<_> = result.iter().map(|c| c.peak.x).collect();
        peak_xs.sort();
        assert!((peak_xs[0] as i32 - 20).abs() <= 1);
        assert!((peak_xs[1] as i32 - 80).abs() <= 1);
    }

    #[test]
    fn test_bbox_contains_all_peaks() {
        // Verify that each deblended object's bbox contains its peak
        let (pixels, labels, data) = make_test_component(
            150,
            100,
            &[(30, 30, 1.0, 2.5), (75, 50, 0.9, 2.5), (120, 70, 0.8, 2.5)],
        );

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        for candidate in &result {
            assert!(
                candidate.bbox.contains(candidate.peak),
                "Candidate bbox {:?} should contain peak {:?}",
                candidate.bbox,
                candidate.peak
            );
        }
    }

    #[test]
    fn test_peak_values_match_image() {
        // Verify that peak_value matches the actual pixel value
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 32,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        for candidate in &result {
            let actual_value = pixels[(candidate.peak.x, candidate.peak.y)];
            assert!(
                (candidate.peak_value - actual_value).abs() < 1e-6,
                "peak_value {} should match pixel value {}",
                candidate.peak_value,
                actual_value
            );
        }
    }

    #[test]
    fn test_single_threshold_level() {
        // Test with n_thresholds = 1 (edge case)
        let (pixels, labels, data) =
            make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 1,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        // Should still produce valid output
        assert!(!result.is_empty(), "Should produce at least one object");

        // Area conservation
        let total_area: usize = result.iter().map(|o| o.area).sum();
        assert_eq!(total_area, data.area, "Area should be conserved");
    }

    #[test]
    fn test_zero_threshold_level() {
        // Test with n_thresholds = 0 (edge case - should still work)
        let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 2.5)]);

        let config = DeblendConfig {
            n_thresholds: 0,
            min_contrast: 0.005,
            min_separation: 3,
            ..Default::default()
        };

        let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

        // Should produce single object
        assert_eq!(result.len(), 1, "Should produce one object");
        assert_eq!(result[0].area, data.area);
    }
}
