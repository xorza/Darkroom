//! Connected component labeling using union-find.
//!
//! Optimized for sparse binary masks (typical in star detection):
//! - Word-level bit scanning using trailing_zeros() to skip background
//! - Block-based parallel labeling with boundary merging
//! - Lock-free union-find with atomic operations

use crate::common::{BitBuffer2, Buffer2};
use common::parallel;
use rayon::iter::ParallelIterator;
use std::sync::atomic::{AtomicU32, Ordering};

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
    /// Create a label map from pre-computed labels (for testing).
    #[cfg(test)]
    pub(crate) fn from_raw(labels: Buffer2<u32>, num_labels: usize) -> Self {
        Self { labels, num_labels }
    }

    /// Create a label map from a binary mask using connected component labeling.
    ///
    /// Uses block-based parallel algorithm:
    /// 1. Divide image into horizontal strips
    /// 2. Label each strip in parallel using word-level bit scanning
    /// 3. Merge labels at strip boundaries using atomic union-find
    /// 4. Flatten labels in parallel
    pub fn from_mask(mask: &BitBuffer2) -> Self {
        let width = mask.width();
        let height = mask.height();

        if width == 0 || height == 0 {
            return Self {
                labels: Buffer2::new_filled(width, height, 0u32),
                num_labels: 0,
            };
        }

        // For small images, use sequential algorithm
        if width * height < 100_000 {
            return Self::from_mask_sequential(mask);
        }

        Self::from_mask_parallel(mask)
    }

    /// Sequential algorithm for small images.
    fn from_mask_sequential(mask: &BitBuffer2) -> Self {
        let width = mask.width();
        let height = mask.height();
        let words_per_row = mask.words_per_row();

        let mut labels = Buffer2::new_filled(width, height, 0u32);
        let mut parent: Vec<u32> = Vec::new();
        let mut next_label = 1u32;

        let mask_words = mask.words();

        for y in 0..height {
            let row_start = y * width;
            let word_row_start = y * words_per_row;

            // Process each word in the row
            for word_idx in 0..words_per_row {
                let mut word = mask_words[word_row_start + word_idx];
                let base_x = word_idx * 64;

                // Skip empty words
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
                                Self::union(&mut parent, l, t);
                            }
                            l.min(t)
                        }
                    };

                    // Clear this bit and continue
                    word &= word - 1;
                }
            }
        }

        if parent.is_empty() {
            return Self {
                labels,
                num_labels: 0,
            };
        }

        let (label_map, num_labels) = Self::build_label_map(&mut parent);

        // Apply label mapping
        for label in labels.pixels_mut().iter_mut() {
            if *label != 0 {
                *label = label_map[*label as usize];
            }
        }

        Self { labels, num_labels }
    }

    /// Parallel algorithm for large images using block-based CCL.
    fn from_mask_parallel(mask: &BitBuffer2) -> Self {
        let width = mask.width();
        let height = mask.height();
        let words_per_row = mask.words_per_row();

        // Determine number of strips (one per thread, minimum 64 rows per strip)
        let num_threads = rayon::current_num_threads();
        let min_rows_per_strip = 64;
        let num_strips = (height / min_rows_per_strip).clamp(1, num_threads);
        let rows_per_strip = height / num_strips;

        // Pre-allocate labels buffer
        let mut labels = Buffer2::new_filled(width, height, 0u32);

        // Estimate max labels: worst case is checkerboard pattern = pixels/2
        // But for star detection, it's much sparser. Use 10% of foreground estimate.
        let estimated_foreground = (width * height) / 10;
        let max_labels = estimated_foreground.max(1024);

        // Atomic parent array for lock-free union-find
        let parent: Vec<AtomicU32> = (0..max_labels).map(|_| AtomicU32::new(0)).collect();
        let next_label = AtomicU32::new(1);

        let mask_words = mask.words();

        // Phase 1: Label each strip in parallel
        {
            let labels_ptr = labels.pixels_mut().as_mut_ptr();

            // SAFETY: Each strip writes to disjoint rows
            let labels_slice =
                unsafe { std::slice::from_raw_parts_mut(labels_ptr, width * height) };

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

                    // Get mutable slice for this strip
                    let strip_start = y_start * width;
                    let strip_end = y_end * width;
                    let strip_labels = unsafe {
                        std::slice::from_raw_parts_mut(
                            labels_slice.as_mut_ptr().add(strip_start),
                            strip_end - strip_start,
                        )
                    };

                    s.spawn(move |_| {
                        Self::label_strip(
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
        }

        // Phase 2: Merge labels at strip boundaries
        for strip_idx in 1..num_strips {
            let boundary_y = strip_idx * rows_per_strip;
            Self::merge_strip_boundary(mask, &labels, width, boundary_y, &parent);
        }

        // Get final label count
        let total_labels = next_label.load(Ordering::SeqCst) - 1;
        if total_labels == 0 {
            return Self {
                labels,
                num_labels: 0,
            };
        }

        // Phase 3: Build final label mapping
        let mut parent_vec: Vec<u32> = parent
            .iter()
            .take(total_labels as usize)
            .map(|a| a.load(Ordering::SeqCst))
            .collect();

        let (label_map, num_labels) = Self::build_label_map(&mut parent_vec);

        // Phase 4: Apply label mapping in parallel
        let labels_data = labels.pixels_mut();
        parallel::par_chunks_auto(labels_data).for_each(|(_, chunk)| {
            for label in chunk.iter_mut() {
                if *label != 0 && (*label as usize) < label_map.len() {
                    *label = label_map[*label as usize];
                }
            }
        });

        Self { labels, num_labels }
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
                            // Initialize parent to self
                            if (label as usize) < parent.len() {
                                parent[label as usize - 1].store(label, Ordering::SeqCst);
                            }
                            label
                        }
                        (Some(l), None) => l,
                        (None, Some(t)) => t,
                        (Some(l), Some(t)) => {
                            if l != t {
                                Self::atomic_union(parent, l, t);
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

            // If both pixels are foreground and have different labels, merge them
            if mask.get(above_idx) && mask.get(below_idx) {
                let above_label = labels[above_idx];
                let below_label = labels[below_idx];

                if above_label != 0 && below_label != 0 && above_label != below_label {
                    Self::atomic_union(parent, above_label, below_label);
                }
            }
        }
    }

    /// Atomic find with path compression.
    fn atomic_find(parent: &[AtomicU32], label: u32) -> u32 {
        let idx = (label - 1) as usize;
        if idx >= parent.len() {
            return label;
        }

        let mut current = label;
        loop {
            let p = parent[(current - 1) as usize].load(Ordering::SeqCst);
            if p == current || p == 0 {
                return current;
            }
            current = p;
        }
    }

    /// Atomic union with lock-free CAS.
    fn atomic_union(parent: &[AtomicU32], a: u32, b: u32) {
        let mut root_a = Self::atomic_find(parent, a);
        let mut root_b = Self::atomic_find(parent, b);

        while root_a != root_b {
            // Always make the larger root point to the smaller
            if root_a > root_b {
                std::mem::swap(&mut root_a, &mut root_b);
            }

            let idx_b = (root_b - 1) as usize;
            if idx_b >= parent.len() {
                break;
            }

            // Try to set parent[root_b] = root_a
            match parent[idx_b].compare_exchange(root_b, root_a, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(current) => {
                    // Someone else modified it, retry with new roots
                    root_a = Self::atomic_find(parent, root_a);
                    root_b = Self::atomic_find(parent, current);
                }
            }
        }
    }

    /// Number of connected components (excluding background).
    #[inline]
    pub fn num_labels(&self) -> usize {
        self.num_labels
    }

    /// Width of the label map.
    #[inline]
    #[allow(dead_code)]
    pub fn width(&self) -> usize {
        self.labels.width()
    }

    /// Height of the label map.
    #[inline]
    #[allow(dead_code)]
    pub fn height(&self) -> usize {
        self.labels.height()
    }

    /// Get the underlying buffer.
    #[inline]
    #[allow(dead_code)]
    pub fn buffer(&self) -> &Buffer2<u32> {
        &self.labels
    }

    /// Get the label at a linear index.
    #[inline]
    #[allow(dead_code)]
    pub fn get(&self, idx: usize) -> u32 {
        self.labels[idx]
    }

    /// Get the raw pixel slice.
    #[inline]
    #[allow(dead_code)]
    pub fn pixels(&self) -> &[u32] {
        self.labels.pixels()
    }

    /// Iterate over labels.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &u32> {
        self.labels.iter()
    }

    /// Iterate over labels mutably.
    #[inline]
    #[allow(dead_code)]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut u32> {
        self.labels.pixels_mut().iter_mut()
    }

    /// Build a mapping from provisional labels to final sequential labels.
    fn build_label_map(parent: &mut [u32]) -> (Vec<u32>, usize) {
        let mut root_to_final = vec![0u32; parent.len() + 1];
        let mut num_labels = 0usize;

        for label in 1..=parent.len() as u32 {
            let root = Self::find(parent, label);
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

    /// Find root of a label with path compression.
    fn find(parent: &mut [u32], label: u32) -> u32 {
        let idx = (label - 1) as usize;
        if idx >= parent.len() {
            return label;
        }
        if parent[idx] != label && parent[idx] != 0 {
            parent[idx] = Self::find(parent, parent[idx]);
        }
        if parent[idx] == 0 { label } else { parent[idx] }
    }

    /// Union two labels (for sequential algorithm).
    fn union(parent: &mut [u32], a: u32, b: u32) {
        let root_a = Self::find(parent, a);
        let root_b = Self::find(parent, b);
        if root_a != root_b {
            if root_a < root_b {
                parent[(root_b - 1) as usize] = root_a;
            } else {
                parent[(root_a - 1) as usize] = root_b;
            }
        }
    }
}

impl std::ops::Index<usize> for LabelMap {
    type Output = u32;

    #[inline]
    fn index(&self, idx: usize) -> &Self::Output {
        &self.labels[idx]
    }
}
