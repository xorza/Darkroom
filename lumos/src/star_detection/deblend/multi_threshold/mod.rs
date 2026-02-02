//! Multi-threshold deblending algorithm for separating blended sources.
//!
//! This implements a SExtractor-style deblending approach that:
//! 1. Uses multiple thresholds between detection level and peak
//! 2. Builds a tree structure tracking how regions split at higher thresholds
//! 3. Applies a contrast criterion to decide if branches are separate objects
//!
//! Reference: Bertin & Arnouts (1996), A&AS 117, 393

use arrayvec::ArrayVec;

use smallvec::SmallVec;

use super::{ComponentData, DeblendedCandidate, MAX_PEAKS, Pixel};
use crate::common::Buffer2;
use crate::math::{Aabb, Vec2us};
use crate::star_detection::candidate_detection::LabelMap;
use crate::star_detection::config::DeblendConfig;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;

// ============================================================================
// Constants
// ============================================================================

/// Maximum children per node (same as MAX_PEAKS since each child becomes a candidate).
const MAX_CHILDREN: usize = MAX_PEAKS;

/// Sentinel value indicating no pixel value at grid position.
const NO_PIXEL: f32 = f32::NEG_INFINITY;

/// Sentinel value indicating no node assignment.
const NO_NODE: u32 = u32::MAX;

// ============================================================================
// Types
// ============================================================================

/// Grid-based pixel lookup for fast neighbor access during connected component finding.
///
/// Replaces HashMap<Vec2us, f32> and HashSet<Vec2us> with flat arrays indexed by
/// local coordinates within the bounding box. This eliminates hash computation
/// overhead which was a major bottleneck (17% of CPU time).
#[derive(Debug)]
struct PixelGrid {
    /// Pixel values indexed by local coordinates. NO_PIXEL means no pixel at that position.
    values: Vec<f32>,
    /// Visited flags for BFS traversal, packed as bits (64 flags per u64).
    visited: Vec<u64>,
    /// Bounding box offset (min x coordinate).
    offset_x: usize,
    /// Bounding box offset (min y coordinate).
    offset_y: usize,
    /// Grid width (bbox width + 2 for boundary padding).
    width: usize,
    /// Grid height (bbox height + 2 for boundary padding).
    height: usize,
}

impl PixelGrid {
    /// Create an empty pixel grid.
    fn empty() -> Self {
        Self {
            values: Vec::new(),
            visited: Vec::new(),
            offset_x: 0,
            offset_y: 0,
            width: 0,
            height: 0,
        }
    }

    /// Reset and populate the grid with new pixels, reusing allocations when possible.
    ///
    /// The grid is sized to fit the bounding box of all pixels plus a 1-pixel
    /// border to simplify boundary checks in neighbor traversal.
    fn reset_with_pixels(&mut self, pixels: &[Pixel]) {
        if pixels.is_empty() {
            self.width = 0;
            self.height = 0;
            return;
        }

        // Find bounding box
        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0usize;
        let mut max_y = 0usize;

        for p in pixels {
            min_x = min_x.min(p.pos.x);
            min_y = min_y.min(p.pos.y);
            max_x = max_x.max(p.pos.x);
            max_y = max_y.max(p.pos.y);
        }

        // Add 1-pixel border on each side for safe neighbor access
        let offset_x = min_x.saturating_sub(1);
        let offset_y = min_y.saturating_sub(1);
        let width = (max_x - offset_x) + 3; // +1 for inclusive, +2 for borders
        let height = (max_y - offset_y) + 3;

        let size = width * height;

        // Resize and clear vectors, reusing allocation
        self.values.clear();
        self.values.resize(size, NO_PIXEL);

        // Packed bit vector: ceil(size / 64) u64 words
        let visited_words = size.div_ceil(64);
        self.visited.clear();
        self.visited.resize(visited_words, 0);

        self.offset_x = offset_x;
        self.offset_y = offset_y;
        self.width = width;
        self.height = height;

        // Populate grid with pixel values
        for p in pixels {
            let idx = (p.pos.y - offset_y) * width + (p.pos.x - offset_x);
            self.values[idx] = p.value;
        }
    }

    /// Check if position has been visited (packed bit access).
    #[inline]
    fn is_visited(&self, x: usize, y: usize) -> bool {
        if self.width == 0 {
            return true;
        }
        let lx = x.wrapping_sub(self.offset_x);
        let ly = y.wrapping_sub(self.offset_y);
        if lx >= self.width || ly >= self.height {
            return true; // Treat out-of-bounds as visited
        }
        let idx = ly * self.width + lx;
        let word = idx / 64;
        let bit = idx % 64;
        // SAFETY: bounds checked above
        unsafe { (*self.visited.get_unchecked(word) & (1u64 << bit)) != 0 }
    }

    /// Mark position as visited. Returns true if it was not already visited (packed bit access).
    #[inline]
    fn mark_visited(&mut self, x: usize, y: usize) -> bool {
        if self.width == 0 {
            return false;
        }
        let lx = x.wrapping_sub(self.offset_x);
        let ly = y.wrapping_sub(self.offset_y);
        if lx >= self.width || ly >= self.height {
            return false;
        }
        let idx = ly * self.width + lx;
        let word = idx / 64;
        let bit = idx % 64;
        let mask = 1u64 << bit;
        // SAFETY: bounds checked above
        unsafe {
            let word_ptr = self.visited.get_unchecked_mut(word);
            let was_visited = (*word_ptr & mask) != 0;
            *word_ptr |= mask;
            !was_visited
        }
    }
}

/// Grid-based node assignment for tracking which tree node each pixel belongs to.
///
/// Replaces HashMap<Vec2us, usize> with a flat array for O(1) lookup/update.
#[derive(Debug)]
struct NodeGrid {
    /// Node index for each pixel position. NO_NODE means unassigned.
    nodes: Vec<u32>,
    /// Bounding box offset (min x coordinate).
    offset_x: usize,
    /// Bounding box offset (min y coordinate).
    offset_y: usize,
    /// Grid width.
    width: usize,
    /// Grid height.
    height: usize,
}

impl NodeGrid {
    /// Create an empty node grid.
    fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            offset_x: 0,
            offset_y: 0,
            width: 0,
            height: 0,
        }
    }

    /// Initialize the grid from component pixels, reusing allocation when possible.
    fn reset_with_pixels(&mut self, pixels: &[Pixel]) {
        if pixels.is_empty() {
            self.width = 0;
            self.height = 0;
            return;
        }

        // Find bounding box
        let mut min_x = usize::MAX;
        let mut min_y = usize::MAX;
        let mut max_x = 0usize;
        let mut max_y = 0usize;

        for p in pixels {
            min_x = min_x.min(p.pos.x);
            min_y = min_y.min(p.pos.y);
            max_x = max_x.max(p.pos.x);
            max_y = max_y.max(p.pos.y);
        }

        self.offset_x = min_x;
        self.offset_y = min_y;
        self.width = max_x - min_x + 1;
        self.height = max_y - min_y + 1;

        let size = self.width * self.height;
        self.nodes.clear();
        self.nodes.resize(size, NO_NODE);
    }

    /// Get node index at position, or None if unassigned.
    #[inline]
    fn get(&self, x: usize, y: usize) -> Option<usize> {
        if self.width == 0 {
            return None;
        }
        let lx = x.wrapping_sub(self.offset_x);
        let ly = y.wrapping_sub(self.offset_y);
        if lx >= self.width || ly >= self.height {
            return None;
        }
        let idx = ly * self.width + lx;
        let node = self.nodes[idx];
        if node == NO_NODE {
            None
        } else {
            Some(node as usize)
        }
    }

    /// Set node index at position.
    #[inline]
    fn set(&mut self, x: usize, y: usize, node_idx: usize) {
        if self.width == 0 {
            return;
        }
        let lx = x.wrapping_sub(self.offset_x);
        let ly = y.wrapping_sub(self.offset_y);
        if lx >= self.width || ly >= self.height {
            return;
        }
        let idx = ly * self.width + lx;
        self.nodes[idx] = node_idx as u32;
    }
}

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
    /// Grid for fast pixel lookup (replaces HashMap).
    pixel_grid: PixelGrid,
}

impl TreeBuildBuffers {
    fn new(capacity: usize) -> Self {
        Self {
            above_threshold: Vec::with_capacity(capacity),
            parent_pixels_above: Vec::with_capacity(capacity),
            bfs_queue: Vec::with_capacity(capacity.min(1024)),
            regions: Vec::with_capacity(MAX_PEAKS),
            pixel_grid: PixelGrid::empty(),
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

    // Early exit: Component too small to contain multiple separable stars
    // Need at least 2 * min_separation^2 pixels for two stars to be separable
    let min_area_for_deblend = config.min_separation * config.min_separation * 2;
    if data.area < min_area_for_deblend {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    // Find peak value and detection threshold
    let peak = data.find_peak(pixels, labels);
    let peak_value = peak.value;
    let detection_threshold = data
        .iter_pixels(pixels, labels)
        .map(|p| p.value)
        .fold(f32::MAX, f32::min);

    // Early exit: Peak barely above threshold - no substructure possible
    // A secondary peak must be at least min_contrast * primary, and above detection
    // So primary must be at least 1/(1-min_contrast) above detection for meaningful deblending
    let min_ratio = 1.0 / (1.0 - config.min_contrast.min(0.99));
    if peak_value < detection_threshold * min_ratio {
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

/// Number of consecutive levels without splits before early termination.
/// Once all regions have stabilized (no further splits), continuing is unlikely
/// to find new structure. This saves ~30-50% of iterations in typical cases.
const EARLY_TERMINATION_LEVELS: usize = 4;

/// Inline capacity for deblend tree SmallVec.
/// Measurements show avg ~3 nodes, max ~170, so 16 covers most cases on stack.
const TREE_INLINE_CAP: usize = 16;

/// SmallVec type for deblend trees - avoids heap for typical small trees.
type DeblendTree = SmallVec<[DeblendNode; TREE_INLINE_CAP]>;

/// Build the deblending tree by tracking connectivity at each threshold level.
///
/// Uses exponentially spaced thresholds for better resolution at faint levels.
/// Implements early termination: stops if no splits occur for N consecutive levels.
fn build_deblend_tree(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    low: f32,
    high: f32,
    n_thresholds: usize,
    min_separation: usize,
) -> DeblendTree {
    if data.area == 0 {
        return SmallVec::new();
    }

    // Use exponential spacing: threshold[i] = low * (high/low)^(i/n)
    let ratio = (high / low).max(1.0);

    let mut tree: DeblendTree = SmallVec::new();

    // Collect component pixels once (multi-threshold needs repeated access)
    let component_pixels: Vec<Pixel> = data.iter_pixels(pixels, labels).collect();

    // Track which node each pixel belongs to at current level (grid-based for O(1) access)
    let mut pixel_to_node = NodeGrid::empty();
    pixel_to_node.reset_with_pixels(&component_pixels);

    // Reusable buffers to avoid allocations per threshold level
    let mut buffers = TreeBuildBuffers::new(component_pixels.len());

    // Early termination: track consecutive levels without splits
    let mut levels_without_splits = 0;

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

        // Find connected regions using grid-based lookup
        find_connected_regions_grid(
            &buffers.above_threshold,
            &mut buffers.regions,
            &mut buffers.pixel_grid,
            &mut buffers.bfs_queue,
        );

        let tree_size_before = tree.len();

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
                &mut buffers.pixel_grid,
                &mut buffers.bfs_queue,
            );
        }

        // Check for early termination: no new nodes created means no splits
        if tree.len() == tree_size_before && level > 0 {
            levels_without_splits += 1;
            if levels_without_splits >= EARLY_TERMINATION_LEVELS {
                break;
            }
        } else {
            levels_without_splits = 0;
        }
    }

    tree
}

/// Process the first threshold level - create root nodes.
fn process_root_level(
    tree: &mut DeblendTree,
    pixel_to_node: &mut NodeGrid,
    regions: &[Vec<Pixel>],
) {
    for region in regions {
        let node_idx = tree.len();
        let peak = find_region_peak(region);
        let flux = region.iter().map(|p| p.value).sum();

        for p in region {
            pixel_to_node.set(p.pos.x, p.pos.y, node_idx);
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
    tree: &mut DeblendTree,
    pixel_to_node: &mut NodeGrid,
    component_pixels: &[Pixel],
    regions: &[Vec<Pixel>],
    threshold: f32,
    min_separation: usize,
    parent_pixels_buf: &mut Vec<Pixel>,
    pixel_grid: &mut PixelGrid,
    bfs_queue: &mut Vec<Pixel>,
) {
    for region in regions {
        // Find the single parent node for this region
        // (all pixels in a connected region should come from same parent)
        let parent_idx = match find_single_parent_grid(region, pixel_to_node) {
            Some(idx) => idx,
            None => continue, // Skip if no parent or multiple parents
        };

        // Count pixels belonging to this parent that are above threshold
        // (avoid allocation by counting first)
        let parent_above_count = component_pixels
            .iter()
            .filter(|p| {
                pixel_to_node.get(p.pos.x, p.pos.y) == Some(parent_idx) && p.value >= threshold
            })
            .count();

        // Check if multiple distinct regions formed from same parent
        if region.len() < parent_above_count {
            // Collect parent pixels above threshold (reuse buffer)
            parent_pixels_buf.clear();
            parent_pixels_buf.extend(
                component_pixels
                    .iter()
                    .filter(|p| {
                        pixel_to_node.get(p.pos.x, p.pos.y) == Some(parent_idx)
                            && p.value >= threshold
                    })
                    .copied(),
            );

            // Find child regions using grid-based lookup
            let mut child_regions: ArrayVec<Vec<Pixel>, MAX_CHILDREN> = ArrayVec::new();
            find_connected_regions_grid_into(
                parent_pixels_buf,
                &mut child_regions,
                pixel_grid,
                bfs_queue,
            );

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

/// Find the single parent node for a region using grid lookup, or None if multiple/no parents.
#[inline]
fn find_single_parent_grid(region: &[Pixel], pixel_to_node: &NodeGrid) -> Option<usize> {
    let mut parent: Option<usize> = None;

    for p in region {
        if let Some(idx) = pixel_to_node.get(p.pos.x, p.pos.y) {
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
    tree: &mut DeblendTree,
    pixel_to_node: &mut NodeGrid,
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
            pixel_to_node.set(p.pos.x, p.pos.y, child_idx);
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

/// Maximum expected tree size for stack allocation.
/// Trees are small: O(n_thresholds * MAX_PEAKS) but practically much smaller
/// since most components don't split at every level.
const MAX_TREE_SIZE: usize = 128;

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
    // Use stack-allocated array for small trees, heap for larger ones
    if tree.len() > MAX_TREE_SIZE {
        return find_significant_branches_heap(tree, min_contrast);
    }

    let mut is_child_storage = [false; MAX_TREE_SIZE];
    let is_child = &mut is_child_storage[..tree.len()];

    for node in tree {
        for &child_idx in &node.children {
            is_child[child_idx] = true;
        }
    }

    let mut leaves: SmallVec<[usize; MAX_PEAKS]> = SmallVec::new();

    // Process each root
    for (i, &child) in is_child.iter().enumerate() {
        if !child {
            collect_significant_leaves(tree, i, min_contrast, &mut leaves);
        }
    }

    // If no leaves found (all contrast criteria failed), return roots
    if leaves.is_empty() {
        for (i, &child) in is_child.iter().enumerate() {
            if !child {
                leaves.push(i);
            }
        }
    }

    leaves
}

/// Heap-allocated fallback for unusually large trees (> MAX_TREE_SIZE nodes).
#[cold]
fn find_significant_branches_heap(
    tree: &[DeblendNode],
    min_contrast: f32,
) -> SmallVec<[usize; MAX_PEAKS]> {
    let mut is_child = vec![false; tree.len()];

    for node in tree {
        for &child_idx in &node.children {
            is_child[child_idx] = true;
        }
    }

    let mut leaves: SmallVec<[usize; MAX_PEAKS]> = SmallVec::new();

    for (i, &child) in is_child.iter().enumerate() {
        if !child {
            collect_significant_leaves(tree, i, min_contrast, &mut leaves);
        }
    }

    if leaves.is_empty() {
        for (i, &child) in is_child.iter().enumerate() {
            if !child {
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

/// Find connected regions using grid-based lookup.
///
/// This is the optimized version that uses PixelGrid instead of HashMap/HashSet.
fn find_connected_regions_grid(
    pixels: &[Pixel],
    regions: &mut Vec<Vec<Pixel>>,
    grid: &mut PixelGrid,
    queue: &mut Vec<Pixel>,
) {
    regions.clear();

    if pixels.is_empty() {
        return;
    }

    // Rebuild grid with current pixels
    grid.reset_with_pixels(pixels);

    for p in pixels {
        if grid.is_visited(p.pos.x, p.pos.y) {
            continue;
        }

        // BFS to find connected component
        let mut region = Vec::new();
        queue.clear();
        queue.push(*p);
        grid.mark_visited(p.pos.x, p.pos.y);

        while let Some(current) = queue.pop() {
            region.push(current);
            visit_neighbors_grid(current.pos, grid, queue);
        }

        regions.push(region);
    }
}

/// Find connected regions into an ArrayVec (limited capacity) using grid-based lookup.
fn find_connected_regions_grid_into<const N: usize>(
    pixels: &[Pixel],
    regions: &mut ArrayVec<Vec<Pixel>, N>,
    grid: &mut PixelGrid,
    queue: &mut Vec<Pixel>,
) {
    regions.clear();

    if pixels.is_empty() {
        return;
    }

    // Rebuild grid with current pixels
    grid.reset_with_pixels(pixels);

    for p in pixels {
        if regions.is_full() {
            break;
        }

        if grid.is_visited(p.pos.x, p.pos.y) {
            continue;
        }

        let mut region = Vec::new();
        queue.clear();
        queue.push(*p);
        grid.mark_visited(p.pos.x, p.pos.y);

        while let Some(current) = queue.pop() {
            region.push(current);
            visit_neighbors_grid(current.pos, grid, queue);
        }

        regions.push(region);
    }
}

/// Visit 8-connected neighbors using grid-based lookup.
///
/// This is the hot path - optimized with unchecked indexing since the grid
/// has a 1-pixel border ensuring all neighbor accesses are in-bounds.
#[inline]
fn visit_neighbors_grid(pos: Vec2us, grid: &mut PixelGrid, queue: &mut Vec<Pixel>) {
    // Convert to local coordinates once
    // The grid always has a 1-pixel border, so lx/ly are always >= 1 for valid pixels
    let lx = pos.x - grid.offset_x;
    let ly = pos.y - grid.offset_y;
    let width = grid.width;

    // SAFETY: The grid has a 1-pixel border on all sides (added in reset_with_pixels).
    // Since `pos` came from a pixel in the grid, local coordinates lx, ly are >= 1
    // (because offset_x = min_x - 1, so lx = pos.x - (min_x - 1) >= 1).
    // Therefore all 8 neighbors (lx±1, ly±1) are guaranteed to be valid indices.
    // The border positions contain NO_PIXEL so they won't be added to the queue.

    // All 8 neighbors - unrolled for better performance
    // Using wrapping arithmetic to avoid overflow checks, then bounds-checked indexing

    // Top-left
    try_visit_neighbor_local(lx.wrapping_sub(1), ly.wrapping_sub(1), width, grid, queue);
    // Top
    try_visit_neighbor_local(lx, ly.wrapping_sub(1), width, grid, queue);
    // Top-right
    try_visit_neighbor_local(lx + 1, ly.wrapping_sub(1), width, grid, queue);
    // Left
    try_visit_neighbor_local(lx.wrapping_sub(1), ly, width, grid, queue);
    // Right
    try_visit_neighbor_local(lx + 1, ly, width, grid, queue);
    // Bottom-left
    try_visit_neighbor_local(lx.wrapping_sub(1), ly + 1, width, grid, queue);
    // Bottom
    try_visit_neighbor_local(lx, ly + 1, width, grid, queue);
    // Bottom-right
    try_visit_neighbor_local(lx + 1, ly + 1, width, grid, queue);
}

/// Try to visit a neighbor at local grid coordinates.
/// Performs bounds check then uses unchecked access for the actual operations.
#[inline]
fn try_visit_neighbor_local(
    lx: usize,
    ly: usize,
    width: usize,
    grid: &mut PixelGrid,
    queue: &mut Vec<Pixel>,
) {
    // Bounds check - wrapping_sub produces large values that fail this check
    if lx >= grid.width || ly >= grid.height {
        return;
    }

    let idx = ly * width + lx;

    // SAFETY: bounds checked above
    let value = unsafe { *grid.values.get_unchecked(idx) };

    // NO_PIXEL means no pixel at this position (border or gap)
    if value == NO_PIXEL {
        return;
    }

    // Check and set visited bit
    let word = idx / 64;
    let bit = idx % 64;
    let mask = 1u64 << bit;

    // SAFETY: word index is derived from valid idx, which is < width * height
    // and visited.len() == ceil(width * height / 64), so word < visited.len()
    let word_ptr = unsafe { grid.visited.get_unchecked_mut(word) };

    if (*word_ptr & mask) != 0 {
        return; // Already visited
    }

    *word_ptr |= mask;

    // Convert back to absolute coordinates
    let abs_x = lx + grid.offset_x;
    let abs_y = ly + grid.offset_y;

    queue.push(Pixel {
        pos: Vec2us::new(abs_x, abs_y),
        value,
    });
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
