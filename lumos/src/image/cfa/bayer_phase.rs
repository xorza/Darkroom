//! [`BayerPhase`]: one of a 2×2 pattern's four phases, and its photosites as a dense plane.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// One of a 2×2 pattern's four phases: the photosites at `(x + 2i, y + 2j)` from its origin
/// `(x, y)`, all behind one filter. A Bayer phase is one color, and its photosites, side by side,
/// make a dense plane of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BayerPhase {
    origin: Vec2us,
}

impl BayerPhase {
    /// The four phases, row by row: `(0, 0)`, the largest, first.
    pub(crate) const ALL: [Self; 4] = [
        Self::at(0, 0),
        Self::at(1, 0),
        Self::at(0, 1),
        Self::at(1, 1),
    ];

    const fn at(x: usize, y: usize) -> Self {
        Self {
            origin: Vec2us::new(x, y),
        }
    }

    /// The phase's first photosite.
    pub(crate) const fn origin(self) -> Vec2us {
        self.origin
    }

    /// The size of the phase's dense plane in a `size` mosaic.
    pub(crate) const fn size(self, size: Size2us) -> Size2us {
        Size2us::new(
            (size.width - self.origin.x).div_ceil(2),
            (size.height - self.origin.y).div_ceil(2),
        )
    }

    /// The mosaic position of `at` of the phase's dense plane.
    pub(crate) const fn to_mosaic(self, at: Vec2us) -> Vec2us {
        Vec2us::new(2 * at.x + self.origin.x, 2 * at.y + self.origin.y)
    }

    /// Copy the phase of `mosaic` into `plane`, of [`Self::size`], row-major.
    ///
    /// Row-parallel, as [`Self::interleave`] is: they are a few percent of a cosmic-ray pass, but
    /// they are the whole of its serial fraction otherwise.
    pub(crate) fn deinterleave(self, mosaic: &Buffer2<f32>, plane: &mut Buffer2<f32>) {
        let plane_size = self.size(Size2us::from(mosaic));
        let Vec2us { x: a, y: b } = self.origin;
        plane.reshape(plane_size.width, plane_size.height, 0.0);
        plane
            .par_chunks_mut(plane_size.width.max(1))
            .enumerate()
            .for_each(|(y, row)| {
                let source = mosaic.row(2 * y + b);
                for (x, value) in row.iter_mut().enumerate() {
                    *value = source[2 * x + a];
                }
            });
    }

    /// Copy the phase of the `mosaic` mask into `plane`, of [`Self::size`]: [`Self::deinterleave`]
    /// word by word, a plane word from the even or odd bits of two mosaic words.
    ///
    /// The plane's padding stays clear: a plane pixel past its width reads the mosaic past its
    /// width, which is padding or no word at all.
    pub(crate) fn deinterleave_mask(self, mosaic: &BitBuffer2, plane: &mut BitBuffer2) {
        let Vec2us { x: a, y: b } = self.origin;
        plane.clear_to(self.size(mosaic.size));
        plane.par_rows_mut().enumerate().for_each(|(y, row)| {
            let source = mosaic.row_words(2 * y + b);
            let half = |idx: usize| source.get(idx).map_or(0, |&word| even_bits(word >> a));
            for (w, word) in row.iter_mut().enumerate() {
                *word = half(2 * w) | (half(2 * w + 1) << 32);
            }
        });
    }

    /// Write `plane` back into the phase of `mosaic`: the inverse of [`Self::deinterleave`].
    pub(crate) fn interleave(self, plane: &Buffer2<f32>, mosaic: &mut Buffer2<f32>) {
        let size = Size2us::from(&*mosaic);
        debug_assert_eq!(
            Size2us::from(plane),
            self.size(size),
            "the plane is the phase's"
        );
        let Vec2us { x: a, y: b } = self.origin;
        mosaic
            .par_chunks_mut(size.width)
            .enumerate()
            .filter(|(y, _)| y % 2 == b)
            .for_each(|(y, row)| {
                for (x, &value) in plane.row(y / 2).iter().enumerate() {
                    row[2 * x + a] = value;
                }
            });
    }
}

/// The even bits of `word`, packed into its low 32 bits in order.
const fn even_bits(word: u64) -> u64 {
    let mut bits = word & 0x5555_5555_5555_5555;
    bits = (bits | (bits >> 1)) & 0x3333_3333_3333_3333;
    bits = (bits | (bits >> 2)) & 0x0f0f_0f0f_0f0f_0f0f;
    bits = (bits | (bits >> 4)) & 0x00ff_00ff_00ff_00ff;
    bits = (bits | (bits >> 8)) & 0x0000_ffff_0000_ffff;
    (bits | (bits >> 16)) & 0x0000_0000_ffff_ffff
}

#[cfg(test)]
mod tests {
    use imaginarium::Buffer2;

    use crate::bit_buffer2::BitBuffer2;
    use crate::image::cfa::bayer_phase::{BayerPhase, even_bits};
    use crate::math::size2us::Size2us;
    use crate::math::vec2us::Vec2us;

    /// Phase (1, 0) of a 5 × 3 mosaic holding its own indices is columns 1 and 3 of rows 0 and 2:
    /// a 2 × 2 plane of 1, 3, 11, 13, whose (1, 1) is the mosaic's (3, 2). Phase (0, 1) is row 1's
    /// columns 0, 2 and 4.
    #[test]
    fn deinterleave_takes_one_phase() {
        let [_, right, below, _] = BayerPhase::ALL;
        let size = Size2us::new(5, 3);
        let mosaic = Buffer2::from_fn(5, 3, |x, y| (y * 5 + x) as f32);
        let mut plane = Buffer2::default();
        right.deinterleave(&mosaic, &mut plane);
        assert_eq!(Size2us::from(&plane), Size2us::new(2, 2));
        assert_eq!(plane.pixels(), [1.0, 3.0, 11.0, 13.0]);
        assert_eq!(right.size(size), Size2us::new(2, 2));
        assert_eq!(right.to_mosaic(Vec2us::new(1, 1)), Vec2us::new(3, 2));
        below.deinterleave(&mosaic, &mut plane);
        assert_eq!(Size2us::from(&plane), Size2us::new(3, 1));
        assert_eq!(plane.pixels(), [5.0, 7.0, 9.0]);
        let mut written = Buffer2::new_default(5, 3);
        below.interleave(&Buffer2::new(3, 1, vec![-1.0, -2.0, -3.0]), &mut written);
        assert_eq!(
            written.pixels(),
            [
                0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, -2.0, 0.0, -3.0, 0.0, 0.0, 0.0, 0.0, 0.0
            ]
        );
    }

    /// `even_bits` keeps bits 0, 2, 4, 6 of 0b0110_0101, which are 1, 1, 0, 1: 0b1011. All ones
    /// keep 32, and the odd bits alone keep none.
    ///
    /// A mask's phase is the pixels `to_mosaic` reads, on every width about the word edges: 64 and
    /// 128 split a row's words evenly, 63, 65, 127, 129 and 257 leave a phase a bit more or less
    /// than the mosaic's half, and at 100 the plane's two words read past the mosaic row's two.
    /// A width of 1 leaves the odd columns no pixels. A plane reused across phases keeps no bit.
    #[test]
    fn deinterleave_mask_takes_one_phase_word_by_word() {
        assert_eq!(even_bits(0b0110_0101), 0b1011);
        assert_eq!(even_bits(u64::MAX), 0xffff_ffff);
        assert_eq!(even_bits(0xaaaa_aaaa_aaaa_aaaa), 0);
        let mut plane = BitBuffer2::empty();
        for width in [1, 2, 63, 64, 65, 100, 127, 128, 129, 257] {
            let size = Size2us::new(width, 5);
            let mut mosaic = BitBuffer2::new_default(size);
            mosaic.fill_from_predicate(|i| (i * 7 + i / 3).is_multiple_of(5));
            for phase in BayerPhase::ALL {
                phase.deinterleave_mask(&mosaic, &mut plane);
                let plane_size = phase.size(size);
                assert_eq!(plane.size, plane_size, "{width} {phase:?}");
                assert!(plane.padding_is_clear(), "{width} {phase:?}");
                for y in 0..plane_size.height {
                    for x in 0..plane_size.width {
                        let at = Vec2us::new(x, y);
                        assert_eq!(
                            plane.get_at(at),
                            mosaic.get_at(phase.to_mosaic(at)),
                            "{width} {phase:?} {at:?}"
                        );
                    }
                }
            }
        }
    }
}
