//! Foreground pixels as horizontal runs, and how adjacent rows' runs connect.
//!
//! Labeling never touches individual pixels: a row is scanned into runs by word-level bit
//! operations, and connectivity is then a question about intervals rather than about neighbours.
//! That is what makes a sparse mask cheap — an empty 64-pixel word is one comparison.

use std::ops::Range;

use crate::stacking::star_detection::config::detection_config::Connectivity;
use crate::stacking::star_detection::labeling::union_find::UnionFind;

/// A horizontal run of foreground pixels.
#[derive(Debug, Clone, Copy)]
pub(super) struct Run {
    pub(super) start: u32, // Starting x coordinate (inclusive)
    pub(super) end: u32,   // Ending x coordinate (exclusive)
    pub(super) label: u32, // Provisional label
}

impl Run {
    /// The x interval of the adjacent row to scan for runs that could connect to this one.
    ///
    /// Eight-connectivity reaches one pixel further each way, so a run touching this one only
    /// at a diagonal still falls inside the window.
    #[inline]
    pub(super) fn search_window(&self, connectivity: Connectivity) -> Range<u32> {
        match connectivity {
            Connectivity::Four => self.start..self.end,
            Connectivity::Eight => self.start.saturating_sub(1)..self.end + 1,
        }
    }
}

/// Check if two runs from adjacent rows are connected.
#[inline]
pub(super) fn runs_connected(prev: &Run, curr: &Run, connectivity: Connectivity) -> bool {
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

/// Merge current row's runs with previous row's runs via union-find.
///
/// For each run in `curr_runs`, finds overlapping runs in `prev_runs` and merges
/// their labels. Runs without overlap get a new label via `uf.make_set()`.
#[inline]
pub(super) fn merge_runs_with_prev(
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
