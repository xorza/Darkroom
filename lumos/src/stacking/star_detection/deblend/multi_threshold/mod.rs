//! Multi-threshold deblending algorithm for separating blended sources.
//!
//! This implements a SExtractor-style deblending approach that:
//! 1. Uses multiple thresholds between detection level and peak
//! 2. Builds a tree structure tracking how regions split at higher thresholds
//! 3. Applies a contrast criterion to decide if branches are separate objects
//!
//! Reference: Bertin & Arnouts (1996), A&AS 117, 393

use std::cmp::Ordering;
use std::ops::Index;

use arrayvec::ArrayVec;

use smallvec::SmallVec;

use crate::math::rect::URect;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::deblend::{
    ComponentData, MAX_PEAKS, Pixel, assign_to_nearest_peak, peaks_too_close,
};
use crate::stacking::star_detection::labeling::LabelMap;
use imaginarium::Buffer2;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "internals"))]
mod bench;

/// Maximum children per node (same as MAX_PEAKS since each child becomes a candidate).
const MAX_CHILDREN: usize = MAX_PEAKS;

/// Sentinel value indicating no pixel value at grid position.
const NO_PIXEL: f32 = f32::NEG_INFINITY;

/// Half-open bounding box of a pixel set. Both grids size themselves from this, then differ only
/// in whether they pad it — `PixelGrid` does, for unchecked neighbour reads; `NodeGrid` does not.
///
/// Returns `URect::empty()` for an empty slice; both callers return before that can matter.
fn bounding_box(pixels: &[Pixel]) -> URect {
    let mut bbox = URect::empty();
    for p in pixels {
        bbox.include(p.pos);
    }
    bbox
}

/// Grid-based pixel lookup for fast neighbor access during connected component finding.
///
/// Replaces HashMap<Vec2us, f32> and HashSet<Vec2us> with flat arrays indexed by
/// local coordinates within the bounding box. This eliminates hash computation
/// overhead which was a major bottleneck (17% of CPU time).
///
/// Uses a generation counter for the visited array to avoid clearing on each reset.
/// Each cell stores the generation when it was last visited; comparing against
/// current_generation gives O(1) reset instead of O(n) clearing.
#[derive(Debug)]
struct PixelGrid {
    /// Pixel values indexed by local coordinates.
    values: Vec<f32>,
    /// Generation when each cell's value was last set. Cell has a pixel if
    /// values_generation[idx] == current_generation.
    values_generation: Vec<u32>,
    /// Generation when each cell was last visited. Cell is visited if
    /// visited_generation[idx] == visited_generation_counter.
    visited_generation: Vec<u32>,
    /// Generation counter for values (incremented on each reset_with_pixels).
    current_generation: u32,
    /// Separate generation counter for visited state (incremented on each new BFS).
    visited_generation_counter: u32,
    /// Bounding box offset — one cell outside the component's top-left corner.
    offset: Vec2us,
    /// Grid extent, the bbox plus one cell of boundary padding on every side.
    size: Size2us,
}

impl PixelGrid {
    /// Create an empty pixel grid.
    fn empty() -> Self {
        Self {
            values: Vec::new(),
            values_generation: Vec::new(),
            visited_generation: Vec::new(),
            current_generation: 0,
            visited_generation_counter: 0,
            offset: Vec2us::ZERO,
            size: Size2us::default(),
        }
    }

    /// Reset and populate the grid with new pixels, reusing allocations when possible.
    ///
    /// The grid is sized to fit the bounding box of all pixels plus a 1-pixel
    /// border to simplify boundary checks in neighbor traversal.
    ///
    /// Uses generation counters for both values and visited arrays: instead of
    /// clearing O(n) cells, we just increment generation counters O(1).
    fn reset_with_pixels(&mut self, pixels: &[Pixel]) {
        if pixels.is_empty() {
            self.size = Size2us::default();
            return;
        }

        // Increment generation to invalidate all previous values and visited marks.
        // Skip 0 because generation arrays are initialized to 0 — wrapping to 0
        // would make all cells appear valid.
        self.current_generation = self.current_generation.wrapping_add(1);
        if self.current_generation == 0 {
            self.current_generation = 1;
        }
        self.visited_generation_counter = self.current_generation;

        let bbox = bounding_box(pixels);

        // Guaranteed 1-pixel border on all sides for safe unchecked neighbor access. The grid is
        // indexed by (pos - offset), so the border cells sit at local coordinate 0 on each axis;
        // they hold no pixel value (the generation check returns NO_PIXEL), so BFS never
        // propagates into them. `wrapping_sub` is for a component touching row or column 0: the
        // offset wraps to usize::MAX and the index arithmetic wraps back with it.
        let offset = Vec2us::new(bbox.min.x.wrapping_sub(1), bbox.min.y.wrapping_sub(1));
        let size = Size2us::new(bbox.width() + 2, bbox.height() + 2);

        let cells = size.pixel_count();

        // Grow vectors if needed (never shrink — reuse allocations)
        if self.values.len() < cells {
            self.values.resize(cells, 0.0);
        }
        if self.values_generation.len() < cells {
            self.values_generation.resize(cells, 0);
        }
        if self.visited_generation.len() < cells {
            self.visited_generation.resize(cells, 0);
        }

        self.offset = offset;
        self.size = size;

        // Populate grid with pixel values (generation-stamped)
        let generation = self.current_generation;
        for p in pixels {
            let idx = size.index_of(Vec2us::new(
                p.pos.x.wrapping_sub(offset.x),
                p.pos.y.wrapping_sub(offset.y),
            ));
            // SAFETY: idx is within size because p.pos is within bounding box + border
            unsafe {
                *self.values.get_unchecked_mut(idx) = p.value;
                *self.values_generation.get_unchecked_mut(idx) = generation;
            }
        }
    }

    /// Get pixel value at local index, or NO_PIXEL if not present in current generation.
    #[inline]
    unsafe fn get_value_unchecked(&self, idx: usize) -> f32 {
        // SAFETY: every operation below relies only on the precondition this function's own
        // safety contract already states.
        unsafe {
            if *self.values_generation.get_unchecked(idx) == self.current_generation {
                *self.values.get_unchecked(idx)
            } else {
                NO_PIXEL
            }
        }
    }

    /// Check visited and mark at local index. Returns true if newly visited.
    #[inline]
    unsafe fn try_mark_visited_unchecked(&mut self, idx: usize) -> bool {
        // SAFETY: every operation below relies only on the precondition this function's own
        // safety contract already states.
        unsafe {
            let gen_ptr = self.visited_generation.get_unchecked_mut(idx);
            if *gen_ptr == self.visited_generation_counter {
                false
            } else {
                *gen_ptr = self.visited_generation_counter;
                true
            }
        }
    }
}

/// Grid-based node assignment for tracking which tree node each pixel belongs to.
///
/// Replaces HashMap<Vec2us, usize> with a flat array for O(1) lookup/update.
/// Uses a generation counter to avoid O(n) clearing on each reset.
#[derive(Debug)]
struct NodeGrid {
    /// Node index for each pixel position.
    nodes: Vec<u32>,
    /// Generation when each cell's node was last set.
    nodes_generation: Vec<u32>,
    /// Current generation counter.
    current_generation: u32,
    /// Bounding box offset — the component's top-left corner in image coordinates.
    offset: Vec2us,
    /// Grid extent.
    size: Size2us,
}

impl NodeGrid {
    /// Create an empty node grid.
    fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            nodes_generation: Vec::new(),
            current_generation: 0,
            offset: Vec2us::ZERO,
            size: Size2us::default(),
        }
    }

    /// Initialize the grid from component pixels, reusing allocation when possible.
    /// Uses generation counter to avoid O(n) clearing.
    fn reset_with_pixels(&mut self, pixels: &[Pixel]) {
        if pixels.is_empty() {
            self.size = Size2us::default();
            return;
        }

        self.current_generation = self.current_generation.wrapping_add(1);
        if self.current_generation == 0 {
            self.current_generation = 1;
        }

        // No border here, unlike `PixelGrid`: this grid is only ever indexed through
        // `cell_index`, which bounds-checks.
        let bbox = bounding_box(pixels);
        self.offset = bbox.min;
        self.size = Size2us::new(bbox.width(), bbox.height());

        let cells = self.size.pixel_count();
        if self.nodes.len() < cells {
            self.nodes.resize(cells, 0);
        }
        if self.nodes_generation.len() < cells {
            self.nodes_generation.resize(cells, 0);
        }
    }

    /// Local cell index for an image position, or None if outside the grid.
    #[inline]
    fn cell_index(&self, pos: Vec2us) -> Option<usize> {
        // wrapping_sub keeps the underflow case (position left of / above the offset) inside the
        // one `contains` check below instead of needing a separate signed comparison.
        let local = Vec2us::new(
            pos.x.wrapping_sub(self.offset.x),
            pos.y.wrapping_sub(self.offset.y),
        );
        self.size.contains(local).then(|| self.size.index_of(local))
    }

    /// Get node index at position, or None if unassigned.
    #[inline]
    fn get(&self, pos: Vec2us) -> Option<usize> {
        let idx = self.cell_index(pos)?;
        if self.nodes_generation[idx] != self.current_generation {
            None
        } else {
            Some(self.nodes[idx] as usize)
        }
    }

    /// Set node index at position.
    #[inline]
    fn set(&mut self, pos: Vec2us, node_idx: usize) {
        let Some(idx) = self.cell_index(pos) else {
            return;
        };
        self.nodes[idx] = node_idx as u32;
        self.nodes_generation[idx] = self.current_generation;
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

/// A set of pixel regions held in one flat buffer.
///
/// Every region's pixels sit end to end in `pixels`, delimited by `ends`. Regions are found and
/// consumed inside a single threshold level and nothing ever takes ownership of one, so they can
/// share a buffer that is simply truncated for reuse — which is what replaced a `Vec<Vec<Pixel>>`
/// and the pool of recycled inner `Vec`s that existed to stop it allocating per region.
#[derive(Debug, Default)]
struct RegionSet {
    /// Every region's pixels, concatenated.
    pixels: Vec<Pixel>,
    /// End offset of each region in `pixels`. Region `i` starts where region `i - 1` ended, and
    /// the first at 0 — regions are only ever appended, never removed, so the starts stay
    /// implicit.
    ends: Vec<u32>,
}

impl RegionSet {
    /// Keeps both allocations; the next search refills them.
    fn clear(&mut self) {
        self.pixels.clear();
        self.ends.clear();
    }

    fn len(&self) -> usize {
        self.ends.len()
    }

    /// Close the run of pixels appended since the last region as a region of its own.
    fn close_region(&mut self) {
        debug_assert!(
            u32::try_from(self.pixels.len()).is_ok(),
            "a component cannot exceed u32 pixels"
        );
        self.ends.push(self.pixels.len() as u32);
    }

    fn iter(&self) -> impl Iterator<Item = &[Pixel]> {
        (0..self.len()).map(|i| &self[i])
    }
}

impl Index<usize> for RegionSet {
    type Output = [Pixel];

    fn index(&self, index: usize) -> &[Pixel] {
        let start = if index == 0 { 0 } else { self.ends[index - 1] };
        &self.pixels[start as usize..self.ends[index] as usize]
    }
}

/// The scratch a connected-region search reuses: the grid it labels and the queue it walks. Both
/// are needed by every search, so they travel together.
#[derive(Debug)]
struct RegionScratch {
    /// Grid for fast pixel lookup (replaces HashMap).
    grid: PixelGrid,
    /// BFS queue for connected component finding (flat grid indices).
    queue: Vec<u32>,
}

impl RegionScratch {
    fn new() -> Self {
        Self {
            grid: PixelGrid::empty(),
            queue: Vec::new(),
        }
    }

    /// Run BFS from a seed pixel, appending the connected region to `out`.
    ///
    /// Returns false when the seed was already visited, leaving `out` untouched.
    #[inline]
    fn bfs_region(&mut self, seed: &Pixel, out: &mut RegionSet) -> bool {
        let Self { grid, queue } = self;
        // Hoisted out of the loop below: `grid` is borrowed mutably inside it, so the extent and
        // offset can't be re-read from the struct there.
        let size = grid.size;
        let offset = grid.offset;
        let width = size.width;

        let start_idx = size.index_of(Vec2us::new(
            seed.pos.x.wrapping_sub(offset.x),
            seed.pos.y.wrapping_sub(offset.y),
        ));

        // SAFETY: pixel is within grid bounds (placed during reset_with_pixels)
        if unsafe { !grid.try_mark_visited_unchecked(start_idx) } {
            return false;
        }

        queue.clear();
        queue.push(start_idx as u32);

        while let Some(idx) = queue.pop() {
            let idx = idx as usize;
            // SAFETY: idx was validated when pushed to queue
            let value = unsafe { grid.get_value_unchecked(idx) };
            let local = size.point_of(idx);
            out.pixels.push(Pixel {
                pos: Vec2us::new(
                    local.x.wrapping_add(offset.x),
                    local.y.wrapping_add(offset.y),
                ),
                value,
            });
            // SAFETY: grid has guaranteed 1-pixel border (wrapping_sub in reset_with_pixels),
            // so all 8 neighbors of any valid pixel are in-bounds.
            unsafe { visit_neighbors_grid(idx, width, grid, queue) };
        }

        out.close_region();
        true
    }
}

/// `deblend_multi_threshold` with pre-allocated buffers to avoid per-call allocations.
#[derive(Debug)]
pub(crate) struct DeblendBuffers {
    /// Collected component pixels.
    component_pixels: Vec<Pixel>,
    /// Node assignment grid.
    pixel_to_node: NodeGrid,
    /// Pixels above current threshold.
    above_threshold: Vec<Pixel>,
    /// Pixels belonging to a parent that are above threshold.
    parent_pixels_above: Vec<Pixel>,
    /// The regions the component broke into at the current threshold level.
    regions: RegionSet,
    /// The regions one parent split into. Separate from `regions` because it is filled while
    /// `regions` is being iterated, so the two cannot share a buffer.
    child_regions: RegionSet,
    region_scratch: RegionScratch,
}

impl DeblendBuffers {
    pub(crate) fn new() -> Self {
        Self {
            component_pixels: Vec::new(),
            pixel_to_node: NodeGrid::empty(),
            above_threshold: Vec::new(),
            parent_pixels_above: Vec::new(),
            regions: RegionSet::default(),
            child_regions: RegionSet::default(),
            region_scratch: RegionScratch::new(),
        }
    }
}

/// Multi-threshold deblending with caller-provided reusable buffers.
///
/// Buffers are reused across threshold levels within a single component, and
/// can be reused across multiple components by the caller (e.g., one per rayon thread).
pub(crate) fn deblend_multi_threshold(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    n_thresholds: usize,
    min_separation: usize,
    min_contrast: f32,
    buffers: &mut DeblendBuffers,
) -> SmallVec<[Region; MAX_PEAKS]> {
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
    if min_contrast >= 1.0 {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    // Early exit: Component too small to contain multiple separable stars
    // Need at least 2 * min_separation^2 pixels for two stars to be separable
    let min_area_for_deblend = min_separation * min_separation * 2;
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
    let min_ratio = 1.0 / (1.0 - min_contrast.min(0.99));
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
        n_thresholds,
        min_separation,
        buffers,
    );

    // If tree has only one leaf (no branching), return single object
    if tree.is_empty() {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    // Find leaf nodes (objects) using contrast criterion
    let leaves = find_significant_branches(&tree, min_contrast);

    if leaves.len() <= 1 {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    // Assign all pixels to nearest leaf peak
    assign_pixels_to_objects(data, pixels, labels, &tree, &leaves)
}

/// Inline capacity for deblend tree SmallVec.
/// Measurements show avg ~3 nodes, max ~170, so 16 covers most cases on stack.
const TREE_INLINE_CAP: usize = 16;

/// SmallVec type for deblend trees - avoids heap for typical small trees.
type DeblendTree = SmallVec<[DeblendNode; TREE_INLINE_CAP]>;

/// Build the deblending tree by tracking connectivity at each threshold level.
///
/// Uses exponentially spaced thresholds for better resolution at faint levels.
#[allow(clippy::too_many_arguments)]
fn build_deblend_tree(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    low: f32,
    high: f32,
    n_thresholds: usize,
    min_separation: usize,
    buffers: &mut DeblendBuffers,
) -> DeblendTree {
    if data.area == 0 {
        return SmallVec::new();
    }

    // Use exponential spacing: threshold[i] = low * (high/low)^(i/n).
    // A component whose floor pixel is exactly (or near) zero would send
    // `high/low` to infinity and `low * infinity^t` to NaN for every t > 0 —
    // every above_threshold set above level 0 comes up empty since `>= NaN` is
    // always false, so the tree silently never splits. Floor `low` relative to
    // `high` so the ratio, and every threshold in the ladder, stays finite.
    let low = low.max(high * 1e-6).max(f32::MIN_POSITIVE);
    let ratio = (high / low).max(1.0);

    let mut tree: DeblendTree = SmallVec::new();

    // Collect component pixels once (multi-threshold needs repeated access)
    buffers.component_pixels.clear();
    buffers
        .component_pixels
        .extend(data.iter_pixels(pixels, labels));

    // Track which node each pixel belongs to at current level
    buffers
        .pixel_to_node
        .reset_with_pixels(&buffers.component_pixels);

    // Process each threshold level from low to high
    for level in 0..=n_thresholds {
        let t = level as f32 / n_thresholds.max(1) as f32;
        let threshold = low * ratio.powf(t);

        // Filter pixels above threshold (reuse buffer)
        buffers.above_threshold.clear();
        buffers.above_threshold.extend(
            buffers
                .component_pixels
                .iter()
                .filter(|p| p.value >= threshold),
        );

        if buffers.above_threshold.is_empty() {
            break;
        }

        // Find connected regions using grid-based lookup
        find_connected_regions_grid(
            &buffers.above_threshold,
            &mut buffers.regions,
            &mut buffers.region_scratch,
            NO_REGION_LIMIT,
        );

        if level == 0 {
            process_root_level(&mut tree, &mut buffers.pixel_to_node, &buffers.regions);
        } else {
            process_higher_level(&mut tree, buffers, min_separation);
        }
    }

    tree
}

/// Process the first threshold level - create root nodes.
fn process_root_level(tree: &mut DeblendTree, pixel_to_node: &mut NodeGrid, regions: &RegionSet) {
    for region in regions.iter() {
        let node_idx = tree.len();
        let peak = find_region_peak(region);
        let flux = region.iter().map(|p| p.value).sum();

        for p in region {
            pixel_to_node.set(p.pos, node_idx);
        }

        tree.push(DeblendNode {
            peak,
            flux,
            children: SmallVec::new(),
        });
    }
}

/// Process higher threshold levels - check for region splits.
fn process_higher_level(
    tree: &mut DeblendTree,
    buffers: &mut DeblendBuffers,
    min_separation: usize,
) {
    // Destructured rather than reached through `buffers.` so the read of `regions` and the writes
    // to the scratch below it borrow disjointly across the loop.
    let DeblendBuffers {
        pixel_to_node,
        above_threshold,
        regions,
        parent_pixels_above,
        child_regions,
        region_scratch,
        ..
    } = buffers;

    for region in regions.iter() {
        // Find the single parent node for this region
        // (all pixels in a connected region should come from same parent)
        let parent_idx = match find_single_parent_grid(region, pixel_to_node) {
            Some(idx) => idx,
            None => continue, // Skip if no parent or multiple parents
        };

        // Rescanning per region rather than bucketing every parent's count in one pass before the
        // loop, which would be O(P + R) instead of O(R*P) and would also be wrong:
        // `create_child_nodes` reassigns `pixel_to_node` *inside* this loop and does it partially,
        // leaving pixels on the parent when a child peak is too close to an existing one or when
        // there are more than MAX_CHILDREN of them. A count taken up front would be stale by
        // exactly those pixels and would report splits that never happened.
        //
        // `above_threshold` is already every component pixel at or above this level's threshold,
        // so the parent's share of it needs only the node test — walking `component_pixels` here
        // would re-apply the value test over the whole component to reach the same set.
        parent_pixels_above.clear();
        parent_pixels_above.extend(
            above_threshold
                .iter()
                .filter(|p| pixel_to_node.get(p.pos) == Some(parent_idx))
                .copied(),
        );

        // Fewer pixels in this region than the parent has above the threshold means they did not
        // all stay connected: something else formed alongside it, so the parent split.
        if region.len() < parent_pixels_above.len() {
            // Find child regions using grid-based lookup. The call drains the previous split's
            // regions back into the pool before refilling.
            find_connected_regions_grid(
                parent_pixels_above,
                child_regions,
                region_scratch,
                MAX_CHILDREN,
            );

            if child_regions.len() > 1 {
                create_child_nodes(
                    tree,
                    pixel_to_node,
                    parent_idx,
                    child_regions,
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
        if let Some(idx) = pixel_to_node.get(p.pos) {
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
    child_regions: &RegionSet,
    min_separation: usize,
) {
    let min_sep_sq = min_separation * min_separation;
    let mut child_indices: ArrayVec<usize, MAX_CHILDREN> = ArrayVec::new();

    for child_region in child_regions.iter() {
        if child_indices.is_full() {
            break;
        }

        let child_peak = find_region_peak(child_region);

        // Check minimum separation from existing children (squared Euclidean via
        // the shared `peaks_too_close`, matching local_maxima's metric).
        let too_close = child_indices
            .iter()
            .any(|&idx| peaks_too_close(child_peak.pos, tree[idx].peak.pos, min_sep_sq));

        if too_close {
            continue;
        }

        let child_idx = tree.len();
        let child_flux = child_region.iter().map(|p| p.value).sum();

        for p in child_region {
            pixel_to_node.set(p.pos, child_idx);
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

/// Maximum expected tree size for stack allocation.
/// Trees are small: O(n_thresholds * MAX_PEAKS) but practically much smaller
/// since most components don't split at every level.
const MAX_TREE_SIZE: usize = 128;

/// Find significant branches (leaves) that pass the contrast criterion.
///
/// Returns indices of nodes that should be treated as separate objects.
/// Uses stack allocation for small trees, heap for larger ones.
fn find_significant_branches(
    tree: &[DeblendNode],
    min_contrast: f32,
) -> SmallVec<[usize; MAX_PEAKS]> {
    if tree.is_empty() {
        return SmallVec::new();
    }

    // Stack-allocated for typical small trees, heap fallback for large ones
    if tree.len() > MAX_TREE_SIZE {
        let is_child = vec![false; tree.len()];
        return collect_roots_and_leaves(tree, min_contrast, is_child);
    }

    let mut is_child_storage = [false; MAX_TREE_SIZE];
    let is_child = &mut is_child_storage[..tree.len()];
    collect_roots_and_leaves(tree, min_contrast, is_child)
}

/// Mark child flags, find roots, collect significant leaves with fallback.
fn collect_roots_and_leaves(
    tree: &[DeblendNode],
    min_contrast: f32,
    mut is_child: impl AsMut<[bool]>,
) -> SmallVec<[usize; MAX_PEAKS]> {
    let is_child = is_child.as_mut();

    for node in tree {
        for &child_idx in &node.children {
            is_child[child_idx] = true;
        }
    }

    let mut leaves: SmallVec<[usize; MAX_PEAKS]> = SmallVec::new();

    for (i, &child) in is_child.iter().enumerate() {
        if !child {
            // `i` is a root: its flux is the island's total isophotal flux —
            // the global bar for every contrast test within the island.
            collect_significant_leaves(tree, i, tree[i].flux, min_contrast, &mut leaves);
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

/// Recursively collect leaf nodes that pass the contrast criterion.
///
/// Per the SExtractor algorithm a branch is a separate object when its flux is
/// at least `min_contrast` of the island's **root/total isophotal flux**
/// (`root_flux`), not of its immediate parent — so the bar is one global value
/// per island instead of shrinking with depth. A parent-relative bar
/// over-splits the bright wings of large/saturated stars in crowded fields,
/// injecting spurious detections that poison registration's triangle matching.
fn collect_significant_leaves(
    tree: &[DeblendNode],
    node_idx: usize,
    root_flux: f32,
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

    // A child splits off only if it clears `min_contrast` of the island total.
    let min_flux = min_contrast * root_flux;

    let mut num_pass = 0;
    for &child_idx in &node.children {
        if tree[child_idx].flux >= min_flux {
            num_pass += 1;
        }
    }

    if num_pass <= 1 {
        // Fewer than two children clear the bar - treat this node as a leaf.
        if leaves.len() < MAX_PEAKS {
            leaves.push(node_idx);
        }
    } else {
        // Multiple children clear the bar - recurse, carrying the same root bar.
        for &child_idx in &node.children {
            if tree[child_idx].flux >= min_flux {
                collect_significant_leaves(tree, child_idx, root_flux, min_contrast, leaves);
            }
        }
    }
}

/// Assign pixels to their nearest object using the leaf peak positions, then build one `Region` per
/// occupied peak. Delegates the Voronoi assignment to the shared `assign_to_nearest_peak`.
fn assign_pixels_to_objects(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    tree: &[DeblendNode],
    leaf_indices: &[usize],
) -> SmallVec<[Region; MAX_PEAKS]> {
    if leaf_indices.is_empty() {
        return smallvec::smallvec![create_single_object(data, pixels, labels)];
    }

    let peaks: ArrayVec<Pixel, MAX_PEAKS> = leaf_indices
        .iter()
        .take(MAX_PEAKS)
        .map(|&i| tree[i].peak)
        .collect();

    assign_to_nearest_peak(data, pixels, labels, &peaks)
        .into_iter()
        .collect()
}

/// Passed as `max_regions` by a caller that wants every region a component breaks into.
const NO_REGION_LIMIT: usize = usize::MAX;

/// Find connected regions using grid-based BFS, replacing whatever `regions` held.
///
/// Stops once `max_regions` have been found. Uses PixelGrid for O(1) neighbor lookup with
/// flat-index BFS queue.
fn find_connected_regions_grid(
    pixels: &[Pixel],
    regions: &mut RegionSet,
    scratch: &mut RegionScratch,
    max_regions: usize,
) {
    regions.clear();
    if pixels.is_empty() {
        return;
    }
    scratch.grid.reset_with_pixels(pixels);

    for p in pixels {
        if regions.len() == max_regions {
            break;
        }
        scratch.bfs_region(p, regions);
    }
}

/// Visit 8-connected neighbors using grid-based lookup with flat indices.
///
/// This is the hot path - fully unchecked since the grid always has a 1-pixel
/// border (guaranteed by wrapping_sub in reset_with_pixels). Border cells have
/// NO_PIXEL via generation check so they won't propagate BFS further.
///
/// # Safety
/// `idx` must be a valid local index within the grid with at least 1 cell of
/// padding on all sides.
#[inline]
unsafe fn visit_neighbors_grid(
    idx: usize,
    width: usize,
    grid: &mut PixelGrid,
    queue: &mut Vec<u32>,
) {
    // SAFETY: every operation below relies only on the precondition this function's own
    // safety contract already states.
    unsafe {
        // Pre-compute all 8 neighbor indices (guaranteed in-bounds by border)
        let up = idx - width;
        let down = idx + width;

        try_visit_idx(up - 1, grid, queue); // top-left
        try_visit_idx(up, grid, queue); // top
        try_visit_idx(up + 1, grid, queue); // top-right
        try_visit_idx(idx - 1, grid, queue); // left
        try_visit_idx(idx + 1, grid, queue); // right
        try_visit_idx(down - 1, grid, queue); // bottom-left
        try_visit_idx(down, grid, queue); // bottom
        try_visit_idx(down + 1, grid, queue); // bottom-right
    }
}

/// Try to visit a neighbor at a flat grid index. Fully unchecked.
///
/// # Safety
/// `idx` must be a valid index within the grid arrays.
#[inline]
unsafe fn try_visit_idx(idx: usize, grid: &mut PixelGrid, queue: &mut Vec<u32>) {
    // SAFETY: every operation below relies only on the precondition this function's own
    // safety contract already states.
    unsafe {
        // Check if cell has a pixel in current generation
        let value = grid.get_value_unchecked(idx);
        if value == NO_PIXEL {
            return;
        }
        // Check and mark visited
        if grid.try_mark_visited_unchecked(idx) {
            queue.push(idx as u32);
        }
    }
}

/// Find the peak (brightest pixel) in a region.
#[inline]
fn find_region_peak(region: &[Pixel]) -> Pixel {
    region
        .iter()
        .max_by(|a, b| a.value.partial_cmp(&b.value).unwrap_or(Ordering::Equal))
        .copied()
        .expect("region must not be empty")
}

/// Create a single object from all pixels (no deblending).
#[inline]
fn create_single_object(data: &ComponentData, pixels: &Buffer2<f32>, labels: &LabelMap) -> Region {
    let peak = data.find_peak(pixels, labels);

    Region {
        bbox: data.bbox,
        peak: peak.pos,
        peak_value: peak.value,
        area: data.area,
    }
}
