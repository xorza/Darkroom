//! [`Labeler`]: connected-component labeling that keeps its scratch from frame to frame.

use std::mem;
use std::ops::Range;

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::concurrency::unsafe_send_ptr::UnsafeSendPtr;
use crate::math::vec2us::Vec2us;
use crate::star_detection::config::detection_config::Connectivity;
use crate::star_detection::labeling::LabelMap;
use crate::star_detection::labeling::component_data::ComponentData;
use crate::star_detection::labeling::run::{
    Run, extract_runs_from_row, merge_runs_with_prev, runs_connected,
};
use crate::star_detection::labeling::union_find::UnionFind;
use imaginarium::Buffer2;

/// Rows a strip must cover to be worth splitting off, so an image is not cut into bands whose
/// per-strip overhead and boundary stitching outweigh the labeling. An image under this height
/// stays a single strip.
const MIN_ROWS_PER_STRIP: usize = 64;

/// RLE-based connected-component labeling: strip the mask into horizontal bands, label each in
/// parallel against one shared union-find, stitch the labels across the band boundaries, then
/// write the dense relabeling back in parallel and collect each component's box and area from
/// its runs. Components are numbered in raster order of their first pixel, whatever the strips.
///
/// Every buffer this needs lives here and is refilled, never reallocated once it is large
/// enough, so a frame after the first allocates nothing.
#[derive(Debug, Default)]
pub(crate) struct Labeler {
    union_find: UnionFind,
    strips: Vec<Strip>,
    /// Provisional label → final label; see [`UnionFind::build_label_map`].
    mapping: Vec<u32>,
    /// The component list the last label map gave back, refilled by the next.
    components: Vec<ComponentData>,
}

/// One horizontal band of the mask and the runs found in it.
#[derive(Debug, Default)]
struct Strip {
    /// Every run in the band, with its row.
    runs: Vec<(u32, Run)>,
    /// Runs of the band's first row, for stitching to the band above.
    first_row_runs: Vec<Run>,
    /// Runs of the band's last row, for stitching to the band below.
    last_row_runs: Vec<Run>,
    prev_runs: Vec<Run>,
    curr_runs: Vec<Run>,
}

impl Labeler {
    /// Label the foreground of `mask` into `labels`, which must be zeroed and the mask's size.
    ///
    /// One band for an image under [`MIN_ROWS_PER_STRIP`] rows, where the boundary stitch has
    /// nothing to do — small inputs take the same path as large ones.
    pub(crate) fn label(
        &mut self,
        mask: &BitBuffer2,
        connectivity: Connectivity,
        labels: Buffer2<u32>,
    ) -> LabelMap {
        let height = mask.size.height;
        let num_strips = (height / MIN_ROWS_PER_STRIP).clamp(1, rayon::current_num_threads());
        self.label_in_strips(mask, connectivity, labels, num_strips)
    }

    /// [`Self::label`] cut into `num_strips` bands of `height / num_strips` rows, the last taking
    /// the remainder. At least one, and at most one per row.
    fn label_in_strips(
        &mut self,
        mask: &BitBuffer2,
        connectivity: Connectivity,
        mut labels: Buffer2<u32>,
        num_strips: usize,
    ) -> LabelMap {
        let width = mask.size.width;
        let height = mask.size.height;
        // Release asserts: the label writes below are unchecked off these dimensions. Once per
        // frame.
        assert_eq!(width, labels.width());
        assert_eq!(height, labels.height());

        let mut components = mem::take(&mut self.components);
        components.clear();
        if width == 0 || height == 0 {
            return LabelMap { labels, components };
        }

        let Self {
            union_find,
            strips,
            mapping,
            ..
        } = self;
        debug_assert!(
            (1..=height).contains(&num_strips),
            "between one strip and one per row"
        );
        let words_per_row = mask.words_per_row();
        let rows_per_strip = height / num_strips;
        let strip_rows = |strip_idx: usize| {
            let end = if strip_idx == num_strips - 1 {
                height
            } else {
                (strip_idx + 1) * rows_per_strip
            };
            strip_idx * rows_per_strip..end
        };

        // The foreground pixel count bounds the provisional labels exactly: each run is at least
        // one foreground pixel and takes one label.
        union_find.reset(mask.count_ones());
        if strips.len() < num_strips {
            strips.resize_with(num_strips, Strip::default);
        }
        let strips = &mut strips[..num_strips];

        strips
            .par_iter_mut()
            .enumerate()
            .for_each(|(strip_idx, strip)| {
                strip.label_rows(
                    &mask.words,
                    width,
                    words_per_row,
                    strip_rows(strip_idx),
                    union_find,
                    connectivity,
                );
            });

        for pair in strips.windows(2) {
            stitch_boundary(
                &pair[0].last_row_runs,
                &pair[1].first_row_runs,
                union_find,
                connectivity,
            );
        }

        let count = union_find.build_label_map(
            strips
                .iter()
                .flat_map(|strip| strip.runs.iter().map(|&(_, run)| run.label)),
            mapping,
        );
        if count == 0 {
            return LabelMap { labels, components };
        }

        let labels_ptr = UnsafeSendPtr::new(labels.pixels_mut().as_mut_ptr());
        let mapping = &*mapping;
        strips.par_iter().for_each(|strip| {
            for &(y, run) in &strip.runs {
                let row_start = y as usize * width;
                let final_label = mapping[run.label as usize];
                let ptr = labels_ptr.get();
                for x in run.start..run.end {
                    // SAFETY: runs cover disjoint pixels, all inside the `width × height` plane
                    // the asserts above bound.
                    unsafe {
                        *ptr.add(row_start + x as usize) = final_label;
                    }
                }
            }
        });

        components.resize(count, ComponentData::default());
        for strip in strips.iter() {
            for &(y, run) in &strip.runs {
                let label = mapping[run.label as usize];
                let component = &mut components[label as usize - 1];
                component.label = label;
                component
                    .bbox
                    .include(Vec2us::new(run.start as usize, y as usize));
                component
                    .bbox
                    .include(Vec2us::new(run.end as usize - 1, y as usize));
                component.area += (run.end - run.start) as usize;
            }
        }

        LabelMap { labels, components }
    }

    /// Take back a label map's component list for the next frame to refill.
    pub(crate) fn recycle(&mut self, components: Vec<ComponentData>) {
        self.components = components;
    }
}

impl Strip {
    /// Find the runs of `rows`, joining each to the runs of the row above it.
    fn label_rows(
        &mut self,
        mask_words: &[u64],
        width: usize,
        words_per_row: usize,
        rows: Range<usize>,
        union_find: &UnionFind,
        connectivity: Connectivity,
    ) {
        let Self {
            runs,
            first_row_runs,
            last_row_runs,
            prev_runs,
            curr_runs,
        } = self;
        runs.clear();
        first_row_runs.clear();
        last_row_runs.clear();
        prev_runs.clear();

        let first = rows.start;
        let last = rows.end - 1;
        for y in rows {
            curr_runs.clear();
            extract_runs_from_row(
                mask_words,
                y * words_per_row,
                words_per_row,
                width,
                curr_runs,
            );

            if curr_runs.is_empty() {
                prev_runs.clear();
                continue;
            }

            merge_runs_with_prev(curr_runs, prev_runs, connectivity, union_find);
            runs.extend(curr_runs.iter().map(|&run| (y as u32, run)));

            if y == first {
                first_row_runs.clone_from(curr_runs);
            }
            if y == last {
                last_row_runs.clone_from(curr_runs);
            }

            mem::swap(prev_runs, curr_runs);
        }
    }
}

/// Merge labels across a strip boundary by sweeping the two sorted run lists.
fn stitch_boundary(
    above_runs: &[Run],
    below_runs: &[Run],
    union_find: &UnionFind,
    connectivity: Connectivity,
) {
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

        for a in above_runs[above_idx..]
            .iter()
            .take_while(|a| a.start < below_window.end)
        {
            if runs_connected(a, below, connectivity) && a.label != below.label {
                union_find.union(a.label, below.label);
            }
        }

        below_idx += 1;
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::bit_buffer2::BitBuffer2;
    use crate::star_detection::config::detection_config::Connectivity;
    use crate::star_detection::labeling::LabelMap;
    use crate::star_detection::labeling::labeler::Labeler;

    /// Label `mask` in exactly `num_strips` bands, whatever the thread count — so a test reaches
    /// the stitch across band boundaries on any machine.
    pub(crate) fn label_in_strips(
        mask: &BitBuffer2,
        connectivity: Connectivity,
        num_strips: usize,
    ) -> LabelMap {
        let labels = Buffer2::new_filled(mask.size.width, mask.size.height, 0u32);
        Labeler::default().label_in_strips(mask, connectivity, labels, num_strips)
    }
}
