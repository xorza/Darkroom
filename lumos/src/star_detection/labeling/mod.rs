//! Connected component labeling using union-find.
//!
//! Optimized for sparse binary masks (typical in star detection):
//! - Run-length encoding (RLE) based labeling for efficient processing
//! - Word-level bit scanning to skip background regions
//! - Block-based parallel labeling with boundary merging
//! - Lock-free union-find with atomic operations
//! - Minimal allocations via buffer reuse

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU32, Ordering};

use rayon::prelude::*;

use crate::common::{BitBuffer2, Buffer2, UnsafeSendPtr};
use crate::star_detection::buffer_pool::BufferPool;

use crate::star_detection::config::Connectivity;

#[cfg(test)]
pub(crate) mod test_utils;

/// Pixel count below which sequential CCL is faster than parallel.
/// Determined by benchmark: parallel overhead dominates for small images.
const PARALLEL_CCL_THRESHOLD: usize = 65_000;

/// Minimum rows per strip in parallel CCL to avoid excessive strip overhead.
const MIN_ROWS_PER_STRIP: usize = 64;

// ============================================================================
// Run-Length Encoding
// ============================================================================

/// A horizontal run of foreground pixels.
#[derive(Debug, Clone, Copy)]
pub(super) struct Run {
    start: u32, // Starting x coordinate (inclusive)
    end: u32,   // Ending x coordinate (exclusive)
    label: u32, // Provisional label
}

impl Run {
    /// Get the search window for finding overlapping runs in the previous row.
    /// Returns (start, end) where end is exclusive.
    #[inline]
    fn search_window(&self, connectivity: Connectivity) -> (u32, u32) {
        match connectivity {
            Connectivity::Four => (self.start, self.end),
            Connectivity::Eight => (self.start.saturating_sub(1), self.end + 1),
        }
    }
}

/// Check if two runs from adjacent rows are connected.
#[inline]
fn runs_connected(prev: &Run, curr: &Run, connectivity: Connectivity) -> bool {
    match connectivity {
        Connectivity::Four => prev.start < curr.end && prev.end > curr.start,
        Connectivity::Eight => prev.start < curr.end + 1 && prev.end + 1 > curr.start,
    }
}

/// Extract runs from a single row of the mask using word-level bit scanning.
///
/// Uses trailing zero counting (CTZ) for efficient run boundary detection.
/// This is faster than bit-by-bit scanning for mixed words.
#[inline]
pub(super) fn extract_runs_from_row(
    mask_words: &[u64],
    word_row_start: usize,
    words_per_row: usize,
    width: usize,
    runs: &mut Vec<Run>,
) {
    let mut in_run = false;
    let mut run_start = 0u32;

    for word_idx in 0..words_per_row {
        let word = mask_words[word_row_start + word_idx];
        let base_x = (word_idx * 64) as u32;

        if word == 0 {
            // All zeros - close any open run
            if in_run {
                let end = base_x.min(width as u32);
                runs.push(Run {
                    start: run_start,
                    end,
                    label: 0,
                });
                in_run = false;
            }
            continue;
        }

        if word == !0u64 {
            // All ones - extend or start run
            if !in_run {
                run_start = base_x;
                in_run = true;
            }
            continue;
        }

        // Mixed word - use CTZ-based scanning for run transitions
        extract_runs_from_mixed_word(
            word,
            base_x,
            width as u32,
            &mut in_run,
            &mut run_start,
            runs,
        );
    }

    // Close final run if still open
    if in_run {
        runs.push(Run {
            start: run_start,
            end: width as u32,
            label: 0,
        });
    }
}

/// Extract runs from a mixed word (contains both 0s and 1s) using CTZ.
#[inline]
fn extract_runs_from_mixed_word(
    word: u64,
    base_x: u32,
    width: u32,
    in_run: &mut bool,
    run_start: &mut u32,
    runs: &mut Vec<Run>,
) {
    let word_end = (base_x + 64).min(width);
    let mut pos = base_x;

    loop {
        if pos >= word_end {
            break;
        }

        let bit_offset = pos - base_x;
        let remaining_bits = word >> bit_offset;

        if *in_run {
            // Find next 0 bit (end of run)
            if remaining_bits == !0u64 >> bit_offset {
                break;
            }
            let inverted = !remaining_bits;
            let zeros_until_end = inverted.trailing_zeros();
            let end_pos = pos + zeros_until_end;

            if end_pos >= word_end {
                break;
            }

            runs.push(Run {
                start: *run_start,
                end: end_pos,
                label: 0,
            });
            *in_run = false;
            pos = end_pos;
        } else {
            // Find next 1 bit (start of run)
            if remaining_bits == 0 {
                break;
            }
            let zeros_until_start = remaining_bits.trailing_zeros();
            let start_pos = pos + zeros_until_start;

            if start_pos >= word_end {
                break;
            }

            *run_start = start_pos;
            *in_run = true;
            pos = start_pos;
        }
    }
}

// ============================================================================
// LabelMap
// ============================================================================

/// A 2D label map from connected component analysis.
#[derive(Debug)]
pub struct LabelMap {
    labels: Buffer2<u32>,
    num_labels: usize,
}

impl LabelMap {
    /// Create a label map by acquiring a buffer from a pool.
    ///
    /// # Arguments
    /// * `mask` - Binary mask of foreground pixels
    /// * `connectivity` - Four (default) or Eight connectivity
    /// * `pool` - Buffer pool to acquire the u32 buffer from
    pub fn from_pool(mask: &BitBuffer2, connectivity: Connectivity, pool: &mut BufferPool) -> Self {
        assert_eq!(mask.width(), pool.width());
        assert_eq!(mask.height(), pool.height());

        let mut labels = pool.acquire_u32();
        // Clear the buffer (it may contain old labels)
        labels.pixels_mut().fill(0);

        Self::from_buffer(mask, connectivity, labels)
    }

    /// Create a label map from a binary mask with a pre-allocated buffer.
    ///
    /// Uses block-based parallel algorithm for large images:
    /// 1. Divide image into horizontal strips
    /// 2. Label each strip in parallel using word-level bit scanning
    /// 3. Merge labels at strip boundaries using atomic union-find
    /// 4. Flatten labels in parallel
    ///
    /// # Arguments
    /// * `mask` - Binary mask of foreground pixels
    /// * `connectivity` - Four or Eight connectivity
    /// * `labels` - Pre-allocated buffer (must be zeroed, same dimensions as mask)
    pub fn from_buffer(
        mask: &BitBuffer2,
        connectivity: Connectivity,
        mut labels: Buffer2<u32>,
    ) -> Self {
        let width = mask.width();
        let height = mask.height();

        assert_eq!(width, labels.width());
        assert_eq!(height, labels.height());

        if width == 0 || height == 0 {
            return Self {
                labels,
                num_labels: 0,
            };
        }

        let num_labels = if width * height < PARALLEL_CCL_THRESHOLD {
            label_mask_sequential(mask, &mut labels, connectivity)
        } else {
            label_mask_parallel(mask, &mut labels, connectivity)
        };

        Self { labels, num_labels }
    }

    /// Release this LabelMap's buffer back to the pool.
    pub fn release_to_pool(self, pool: &mut BufferPool) {
        pool.release_u32(self.labels);
    }

    /// Number of connected components (excluding background).
    #[inline]
    pub fn num_labels(&self) -> usize {
        self.num_labels
    }

    #[inline]
    pub fn width(&self) -> usize {
        self.labels.width()
    }

    #[inline]
    pub fn height(&self) -> usize {
        self.labels.height()
    }

    /// Get the raw labels slice.
    #[inline]
    pub fn labels(&self) -> &[u32] {
        self.labels.pixels()
    }
}

impl std::ops::Index<usize> for LabelMap {
    type Output = u32;

    #[inline]
    fn index(&self, idx: usize) -> &Self::Output {
        &self.labels[idx]
    }
}

// ============================================================================
// Shared run-merge helper
// ============================================================================

/// Trait abstracting union-find operations for run merging.
trait RunMergeUF {
    fn union(&mut self, a: u32, b: u32);
    fn make_set(&mut self) -> u32;
}

impl RunMergeUF for UnionFind {
    #[inline]
    fn union(&mut self, a: u32, b: u32) {
        UnionFind::union(self, a, b);
    }
    #[inline]
    fn make_set(&mut self) -> u32 {
        UnionFind::make_set(self)
    }
}

/// Wrapper to adapt `&AtomicUnionFind` (which uses `&self`) to `RunMergeUF` (which uses `&mut self`).
#[derive(Debug)]
struct AtomicUFRef<'a>(&'a AtomicUnionFind);

impl RunMergeUF for AtomicUFRef<'_> {
    #[inline]
    fn union(&mut self, a: u32, b: u32) {
        self.0.union(a, b);
    }
    #[inline]
    fn make_set(&mut self) -> u32 {
        self.0.make_set()
    }
}

/// Merge current row's runs with previous row's runs via union-find.
///
/// For each run in `curr_runs`, finds overlapping runs in `prev_runs` and merges
/// their labels. Runs without overlap get a new label via `uf.make_set()`.
#[inline]
fn merge_runs_with_prev(
    curr_runs: &mut [Run],
    prev_runs: &[Run],
    connectivity: Connectivity,
    uf: &mut impl RunMergeUF,
) {
    let mut prev_idx = 0;
    for run in curr_runs.iter_mut() {
        let (search_start, search_end) = run.search_window(connectivity);

        while prev_idx < prev_runs.len() && prev_runs[prev_idx].end <= search_start {
            prev_idx += 1;
        }

        let mut assigned_label = None;
        let mut check_idx = prev_idx;
        while check_idx < prev_runs.len() && prev_runs[check_idx].start < search_end {
            let prev_run = &prev_runs[check_idx];
            if runs_connected(prev_run, run, connectivity) {
                match assigned_label {
                    Some(label) if label != prev_run.label => uf.union(label, prev_run.label),
                    None => assigned_label = Some(prev_run.label),
                    _ => {}
                }
            }
            check_idx += 1;
        }

        run.label = assigned_label.unwrap_or_else(|| uf.make_set());
    }
}

// ============================================================================
// Sequential labeling (small images)
// ============================================================================

/// Sequential RLE-based CCL for small images.
pub(super) fn label_mask_sequential(
    mask: &BitBuffer2,
    labels: &mut Buffer2<u32>,
    connectivity: Connectivity,
) -> usize {
    let width = mask.width();
    let height = mask.height();
    let words_per_row = mask.words_per_row();
    let mask_words = mask.words();

    let mut uf = UnionFind::new();
    let mut prev_runs: Vec<Run> = Vec::with_capacity(width / 4);
    let mut curr_runs: Vec<Run> = Vec::with_capacity(width / 4);

    for y in 0..height {
        curr_runs.clear();
        extract_runs_from_row(
            mask_words,
            y * words_per_row,
            words_per_row,
            width,
            &mut curr_runs,
        );

        if curr_runs.is_empty() {
            prev_runs.clear();
            continue;
        }

        merge_runs_with_prev(&mut curr_runs, &prev_runs, connectivity, &mut uf);

        // Write labels to output
        let row_start = y * width;
        for run in &curr_runs {
            for x in run.start..run.end {
                labels[row_start + x as usize] = run.label;
            }
        }

        std::mem::swap(&mut prev_runs, &mut curr_runs);
    }

    uf.flatten_labels(labels.pixels_mut())
}

// ============================================================================
// Parallel labeling (large images)
// ============================================================================

/// Result from labeling a strip.
#[derive(Debug)]
struct StripResult {
    /// All runs with their row indices
    runs: Vec<(u32, Run)>,
    /// Runs from the last row of the strip (for boundary merging)
    last_row_runs: Vec<Run>,
    /// Runs from the first row of the strip (for boundary merging)
    first_row_runs: Vec<Run>,
}

/// Parallel RLE-based CCL for large images.
pub(super) fn label_mask_parallel(
    mask: &BitBuffer2,
    labels: &mut Buffer2<u32>,
    connectivity: Connectivity,
) -> usize {
    let width = mask.width();
    let height = mask.height();
    let words_per_row = mask.words_per_row();
    let mask_words = mask.words();

    let num_threads = rayon::current_num_threads();
    let num_strips = (height / MIN_ROWS_PER_STRIP).clamp(1, num_threads);
    let rows_per_strip = height / num_strips;

    // Atomic union-find for parallel merging (5% of pixels as upper bound)
    let uf = AtomicUnionFind::new(((width * height) / 20).max(1024));

    // Phase 1: Label each strip in parallel
    let strip_results: Vec<StripResult> = (0..num_strips)
        .into_par_iter()
        .map(|strip_idx| {
            let y_start = strip_idx * rows_per_strip;
            let y_end = if strip_idx == num_strips - 1 {
                height
            } else {
                (strip_idx + 1) * rows_per_strip
            };
            label_strip(
                mask_words,
                width,
                words_per_row,
                y_start,
                y_end,
                &uf,
                connectivity,
            )
        })
        .collect();

    // Phase 2: Merge labels at strip boundaries using O(n+m) sorted merge
    for strip_idx in 1..num_strips {
        merge_strip_boundary_sorted(
            &strip_results[strip_idx - 1].last_row_runs,
            &strip_results[strip_idx].first_row_runs,
            &uf,
            connectivity,
        );
    }

    let total_labels = uf.label_count();
    if total_labels == 0 {
        return 0;
    }

    // Phase 3: Build final label mapping
    let label_map = uf.build_label_map(total_labels);

    // Phase 4: Write labels in parallel - iterate strips directly
    let labels_ptr = UnsafeSendPtr::new(labels.pixels_mut().as_mut_ptr());

    strip_results.par_iter().for_each(|strip| {
        for &(y, run) in &strip.runs {
            let row_start = y as usize * width;
            let final_label = label_map
                .get(run.label as usize)
                .copied()
                .expect("label out of range in label_map");
            // SAFETY: Each run writes to disjoint pixels
            let ptr = labels_ptr.get();
            for x in run.start..run.end {
                unsafe {
                    *ptr.add(row_start + x as usize) = final_label;
                }
            }
        }
    });

    label_map
        .iter()
        .filter(|&&l| l > 0)
        .max()
        .copied()
        .unwrap_or(0) as usize
}

/// Label a single strip and return runs with boundary information.
fn label_strip(
    mask_words: &[u64],
    width: usize,
    words_per_row: usize,
    y_start: usize,
    y_end: usize,
    uf: &AtomicUnionFind,
    connectivity: Connectivity,
) -> StripResult {
    let strip_height = y_end - y_start;
    // Pre-allocate based on expected density (~2% foreground, ~1 run per 64 pixels)
    let expected_runs = (strip_height * width) / 64;

    let mut result = StripResult {
        runs: Vec::with_capacity(expected_runs),
        last_row_runs: Vec::new(),
        first_row_runs: Vec::new(),
    };

    let mut prev_runs: Vec<Run> = Vec::with_capacity(width / 4);
    let mut curr_runs: Vec<Run> = Vec::with_capacity(width / 4);

    for y in y_start..y_end {
        curr_runs.clear();
        extract_runs_from_row(
            mask_words,
            y * words_per_row,
            words_per_row,
            width,
            &mut curr_runs,
        );

        if curr_runs.is_empty() {
            prev_runs.clear();
            continue;
        }

        merge_runs_with_prev(
            &mut curr_runs,
            &prev_runs,
            connectivity,
            &mut AtomicUFRef(uf),
        );

        for run in &curr_runs {
            result.runs.push((y as u32, *run));
        }

        // Store boundary rows
        if y == y_start {
            result.first_row_runs = curr_runs.clone();
        }
        if y == y_end - 1 {
            result.last_row_runs = curr_runs.clone();
        }

        std::mem::swap(&mut prev_runs, &mut curr_runs);
    }

    result
}

/// Merge labels at strip boundary using O(n+m) sorted merge.
fn merge_strip_boundary_sorted(
    above_runs: &[Run],
    below_runs: &[Run],
    uf: &AtomicUnionFind,
    connectivity: Connectivity,
) {
    if above_runs.is_empty() || below_runs.is_empty() {
        return;
    }

    let mut above_idx = 0;
    let mut below_idx = 0;

    while above_idx < above_runs.len() && below_idx < below_runs.len() {
        let above = &above_runs[above_idx];
        let below = &below_runs[below_idx];

        let (above_search_start, above_search_end) = above.search_window(connectivity);
        let (below_search_start, below_search_end) = below.search_window(connectivity);

        if above_search_end <= below_search_start {
            above_idx += 1;
            continue;
        }
        if below_search_end <= above_search_start {
            below_idx += 1;
            continue;
        }

        // Check all above runs that could connect to this below run
        let mut check_above = above_idx;
        while check_above < above_runs.len() {
            let a = &above_runs[check_above];
            if a.start >= below_search_end {
                break;
            }
            if runs_connected(a, below, connectivity) && a.label != below.label {
                uf.union(a.label, below.label);
            }
            check_above += 1;
        }

        below_idx += 1;
    }
}

// ============================================================================
// Union-Find (sequential)
// ============================================================================

/// Sequential union-find for small images.
#[derive(Debug)]
struct UnionFind {
    parent: Vec<u32>,
    next_label: u32,
}

impl UnionFind {
    fn new() -> Self {
        Self {
            parent: Vec::with_capacity(256),
            next_label: 1,
        }
    }

    #[inline]
    fn make_set(&mut self) -> u32 {
        let label = self.next_label;
        self.parent.push(label);
        self.next_label += 1;
        label
    }

    /// Find root with iterative path compression (two-pass).
    #[inline]
    fn find(&mut self, label: u32) -> u32 {
        let idx = (label - 1) as usize;
        if idx >= self.parent.len() {
            return label;
        }

        // First pass: find root
        let mut root = label;
        loop {
            let root_idx = (root - 1) as usize;
            if root_idx >= self.parent.len() {
                break;
            }
            let parent = self.parent[root_idx];
            if parent == root || parent == 0 {
                break;
            }
            root = parent;
        }

        // Second pass: compress path
        let mut current = label;
        while current != root {
            let current_idx = (current - 1) as usize;
            if current_idx >= self.parent.len() {
                break;
            }
            let parent = self.parent[current_idx];
            if parent == current || parent == 0 {
                break;
            }
            self.parent[current_idx] = root;
            current = parent;
        }

        root
    }

    #[inline]
    fn union(&mut self, a: u32, b: u32) {
        let root_a = self.find(a);
        let root_b = self.find(b);
        if root_a != root_b {
            let (smaller, larger) = if root_a < root_b {
                (root_a, root_b)
            } else {
                (root_b, root_a)
            };
            self.parent[(larger - 1) as usize] = smaller;
        }
    }

    /// Flatten labels to sequential 1..n using single-pass approach.
    fn flatten_labels(&mut self, labels: &mut [u32]) -> usize {
        if self.parent.is_empty() {
            return 0;
        }

        let len = self.parent.len();
        let mut label_map = vec![0u32; len + 1];
        let mut num_labels = 0u32;

        // Single pass: find roots and assign sequential labels
        for i in 1..=len as u32 {
            let root = self.find(i);
            if label_map[root as usize] == 0 {
                num_labels += 1;
                label_map[root as usize] = num_labels;
            }
            label_map[i as usize] = label_map[root as usize];
        }

        // Apply mapping
        for l in labels.iter_mut() {
            if *l != 0 && (*l as usize) < label_map.len() {
                *l = label_map[*l as usize];
            }
        }

        num_labels as usize
    }
}

// ============================================================================
// Union-Find (atomic/parallel)
// ============================================================================

/// Lock-free atomic union-find for parallel labeling.
struct AtomicUnionFind {
    parent: Vec<AtomicU32>,
    next_label: AtomicU32,
}

impl std::fmt::Debug for AtomicUnionFind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtomicUnionFind")
            .field("len", &self.parent.len())
            .field("next_label", &self.next_label.load(Ordering::Relaxed))
            .finish()
    }
}

impl AtomicUnionFind {
    fn new(capacity: usize) -> Self {
        Self {
            parent: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            next_label: AtomicU32::new(1),
        }
    }

    #[inline]
    fn make_set(&self) -> u32 {
        // SeqCst: labels must be globally unique across threads.
        let label = self.next_label.fetch_add(1, Ordering::SeqCst);
        assert!(
            (label as usize) <= self.parent.len(),
            "AtomicUnionFind capacity exceeded: label {label} > capacity {}",
            self.parent.len()
        );
        self.parent[label as usize - 1].store(label, Ordering::SeqCst);
        label
    }

    #[inline]
    fn find(&self, label: u32) -> u32 {
        let mut current = label;
        loop {
            let idx = (current - 1) as usize;
            if idx >= self.parent.len() {
                return current;
            }
            // Relaxed: find is idempotent — stale reads just cause extra
            // iterations, union's CAS provides the synchronization.
            let parent = self.parent[idx].load(Ordering::Relaxed);
            if parent == current || parent == 0 {
                return current;
            }
            current = parent;
        }
    }

    fn union(&self, a: u32, b: u32) {
        let mut root_a = self.find(a);
        let mut root_b = self.find(b);

        while root_a != root_b {
            if root_a > root_b {
                std::mem::swap(&mut root_a, &mut root_b);
            }

            let idx_b = (root_b - 1) as usize;
            if idx_b >= self.parent.len() {
                break;
            }

            // AcqRel: acquire sees prior unions, release publishes this union.
            // Relaxed on failure: we re-find roots anyway.
            match self.parent[idx_b].compare_exchange_weak(
                root_b,
                root_a,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(current) => {
                    root_a = self.find(root_a);
                    root_b = self.find(current);
                }
            }
        }
    }

    #[inline]
    fn label_count(&self) -> usize {
        (self.next_label.load(Ordering::Relaxed) - 1) as usize
    }

    /// Build sequential label mapping using single-pass approach.
    fn build_label_map(&self, total_labels: usize) -> Vec<u32> {
        let mut label_map = vec![0u32; total_labels + 1];
        let mut num_labels = 0u32;

        for i in 1..=total_labels {
            let root = self.find(i as u32);
            if label_map[root as usize] == 0 {
                num_labels += 1;
                label_map[root as usize] = num_labels;
            }
            label_map[i] = label_map[root as usize];
        }

        label_map
    }
}
