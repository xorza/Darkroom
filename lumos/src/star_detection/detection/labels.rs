//! Connected component labeling using union-find.
//!
//! Optimized for sparse binary masks (typical in star detection):
//! - Run-length encoding (RLE) based labeling for efficient processing
//! - Word-level bit scanning using trailing_zeros() to skip background
//! - Block-based parallel labeling with boundary merging
//! - Lock-free union-find with atomic operations

use std::sync::atomic::{AtomicU32, Ordering};

use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::config::Connectivity;

// ============================================================================
// Run-Length Encoding
// ============================================================================

/// A horizontal run of foreground pixels.
#[derive(Debug, Clone, Copy)]
struct Run {
    /// Starting x coordinate (inclusive).
    start: u16,
    /// Ending x coordinate (exclusive).
    end: u16,
    /// Provisional label assigned to this run.
    label: u32,
}

/// Check if two runs overlap vertically (are connected).
///
/// For 4-connectivity: runs must share at least one x coordinate.
/// For 8-connectivity: runs can be diagonally adjacent (off by one).
#[inline]
fn runs_overlap(prev: &Run, curr: &Run, connectivity: Connectivity) -> bool {
    match connectivity {
        Connectivity::Four => {
            // 4-conn: runs overlap if they share any x coordinate
            prev.start < curr.end && prev.end > curr.start
        }
        Connectivity::Eight => {
            // 8-conn: runs overlap if they touch (including diagonally)
            // Diagonal adjacency means prev.end == curr.start or prev.start == curr.end
            prev.start < curr.end + 1 && prev.end + 1 > curr.start
        }
    }
}

/// Extract runs from a single row of the mask using word-level bit scanning.
#[inline]
fn extract_runs_from_row(
    mask_words: &[u64],
    word_row_start: usize,
    words_per_row: usize,
    width: usize,
    runs: &mut Vec<Run>,
) {
    let mut in_run = false;
    let mut run_start = 0u16;

    for word_idx in 0..words_per_row {
        let word = mask_words[word_row_start + word_idx];
        let base_x = word_idx * 64;

        if word == 0 {
            // All zeros - close any open run
            if in_run {
                let end = (base_x as u16).min(width as u16);
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
                run_start = base_x as u16;
                in_run = true;
            }
            continue;
        }

        // Mixed word - process bit by bit
        for bit in 0..64 {
            let x = base_x + bit;
            if x >= width {
                break;
            }

            let is_set = (word >> bit) & 1 != 0;
            if is_set && !in_run {
                run_start = x as u16;
                in_run = true;
            } else if !is_set && in_run {
                runs.push(Run {
                    start: run_start,
                    end: x as u16,
                    label: 0,
                });
                in_run = false;
            }
        }
    }

    // Close final run if still open
    if in_run {
        runs.push(Run {
            start: run_start,
            end: width as u16,
            label: 0,
        });
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

        let num_labels = if width * height < 100_000 {
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

/// Sequential RLE-based algorithm for small images.
///
/// Uses run-length encoding for efficient processing:
/// 1. Extract runs from each row
/// 2. Label runs and merge with overlapping runs from previous row
/// 3. Write labels to output buffer
fn label_mask_sequential(
    mask: &BitBuffer2,
    labels: &mut Buffer2<u32>,
    connectivity: Connectivity,
) -> usize {
    let width = mask.width();
    let height = mask.height();
    let words_per_row = mask.words_per_row();
    let mask_words = mask.words();

    let mut parent: Vec<u32> = Vec::new();
    let mut next_label = 1u32;

    // Storage for runs from current and previous rows
    let mut prev_runs: Vec<Run> = Vec::with_capacity(width / 4);
    let mut curr_runs: Vec<Run> = Vec::with_capacity(width / 4);

    for y in 0..height {
        let word_row_start = y * words_per_row;

        // Extract runs from current row
        curr_runs.clear();
        extract_runs_from_row(
            mask_words,
            word_row_start,
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
            // Find overlapping runs from previous row
            let mut assigned_label = None;

            // For 8-connectivity, we need to check one position earlier
            let search_start = if connectivity == Connectivity::Eight && run.start > 0 {
                run.start - 1
            } else {
                run.start
            };

            while prev_idx < prev_runs.len() && prev_runs[prev_idx].end <= search_start {
                prev_idx += 1;
            }

            // For 8-connectivity, check one position further
            let search_end = if connectivity == Connectivity::Eight {
                run.end + 1
            } else {
                run.end
            };

            let mut check_idx = prev_idx;
            while check_idx < prev_runs.len() && prev_runs[check_idx].start < search_end {
                let prev_run = &prev_runs[check_idx];
                if runs_overlap(prev_run, run, connectivity) {
                    if let Some(label) = assigned_label {
                        // Already have a label - union with overlapping run
                        if label != prev_run.label {
                            union(&mut parent, label, prev_run.label);
                        }
                    } else {
                        // First overlap - take its label
                        assigned_label = Some(prev_run.label);
                    }
                }
                check_idx += 1;
            }

            // Assign label
            run.label = match assigned_label {
                Some(label) => label,
                None => {
                    // New component
                    parent.push(next_label);
                    let label = next_label;
                    next_label += 1;
                    label
                }
            };

            // Write labels to output buffer
            let row_start = y * width;
            for x in run.start..run.end {
                labels[row_start + x as usize] = run.label;
            }
        }

        // Swap current and previous runs
        std::mem::swap(&mut prev_runs, &mut curr_runs);
    }

    if parent.is_empty() {
        return 0;
    }

    let (label_map, num_labels) = build_label_map(&mut parent);

    // Apply label mapping
    for label in labels.pixels_mut().iter_mut() {
        if *label != 0 {
            *label = label_map[*label as usize];
        }
    }

    num_labels
}

// ============================================================================
// Parallel labeling (large images)
// ============================================================================

/// Parallel RLE-based algorithm for large images.
///
/// Uses run-length encoding for efficient processing:
/// 1. Extract runs from each strip in parallel
/// 2. Label runs within each strip
/// 3. Merge labels at strip boundaries
/// 4. Write labels to output buffer in parallel
fn label_mask_parallel(
    mask: &BitBuffer2,
    labels: &mut Buffer2<u32>,
    connectivity: Connectivity,
) -> usize {
    let width = mask.width();
    let height = mask.height();
    let words_per_row = mask.words_per_row();
    let mask_words = mask.words();

    // Determine strip configuration
    let num_threads = rayon::current_num_threads();
    let min_rows_per_strip = 64;
    let num_strips = (height / min_rows_per_strip).clamp(1, num_threads);
    let rows_per_strip = height / num_strips;

    // Estimate max labels: sparse masks typical in star detection
    // Use 5% of pixels as upper bound, minimum 1024 for small images
    let max_labels = ((width * height) / 20).max(1024);

    // Atomic parent array for lock-free union-find
    let parent: Vec<AtomicU32> = (0..max_labels).map(|_| AtomicU32::new(0)).collect();
    let next_label = AtomicU32::new(1);

    // Phase 1: Extract runs and label each strip in parallel
    // Each strip stores its runs with row indices for boundary merging
    use rayon::prelude::*;
    let strip_runs: Vec<Vec<(usize, Run)>> = (0..num_strips)
        .into_par_iter()
        .map(|strip_idx| {
            let y_start = strip_idx * rows_per_strip;
            let y_end = if strip_idx == num_strips - 1 {
                height
            } else {
                (strip_idx + 1) * rows_per_strip
            };

            label_strip_rle(
                mask_words,
                width,
                words_per_row,
                y_start,
                y_end,
                &parent,
                &next_label,
                connectivity,
            )
        })
        .collect();

    // Phase 2: Merge labels at strip boundaries using runs
    for strip_idx in 1..num_strips {
        let boundary_y = strip_idx * rows_per_strip;
        merge_strip_boundary_rle(
            &strip_runs[strip_idx - 1],
            &strip_runs[strip_idx],
            boundary_y,
            &parent,
            connectivity,
        );
    }

    // Get final label count
    let total_labels = (next_label.load(Ordering::Relaxed) - 1) as usize;
    if total_labels == 0 {
        return 0;
    }

    // Phase 3: Build label mapping
    let mut label_map = vec![0u32; total_labels + 1];

    // First pass: find roots and assign sequential labels
    let mut num_labels = 0u32;
    for i in 0..total_labels {
        let root = atomic_find_readonly(&parent, (i + 1) as u32);
        if label_map[root as usize] == 0 {
            num_labels += 1;
            label_map[root as usize] = num_labels;
        }
    }

    // Second pass: map each label to its root's final label
    for i in (1..=total_labels).rev() {
        let root = atomic_find_readonly(&parent, i as u32);
        label_map[i] = label_map[root as usize];
    }

    // Phase 4: Write labels to output buffer in parallel
    // Flatten all runs and write in parallel chunks
    let all_runs: Vec<(usize, Run)> = strip_runs.into_iter().flatten().collect();

    // SAFETY: Each run writes to disjoint pixels, and we use atomic operations
    // to ensure thread safety. We convert the pointer to usize to satisfy Send.
    let labels_ptr = labels.pixels_mut().as_mut_ptr() as usize;

    all_runs.par_iter().for_each(|&(y, run)| {
        let row_start = y * width;
        let final_label = if (run.label as usize) < label_map.len() {
            label_map[run.label as usize]
        } else {
            0
        };

        // SAFETY: Each run writes to disjoint pixels
        let ptr = labels_ptr as *mut u32;
        for x in run.start..run.end {
            unsafe {
                *ptr.add(row_start + x as usize) = final_label;
            }
        }
    });

    num_labels as usize
}

/// Label a strip using RLE and return runs with row indices.
#[allow(clippy::too_many_arguments)]
fn label_strip_rle(
    mask_words: &[u64],
    width: usize,
    words_per_row: usize,
    y_start: usize,
    y_end: usize,
    parent: &[AtomicU32],
    next_label: &AtomicU32,
    connectivity: Connectivity,
) -> Vec<(usize, Run)> {
    let mut all_runs: Vec<(usize, Run)> = Vec::new();
    let mut prev_runs: Vec<Run> = Vec::with_capacity(width / 4);
    let mut curr_runs: Vec<Run> = Vec::with_capacity(width / 4);

    for y in y_start..y_end {
        let word_row_start = y * words_per_row;

        // Extract runs from current row
        curr_runs.clear();
        extract_runs_from_row(
            mask_words,
            word_row_start,
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
            let mut assigned_label = None;

            // For 8-connectivity, we need to check one position earlier
            let search_start = if connectivity == Connectivity::Eight && run.start > 0 {
                run.start - 1
            } else {
                run.start
            };

            // Skip runs that end before search region starts
            while prev_idx < prev_runs.len() && prev_runs[prev_idx].end <= search_start {
                prev_idx += 1;
            }

            // For 8-connectivity, check one position further
            let search_end = if connectivity == Connectivity::Eight {
                run.end + 1
            } else {
                run.end
            };

            // Check all overlapping runs from previous row
            let mut check_idx = prev_idx;
            while check_idx < prev_runs.len() && prev_runs[check_idx].start < search_end {
                let prev_run = &prev_runs[check_idx];
                if runs_overlap(prev_run, run, connectivity) {
                    if let Some(label) = assigned_label {
                        if label != prev_run.label {
                            atomic_union(parent, label, prev_run.label);
                        }
                    } else {
                        assigned_label = Some(prev_run.label);
                    }
                }
                check_idx += 1;
            }

            // Assign label
            run.label = match assigned_label {
                Some(label) => label,
                None => {
                    let label = next_label.fetch_add(1, Ordering::SeqCst);
                    if (label as usize) < parent.len() {
                        parent[label as usize - 1].store(label, Ordering::SeqCst);
                    }
                    label
                }
            };

            // Store run with row index
            all_runs.push((y, *run));
        }

        std::mem::swap(&mut prev_runs, &mut curr_runs);
    }

    all_runs
}

/// Merge labels at strip boundary using runs.
fn merge_strip_boundary_rle(
    above_runs: &[(usize, Run)],
    below_runs: &[(usize, Run)],
    boundary_y: usize,
    parent: &[AtomicU32],
    connectivity: Connectivity,
) {
    // Find runs from the row just above the boundary
    let above_boundary_runs: Vec<&Run> = above_runs
        .iter()
        .filter(|(y, _)| *y == boundary_y - 1)
        .map(|(_, run)| run)
        .collect();

    // Find runs from the row at the boundary
    let below_boundary_runs: Vec<&Run> = below_runs
        .iter()
        .filter(|(y, _)| *y == boundary_y)
        .map(|(_, run)| run)
        .collect();

    // Merge overlapping runs
    for above_run in &above_boundary_runs {
        for below_run in &below_boundary_runs {
            // Check if runs overlap and have different labels
            if runs_overlap(above_run, below_run, connectivity)
                && above_run.label != below_run.label
            {
                atomic_union(parent, above_run.label, below_run.label);
            }
        }
    }
}

// ============================================================================
// Union-Find (sequential)
// ============================================================================

/// Build a mapping from provisional labels to final sequential labels.
fn build_label_map(parent: &mut [u32]) -> (Vec<u32>, usize) {
    let mut root_to_final = vec![0u32; parent.len() + 1];
    let mut num_labels = 0usize;

    for label in 1..=parent.len() as u32 {
        let root = find(parent, label);
        if root_to_final[root as usize] == 0 {
            num_labels += 1;
            root_to_final[root as usize] = num_labels as u32;
        }
    }

    let mut label_map = vec![0u32; parent.len() + 1];
    for (i, &root) in parent.iter().enumerate() {
        label_map[i + 1] = root_to_final[root as usize];
    }

    (label_map, num_labels)
}

/// Find root with path compression.
fn find(parent: &mut [u32], label: u32) -> u32 {
    let idx = (label - 1) as usize;
    if idx >= parent.len() {
        return label;
    }
    if parent[idx] != label && parent[idx] != 0 {
        parent[idx] = find(parent, parent[idx]);
    }
    if parent[idx] == 0 { label } else { parent[idx] }
}

/// Union two labels.
fn union(parent: &mut [u32], a: u32, b: u32) {
    let root_a = find(parent, a);
    let root_b = find(parent, b);
    if root_a != root_b {
        let (smaller, larger) = if root_a < root_b {
            (root_a, root_b)
        } else {
            (root_b, root_a)
        };
        parent[(larger - 1) as usize] = smaller;
    }
}

// ============================================================================
// Union-Find (atomic/parallel)
// ============================================================================

/// Atomic find with path compression (used during labeling).
fn atomic_find(parent: &[AtomicU32], label: u32) -> u32 {
    let mut current = label;
    loop {
        let idx = (current - 1) as usize;
        if idx >= parent.len() {
            return current;
        }
        let p = parent[idx].load(Ordering::Relaxed);
        if p == current || p == 0 {
            return current;
        }
        current = p;
    }
}

/// Read-only find for final label mapping (no writes needed).
#[inline]
fn atomic_find_readonly(parent: &[AtomicU32], label: u32) -> u32 {
    let mut current = label;
    loop {
        let idx = (current - 1) as usize;
        if idx >= parent.len() {
            return current;
        }
        let p = parent[idx].load(Ordering::Relaxed);
        if p == current || p == 0 {
            return current;
        }
        current = p;
    }
}

/// Atomic union with lock-free CAS.
fn atomic_union(parent: &[AtomicU32], a: u32, b: u32) {
    let mut root_a = atomic_find(parent, a);
    let mut root_b = atomic_find(parent, b);

    while root_a != root_b {
        // Make larger root point to smaller
        if root_a > root_b {
            std::mem::swap(&mut root_a, &mut root_b);
        }

        let idx_b = (root_b - 1) as usize;
        if idx_b >= parent.len() {
            break;
        }

        match parent[idx_b].compare_exchange_weak(
            root_b,
            root_a,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(current) => {
                root_a = atomic_find(parent, root_a);
                root_b = atomic_find(parent, current);
            }
        }
    }
}
