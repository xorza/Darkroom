//! Connected component labeling using union-find.
//!
//! Optimized for sparse binary masks (typical in star detection):
//! - Run-length encoding (RLE) based labeling for efficient processing
//! - Word-level bit scanning to skip background regions
//! - Strip-parallel labeling with boundary merging, down to the single strip a small image needs
//! - Lock-free union-find with atomic operations
//! - Minimal allocations via buffer reuse

#[cfg(all(test, feature = "internals"))]
mod bench;
#[cfg(test)]
mod tests;

mod run;
mod union_find;

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::concurrency::UnsafeSendPtr;
use crate::stacking::star_detection::resources::DetectionResources;
use imaginarium::Buffer2;

use crate::stacking::star_detection::config::detection_config::Connectivity;
use crate::stacking::star_detection::labeling::run::{
    Run, extract_runs_from_row, merge_runs_with_prev, runs_connected,
};
use crate::stacking::star_detection::labeling::union_find::UnionFind;

/// Rows a strip must cover to be worth splitting off, so an image is not cut into bands whose
/// per-strip overhead and boundary stitching outweigh the labeling. An image under this height
/// stays a single strip.
const MIN_ROWS_PER_STRIP: usize = 64;

/// A 2D label map from connected component analysis.
#[derive(Debug)]
pub(crate) struct LabelMap {
    labels: Buffer2<u32>,
    num_labels: usize,
}

impl LabelMap {
    /// Create a label map by acquiring a reusable buffer.
    ///
    /// # Arguments
    /// * `mask` - Binary mask of foreground pixels
    /// * `connectivity` - Four (default) or Eight connectivity
    /// * `resources` - Detection resources that provide the label buffer
    pub(crate) fn from_pool(
        mask: &BitBuffer2,
        connectivity: Connectivity,
        resources: &mut DetectionResources,
    ) -> Self {
        debug_assert_eq!(mask.size, resources.dimensions);

        let mut labels = resources.acquire_u32();
        // Clear the buffer (it may contain old labels)
        labels.pixels_mut().fill(0);

        Self::from_buffer(mask, connectivity, labels)
    }

    /// Create a label map from a binary mask with a pre-allocated buffer.
    ///
    /// See [`label_mask`] for the algorithm.
    ///
    /// # Arguments
    /// * `mask` - Binary mask of foreground pixels
    /// * `connectivity` - Four or Eight connectivity
    /// * `labels` - Pre-allocated buffer (must be zeroed, same dimensions as mask)
    fn from_buffer(
        mask: &BitBuffer2,
        connectivity: Connectivity,
        mut labels: Buffer2<u32>,
    ) -> Self {
        let width = mask.size.width;
        let height = mask.size.height;

        assert_eq!(width, labels.width());
        assert_eq!(height, labels.height());

        if width == 0 || height == 0 {
            return Self {
                labels,
                num_labels: 0,
            };
        }

        let num_labels = label_mask(mask, &mut labels, connectivity);

        Self { labels, num_labels }
    }

    /// Release this LabelMap's buffer back to the pool.
    pub(crate) fn release_to_pool(self, pool: &mut DetectionResources) {
        pool.release_u32(self.labels);
    }

    /// Number of connected components (excluding background).
    #[inline]
    pub(crate) fn num_labels(&self) -> usize {
        self.num_labels
    }

    #[inline]
    pub(crate) fn width(&self) -> usize {
        self.labels.width()
    }

    #[inline]
    pub(crate) fn height(&self) -> usize {
        self.labels.height()
    }

    /// Get the raw labels slice.
    #[inline]
    pub(crate) fn labels(&self) -> &[u32] {
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

/// RLE-based connected-component labeling: strip the mask into horizontal bands, label each in
/// parallel against one shared union-find, stitch the labels across the band boundaries, then
/// write the dense relabeling back in parallel.
///
/// One band for an image under [`MIN_ROWS_PER_STRIP`] rows, where the boundary stitch has nothing
/// to do — small inputs take the same path as large ones rather than a second implementation.
fn label_mask(mask: &BitBuffer2, labels: &mut Buffer2<u32>, connectivity: Connectivity) -> usize {
    let width = mask.size.width;
    let height = mask.size.height;
    let words_per_row = mask.words_per_row();
    let mask_words = &mask.words;

    let num_threads = rayon::current_num_threads();
    let num_strips = (height / MIN_ROWS_PER_STRIP).clamp(1, num_threads);
    let rows_per_strip = height / num_strips;

    // Capacity = foreground pixel count, an exact upper bound on provisional labels (each run
    // is ≥1 foreground pixel and make_set runs once per run), so it can never overflow.
    let uf = UnionFind::new(mask.count_ones().max(1024));

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

    // Phase 2: merge labels across strip boundaries (two-pointer sweep over sorted runs)
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
                .map
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

    label_map.count
}

/// Label a single strip and return runs with boundary information.
fn label_strip(
    mask_words: &[u64],
    width: usize,
    words_per_row: usize,
    y_start: usize,
    y_end: usize,
    uf: &UnionFind,
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

        merge_runs_with_prev(&mut curr_runs, &prev_runs, connectivity, uf);

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

/// Merge labels across a strip boundary by sweeping the two sorted run lists.
fn merge_strip_boundary_sorted(
    above_runs: &[Run],
    below_runs: &[Run],
    uf: &UnionFind,
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

        let above_window = above.search_window(connectivity);
        let below_window = below.search_window(connectivity);

        if above_window.end <= below_window.start {
            above_idx += 1;
            continue;
        }
        if below_window.end <= above_window.start {
            below_idx += 1;
            continue;
        }

        // Check all above runs that could connect to this below run
        let mut check_above = above_idx;
        while check_above < above_runs.len() {
            let a = &above_runs[check_above];
            if a.start >= below_window.end {
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

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::bit_buffer2::BitBuffer2;
    use crate::stacking::star_detection::config::detection_config::Connectivity;
    use crate::stacking::star_detection::labeling::LabelMap;

    impl LabelMap {
        /// Adopt pre-computed labels, bypassing connected-component analysis entirely.
        pub(crate) fn from_raw(labels: Buffer2<u32>, num_labels: usize) -> Self {
            Self { labels, num_labels }
        }

        /// Label `mask` into a freshly allocated buffer instead of one drawn from the pool.
        pub(crate) fn from_mask(mask: &BitBuffer2, connectivity: Connectivity) -> Self {
            let labels = Buffer2::new_filled(mask.size.width, mask.size.height, 0u32);
            Self::from_buffer(mask, connectivity, labels)
        }
    }
}
