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

use std::ops::Range;
use std::sync::atomic::{AtomicU32, Ordering};

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::concurrency::UnsafeSendPtr;
use crate::stacking::star_detection::resources::DetectionResources;
use imaginarium::Buffer2;

use crate::stacking::star_detection::config::detection_config::Connectivity;

/// Rows a strip must cover to be worth splitting off, so an image is not cut into bands whose
/// per-strip overhead and boundary stitching outweigh the labeling. An image under this height
/// stays a single strip.
const MIN_ROWS_PER_STRIP: usize = 64;

/// A horizontal run of foreground pixels.
#[derive(Debug, Clone, Copy)]
struct Run {
    start: u32, // Starting x coordinate (inclusive)
    end: u32,   // Ending x coordinate (exclusive)
    label: u32, // Provisional label
}

impl Run {
    /// The x interval of the adjacent row to scan for runs that could connect to this one.
    ///
    /// Eight-connectivity reaches one pixel further each way, so a run touching this one only
    /// at a diagonal still falls inside the window.
    #[inline]
    fn search_window(&self, connectivity: Connectivity) -> Range<u32> {
        match connectivity {
            Connectivity::Four => self.start..self.end,
            Connectivity::Eight => self.start.saturating_sub(1)..self.end + 1,
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
fn extract_runs_from_row(
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

/// Merge current row's runs with previous row's runs via union-find.
///
/// For each run in `curr_runs`, finds overlapping runs in `prev_runs` and merges
/// their labels. Runs without overlap get a new label via `uf.make_set()`.
#[inline]
fn merge_runs_with_prev(
    curr_runs: &mut [Run],
    prev_runs: &[Run],
    connectivity: Connectivity,
    uf: &UnionFind,
) {
    let mut prev_idx = 0;
    for run in curr_runs.iter_mut() {
        let window = run.search_window(connectivity);

        while prev_idx < prev_runs.len() && prev_runs[prev_idx].end <= window.start {
            prev_idx += 1;
        }

        let mut assigned_label = None;
        let mut check_idx = prev_idx;
        while check_idx < prev_runs.len() && prev_runs[check_idx].start < window.end {
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

/// Lock-free union-find over provisional run labels.
///
/// Operations take `&self` because the strips share one instance across threads.
struct UnionFind {
    parent: Vec<AtomicU32>,
    next_label: AtomicU32,
}

impl std::fmt::Debug for UnionFind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnionFind")
            .field("len", &self.parent.len())
            .field("next_label", &self.next_label.load(Ordering::Relaxed))
            .finish()
    }
}

/// Dense 1..=N relabeling from [`UnionFind::build_label_map`]: `map[provisional]` is the
/// final label, and `count` is the number of distinct components (the max final label).
#[derive(Debug)]
struct LabelMapping {
    map: Vec<u32>,
    count: usize,
}

impl UnionFind {
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
            "UnionFind capacity exceeded: label {label} > capacity {}",
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

    /// Build the dense 1..=N label mapping (single pass) together with the component count.
    fn build_label_map(&self, total_labels: usize) -> LabelMapping {
        let mut map = vec![0u32; total_labels + 1];
        let mut count = 0u32;

        for i in 1..=total_labels {
            let root = self.find(i as u32);
            if map[root as usize] == 0 {
                count += 1;
                map[root as usize] = count;
            }
            map[i] = map[root as usize];
        }

        LabelMapping {
            map,
            count: count as usize,
        }
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
