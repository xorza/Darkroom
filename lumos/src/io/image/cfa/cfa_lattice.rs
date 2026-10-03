//! [`CfaLattice`]: the colour of each photosite, and each colour's same-colour neighbours.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::io::image::cfa::CfaType;
use crate::math::size2us::Size2us;
use crate::math::statistics::median_mut;
use crate::math::vec2us::Vec2us;

/// One same-colour neighbour of a phase: its offset, and its squared distance, which orders the
/// stencil and decides its ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tap {
    dx: i32,
    dy: i32,
    distance2: i32,
}

/// A colour filter array as a lattice: the colour of a photosite, and for every phase of the
/// pattern the same-colour neighbours nearest first, in one flat table.
///
/// Every consumer that needs a photosite's same-colour surroundings reads them here: the defect and
/// null repair, the cosmic-ray scans, and the deinterleave a 2-periodic pattern allows. A count of
/// neighbours always takes whole groups of equal distance: cutting a group by scan order would
/// favour one direction over the others at the same distance, which is how the X-Trans repair came
/// to lean up and to the left.
#[derive(Debug)]
pub(crate) struct CfaLattice {
    period: usize,
    /// The taps of phase `p` are `taps[starts[p]..starts[p + 1]]`, by distance and then by a fixed
    /// order that does not matter, since groups are never cut.
    taps: Vec<Tap>,
    starts: Vec<u32>,
}

impl CfaLattice {
    pub(crate) fn new(cfa: &CfaType) -> Self {
        // Each window holds more of every colour than the largest count a consumer asks for, 24,
        // with the tie group that count may land in.
        let (period, radius): (usize, i32) = match cfa {
            CfaType::Mono => (1, 3),
            CfaType::Bayer(_) => (2, 5),
            CfaType::XTrans(_) => (6, 6),
        };
        // The pattern is periodic, so a neighbour's colour is its phase's: shifting by whole periods
        // keeps every position non-negative.
        let shift = (radius.unsigned_abs() as usize).div_ceil(period) * period;
        let mut taps = Vec::new();
        let mut starts = Vec::with_capacity(period * period + 1);
        for phase in 0..period * period {
            starts.push(u32::try_from(taps.len()).expect("the table is small"));
            let origin = Vec2us::new(phase % period, phase / period);
            let colour = cfa.color_at(origin);
            let first = taps.len();
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if (dx, dy) == (0, 0) {
                        continue;
                    }
                    let position = Vec2us::new(
                        (origin.x + shift).wrapping_add_signed(dx as isize),
                        (origin.y + shift).wrapping_add_signed(dy as isize),
                    );
                    if cfa.color_at(position) == colour {
                        taps.push(Tap {
                            dx,
                            dy,
                            distance2: dx * dx + dy * dy,
                        });
                    }
                }
            }
            taps[first..].sort_by_key(|tap| (tap.distance2, tap.dy, tap.dx));
        }
        starts.push(u32::try_from(taps.len()).expect("the table is small"));
        Self {
            period,
            taps,
            starts,
        }
    }

    fn stencil(&self, position: Vec2us) -> &[Tap] {
        let phase = (position.y % self.period) * self.period + position.x % self.period;
        &self.taps[self.starts[phase] as usize..self.starts[phase + 1] as usize]
    }

    /// The nearest `count` same-colour neighbours of `position` that lie inside `size` and that
    /// `accept` takes, by flat index into `values`, and every other one at the distance of the last:
    /// a group of equal distance is taken whole. Fewer when the window runs out.
    pub(crate) fn gather(
        &self,
        values: &[f32],
        size: Size2us,
        position: Vec2us,
        count: usize,
        accept: impl Fn(usize) -> bool,
        out: &mut Gathered,
    ) {
        out.values.clear();
        out.distances.clear();
        for tap in self.stencil(position) {
            if out.values.len() >= count && out.distances.last() != Some(&tap.distance2) {
                break;
            }
            let (Some(x), Some(y)) = (
                position.x.checked_add_signed(tap.dx as isize),
                position.y.checked_add_signed(tap.dy as isize),
            ) else {
                continue;
            };
            if x >= size.width || y >= size.height {
                continue;
            }
            let index = y * size.width + x;
            if accept(index) {
                out.values.push(values[index]);
                out.distances.push(tap.distance2);
            }
        }
    }

    /// The median of `position`'s nearest same-colour neighbours that `mask` does not name, so a
    /// defect is never repaired from another: 8 with their ties for mono and Bayer, which for a
    /// Bayer green are its four diagonal greens at √2 and the four at 2, and 24 with their ties for
    /// X-Trans. The pixel's own value when none is valid.
    pub(crate) fn median(
        &self,
        pixels: &Buffer2<f32>,
        position: Vec2us,
        mask: Option<&BitBuffer2>,
        scratch: &mut Gathered,
    ) -> f32 {
        let size = Size2us::new(pixels.width(), pixels.height());
        let count = if self.period == 6 { 24 } else { 8 };
        self.gather(
            pixels.pixels(),
            size,
            position,
            count,
            |index| !mask.is_some_and(|mask| mask.get(index)),
            scratch,
        );
        if scratch.values.is_empty() {
            return pixels[size.index_of(position)];
        }
        median_mut(&mut scratch.values)
    }

    /// Copy phase `(a, b)` of a 2-periodic mosaic into a dense plane of its
    /// [`Self::phase_size`], row-major. Each phase of a Bayer pattern is one colour.
    ///
    /// Row-parallel, as [`Self::interleave`] is: they are a few percent of a cosmic-ray pass, but
    /// they are the whole of its serial fraction otherwise.
    pub(crate) fn deinterleave(
        &self,
        mosaic: &[f32],
        size: Size2us,
        phase: Vec2us,
        plane: &mut Vec<f32>,
    ) {
        debug_assert_eq!(self.period, 2);
        let plane_size = Self::phase_size(size, phase);
        plane.resize(plane_size.pixel_count(), 0.0);
        plane
            .par_chunks_mut(plane_size.width.max(1))
            .enumerate()
            .for_each(|(y, row)| {
                let source = &mosaic[(2 * y + phase.y) * size.width..][..size.width];
                for (x, value) in row.iter_mut().enumerate() {
                    *value = source[2 * x + phase.x];
                }
            });
    }

    /// Write a dense plane back into phase `(a, b)` of the mosaic: the inverse of
    /// [`Self::deinterleave`].
    pub(crate) fn interleave(
        &self,
        plane: &[f32],
        size: Size2us,
        phase: Vec2us,
        mosaic: &mut [f32],
    ) {
        debug_assert_eq!(self.period, 2);
        let plane_width = Self::phase_size(size, phase).width;
        mosaic
            .par_chunks_mut(size.width)
            .enumerate()
            .filter(|(y, _)| y % 2 == phase.y)
            .for_each(|(y, row)| {
                for (x, &value) in plane[(y / 2) * plane_width..][..plane_width]
                    .iter()
                    .enumerate()
                {
                    row[2 * x + phase.x] = value;
                }
            });
    }

    /// The size of phase `(a, b)`'s dense plane in a `size` mosaic.
    pub(crate) const fn phase_size(size: Size2us, phase: Vec2us) -> Size2us {
        Size2us::new(
            (size.width - phase.x).div_ceil(2),
            (size.height - phase.y).div_ceil(2),
        )
    }
}

/// The values one [`CfaLattice::gather`] took, with each one's squared distance, nearest first.
#[derive(Debug, Default)]
pub(crate) struct Gathered {
    pub(crate) values: Vec<f32>,
    pub(crate) distances: Vec<i32>,
}

impl Gathered {
    /// How many of the values the nearest `count` take with their ties: the end of the group of
    /// equal distance that the `count`-th value is in.
    pub(crate) fn tie_end(&self, count: usize) -> usize {
        if count >= self.distances.len() {
            return self.distances.len();
        }
        let last = self.distances[count - 1];
        count
            + self.distances[count..]
                .iter()
                .take_while(|&&distance| distance == last)
                .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internals::cfa::XTRANS_PATTERN;
    use crate::io::raw::demosaic::bayer::CfaPattern;

    fn offsets(lattice: &CfaLattice, position: Vec2us, count: usize) -> Vec<(i32, i32)> {
        let stencil = lattice.stencil(position);
        let last = stencil[count - 1].distance2;
        stencil
            .iter()
            .take_while(|tap| tap.distance2 <= last)
            .map(|tap| (tap.dx, tap.dy))
            .collect()
    }

    fn sorted(mut offsets: Vec<(i32, i32)>) -> Vec<(i32, i32)> {
        offsets.sort_unstable();
        offsets
    }

    /// The nearest eight: mono's are the 8-connected ring; a Bayer red's are the stride-2 ring at
    /// distances 2 and 2√2; a Bayer green's are its four diagonal greens at √2 and the four at 2,
    /// where the stride-2 ring alone left the nearest four out (review 18.3).
    #[test]
    fn the_nearest_eight_are_the_nearest_rings() {
        let ring = |step: i32, diagonal: i32| {
            sorted(vec![
                (-diagonal, -diagonal),
                (0, -step),
                (diagonal, -diagonal),
                (-step, 0),
                (step, 0),
                (-diagonal, diagonal),
                (0, step),
                (diagonal, diagonal),
            ])
        };
        let mono = CfaLattice::new(&CfaType::Mono);
        assert_eq!(sorted(offsets(&mono, Vec2us::new(3, 3), 8)), ring(1, 1));
        let bayer = CfaLattice::new(&CfaType::Bayer(CfaPattern::Rggb));
        assert_eq!(sorted(offsets(&bayer, Vec2us::new(0, 0), 8)), ring(2, 2));
        assert_eq!(sorted(offsets(&bayer, Vec2us::new(1, 0), 8)), ring(2, 1));
    }

    /// Every X-Trans phase takes whole shells of equal distance: no same-colour neighbour outside the
    /// selection is as near as the farthest one in it, so no direction at a shared distance is
    /// favoured (review 18.4). And the selection holds at least the 24 asked for.
    #[test]
    fn x_trans_takes_whole_shells() {
        let lattice = CfaLattice::new(&CfaType::XTrans(XTRANS_PATTERN));
        for phase in 0..36 {
            let position = Vec2us::new(phase % 6, phase / 6);
            let stencil = lattice.stencil(position);
            let taken = offsets(&lattice, position, 24);
            assert!(taken.len() >= 24, "phase {phase}");
            let farthest = stencil[taken.len() - 1].distance2;
            assert!(
                stencil[taken.len()..]
                    .iter()
                    .all(|tap| tap.distance2 > farthest),
                "phase {phase}"
            );
        }
    }

    /// With each colour held at its own constant, every pixel's same-colour median is its colour's
    /// value, interior or border: a neighbour of another colour would mix them.
    #[test]
    fn the_median_reads_only_its_own_colour() {
        for cfa in [
            CfaType::XTrans(XTRANS_PATTERN),
            CfaType::Bayer(CfaPattern::Gbrg),
        ] {
            let lattice = CfaLattice::new(&cfa);
            let size = Size2us::new(25, 19);
            let level = |colour: u8| 0.125 * f32::from(colour + 1);
            let pixels = Buffer2::new(
                size.width,
                size.height,
                (0..size.pixel_count())
                    .map(|index| level(cfa.color_at(Vec2us::new(index % 25, index / 25))))
                    .collect(),
            );
            let mut scratch = Gathered::default();
            for y in 0..size.height {
                for x in 0..size.width {
                    let position = Vec2us::new(x, y);
                    assert_eq!(
                        lattice.median(&pixels, position, None, &mut scratch),
                        level(cfa.color_at(position)),
                        "{cfa:?} at ({x}, {y})"
                    );
                }
            }
        }
    }

    /// A defect is never repaired from another. A 3 × 3 mono frame with the centre defective, four
    /// neighbours at 10 and four at 1000: unmasked the median is (10 + 1000)/2 = 505; with the
    /// four high ones masked, 10; with all eight masked, the centre's own 999.
    #[test]
    fn the_median_skips_masked_neighbours() {
        let lattice = CfaLattice::new(&CfaType::Mono);
        let size = Size2us::new(3, 3);
        let pixels = Buffer2::new(
            3,
            3,
            vec![
                10.0, 10.0, 10.0, 10.0, 999.0, 1000.0, 1000.0, 1000.0, 1000.0,
            ],
        );
        let centre = Vec2us::new(1, 1);
        let mut scratch = Gathered::default();
        assert_eq!(lattice.median(&pixels, centre, None, &mut scratch), 505.0);
        let mut mask = BitBuffer2::new_default(size);
        for index in [5, 6, 7, 8] {
            mask.set(index, true);
        }
        assert_eq!(
            lattice.median(&pixels, centre, Some(&mask), &mut scratch),
            10.0
        );
        for index in [0, 1, 2, 3] {
            mask.set(index, true);
        }
        assert_eq!(
            lattice.median(&pixels, centre, Some(&mask), &mut scratch),
            999.0
        );
    }

    /// At the corner (0, 0) of a 4 × 4 RGGB frame the red neighbours inside are (2, 0), (0, 2) and
    /// (2, 2) — the window clips the rest — so the median of 50, 60 and 70 is 60.
    #[test]
    fn the_median_clips_at_the_border() {
        let lattice = CfaLattice::new(&CfaType::Bayer(CfaPattern::Rggb));
        let mut pixels = Buffer2::new_filled(4, 4, 10.0);
        pixels[(0, 0)] = 999.0;
        pixels[(2, 0)] = 50.0;
        pixels[(0, 2)] = 60.0;
        pixels[(2, 2)] = 70.0;
        assert_eq!(
            lattice.median(&pixels, Vec2us::new(0, 0), None, &mut Gathered::default()),
            60.0
        );
    }

    /// The tie end completes the group the count lands in: distances [1, 1, 2, 2, 2, 5] take 3 as
    /// 5, 2 as 2, 6 as 6, and more than there are as all.
    #[test]
    fn the_tie_end_completes_a_group() {
        let gathered = Gathered {
            values: vec![0.0; 6],
            distances: vec![1, 1, 2, 2, 2, 5],
        };
        assert_eq!(
            [3, 2, 6, 9].map(|count| gathered.tie_end(count)),
            [5, 2, 6, 6]
        );
    }

    /// Phase (1, 0) of a 5 × 3 mosaic holding its own indices is columns 1 and 3 of rows 0 and 2:
    /// a 2 × 2 plane of 1, 3, 11, 13. Phase (0, 1) is row 1's columns 0, 2 and 4.
    #[test]
    fn deinterleave_takes_one_phase() {
        let lattice = CfaLattice::new(&CfaType::Bayer(CfaPattern::Rggb));
        let size = Size2us::new(5, 3);
        let mosaic: Vec<f32> = (0..15).map(|index| index as f32).collect();
        let mut plane = Vec::new();
        lattice.deinterleave(&mosaic, size, Vec2us::new(1, 0), &mut plane);
        assert_eq!(plane, [1.0, 3.0, 11.0, 13.0]);
        assert_eq!(
            CfaLattice::phase_size(size, Vec2us::new(1, 0)),
            Size2us::new(2, 2)
        );
        lattice.deinterleave(&mosaic, size, Vec2us::new(0, 1), &mut plane);
        assert_eq!(plane, [5.0, 7.0, 9.0]);
        let mut written = vec![0.0; 15];
        lattice.interleave(&[-1.0, -2.0, -3.0], size, Vec2us::new(0, 1), &mut written);
        assert_eq!(
            written,
            [
                0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, -2.0, 0.0, -3.0, 0.0, 0.0, 0.0, 0.0, 0.0
            ]
        );
    }
}
