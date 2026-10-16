//! [`Labeler`]: connected-component labeling that keeps its scratch from frame to frame.

use std::mem;
use std::ops::Range;

use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::math::vec2us::Vec2us;
use crate::star_detection::labeling::component_data::ComponentData;
use crate::star_detection::labeling::run;
use crate::star_detection::labeling::run::{Run, RunJoin};
use crate::star_detection::labeling::union_find::UnionFind;
use crate::star_detection::labeling::{LabelMap, LabelRun};
use crate::star_detection::star_detection_config::extraction_config::Connectivity;

/// Rows a strip must cover to be worth splitting off, so an image is not cut into bands whose
/// per-strip overhead and boundary stitching outweigh the labeling. An image under this height
/// stays a single strip.
const MIN_ROWS_PER_STRIP: usize = 64;

/// RLE-based connected-component labeling: strip the mask into horizontal bands, label each in
/// parallel against one shared union-find, stitch the labels across the band boundaries, then
/// gather each component's runs, and its box and area from them. Components are numbered in
/// raster order of their first pixel, whatever the strips.
///
/// Every buffer this needs lives here and is refilled, never reallocated once it is large
/// enough, so a frame after the first allocates nothing.
#[derive(Debug, Default)]
pub(crate) struct Labeler {
    union_find: UnionFind,
    strips: Vec<Strip>,
    /// Provisional label → final label; see [`UnionFind::build_label_map`].
    mapping: Vec<u32>,
    /// The last mask's components, refilled by the next.
    map: LabelMap,
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
    /// Label the foreground of `mask`.
    ///
    /// One band for an image under [`MIN_ROWS_PER_STRIP`] rows, where the boundary stitch has
    /// nothing to do — small inputs take the same path as large ones.
    pub(crate) fn label(&mut self, mask: &BitBuffer2, connectivity: Connectivity) -> &LabelMap {
        let height = mask.size.height;
        let num_strips = (height / MIN_ROWS_PER_STRIP).clamp(1, rayon::current_num_threads());
        self.label_in_strips(mask, connectivity, num_strips)
    }

    /// [`Self::label`] cut into `num_strips` bands of `height / num_strips` rows, the last taking
    /// the remainder. At least one, and at most one per row.
    fn label_in_strips(
        &mut self,
        mask: &BitBuffer2,
        connectivity: Connectivity,
        num_strips: usize,
    ) -> &LabelMap {
        let width = mask.size.width;
        let height = mask.size.height;

        let Self {
            union_find,
            strips,
            mapping,
            map,
        } = self;
        map.size = mask.size;
        map.components.clear();
        map.runs.clear();
        map.run_starts.clear();
        map.run_starts.push(0);
        if width == 0 || height == 0 {
            return map;
        }

        debug_assert!(
            (1..=height).contains(&num_strips),
            "between one strip and one per row"
        );
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
        let join = RunJoin {
            union_find,
            connectivity,
        };

        strips
            .par_iter_mut()
            .enumerate()
            .for_each(|(strip_idx, strip)| {
                strip.label_rows(mask, strip_rows(strip_idx), join);
            });

        for pair in strips.windows(2) {
            join.stitch(&pair[0].last_row_runs, &pair[1].first_row_runs);
        }

        let count = union_find.build_label_map(
            strips
                .iter()
                .flat_map(|strip| strip.runs.iter().map(|&(_, run)| run.label)),
            mapping,
        );
        if count == 0 {
            return map;
        }

        // Each component's runs counted, its slice placed after the ones before it, then filled
        // in the strips' raster order, which each slice keeps.
        let LabelMap {
            components,
            runs,
            run_starts,
            ..
        } = map;
        components.resize(count, ComponentData::default());
        run_starts.resize(count + 1, 0);
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
                run_starts[label as usize] += 1;
            }
        }
        for index in 1..=count {
            run_starts[index] += run_starts[index - 1];
        }
        let total = run_starts[count] as usize;
        runs.resize(total, LabelRun::default());
        // `cursor[i]` is where component `i`'s next run goes: its start, moving on.
        let cursor = &mut run_starts[..count];
        let mut placed = 0;
        for strip in strips.iter() {
            for &(y, run) in &strip.runs {
                let index = mapping[run.label as usize] as usize - 1;
                runs[cursor[index] as usize] = LabelRun {
                    y,
                    start: run.start,
                    end: run.end,
                };
                cursor[index] += 1;
                placed += 1;
            }
        }
        debug_assert_eq!(placed, total);
        // Each cursor now holds its component's end, the next one's start: shift them back.
        run_starts.copy_within(0..count, 1);
        run_starts[0] = 0;
        map
    }
}

impl Strip {
    /// Find the runs of `rows` of `mask`, joining each to the runs of the row above it.
    fn label_rows(&mut self, mask: &BitBuffer2, rows: Range<usize>, join: RunJoin<'_>) {
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
            run::extract_runs_from_row(mask.row_words(y), mask.size.width, curr_runs);

            if curr_runs.is_empty() {
                prev_runs.clear();
                continue;
            }

            join.merge_with_prev(curr_runs, prev_runs);
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

#[cfg(feature = "internals")]
pub(crate) mod internals {
    #[cfg(test)]
    use crate::bit_buffer2::BitBuffer2;
    use crate::star_detection::labeling::LabelMap;
    use crate::star_detection::labeling::labeler::Labeler;
    #[cfg(test)]
    use crate::star_detection::star_detection_config::extraction_config::Connectivity;

    impl Labeler {
        /// The last mask's components, kept past the labeler.
        pub(crate) fn into_map(self) -> LabelMap {
            self.map
        }
    }

    /// Label `mask` in exactly `num_strips` bands, whatever the thread count — so a test reaches
    /// the stitch across band boundaries on any machine.
    #[cfg(test)]
    pub(crate) fn label_in_strips(
        mask: &BitBuffer2,
        connectivity: Connectivity,
        num_strips: usize,
    ) -> LabelMap {
        let mut labeler = Labeler::default();
        labeler.label_in_strips(mask, connectivity, num_strips);
        labeler.into_map()
    }
}
