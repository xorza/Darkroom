//! [`ComponentPixels`]: one component's pixels, found by position in memory proportional to them.

use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::pixel::Pixel;
use crate::star_detection::star_detection_config::extraction_config::Connectivity;
use glam::USizeVec2;

/// One component's pixels in raster order, with where each row of its box starts.
///
/// A position finds its pixel by a search in its row, so the memory is in proportion to the
/// component's pixels, not to its box: a satellite trail across the frame has a box of the whole
/// frame and pixels along one line. Each pixel's first possible neighbour in the rows above and
/// below is found once, by [`Self::link`], so a walk over neighbours, which a deblend repeats at
/// every level, reads them without a search.
#[derive(Debug, Default)]
pub(crate) struct ComponentPixels {
    pub(crate) pixels: Vec<Pixel>,
    /// Where each row of the box starts in `pixels`, with the end after the last row.
    row_starts: Vec<u32>,
    /// The box's top row.
    top: usize,
    /// Per pixel once [`Self::link`]ed: the first pixel of the row above at or right of its left
    /// neighbour's column, or that row's end.
    above: Vec<u32>,
    /// The same in the row below.
    below: Vec<u32>,
}

impl ComponentPixels {
    /// Replace the pixels with `component`'s.
    pub(crate) fn fill(&mut self, component: &Component<'_>) {
        let bbox = component.bbox();
        self.pixels.clear();
        self.pixels.extend(component.pixels());
        debug_assert!(
            u32::try_from(self.pixels.len()).is_ok(),
            "a component cannot exceed u32 pixels"
        );
        self.top = bbox.min.y;
        self.row_starts.clear();
        let mut cursor = 0;
        for y in bbox.min.y..bbox.max.y {
            self.row_starts.push(cursor as u32);
            while cursor < self.pixels.len() && self.pixels[cursor].pos().y == y {
                cursor += 1;
            }
        }
        self.row_starts.push(cursor as u32);
        self.above.clear();
        self.below.clear();
    }

    /// Link each pixel to its first possible neighbours in the rows above and below, walking each
    /// pair of rows together: what [`Self::for_each_neighbour`] reads, which a tree's build walks
    /// at every level.
    pub(crate) fn link(&mut self) {
        let Self {
            pixels,
            row_starts,
            above,
            below,
            ..
        } = self;
        above.clear();
        above.resize(pixels.len(), 0);
        below.clear();
        below.resize(pixels.len(), 0);
        let rows = row_starts.len() - 1;
        let span = |row: usize| row_starts[row] as usize..row_starts[row + 1] as usize;
        for row in 0..rows {
            let cur = span(row);
            let up = if row > 0 {
                span(row - 1)
            } else {
                cur.start..cur.start
            };
            let down = if row + 1 < rows {
                span(row + 1)
            } else {
                cur.end..cur.end
            };
            let (mut j, mut k) = (up.start, down.start);
            for i in cur {
                let low = pixels[i].pos().x.saturating_sub(1);
                while j < up.end && pixels[j].pos().x < low {
                    j += 1;
                }
                while k < down.end && pixels[k].pos().x < low {
                    k += 1;
                }
                above[i] = j as u32;
                below[i] = k as u32;
            }
        }
    }

    /// The pixels of row `y`, empty outside the box.
    fn row(&self, y: usize) -> Row<'_> {
        let empty = Row {
            start: 0,
            pixels: &[],
        };
        let Some(row) = y.checked_sub(self.top) else {
            return empty;
        };
        if row + 1 >= self.row_starts.len() {
            return empty;
        }
        let start = self.row_starts[row] as usize;
        Row {
            start,
            pixels: &self.pixels[start..self.row_starts[row + 1] as usize],
        }
    }

    /// Give back what the vectors of each pixel hold past `pixels` of them.
    pub(crate) fn shrink_to(&mut self, pixels: usize) {
        for per_pixel in [&mut self.above, &mut self.below] {
            per_pixel.clear();
            per_pixel.shrink_to(pixels);
        }
        self.pixels.clear();
        self.pixels.shrink_to(pixels);
    }

    /// The index of the pixel at `pos`, when the component holds it.
    pub(crate) fn idx_of(&self, pos: USizeVec2) -> Option<usize> {
        let row = self.row(pos.y);
        row.pixels
            .binary_search_by_key(&pos.x, |pixel| pixel.pos().x)
            .ok()
            .map(|offset| row.start + offset)
    }

    /// Call `visit` with the index of each neighbour of pixel `idx` that the component holds,
    /// under `connectivity`.
    pub(super) fn for_each_neighbour(
        &self,
        idx: usize,
        connectivity: Connectivity,
        mut visit: impl FnMut(usize),
    ) {
        debug_assert_eq!(self.above.len(), self.pixels.len(), "the rows are linked");
        let pos = self.pixels[idx].pos();
        if idx > 0 && self.pixels[idx - 1].pos() == USizeVec2::new(pos.x.wrapping_sub(1), pos.y) {
            visit(idx - 1);
        }
        if idx + 1 < self.pixels.len()
            && self.pixels[idx + 1].pos() == USizeVec2::new(pos.x + 1, pos.y)
        {
            visit(idx + 1);
        }
        let (low, high) = match connectivity {
            Connectivity::Four => (pos.x, pos.x),
            Connectivity::Eight => (pos.x.saturating_sub(1), pos.x + 1),
        };
        let row = pos.y - self.top;
        let rows = self.row_starts.len() - 1;
        let mut scan = |mut at: usize, end: usize| {
            while at < end && self.pixels[at].pos().x < low {
                at += 1;
            }
            while at < end && self.pixels[at].pos().x <= high {
                visit(at);
                at += 1;
            }
        };
        if row > 0 {
            scan(self.above[idx] as usize, self.row_starts[row] as usize);
        }
        if row + 1 < rows {
            scan(self.below[idx] as usize, self.row_starts[row + 2] as usize);
        }
    }
}

/// The pixels of one row of a component, and the index of the first among all of its pixels.
#[derive(Debug)]
struct Row<'a> {
    start: usize,
    pixels: &'a [Pixel],
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::star_detection::deblend::component_pixels::ComponentPixels;

    impl ComponentPixels {
        /// Room for `pixels` in each vector of each pixel.
        pub(crate) fn reserve_per_pixel(&mut self, pixels: usize) {
            self.pixels.reserve(pixels);
            self.above.reserve(pixels);
            self.below.reserve(pixels);
        }

        /// The most room any vector of each pixel holds.
        pub(crate) fn per_pixel_capacity(&self) -> usize {
            self.pixels
                .capacity()
                .max(self.above.capacity())
                .max(self.below.capacity())
        }
    }
}

#[cfg(test)]
mod tests {
    use common::Buffer2;

    use crate::star_detection::deblend::component::Component;
    use crate::star_detection::deblend::component_pixels::ComponentPixels;
    use crate::star_detection::labeling::LabelMap;
    use crate::star_detection::labeling::component_data::ComponentData;
    use crate::star_detection::star_detection_config::extraction_config::Connectivity;
    use common::{USize2, USizeRect2};
    use glam::USizeVec2;

    /// A plus of five pixels and one pixel diagonal to its arm: under 8-connectivity the arm's end
    /// (5, 4) reaches the diagonal (6, 3), the centre and the plus's other two diagonal pixels;
    /// under 4-connectivity the centre alone. A position the component does not hold has no index,
    /// and the row index holds a start per row of the box and an end.
    #[test]
    fn neighbours_follow_the_connectivity() {
        let size = USize2::new(10, 10);
        let lit = [(4, 3), (3, 4), (4, 4), (5, 4), (4, 5), (6, 3)];
        let mut pixels = Buffer2::new_filled(size, 0.0f32);
        let mut labels = Buffer2::new_filled(size, 0u32);
        let mut bbox = USizeRect2::empty();
        for &(x, y) in &lit {
            pixels[(x, y)] = 1.0;
            labels[(x, y)] = 1;
            bbox.include(USizeVec2::new(x, y));
        }
        let labels = LabelMap::from_raw(&labels, 1);
        let data = ComponentData {
            bbox,
            label: 1,
            area: lit.len(),
        };
        let mut component = ComponentPixels::default();
        component.fill(&Component::new(&data, &pixels, &labels));
        component.link();
        assert_eq!(component.row_starts.len(), 3 + 1);
        assert_eq!(component.idx_of(USizeVec2::new(7, 3)), None);
        let arm = component.idx_of(USizeVec2::new(5, 4)).unwrap();
        let reached = |connectivity| {
            let mut poss = Vec::new();
            component.for_each_neighbour(arm, connectivity, |idx| {
                let pos = component.pixels[idx].pos();
                poss.push((pos.x, pos.y));
            });
            poss.sort_unstable();
            poss
        };
        assert_eq!(
            reached(Connectivity::Eight),
            [(4, 3), (4, 4), (4, 5), (6, 3)]
        );
        assert_eq!(reached(Connectivity::Four), [(4, 4)]);
    }
}
