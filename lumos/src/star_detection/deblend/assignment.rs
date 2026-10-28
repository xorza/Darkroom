//! [`Assignment`]: the per-peak boxes and areas a component's split accumulates.

use crate::star_detection::deblend;
use crate::star_detection::deblend::pixel::Pixel;
use common::Rect2us;
use glam::USizeVec2;

/// Peaks at or under this many are searched in full for every pixel; past it, a cell grid finds a
/// pixel's nearest peak in the few cells around it, so a component holding thousands of stars
/// splits in time proportional to its pixels rather than to pixels × peaks.
const FULL_SEARCH_PEAKS: usize = 8;

/// The per-peak box and area
/// [`Component::split_at`](crate::star_detection::deblend::component::Component::split_at)
/// accumulates, and the cell grid it finds a pixel's nearest peak with, kept by the caller across
/// components.
#[derive(Debug, Default)]
pub(crate) struct Assignment {
    pub(super) bboxes: Vec<Rect2us>,
    pub(super) areas: Vec<usize>,
    /// The grid's origin, the component's top-left pixel.
    origin: USizeVec2,
    /// The side of a grid cell, in pixels.
    cell: usize,
    /// The grid's extent in cells.
    cells: USizeVec2,
    /// Peak indices ordered by cell, in raster order of cells.
    by_cell: Vec<u32>,
    /// Where each cell's run starts in `by_cell`, with a trailing sentinel.
    cell_starts: Vec<u32>,
    /// Where the next peak of each cell goes while `by_cell` fills.
    cursor: Vec<u32>,
}

impl Assignment {
    /// Clear the boxes and areas for `peaks`, and grid them over `bbox` when there are enough:
    /// cells of side `⌈√(area / peaks)⌉`, about one peak each.
    pub(super) fn reset(&mut self, peaks: &[Pixel], bbox: Rect2us) {
        self.bboxes.clear();
        self.bboxes.resize(peaks.len(), Rect2us::empty());
        self.areas.clear();
        self.areas.resize(peaks.len(), 0);
        if peaks.len() <= FULL_SEARCH_PEAKS {
            return;
        }

        self.origin = bbox.min;
        self.cell = (bbox.area() / peaks.len()).isqrt().max(1);
        self.cells = USizeVec2::new(
            bbox.width().div_ceil(self.cell),
            bbox.height().div_ceil(self.cell),
        );
        let cell_count = self.cells.x * self.cells.y;
        self.cell_starts.clear();
        self.cell_starts.resize(cell_count + 1, 0);
        for peak in peaks {
            let cell = self.cell_of(peak.pos());
            self.cell_starts[cell + 1] += 1;
        }
        for cell in 0..cell_count {
            self.cell_starts[cell + 1] += self.cell_starts[cell];
        }
        self.by_cell.clear();
        self.by_cell.resize(peaks.len(), 0);
        self.cursor.clear();
        self.cursor
            .extend_from_slice(&self.cell_starts[..cell_count]);
        // In index order, so each cell's run keeps its peaks in index order too.
        for (idx, peak) in peaks.iter().enumerate() {
            let cell = self.cell_of(peak.pos());
            self.by_cell[self.cursor[cell] as usize] = idx as u32;
            self.cursor[cell] += 1;
        }
    }

    const fn cell_of(&self, pos: USizeVec2) -> usize {
        let x = (pos.x - self.origin.x) / self.cell;
        let y = (pos.y - self.origin.y) / self.cell;
        y * self.cells.x + x
    }

    /// The index of the peak nearest `pos` by squared distance, the first among equals — what
    /// `deblend::nearest_peak_idx` returns, found ring by ring of cells. A peak in a ring further
    /// out than `r` lies at least `r·cell + 1` away, so once the best is strictly nearer than that
    /// no later peak can win, not even a tie with a lower index.
    pub(super) fn nearest(&self, pos: USizeVec2, peaks: &[Pixel]) -> usize {
        if peaks.len() <= FULL_SEARCH_PEAKS {
            return deblend::nearest_peak_idx(pos, peaks);
        }
        let home = USizeVec2::new(
            (pos.x - self.origin.x) / self.cell,
            (pos.y - self.origin.y) / self.cell,
        );
        let mut best = (usize::MAX, usize::MAX);
        let offer = |cell_x: usize, cell_y: usize, best: &mut (usize, usize)| {
            let cell = cell_y * self.cells.x + cell_x;
            let run = self.cell_starts[cell] as usize..self.cell_starts[cell + 1] as usize;
            for &idx in &self.by_cell[run] {
                let candidate = (
                    deblend::dist_sq(pos, peaks[idx as usize].pos()),
                    idx as usize,
                );
                *best = (*best).min(candidate);
            }
        };
        let reach = self.cells.x.max(self.cells.y);
        for ring in 0..reach {
            let (x0, x1) = (
                home.x.saturating_sub(ring),
                (home.x + ring).min(self.cells.x - 1),
            );
            let (y0, y1) = (
                home.y.saturating_sub(ring),
                (home.y + ring).min(self.cells.y - 1),
            );
            for cell_y in y0..=y1 {
                let edge_row = cell_y.abs_diff(home.y) == ring;
                if edge_row {
                    for cell_x in x0..=x1 {
                        offer(cell_x, cell_y, &mut best);
                    }
                } else {
                    if home.x >= ring {
                        offer(home.x - ring, cell_y, &mut best);
                    }
                    if ring > 0 && home.x + ring < self.cells.x {
                        offer(home.x + ring, cell_y, &mut best);
                    }
                }
            }
            let beyond = ring * self.cell + 1;
            if best.0 < beyond * beyond {
                break;
            }
        }
        best.1
    }
}
