//! Multi-threshold deblending, after SExtractor (Bertin & Arnouts 1996, A&AS 117, 393).
//!
//! 1. Cut the component's residual at `n_thresholds` levels spaced exponentially from the
//!    detection threshold to its peak.
//! 2. Build a tree of how its pixels above each level split into connected regions.
//! 3. A branch is its own object when it holds at least `min_contrast` of the flux the whole
//!    component holds above the detection threshold.

use std::ops::Index;

use arrayvec::ArrayVec;

use crate::math::size2us::Size2us;
use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;
use crate::stacking::star_detection::config::detection_config::Connectivity;
use crate::stacking::star_detection::deblend::component::Component;
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::deblend::{MAX_PEAKS, Pixel, peaks_too_close};

/// Maximum children per node (same as `MAX_PEAKS` since each child becomes a candidate).
const MAX_CHILDREN: usize = MAX_PEAKS;

/// Sentinel value indicating no pixel value at grid position.
const NO_PIXEL: f32 = f32::NEG_INFINITY;

/// What the multi-threshold deblender reads from the detection configuration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MultiThresholdParams {
    /// Levels between the detection threshold and the peak.
    pub(crate) n_thresholds: usize,
    /// A branch's share of the component's flux above the detection threshold.
    pub(crate) min_contrast: f32,
    /// Minimum distance between sibling branches' peaks, in pixels.
    pub(crate) min_separation: usize,
    /// How the pixels above a level join into regions — the labeling's own rule.
    pub(crate) connectivity: Connectivity,
}

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
/// Flat arrays indexed by local coordinates within the bounding box, in place of a hash map
/// keyed by position.
///
/// One generation counter marks both the values and the visited set as current, so a reset is
/// O(1) instead of clearing O(n) cells: a cell holds a pixel, or was visited, when its stamp
/// equals `current_generation`.
#[derive(Debug, Default)]
struct PixelGrid {
    /// Pixel values indexed by local coordinates.
    values: Vec<f32>,
    /// Generation when each cell's value was last set.
    values_generation: Vec<u32>,
    /// Generation when each cell was last visited.
    visited_generation: Vec<u32>,
    /// Incremented on each `reset_with_pixels`.
    current_generation: u32,
    /// Bounding box offset — one cell outside the component's top-left corner.
    offset: Vec2us,
    /// Grid extent, the bbox plus one cell of boundary padding on every side.
    size: Size2us,
}

impl PixelGrid {
    /// Reset and populate the grid with new pixels, reusing allocations when possible.
    ///
    /// The grid is sized to fit the bounding box of all pixels plus a 1-pixel
    /// border to simplify boundary checks in neighbor traversal.
    fn reset_with_pixels(&mut self, pixels: &[Pixel]) {
        if pixels.is_empty() {
            self.size = Size2us::default();
            return;
        }

        // Skip 0 because generation arrays are initialized to 0 — wrapping to 0
        // would make all cells appear valid.
        self.current_generation = self.current_generation.wrapping_add(1);
        if self.current_generation == 0 {
            self.current_generation = 1;
        }

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

    /// Get pixel value at local index, or `NO_PIXEL` if not present in current generation.
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
            if *gen_ptr == self.current_generation {
                false
            } else {
                *gen_ptr = self.current_generation;
                true
            }
        }
    }
}

/// Grid-based node assignment for tracking which tree node each pixel belongs to.
///
/// A flat array in place of a hash map keyed by position, reset in O(1) by a generation counter.
#[derive(Debug, Default)]
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
        if self.nodes_generation[idx] == self.current_generation {
            Some(self.nodes[idx] as usize)
        } else {
            None
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
    /// Residual flux of the branch: the sum over its pixels above the level it split at.
    flux: f32,
    /// Branches that split from this node at a higher level, brightest by flux first.
    children: ArrayVec<u32, MAX_CHILDREN>,
}

/// A set of pixel regions held in one flat buffer.
///
/// Every region's pixels sit end to end in `pixels`, delimited by `ends`. Regions are found and
/// consumed inside a single threshold level and nothing ever takes ownership of one, so they can
/// share a buffer that is simply truncated for reuse.
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
#[derive(Debug, Default)]
struct RegionScratch {
    /// Grid for fast pixel lookup.
    grid: PixelGrid,
    /// BFS queue for connected component finding (flat grid indices).
    queue: Vec<u32>,
}

impl RegionScratch {
    /// Run BFS from a seed pixel, appending the connected region to `out`.
    ///
    /// Returns false when the seed was already visited, leaving `out` untouched.
    #[inline]
    fn bfs_region(
        &mut self,
        seed: &Pixel,
        connectivity: Connectivity,
        out: &mut RegionSet,
    ) -> bool {
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
            unsafe { visit_neighbors_grid(idx, width, connectivity, grid, queue) };
        }

        out.close_region();
        true
    }
}

/// A child region with the two facts its ranking and its node need: flux and peak.
#[derive(Debug, Clone, Copy)]
struct RankedRegion {
    flux: f32,
    peak: Pixel,
    /// The region's index in the set it was found in.
    index: u32,
}

/// The multi-threshold deblender's working sets, reused from component to component.
#[derive(Debug, Default)]
pub(crate) struct TreeBuffers {
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
    /// `child_regions` ranked by flux.
    child_order: Vec<RankedRegion>,
    region_scratch: RegionScratch,
    /// The tree of one component; node 0 is its root.
    tree: Vec<DeblendNode>,
    /// The tree's significant leaves.
    leaves: Vec<u32>,
}

/// Split `component` by its multi-threshold tree: one region when fewer than two branches pass
/// the contrast test, else one region per passing branch, the [`MAX_PEAKS`] brightest by flux.
///
/// `floor` is the detection threshold at the component, in residual units — the lowest level of
/// the ladder, as SExtractor's `DETECT_THRESH` is. It must be positive.
pub(crate) fn deblend_multi_threshold(
    component: &Component<'_>,
    floor: f32,
    params: MultiThresholdParams,
    buffers: &mut TreeBuffers,
) -> ArrayVec<Region, MAX_PEAKS> {
    debug_assert!(floor > 0.0, "the ladder starts at a positive threshold");
    debug_assert!(params.n_thresholds >= 1, "the ladder has at least one step");
    let mut result = ArrayVec::new();
    let peak = component.peak();

    // Branches are disjoint subsets of the root's pixels, all above a positive floor, so no two
    // can each hold `min_contrast ≥ 1` of the root's flux; and a peak at or below the floor
    // leaves the ladder no room to split.
    if params.min_contrast >= 1.0 || peak.value <= floor {
        result.push(component.whole());
        return result;
    }

    build_deblend_tree(
        component,
        ThresholdLadder {
            low: floor,
            high: peak.value,
            n_thresholds: params.n_thresholds,
        },
        params,
        buffers,
    );
    let TreeBuffers { tree, leaves, .. } = buffers;
    find_significant_branches(tree, params.min_contrast, leaves);
    if leaves.len() <= 1 {
        result.push(component.whole());
        return result;
    }

    leaves.sort_unstable_by(|&a, &b| {
        tree[b as usize]
            .flux
            .total_cmp(&tree[a as usize].flux)
            .then(a.cmp(&b))
    });
    let peaks: ArrayVec<Pixel, MAX_PEAKS> = leaves
        .iter()
        .take(MAX_PEAKS)
        .map(|&i| tree[i as usize].peak)
        .collect();
    component.assign_to_nearest(&peaks)
}

/// The exponentially spaced ladder one component is cut at: the floor to start
/// from, the peak to reach, and how many steps in between. The three are only
/// meaningful together — level `i` sits at `low * (high / low) ^ (i / n)`.
#[derive(Debug, Clone, Copy)]
struct ThresholdLadder {
    low: f32,
    high: f32,
    n_thresholds: usize,
}

impl ThresholdLadder {
    /// Level `i` of `0..=n_thresholds`: `low` itself at 0, `high` up to rounding at `n`.
    fn level(self, i: usize) -> f32 {
        self.low * (self.high / self.low).powf(i as f32 / self.n_thresholds as f32)
    }
}

/// Build the deblending tree in `buffers.tree` by tracking connectivity at each level.
///
/// The root is the whole component, holding the flux above `ladder.low`; levels `0..=n` then
/// split it, from `low` itself up to the peak. Exponential spacing puts the levels densest at the
/// faint end, where neighbours first separate.
fn build_deblend_tree(
    component: &Component<'_>,
    ladder: ThresholdLadder,
    params: MultiThresholdParams,
    buffers: &mut TreeBuffers,
) {
    let low = ladder.low;

    let TreeBuffers {
        component_pixels,
        pixel_to_node,
        tree,
        ..
    } = buffers;
    component_pixels.clear();
    component_pixels.extend(component.pixels());
    pixel_to_node.reset_with_pixels(component_pixels);
    for p in component_pixels.iter() {
        pixel_to_node.set(p.pos, 0);
    }
    tree.clear();
    tree.push(DeblendNode {
        peak: component.peak(),
        flux: component_pixels
            .iter()
            .filter(|p| p.value >= low)
            .map(|p| p.value)
            .sum(),
        children: ArrayVec::new(),
    });

    for level in 0..=ladder.n_thresholds {
        let threshold = ladder.level(level);

        let TreeBuffers {
            component_pixels,
            above_threshold,
            regions,
            region_scratch,
            ..
        } = buffers;
        above_threshold.clear();
        above_threshold.extend(component_pixels.iter().filter(|p| p.value >= threshold));
        if above_threshold.is_empty() {
            break;
        }

        find_connected_regions_grid(
            above_threshold,
            params.connectivity,
            regions,
            region_scratch,
        );
        process_level(buffers, params);
    }
}

/// Check every region of the current level for a split of its parent, and add the branches of
/// each split to the tree.
fn process_level(buffers: &mut TreeBuffers, params: MultiThresholdParams) {
    // Destructured rather than reached through `buffers.` so the read of `regions` and the writes
    // to the scratch below it borrow disjointly across the loop.
    let TreeBuffers {
        pixel_to_node,
        above_threshold,
        regions,
        parent_pixels_above,
        child_regions,
        child_order,
        region_scratch,
        tree,
        ..
    } = buffers;

    for region in regions.iter() {
        // All pixels of a connected region come from one parent, unless an earlier split left
        // some of them behind on another node.
        let Some(parent_idx) = find_single_parent_grid(region, pixel_to_node) else {
            continue;
        };
        // A node splits once. The regions its split did not keep — too close to a brighter
        // sibling, or past MAX_CHILDREN — stay part of it; splitting them again at a later level
        // would replace the children it already has.
        if !tree[parent_idx].children.is_empty() {
            continue;
        }

        // Rescanning per region rather than bucketing every parent's count in one pass before the
        // loop: `create_child_nodes` reassigns `pixel_to_node` *inside* this loop, so a count
        // taken up front would be stale for every parent split earlier in the same level.
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
            find_connected_regions_grid(
                parent_pixels_above,
                params.connectivity,
                child_regions,
                region_scratch,
            );
            if child_regions.len() > 1 {
                create_child_nodes(
                    tree,
                    pixel_to_node,
                    parent_idx,
                    child_regions,
                    child_order,
                    params.min_separation,
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
                Some(existing) if existing != idx => return None,
                _ => {}
            }
        }
    }

    parent
}

/// Add the regions `parent_idx` split into as its children: brightest by flux first, each at
/// least `min_separation` from every brighter sibling kept, up to [`MAX_CHILDREN`].
fn create_child_nodes(
    tree: &mut Vec<DeblendNode>,
    pixel_to_node: &mut NodeGrid,
    parent_idx: usize,
    child_regions: &RegionSet,
    child_order: &mut Vec<RankedRegion>,
    min_separation: usize,
) {
    child_order.clear();
    child_order.extend(
        child_regions
            .iter()
            .enumerate()
            .map(|(index, region)| RankedRegion {
                flux: region.iter().map(|p| p.value).sum(),
                peak: Pixel::brightest(region.iter().copied()).expect("a region holds a pixel"),
                index: index as u32,
            }),
    );
    child_order.sort_unstable_by(|a, b| b.flux.total_cmp(&a.flux).then(a.index.cmp(&b.index)));

    let min_sep_sq = min_separation * min_separation;
    let mut children: ArrayVec<u32, MAX_CHILDREN> = ArrayVec::new();
    for ranked in child_order.iter() {
        if children.is_full() {
            break;
        }
        let too_close = children
            .iter()
            .any(|&idx| peaks_too_close(ranked.peak.pos, tree[idx as usize].peak.pos, min_sep_sq));
        if too_close {
            continue;
        }

        let child_idx = tree.len();
        for p in &child_regions[ranked.index as usize] {
            pixel_to_node.set(p.pos, child_idx);
        }
        tree.push(DeblendNode {
            peak: ranked.peak,
            flux: ranked.flux,
            children: ArrayVec::new(),
        });
        children.push(child_idx as u32);
    }

    tree[parent_idx].children = children;
}

/// Collect into `leaves` the nodes of `tree` that stand as separate objects under the contrast
/// test, from the root at index 0.
fn find_significant_branches(tree: &[DeblendNode], min_contrast: f32, leaves: &mut Vec<u32>) {
    leaves.clear();
    if let Some(root) = tree.first() {
        collect_significant_leaves(tree, 0, min_contrast * root.flux, leaves);
    }
}

/// Recursively collect leaf nodes that pass the contrast criterion.
///
/// Per the SExtractor algorithm a branch is a separate object when its flux is at least
/// `min_contrast` of the root's flux, not of its immediate parent — so the bar `min_flux` is one
/// value per component instead of shrinking with depth. A parent-relative bar over-splits the
/// bright wings of large/saturated stars in crowded fields, injecting spurious detections that
/// poison registration's triangle matching.
///
/// The depth is at most `n_thresholds + 1` (one level per split), which
/// `MAX_DEBLEND_N_THRESHOLDS` bounds.
fn collect_significant_leaves(
    tree: &[DeblendNode],
    node_idx: usize,
    min_flux: f32,
    leaves: &mut Vec<u32>,
) {
    let node = &tree[node_idx];
    let passing = node
        .children
        .iter()
        .filter(|&&child| tree[child as usize].flux >= min_flux);

    // Fewer than two children clear the bar: this node is one object.
    if passing.clone().count() <= 1 {
        leaves.push(node_idx as u32);
        return;
    }
    for &child in passing {
        collect_significant_leaves(tree, child as usize, min_flux, leaves);
    }
}

/// Find connected regions using grid-based BFS, replacing whatever `regions` held.
fn find_connected_regions_grid(
    pixels: &[Pixel],
    connectivity: Connectivity,
    regions: &mut RegionSet,
    scratch: &mut RegionScratch,
) {
    regions.clear();
    if pixels.is_empty() {
        return;
    }
    scratch.grid.reset_with_pixels(pixels);

    for p in pixels {
        scratch.bfs_region(p, connectivity, regions);
    }
}

/// Visit the neighbours `connectivity` names using grid-based lookup with flat indices.
///
/// This is the hot path - fully unchecked since the grid always has a 1-pixel
/// border (guaranteed by `wrapping_sub` in `reset_with_pixels`). Border cells have
/// `NO_PIXEL` via generation check so they won't propagate BFS further.
///
/// # Safety
/// `idx` must be a valid local index within the grid with at least 1 cell of
/// padding on all sides.
#[inline]
unsafe fn visit_neighbors_grid(
    idx: usize,
    width: usize,
    connectivity: Connectivity,
    grid: &mut PixelGrid,
    queue: &mut Vec<u32>,
) {
    // SAFETY: every operation below relies only on the precondition this function's own
    // safety contract already states.
    unsafe {
        let up = idx - width;
        let down = idx + width;

        try_visit_idx(up, grid, queue);
        try_visit_idx(idx - 1, grid, queue);
        try_visit_idx(idx + 1, grid, queue);
        try_visit_idx(down, grid, queue);
        if connectivity == Connectivity::Eight {
            try_visit_idx(up - 1, grid, queue);
            try_visit_idx(up + 1, grid, queue);
            try_visit_idx(down - 1, grid, queue);
            try_visit_idx(down + 1, grid, queue);
        }
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
        if grid.get_value_unchecked(idx) == NO_PIXEL {
            return;
        }
        if grid.try_mark_visited_unchecked(idx) {
            queue.push(idx as u32);
        }
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
