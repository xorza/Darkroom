//! Connected component labeling using union-find.
//!
//! Optimized for sparse binary masks (typical in star detection):
//! - Run-length encoding (RLE) based labeling for efficient processing
//! - Word-level bit scanning to skip background regions
//! - Block-based parallel labeling with boundary merging
//! - Lock-free union-find with atomic operations

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU32, Ordering};

use rayon::prelude::*;

use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::config::Connectivity;

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
#[cfg_attr(test, allow(dead_code))]
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
///
/// Uses trailing zero counting to jump directly to bit transitions instead
/// of checking each bit individually. This is significantly faster for
/// words with few transitions.
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

    // If we're in a run, we need to find where it ends (first 0 bit)
    // If we're not in a run, we need to find where one starts (first 1 bit)
    //
    // Strategy: XOR with mask to find transitions, then use CTZ to jump
    // to each transition point.

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
                // All remaining bits are 1s - run continues past this word
                break;
            }
            // Invert to find first 0 (which becomes first 1 after invert)
            let inverted = !remaining_bits;
            let zeros_until_end = inverted.trailing_zeros();
            let end_pos = pos + zeros_until_end;

            if end_pos >= word_end {
                // Run extends past this word
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
                // No more 1 bits in this word
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
///
/// Wraps a `Buffer2<u32>` where each pixel contains the label of its
/// connected component (0 for background, 1..=num_labels for components).
#[derive(Debug)]
pub struct LabelMap {
    labels: Buffer2<u32>,
    num_labels: usize,
}

impl LabelMap {
    // ========================================================================
    // Construction
    // ========================================================================

    /// Create a label map from a binary mask using connected component labeling.
    ///
    /// Uses 4-connectivity (only horizontal/vertical neighbors).
    /// For 8-connectivity, use `from_mask_with_connectivity`.
    ///
    /// Uses block-based parallel algorithm for large images:
    /// 1. Divide image into horizontal strips
    /// 2. Label each strip in parallel using word-level bit scanning
    /// 3. Merge labels at strip boundaries using atomic union-find
    /// 4. Flatten labels in parallel
    pub fn from_mask(mask: &BitBuffer2) -> Self {
        Self::from_mask_with_connectivity(mask, Connectivity::Four)
    }

    /// Create a label map from a binary mask with specified connectivity.
    ///
    /// # Arguments
    /// * `mask` - Binary mask of foreground pixels
    /// * `connectivity` - Four (default) or Eight connectivity
    pub fn from_mask_with_connectivity(mask: &BitBuffer2, connectivity: Connectivity) -> Self {
        let width = mask.width();
        let height = mask.height();

        let mut labels = Buffer2::new_filled(width, height, 0u32);

        if width == 0 || height == 0 {
            return Self {
                labels,
                num_labels: 0,
            };
        }

        // Threshold determined by benchmark: parallel wins at ~65k pixels
        let num_labels = if width * height < 65_000 {
            label_mask_sequential(mask, &mut labels, connectivity)
        } else {
            label_mask_parallel(mask, &mut labels, connectivity)
        };

        Self { labels, num_labels }
    }

    /// Create a label map from pre-computed labels (for testing).
    #[cfg(test)]
    pub(crate) fn from_raw(labels: Buffer2<u32>, num_labels: usize) -> Self {
        Self { labels, num_labels }
    }

    // ========================================================================
    // Accessors
    // ========================================================================

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
// Sequential labeling (small images)
// ============================================================================

/// Sequential RLE-based CCL for small images.
#[cfg_attr(test, allow(dead_code))]
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

        // Label runs and merge with overlapping runs from previous row
        let mut prev_idx = 0;
        for run in &mut curr_runs {
            let (search_start, search_end) = run.search_window(connectivity);

            // Skip runs that end before our search window
            while prev_idx < prev_runs.len() && prev_runs[prev_idx].end <= search_start {
                prev_idx += 1;
            }

            // Find all overlapping runs and merge their labels
            let mut assigned_label = None;
            let mut check_idx = prev_idx;
            while check_idx < prev_runs.len() && prev_runs[check_idx].start < search_end {
                let prev_run = &prev_runs[check_idx];
                if runs_connected(prev_run, run, connectivity) {
                    match assigned_label {
                        Some(label) if label != prev_run.label => {
                            uf.union(label, prev_run.label);
                        }
                        None => assigned_label = Some(prev_run.label),
                        _ => {}
                    }
                }
                check_idx += 1;
            }

            run.label = assigned_label.unwrap_or_else(|| uf.make_set());

            // Write labels to output
            let row_start = y * width;
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

/// Parallel RLE-based CCL for large images.
#[cfg_attr(test, allow(dead_code))]
pub(super) fn label_mask_parallel(
    mask: &BitBuffer2,
    labels: &mut Buffer2<u32>,
    connectivity: Connectivity,
) -> usize {
    let width = mask.width();
    let height = mask.height();
    let words_per_row = mask.words_per_row();
    let mask_words = mask.words();

    // Strip configuration: min 64 rows per strip, max num_threads strips
    let num_threads = rayon::current_num_threads();
    let num_strips = (height / 64).clamp(1, num_threads);
    let rows_per_strip = height / num_strips;

    // Atomic union-find for parallel merging (5% of pixels as upper bound)
    let uf = AtomicUnionFind::new(((width * height) / 20).max(1024));

    // Phase 1: Label each strip in parallel
    let strip_runs: Vec<Vec<(usize, Run)>> = (0..num_strips)
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

    // Phase 2: Merge labels at strip boundaries
    for strip_idx in 1..num_strips {
        let boundary_y = strip_idx * rows_per_strip;
        merge_strip_boundary(
            &strip_runs[strip_idx - 1],
            &strip_runs[strip_idx],
            boundary_y,
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

    // Phase 4: Write labels in parallel
    let all_runs: Vec<(usize, Run)> = strip_runs.into_iter().flatten().collect();
    let labels_ptr = labels.pixels_mut().as_mut_ptr() as usize;

    all_runs.par_iter().for_each(|&(y, run)| {
        let row_start = y * width;
        let final_label = label_map.get(run.label as usize).copied().unwrap_or(0);
        // SAFETY: Each run writes to disjoint pixels
        let ptr = labels_ptr as *mut u32;
        for x in run.start..run.end {
            unsafe {
                *ptr.add(row_start + x as usize) = final_label;
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

/// Label a single strip and return runs with row indices.
fn label_strip(
    mask_words: &[u64],
    width: usize,
    words_per_row: usize,
    y_start: usize,
    y_end: usize,
    uf: &AtomicUnionFind,
    connectivity: Connectivity,
) -> Vec<(usize, Run)> {
    let mut all_runs = Vec::new();
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

        let mut prev_idx = 0;
        for run in &mut curr_runs {
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
            all_runs.push((y, *run));
        }

        std::mem::swap(&mut prev_runs, &mut curr_runs);
    }

    all_runs
}

/// Merge labels at strip boundary.
fn merge_strip_boundary(
    above_runs: &[(usize, Run)],
    below_runs: &[(usize, Run)],
    boundary_y: usize,
    uf: &AtomicUnionFind,
    connectivity: Connectivity,
) {
    let above: Vec<_> = above_runs
        .iter()
        .filter(|(y, _)| *y == boundary_y - 1)
        .map(|(_, r)| r)
        .collect();
    let below: Vec<_> = below_runs
        .iter()
        .filter(|(y, _)| *y == boundary_y)
        .map(|(_, r)| r)
        .collect();

    for a in &above {
        for b in &below {
            if runs_connected(a, b, connectivity) && a.label != b.label {
                uf.union(a.label, b.label);
            }
        }
    }
}

// ============================================================================
// Union-Find (sequential)
// ============================================================================

/// Sequential union-find for small images.
struct UnionFind {
    parent: Vec<u32>,
    next_label: u32,
}

impl UnionFind {
    fn new() -> Self {
        Self {
            parent: Vec::new(),
            next_label: 1,
        }
    }

    /// Create a new set and return its label.
    fn make_set(&mut self) -> u32 {
        let label = self.next_label;
        self.parent.push(label);
        self.next_label += 1;
        label
    }

    /// Find root with path compression.
    fn find(&mut self, label: u32) -> u32 {
        let idx = (label - 1) as usize;
        if idx >= self.parent.len() {
            return label;
        }
        if self.parent[idx] != label && self.parent[idx] != 0 {
            self.parent[idx] = self.find(self.parent[idx]);
        }
        if self.parent[idx] == 0 {
            label
        } else {
            self.parent[idx]
        }
    }

    /// Union two sets (smaller root wins).
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

    /// Flatten labels to sequential 1..n and apply to buffer.
    fn flatten_labels(&mut self, labels: &mut [u32]) -> usize {
        if self.parent.is_empty() {
            return 0;
        }

        // Map roots to sequential labels
        let mut root_to_final = vec![0u32; self.parent.len() + 1];
        let mut num_labels = 0u32;
        for label in 1..=self.parent.len() as u32 {
            let root = self.find(label);
            if root_to_final[root as usize] == 0 {
                num_labels += 1;
                root_to_final[root as usize] = num_labels;
            }
        }

        // Build label map
        let mut label_map = vec![0u32; self.parent.len() + 1];
        for (i, &root) in self.parent.iter().enumerate() {
            label_map[i + 1] = root_to_final[root as usize];
        }

        // Apply mapping
        for l in labels.iter_mut() {
            if *l != 0 {
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

impl AtomicUnionFind {
    fn new(capacity: usize) -> Self {
        Self {
            parent: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            next_label: AtomicU32::new(1),
        }
    }

    /// Create a new set and return its label.
    fn make_set(&self) -> u32 {
        let label = self.next_label.fetch_add(1, Ordering::SeqCst);
        if (label as usize) <= self.parent.len() {
            self.parent[label as usize - 1].store(label, Ordering::SeqCst);
        }
        label
    }

    /// Find root (read-only, no path compression).
    fn find(&self, label: u32) -> u32 {
        let mut current = label;
        loop {
            let idx = (current - 1) as usize;
            if idx >= self.parent.len() {
                return current;
            }
            let p = self.parent[idx].load(Ordering::Relaxed);
            if p == current || p == 0 {
                return current;
            }
            current = p;
        }
    }

    /// Union two sets using lock-free CAS.
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

    fn label_count(&self) -> usize {
        (self.next_label.load(Ordering::Relaxed) - 1) as usize
    }

    /// Build sequential label mapping.
    fn build_label_map(&self, total_labels: usize) -> Vec<u32> {
        let mut label_map = vec![0u32; total_labels + 1];
        let mut num_labels = 0u32;

        // First pass: assign sequential labels to roots
        for i in 0..total_labels {
            let root = self.find((i + 1) as u32);
            if label_map[root as usize] == 0 {
                num_labels += 1;
                label_map[root as usize] = num_labels;
            }
        }

        // Second pass: map each label to its root's final label
        for i in (1..=total_labels).rev() {
            let root = self.find(i as u32);
            label_map[i] = label_map[root as usize];
        }

        label_map
    }
}
