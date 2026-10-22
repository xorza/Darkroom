//! Foreground pixels as horizontal runs, and how adjacent rows' runs connect.
//!
//! Labeling never touches individual pixels: a row is scanned into runs by word-level bit
//! operations, and connectivity is then a question about intervals rather than about neighbours.
//! That is what makes a sparse mask cheap — an empty 64-pixel word is one comparison.

use std::ops::Range;

use crate::star_detection::labeling::union_find::UnionFind;
use crate::star_detection::star_detection_config::extraction_config::Connectivity;

/// A horizontal run of foreground pixels.
#[derive(Debug, Clone, Copy)]
pub(super) struct Run {
    /// The first x of the run.
    pub(super) start: u32,
    /// One past the last x of the run.
    pub(super) end: u32,
    /// The provisional label.
    pub(super) label: u32,
}

impl Run {
    /// The x interval of the adjacent row to scan for runs that could connect to this one.
    ///
    /// Eight-connectivity reaches one pixel further each way, so a run touching this one only
    /// at a diagonal still falls inside the window.
    #[inline]
    pub(super) const fn search_window(&self, connectivity: Connectivity) -> Range<u32> {
        match connectivity {
            Connectivity::Four => self.start..self.end,
            Connectivity::Eight => self.start.saturating_sub(1)..self.end + 1,
        }
    }

    /// Whether this run and `other`, of the adjacent row, connect.
    #[inline]
    const fn connects(&self, other: &Self, connectivity: Connectivity) -> bool {
        match connectivity {
            Connectivity::Four => self.start < other.end && self.end > other.start,
            Connectivity::Eight => self.start < other.end + 1 && self.end + 1 > other.start,
        }
    }
}

/// Extract the runs of one row of a mask, `row_words` of it, using word-level bit scanning.
///
/// Uses trailing zero counting (CTZ) for efficient run boundary detection.
/// This is faster than bit-by-bit scanning for mixed words.
#[inline]
pub(super) fn extract_runs_from_row(row_words: &[u64], width: usize, runs: &mut Vec<Run>) {
    let mut scan = RowScan {
        width: width as u32,
        in_run: false,
        run_start: 0,
    };

    for (word_idx, &word) in row_words.iter().enumerate() {
        let base_x = (word_idx * 64) as u32;

        if word == 0 {
            if scan.in_run {
                let end = base_x.min(scan.width);
                runs.push(Run {
                    start: scan.run_start,
                    end,
                    label: 0,
                });
                scan.in_run = false;
            }
            continue;
        }

        if word == !0u64 {
            if !scan.in_run {
                scan.run_start = base_x;
                scan.in_run = true;
            }
            continue;
        }

        scan.mixed_word(word, base_x, runs);
    }

    if scan.in_run {
        runs.push(Run {
            start: scan.run_start,
            end: scan.width,
            label: 0,
        });
    }
}

/// A row's scan in progress: the row's width, and the run it holds open across words.
#[derive(Debug)]
struct RowScan {
    width: u32,
    in_run: bool,
    run_start: u32,
}

impl RowScan {
    /// Scan a mixed word (both 0s and 1s) starting at `base_x`, by CTZ.
    #[inline]
    fn mixed_word(&mut self, word: u64, base_x: u32, runs: &mut Vec<Run>) {
        let word_end = (base_x + 64).min(self.width);
        let mut pos = base_x;

        loop {
            if pos >= word_end {
                break;
            }

            let bit_offset = pos - base_x;
            let remaining_bits = word >> bit_offset;

            if self.in_run {
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
                    start: self.run_start,
                    end: end_pos,
                    label: 0,
                });
                self.in_run = false;
                pos = end_pos;
            } else {
                if remaining_bits == 0 {
                    break;
                }
                let zeros_until_start = remaining_bits.trailing_zeros();
                let start_pos = pos + zeros_until_start;

                if start_pos >= word_end {
                    break;
                }

                self.run_start = start_pos;
                self.in_run = true;
                pos = start_pos;
            }
        }
    }
}

/// How runs of adjacent rows join: under `connectivity`, into the components of `union_find`.
#[derive(Debug, Clone, Copy)]
pub(super) struct RunJoin<'a> {
    pub(super) union_find: &'a UnionFind,
    pub(super) connectivity: Connectivity,
}

impl RunJoin<'_> {
    /// Merge current row's runs with previous row's runs via union-find.
    ///
    /// For each run in `cur_runs`, finds overlapping runs in `prev_runs` and merges
    /// their labels. Runs without overlap get a new label via `make_set`.
    #[inline]
    pub(super) fn merge_with_prev(self, cur_runs: &mut [Run], prev_runs: &[Run]) {
        let Self {
            union_find: uf,
            connectivity,
        } = self;
        let mut prev_idx = 0;
        for run in cur_runs.iter_mut() {
            let window = run.search_window(connectivity);

            while prev_idx < prev_runs.len() && prev_runs[prev_idx].end <= window.start {
                prev_idx += 1;
            }

            let mut assigned_label = None;
            let mut check_idx = prev_idx;
            while check_idx < prev_runs.len() && prev_runs[check_idx].start < window.end {
                let prev_run = &prev_runs[check_idx];
                if prev_run.connects(run, connectivity) {
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

    /// Merge labels across a strip boundary by sweeping the two sorted run lists.
    pub(super) fn stitch(self, above_runs: &[Run], below_runs: &[Run]) {
        let Self {
            union_find,
            connectivity,
        } = self;
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
                if a.connects(below, connectivity) && a.label != below.label {
                    union_find.union(a.label, below.label);
                }
            }

            below_idx += 1;
        }
    }
}
