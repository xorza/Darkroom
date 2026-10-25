//! [`CfaLattice`]: the color of each photosite, and each photosite's neighbours under the same
//! filter.

use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::masked_plane::MaskedPlane;
use crate::math::statistics;
use common::Vec2us;

/// One same-color neighbour of a phase: its offset, and its squared distance, which orders the
/// stencil and decides its ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tap {
    dx: i32,
    dy: i32,
    distance2: i32,
}

/// A color filter array as a lattice: the color of a photosite, and for every phase of the
/// pattern the neighbours under the same filter nearest first, in one flat table.
///
/// The same filter is the same color, and on a Bayer sensor the same phase too: its two greens,
/// one in the red rows (Gr) and one in the blue rows (Gb), are read through different neighbours
/// and amplifier paths and can sit at different levels, so a green taken from the other phase would
/// carry that imbalance into its repair. A Bayer photosite's neighbours are thus the stride-2
/// lattice through it, as its red and blue already were.
///
/// Every consumer that needs a photosite's same-color surroundings reads them here: the defect and
/// null repair, the cosmic-ray scans, and the deinterleave a 2-periodic pattern allows. A count of
/// neighbours always takes whole groups of equal distance: cutting a group by scan order would
/// favour one direction over the others at the same distance, which is how the X-Trans repair came
/// to lean up and to the left.
#[derive(Debug)]
pub(crate) struct CfaLattice {
    period: usize,
    /// The neighbours a median takes, before the tie group it lands in completes.
    count: usize,
    /// The taps of phase `p` are `taps[starts[p]..starts[p + 1]]`, by distance and then by a fixed
    /// order that does not matter, since groups are never cut.
    taps: Vec<Tap>,
    starts: Vec<u32>,
}

/// A stencil holds this many times the neighbours a median takes, in whole shells: with up to
/// `1 − 1/MASKED_HEADROOM` of a stencil masked, the median still takes its full count.
const MASKED_HEADROOM: usize = 2;

/// The square every stencil is cut from, in pixels either side. Its inscribed disc of whole shells
/// holds [`MASKED_HEADROOM`] times every count: X-Trans red and blue, 2 of each 9 photosites,
/// need 48, which a disc of radius 8.3 holds.
const SEARCH_RADIUS: i32 = 12;

impl CfaLattice {
    pub(crate) fn new(cfa: &CfaType) -> Self {
        let period = cfa.period();
        let count = match cfa {
            CfaType::Mono | CfaType::Mosaic(Mosaic::Bayer(_)) => 8,
            CfaType::Mosaic(Mosaic::XTrans(_)) => 24,
        };
        let radius = SEARCH_RADIUS;
        // The pattern is periodic, so a neighbour's color is its phase's: shifting by whole
        // periods keeps every position non-negative.
        let shift = (radius.unsigned_abs() as usize).div_ceil(period) * period;
        let mut taps = Vec::new();
        let mut starts = Vec::with_capacity(period * period + 1);
        for phase in 0..period * period {
            starts.push(u32::try_from(taps.len()).expect("the table is small"));
            let origin = Vec2us::new(phase % period, phase / period);
            let color = cfa.slot_at(origin);
            let first = taps.len();
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if (dx, dy) == (0, 0) {
                        continue;
                    }
                    let pos = Vec2us::new(
                        (origin.x + shift).wrapping_add_signed(dx as isize),
                        (origin.y + shift).wrapping_add_signed(dy as isize),
                    );
                    let same_filter = match cfa {
                        CfaType::Mosaic(Mosaic::Bayer(_)) => dx % 2 == 0 && dy % 2 == 0,
                        CfaType::Mono | CfaType::Mosaic(Mosaic::XTrans(_)) => {
                            cfa.slot_at(pos) == color
                        }
                    };
                    let distance2 = dx * dx + dy * dy;
                    // Within the inscribed disc, every offset of a shell lies in the square.
                    if same_filter && distance2 <= radius * radius {
                        taps.push(Tap { dx, dy, distance2 });
                    }
                }
            }
            taps[first..].sort_by_key(|tap| (tap.distance2, tap.dy, tap.dx));
            let target = MASKED_HEADROOM * count;
            debug_assert!(taps.len() - first >= target, "phase {phase}");
            let last = taps[first + target - 1].distance2;
            let end = first + taps[first..].partition_point(|tap| tap.distance2 <= last);
            taps.truncate(end);
        }
        starts.push(u32::try_from(taps.len()).expect("the table is small"));
        Self {
            period,
            count,
            taps,
            starts,
        }
    }

    fn stencil(&self, pos: Vec2us) -> &[Tap] {
        let phase = (pos.y % self.period) * self.period + pos.x % self.period;
        &self.taps[self.starts[phase] as usize..self.starts[phase + 1] as usize]
    }

    /// The median of `pos`'s nearest neighbours in `plane` under its filter that the plane's
    /// mask does not name, so a defect is never repaired from another: 8 with their ties for mono
    /// and Bayer, which on a Bayer sensor are the four at 2 and the four at 2√2 of its own phase,
    /// and 24 with their ties for X-Trans; fewer where the frame's edge cuts the window, or where
    /// more than the [`MASKED_HEADROOM`] allows is masked. The pixel's own value when none is
    /// valid.
    pub(crate) fn median(
        &self,
        plane: MaskedPlane<'_>,
        pos: Vec2us,
        scratch: &mut Gathered,
    ) -> f32 {
        let MaskedPlane { pixels, mask } = plane;
        let size = pixels.size();
        let count = self.count;
        scratch.values.clear();
        // The distance of the last neighbour taken: the count stops only between tie groups.
        let mut last_distance = None;
        for tap in self.stencil(pos) {
            if scratch.values.len() >= count && last_distance != Some(tap.distance2) {
                break;
            }
            let (Some(x), Some(y)) = (
                pos.x.checked_add_signed(tap.dx as isize),
                pos.y.checked_add_signed(tap.dy as isize),
            ) else {
                continue;
            };
            if x >= size.width || y >= size.height {
                continue;
            }
            let idx = y * size.width + x;
            if !mask.is_some_and(|mask| mask.get(idx)) {
                scratch.values.push(pixels[idx]);
                last_distance = Some(tap.distance2);
            }
        }
        if scratch.values.is_empty() {
            return pixels[size.idx_of(pos)];
        }
        statistics::median_mut(&mut scratch.values)
    }
}

/// The neighbours one [`CfaLattice::median`] took, nearest first: scratch a caller keeps across
/// medians.
#[derive(Debug, Default)]
pub(crate) struct Gathered {
    values: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use common::{Buffer2, Size2us};

    use super::*;
    use crate::bit_buffer2::BitBuffer2;
    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::internals::cfa::XTRANS_PATTERN;

    fn offsets(lattice: &CfaLattice, pos: Vec2us, count: usize) -> Vec<(i32, i32)> {
        let stencil = lattice.stencil(pos);
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

    /// The nearest eight: mono's are the 8-connected ring; every Bayer photosite's, a green's as a
    /// red's, are the stride-2 ring of its own phase at distances 2 and 2√2. A green's four
    /// diagonal greens at √2 are the other phase, and not among them.
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
        let bayer = CfaLattice::new(&CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)));
        assert_eq!(sorted(offsets(&bayer, Vec2us::new(0, 0), 8)), ring(2, 2));
        assert_eq!(sorted(offsets(&bayer, Vec2us::new(1, 0), 8)), ring(2, 2));
        assert_eq!(sorted(offsets(&bayer, Vec2us::new(0, 1), 8)), ring(2, 2));
    }

    /// Every phase's stencil is whole shells of equal distance holding at least twice its count:
    /// every same-filter offset as near as its farthest tap is in it, none farther, so a median
    /// cut short by its count favours no direction at a shared distance, and half the stencil
    /// masked still leaves the full count. X-Trans red and blue hold 48 at least, where the
    /// square stencil they replace held 38.
    #[test]
    fn every_stencil_is_whole_shells_with_headroom() {
        for cfa in [
            CfaType::Mono,
            CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Grbg)),
            CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
        ] {
            let lattice = CfaLattice::new(&cfa);
            let period = lattice.period;
            for phase in 0..period * period {
                let origin = Vec2us::new(phase % period, phase / period);
                let stencil = lattice.stencil(origin);
                assert!(stencil.len() >= 2 * lattice.count, "{cfa:?} phase {phase}");
                let farthest = stencil[stencil.len() - 1].distance2;
                let mut expected = Vec::new();
                for dy in -20i32..=20 {
                    for dx in -20i32..=20 {
                        let at = Vec2us::new(
                            (60 + origin.x).strict_add_signed(dx as isize),
                            (60 + origin.y).strict_add_signed(dy as isize),
                        );
                        let same = match cfa {
                            CfaType::Mosaic(Mosaic::Bayer(_)) => dx % 2 == 0 && dy % 2 == 0,
                            _ => cfa.slot_at(at) == cfa.slot_at(origin),
                        };
                        if same && (dx, dy) != (0, 0) && dx * dx + dy * dy <= farthest {
                            expected.push((dx, dy));
                        }
                    }
                }
                let taken: Vec<(i32, i32)> = stencil.iter().map(|tap| (tap.dx, tap.dy)).collect();
                assert_eq!(sorted(taken), sorted(expected), "{cfa:?} phase {phase}");
            }
        }
    }

    /// With each filter held at its own constant — each X-Trans color, each Bayer phase, so Gr and
    /// Gb differ — every pixel's median is its filter's value, interior or border: a neighbour
    /// under another filter, a Gb beside a Gr included, would mix them.
    #[test]
    fn the_median_reads_only_its_own_filter() {
        for cfa in [
            CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN)),
            CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Gbrg)),
        ] {
            let lattice = CfaLattice::new(&cfa);
            let size = Size2us::new(25, 19);
            let level = |pos: Vec2us| match cfa {
                CfaType::Mosaic(Mosaic::Bayer(_)) => {
                    0.125 * (1 + pos.x % 2 + 2 * (pos.y % 2)) as f32
                }
                _ => 0.125 * (cfa.slot_at(pos) + 1) as f32,
            };
            let pixels = Buffer2::from_fn(size, |x, y| level(Vec2us::new(x, y)));
            let mut scratch = Gathered::default();
            for y in 0..size.height {
                for x in 0..size.width {
                    let pos = Vec2us::new(x, y);
                    assert_eq!(
                        lattice.median(MaskedPlane::from(&pixels), pos, &mut scratch),
                        level(pos),
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
            (3, 3),
            vec![
                10.0, 10.0, 10.0, 10.0, 999.0, 1000.0, 1000.0, 1000.0, 1000.0,
            ],
        );
        let centre = Vec2us::new(1, 1);
        let mut scratch = Gathered::default();
        assert_eq!(
            lattice.median(MaskedPlane::from(&pixels), centre, &mut scratch),
            505.0
        );
        let mut mask = BitBuffer2::new_default(size);
        for idx in [5, 6, 7, 8] {
            mask.set(idx, true);
        }
        assert_eq!(
            lattice.median(
                MaskedPlane {
                    pixels: &pixels,
                    mask: Some(&mask)
                },
                centre,
                &mut scratch
            ),
            10.0
        );
        for idx in [0, 1, 2, 3] {
            mask.set(idx, true);
        }
        assert_eq!(
            lattice.median(
                MaskedPlane {
                    pixels: &pixels,
                    mask: Some(&mask)
                },
                centre,
                &mut scratch
            ),
            999.0
        );
    }

    /// At the corner (0, 0) of a 4 × 4 RGGB frame the red neighbours inside are (2, 0), (0, 2) and
    /// (2, 2) — the window clips the rest — so the median of 50, 60 and 70 is 60. The Gr at (1, 0)
    /// has its own phase's (3, 0), (1, 2) and (3, 2) inside, at 20, 40 and 30, so its median is 30;
    /// its diagonal Gb neighbours (0, 1) and (2, 1), at 1000, would make it 40.
    #[test]
    fn the_median_clips_at_the_border() {
        let lattice = CfaLattice::new(&CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)));
        let mut pixels = Buffer2::new_filled((4, 4), 10.0);
        pixels[(0, 0)] = 999.0;
        pixels[(2, 0)] = 50.0;
        pixels[(0, 2)] = 60.0;
        pixels[(2, 2)] = 70.0;
        pixels[(3, 0)] = 20.0;
        pixels[(1, 2)] = 40.0;
        pixels[(3, 2)] = 30.0;
        pixels[(0, 1)] = 1000.0;
        pixels[(2, 1)] = 1000.0;
        let mut scratch = Gathered::default();
        assert_eq!(
            lattice.median(MaskedPlane::from(&pixels), Vec2us::new(0, 0), &mut scratch),
            60.0
        );
        assert_eq!(
            lattice.median(MaskedPlane::from(&pixels), Vec2us::new(1, 0), &mut scratch),
            30.0
        );
    }
}
