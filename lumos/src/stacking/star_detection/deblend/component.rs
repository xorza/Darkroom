//! [`Component`]: one connected component of the residual, as the deblenders read it.

use imaginarium::Buffer2;

use crate::math::urect::URect;
use crate::math::vec2us::Vec2us;
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::deblend::{Pixel, dist_sq, nearest_peak_index};
use crate::stacking::star_detection::labeling::LabelMap;
use crate::stacking::star_detection::labeling::component_data::ComponentData;

/// One connected component of the residual: the pixels its label covers, read through its
/// bounding box, and the brightest of them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Component<'a> {
    data: &'a ComponentData,
    residual: &'a Buffer2<f32>,
    labels: &'a LabelMap,
    /// The brightest pixel, the first in raster order among equals.
    peak: Pixel,
}

impl<'a> Component<'a> {
    pub(crate) fn new(
        data: &'a ComponentData,
        residual: &'a Buffer2<f32>,
        labels: &'a LabelMap,
    ) -> Self {
        debug_assert_eq!(
            (residual.width(), residual.height()),
            (labels.width(), labels.height()),
            "residual and labels must have same dimensions"
        );
        let peak = Pixel::brightest(Self::scan(data, residual, labels))
            .expect("a component holds at least one pixel");
        Self {
            data,
            residual,
            labels,
            peak,
        }
    }

    pub(crate) const fn peak(&self) -> Pixel {
        self.peak
    }

    pub(crate) const fn residual(&self) -> &'a Buffer2<f32> {
        self.residual
    }

    pub(crate) const fn bbox(&self) -> URect {
        self.data.bbox
    }

    /// Every pixel of the component, in raster order.
    pub(crate) fn pixels(&self) -> impl Iterator<Item = Pixel> + 'a {
        Self::scan(self.data, self.residual, self.labels)
    }

    /// The component undivided: one region with its brightest pixel as the peak.
    pub(crate) const fn whole(&self) -> Region {
        Region {
            bbox: self.data.bbox,
            peak: self.peak.pos,
            peak_value: self.peak.value,
            area: self.data.area,
        }
    }

    /// The regions `peaks` split the component into, pushed onto `out`: the whole component for
    /// fewer than two peaks, else every pixel assigned to its nearest peak (squared-Euclidean
    /// Voronoi; the first peak wins ties) and one [`Region`] per peak that captured any — the
    /// shared tail of both deblenders. Returns how many it pushed.
    pub(crate) fn split_at(
        &self,
        peaks: &[Pixel],
        scratch: &mut Assignment,
        out: &mut Vec<Region>,
    ) -> usize {
        if peaks.len() <= 1 {
            out.push(self.whole());
            return 1;
        }
        scratch.reset(peaks, self.data.bbox);
        for pixel in self.pixels() {
            let nearest = scratch.nearest(pixel.pos, peaks);
            scratch.bboxes[nearest].include(pixel.pos);
            scratch.areas[nearest] += 1;
        }
        let Assignment { bboxes, areas, .. } = scratch;

        let before = out.len();
        for ((peak, &bbox), &area) in peaks.iter().zip(bboxes.iter()).zip(areas.iter()) {
            if area > 0 {
                debug_assert!(
                    bbox.contains(peak.pos),
                    "assigned region must contain its peak"
                );
                out.push(Region {
                    bbox,
                    peak: peak.pos,
                    peak_value: peak.value,
                    area,
                });
            }
        }
        out.len() - before
    }

    fn scan(
        data: &'a ComponentData,
        residual: &'a Buffer2<f32>,
        labels: &'a LabelMap,
    ) -> impl Iterator<Item = Pixel> + 'a {
        let width = residual.width();
        let bbox = data.bbox;
        let label = data.label;
        (bbox.min.y..bbox.max.y).flat_map(move |y| {
            (bbox.min.x..bbox.max.x).filter_map(move |x| {
                let idx = y * width + x;
                (labels[idx] == label).then(|| Pixel {
                    pos: Vec2us::new(x, y),
                    value: residual[idx],
                })
            })
        })
    }
}

/// Peaks at or under this many are searched in full for every pixel; past it, a cell grid finds a
/// pixel's nearest peak in the few cells around it, so a component holding thousands of stars
/// splits in time proportional to its pixels rather than to pixels × peaks.
const FULL_SEARCH_PEAKS: usize = 8;

/// The per-peak box and area [`Component::split_at`] accumulates, and the cell grid it finds a
/// pixel's nearest peak with, kept by the caller across components.
#[derive(Debug, Default)]
pub(crate) struct Assignment {
    bboxes: Vec<URect>,
    areas: Vec<usize>,
    /// The grid's origin, the component's top-left pixel.
    origin: Vec2us,
    /// The side of a grid cell, in pixels.
    cell: usize,
    /// The grid's extent in cells.
    cells: Vec2us,
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
    fn reset(&mut self, peaks: &[Pixel], bbox: URect) {
        self.bboxes.clear();
        self.bboxes.resize(peaks.len(), URect::empty());
        self.areas.clear();
        self.areas.resize(peaks.len(), 0);
        if peaks.len() <= FULL_SEARCH_PEAKS {
            return;
        }

        self.origin = bbox.min;
        self.cell = (bbox.area() / peaks.len()).isqrt().max(1);
        self.cells = Vec2us::new(
            bbox.width().div_ceil(self.cell),
            bbox.height().div_ceil(self.cell),
        );
        let cell_count = self.cells.x * self.cells.y;
        self.cell_starts.clear();
        self.cell_starts.resize(cell_count + 1, 0);
        for peak in peaks {
            let cell = self.cell_of(peak.pos);
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
        for (index, peak) in peaks.iter().enumerate() {
            let cell = self.cell_of(peak.pos);
            self.by_cell[self.cursor[cell] as usize] = index as u32;
            self.cursor[cell] += 1;
        }
    }

    const fn cell_of(&self, pos: Vec2us) -> usize {
        let x = (pos.x - self.origin.x) / self.cell;
        let y = (pos.y - self.origin.y) / self.cell;
        y * self.cells.x + x
    }

    /// The index of the peak nearest `pos` by squared distance, the first among equals — what
    /// [`nearest_peak_index`] returns, found ring by ring of cells. A peak in a ring further out
    /// than `r` lies at least `r·cell + 1` away, so once the best is strictly nearer than that no
    /// later peak can win, not even a tie with a lower index.
    fn nearest(&self, pos: Vec2us, peaks: &[Pixel]) -> usize {
        if peaks.len() <= FULL_SEARCH_PEAKS {
            return nearest_peak_index(pos, peaks);
        }
        let home = Vec2us::new(
            (pos.x - self.origin.x) / self.cell,
            (pos.y - self.origin.y) / self.cell,
        );
        let mut best = (usize::MAX, usize::MAX);
        let offer = |cell_x: usize, cell_y: usize, best: &mut (usize, usize)| {
            let cell = cell_y * self.cells.x + cell_x;
            let run = self.cell_starts[cell] as usize..self.cell_starts[cell + 1] as usize;
            for &index in &self.by_cell[run] {
                let candidate = (dist_sq(pos, peaks[index as usize].pos), index as usize);
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
