//! Connected component labeling using union-find.

use crate::common::{BitBuffer2, Buffer2};

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
    /// Uses a two-pass algorithm:
    /// 1. First pass: Sequential scan assigning provisional labels with union-find
    /// 2. Second pass: Parallel label flattening using precomputed root mapping
    ///
    /// For images >4M pixels, the second pass is parallelized using rayon.
    pub fn from_mask(mask: &BitBuffer2) -> Self {
        let width = mask.width();
        let height = mask.height();

        let mut labels = Buffer2::new_filled(width, height, 0u32);
        let mut parent: Vec<u32> = Vec::new();

        // First pass: assign provisional labels with union-find
        Self::assign_provisional_labels(mask, &mut labels, &mut parent);

        if parent.is_empty() {
            return Self {
                labels,
                num_labels: 0,
            };
        }

        let (label_map, num_labels) = Self::build_label_map(&mut parent);

        // Second pass: apply label mapping
        // Parallelize for large images (>4M pixels, i.e., 2K×2K and larger)
        let pixel_count = width * height;
        let labels_data = labels.pixels_mut();
        if pixel_count > 4_000_000 {
            use rayon::prelude::*;
            labels_data.par_iter_mut().for_each(|label| {
                if *label != 0 {
                    *label = label_map[*label as usize];
                }
            });
        } else {
            for label in labels_data.iter_mut() {
                if *label != 0 {
                    *label = label_map[*label as usize];
                }
            }
        }

        Self { labels, num_labels }
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

    /// First pass: assign provisional labels using union-find.
    ///
    /// Scans the mask sequentially, assigning labels to foreground pixels and
    /// merging components when neighbors from different components meet.
    fn assign_provisional_labels(
        mask: &BitBuffer2,
        labels: &mut Buffer2<u32>,
        parent: &mut Vec<u32>,
    ) {
        debug_assert_eq!(
            (mask.width(), mask.height()),
            (labels.width(), labels.height()),
            "mask and labels must have same dimensions"
        );

        let width = mask.width();
        let height = mask.height();
        let mut next_label = 1u32;

        for y in 0..height {
            let row_start = y * width;
            for x in 0..width {
                let idx = row_start + x;
                if !mask.get(idx) {
                    continue;
                }

                // Check neighbors: left and top only (for forward scan)
                let left = (x > 0 && mask.get(idx - 1)).then(|| labels[idx - 1]);
                let top = (y > 0 && mask.get(idx - width)).then(|| labels[idx - width]);

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
                            Self::union(parent, l, t);
                        }
                        l.min(t)
                    }
                };
            }
        }
    }

    /// Build a mapping from provisional labels to final sequential labels.
    ///
    /// Flattens the union-find structure and assigns sequential final labels
    /// to each unique component root. Uses single-pass optimization: after
    /// path compression, parent[i] points directly to the root, so we can
    /// build the label map without additional find() calls.
    fn build_label_map(parent: &mut [u32]) -> (Vec<u32>, usize) {
        // First pass: flatten all paths and assign sequential labels to roots
        let mut root_to_final = vec![0u32; parent.len() + 1];
        let mut num_labels = 0usize;

        for label in 1..=parent.len() as u32 {
            let root = Self::find(parent, label);
            if root_to_final[root as usize] == 0 {
                num_labels += 1;
                root_to_final[root as usize] = num_labels as u32;
            }
        }

        // Second pass: build label_map using already-flattened parent array.
        // After path compression above, parent[i-1] == root for all labels,
        // so we can look up final labels directly without calling find().
        let mut label_map = vec![0u32; parent.len() + 1];
        for (i, &root) in parent.iter().enumerate() {
            label_map[i + 1] = root_to_final[root as usize];
        }

        (label_map, num_labels)
    }

    /// Find root of a label with path compression.
    fn find(parent: &mut [u32], label: u32) -> u32 {
        let idx = (label - 1) as usize;
        if parent[idx] != label {
            parent[idx] = Self::find(parent, parent[idx]); // Path compression
        }
        parent[idx]
    }

    /// Union two labels.
    fn union(parent: &mut [u32], a: u32, b: u32) {
        let root_a = Self::find(parent, a);
        let root_b = Self::find(parent, b);
        if root_a != root_b {
            // Union by making smaller root point to larger
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
