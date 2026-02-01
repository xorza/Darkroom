//! Connected component labeling using union-find.
//!
//! Optimized for sparse binary masks (typical in star detection):
//! - Word-level bit scanning using trailing_zeros() to skip background
//! - Block-based parallel labeling with boundary merging
//! - Lock-free union-find with atomic operations

use std::sync::atomic::{AtomicU32, Ordering};

use rayon::iter::ParallelIterator;

use crate::common::{BitBuffer2, Buffer2};
use common::parallel;

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
    /// Uses block-based parallel algorithm for large images:
    /// 1. Divide image into horizontal strips
    /// 2. Label each strip in parallel using word-level bit scanning
    /// 3. Merge labels at strip boundaries using atomic union-find
    /// 4. Flatten labels in parallel
    pub fn from_mask(mask: &BitBuffer2) -> Self {
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
            label_mask_sequential(mask, &mut labels)
        } else {
            label_mask_parallel(mask, &mut labels)
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

/// Sequential algorithm for small images.
fn label_mask_sequential(mask: &BitBuffer2, labels: &mut Buffer2<u32>) -> usize {
    let width = mask.width();
    let height = mask.height();
    let words_per_row = mask.words_per_row();
    let mask_words = mask.words();

    let mut parent: Vec<u32> = Vec::new();
    let mut next_label = 1u32;

    for y in 0..height {
        let row_start = y * width;
        let word_row_start = y * words_per_row;

        for word_idx in 0..words_per_row {
            let mut word = mask_words[word_row_start + word_idx];
            let base_x = word_idx * 64;

            if word == 0 {
                continue;
            }

            // Process set bits using trailing_zeros
            while word != 0 {
                let bit_pos = word.trailing_zeros() as usize;
                let x = base_x + bit_pos;

                if x >= width {
                    break;
                }

                let idx = row_start + x;

                // Check neighbors: left and top
                let left = if x > 0 && mask.get(idx - 1) {
                    Some(labels[idx - 1])
                } else {
                    None
                };
                let top = if y > 0 && mask.get(idx - width) {
                    Some(labels[idx - width])
                } else {
                    None
                };

                labels[idx] = match (left, top) {
                    (None, None) => {
                        parent.push(next_label);
                        next_label += 1;
                        next_label - 1
                    }
                    (Some(l), None) => l,
                    (None, Some(t)) => t,
                    (Some(l), Some(t)) => {
                        if l != t {
                            union(&mut parent, l, t);
                        }
                        l.min(t)
                    }
                };

                word &= word - 1;
            }
        }
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

/// Parallel algorithm for large images using block-based CCL.
fn label_mask_parallel(mask: &BitBuffer2, labels: &mut Buffer2<u32>) -> usize {
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

    // Phase 1: Label each strip in parallel
    let labels_ptr = labels.pixels_mut().as_mut_ptr();
    // SAFETY: Each strip writes to disjoint rows
    let labels_slice = unsafe { std::slice::from_raw_parts_mut(labels_ptr, width * height) };

    rayon::scope(|s| {
        for strip_idx in 0..num_strips {
            let y_start = strip_idx * rows_per_strip;
            let y_end = if strip_idx == num_strips - 1 {
                height
            } else {
                (strip_idx + 1) * rows_per_strip
            };

            let parent_ref = &parent;
            let next_label_ref = &next_label;

            let strip_start = y_start * width;
            let strip_len = (y_end - y_start) * width;
            let strip_labels = unsafe {
                std::slice::from_raw_parts_mut(
                    labels_slice.as_mut_ptr().add(strip_start),
                    strip_len,
                )
            };

            s.spawn(move |_| {
                label_strip(
                    mask_words,
                    strip_labels,
                    width,
                    words_per_row,
                    y_start,
                    y_end,
                    parent_ref,
                    next_label_ref,
                    mask,
                );
            });
        }
    });

    // Phase 2: Merge labels at strip boundaries
    for strip_idx in 1..num_strips {
        merge_strip_boundary(mask, labels, width, strip_idx * rows_per_strip, &parent);
    }

    // Get final label count
    let total_labels = (next_label.load(Ordering::Relaxed) - 1) as usize;
    if total_labels == 0 {
        return 0;
    }

    // Phase 3: Build label mapping (reuses single allocation)
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

    // Phase 4: Apply label mapping in parallel
    parallel::par_chunks_auto(labels.pixels_mut()).for_each(|(_, chunk)| {
        for label in chunk {
            let l = *label as usize;
            if l != 0 && l < label_map.len() {
                *label = label_map[l];
            }
        }
    });

    num_labels as usize
}

/// Label a horizontal strip using word-level bit scanning.
#[allow(clippy::too_many_arguments)]
fn label_strip(
    mask_words: &[u64],
    strip_labels: &mut [u32],
    width: usize,
    words_per_row: usize,
    y_start: usize,
    y_end: usize,
    parent: &[AtomicU32],
    next_label: &AtomicU32,
    mask: &BitBuffer2,
) {
    for y in y_start..y_end {
        let local_y = y - y_start;
        let row_start = local_y * width;
        let word_row_start = y * words_per_row;

        for word_idx in 0..words_per_row {
            let mut word = mask_words[word_row_start + word_idx];
            let base_x = word_idx * 64;

            if word == 0 {
                continue;
            }

            while word != 0 {
                let bit_pos = word.trailing_zeros() as usize;
                let x = base_x + bit_pos;

                if x >= width {
                    break;
                }

                let local_idx = row_start + x;
                let global_idx = y * width + x;

                // Check left neighbor (within strip)
                let left = if x > 0 && mask.get(global_idx - 1) {
                    Some(strip_labels[local_idx - 1])
                } else {
                    None
                };

                // Check top neighbor (within strip only - cross-strip handled in merge)
                let top = if local_y > 0 && mask.get(global_idx - width) {
                    Some(strip_labels[local_idx - width])
                } else {
                    None
                };

                strip_labels[local_idx] = match (left, top) {
                    (None, None) => {
                        let label = next_label.fetch_add(1, Ordering::SeqCst);
                        if (label as usize) < parent.len() {
                            parent[label as usize - 1].store(label, Ordering::SeqCst);
                        }
                        label
                    }
                    (Some(l), None) => l,
                    (None, Some(t)) => t,
                    (Some(l), Some(t)) => {
                        if l != t {
                            atomic_union(parent, l, t);
                        }
                        l.min(t)
                    }
                };

                word &= word - 1;
            }
        }
    }
}

/// Merge labels at strip boundary.
fn merge_strip_boundary(
    mask: &BitBuffer2,
    labels: &Buffer2<u32>,
    width: usize,
    boundary_y: usize,
    parent: &[AtomicU32],
) {
    let above_row_start = (boundary_y - 1) * width;
    let below_row_start = boundary_y * width;

    for x in 0..width {
        let above_idx = above_row_start + x;
        let below_idx = below_row_start + x;

        if mask.get(above_idx) && mask.get(below_idx) {
            let above_label = labels[above_idx];
            let below_label = labels[below_idx];

            if above_label != 0 && below_label != 0 && above_label != below_label {
                atomic_union(parent, above_label, below_label);
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
