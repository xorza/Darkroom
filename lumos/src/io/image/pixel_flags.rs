use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// One pixel's data-quality bits.
///
/// Each bit is a fact a producer knew exactly when it set it; each consumer decides what the facts
/// mean for it. That is the HST and JWST `DQ` convention: a step records, it does not repair in
/// secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Flags(u8);

impl Flags {
    /// The source holds no measurement: FITS NaN or `BLANK`, or a LibRaw `zero_is_bad` zero. The
    /// sample under it is a finite fill, not data.
    pub(crate) const NO_DATA: Self = Self(1);

    #[inline]
    pub(crate) const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    const fn index(self) -> usize {
        debug_assert!(self.0.is_power_of_two(), "a single flag");
        self.0.trailing_zeros() as usize
    }
}

/// Which pixels of an image carry which [`Flags`].
///
/// One byte per pixel, and one per image rather than one per channel: a pixel is flagged when any
/// of its channels is, which is the granularity the combine's per-pixel gate acts on. An image with
/// no flagged pixel carries none of these at all.
#[derive(Debug, Clone)]
pub(crate) struct PixelFlags {
    bits: Buffer2<u8>,
    /// Pixels holding each flag, by bit position. Kept current by every mutation, so whether a flag
    /// is present anywhere is answered without a scan.
    counts: [usize; 8],
}

impl PixelFlags {
    /// The pixels where any of `planes` holds a non-finite sample, flagged [`Flags::NO_DATA`], or
    /// `None` when none do.
    ///
    /// Every plane must hold `size.width * size.height` samples in row-major order. The caller
    /// establishes from the decoder's own per-plane null counts that there is something to find
    /// before paying for this scan, so the `None` return is the wholly-defensive case rather than
    /// the common one.
    pub(crate) fn of_non_finite(size: Size2us, planes: &[&[f32]]) -> Option<Self> {
        debug_assert!(
            planes
                .iter()
                .all(|plane| plane.len() == size.width * size.height),
            "every plane must match the flag geometry"
        );
        Self::where_true(size, Flags::NO_DATA, |index| {
            planes.iter().any(|plane| !plane[index].is_finite())
        })
    }

    /// `flag`, a single flag, on every pixel whose row-major index satisfies `holds`; `None` when
    /// no pixel does.
    pub(crate) fn where_true(
        size: Size2us,
        flag: Flags,
        holds: impl Fn(usize) -> bool + Sync,
    ) -> Option<Self> {
        let mut bits = Buffer2::new_default(size.width, size.height);
        bits.pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each(|(index, byte)| {
                if holds(index) {
                    *byte = flag.0;
                }
            });
        let flags = Self::from_plane(bits);
        flags.contains(flag).then_some(flags)
    }

    /// Flags restored from the bytes [`Self::bytes`] holds, as a spill wrote them.
    pub(crate) fn from_bytes(size: Size2us, bytes: &[u8]) -> Self {
        Self::from_plane(Buffer2::new(size.width, size.height, bytes.to_vec()))
    }

    fn from_plane(bits: Buffer2<u8>) -> Self {
        let mut counts = [0; 8];
        for &byte in bits.pixels() {
            let mut remaining = byte;
            while remaining != 0 {
                counts[remaining.trailing_zeros() as usize] += 1;
                remaining &= remaining - 1;
            }
        }
        Self { bits, counts }
    }

    pub(crate) const fn size(&self) -> Size2us {
        Size2us::new(self.bits.width(), self.bits.height())
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        self.bits.pixels()
    }

    /// The flags of the pixel at `index`, row-major.
    #[inline]
    pub(crate) fn at(&self, index: usize) -> Flags {
        Flags(self.bits.pixels()[index])
    }

    #[inline]
    pub(crate) fn at_pos(&self, pos: Vec2us) -> Flags {
        self.at(self.size().index_of(pos))
    }

    /// How many pixels hold `flag`, a single flag.
    pub(crate) const fn count(&self, flag: Flags) -> usize {
        self.counts[flag.index()]
    }

    /// Whether any pixel holds `flag`, a single flag.
    pub(crate) const fn contains(&self, flag: Flags) -> bool {
        self.count(flag) > 0
    }

    /// The pixels holding `flag`, as the bit buffer a neighbour search takes, so a pixel
    /// reconstructed from its neighbours never draws on one that has nothing to give.
    pub(crate) fn mask_of(&self, flag: Flags) -> BitBuffer2 {
        let mut mask = BitBuffer2::new_default(self.size());
        let bytes = self.bits.pixels();
        mask.fill_from_predicate(|index| Flags(bytes[index]).intersects(flag));
        mask
    }

    /// The pixels without [`Flags::NO_DATA`] as a plane: `1.0` where a pixel holds a measurement,
    /// `0.0` where it does not.
    ///
    /// The form the measurement consumers want it in — the combine gates on a coverage plane, and
    /// the warp resamples this one through the same kernel as the image to find how much real data
    /// backs each output pixel.
    pub(crate) fn validity_plane(&self) -> Buffer2<f32> {
        let size = self.size();
        let bytes = self.bits.pixels();
        Buffer2::new(
            size.width,
            size.height,
            bytes
                .par_iter()
                .map(|&byte| {
                    if Flags(byte).intersects(Flags::NO_DATA) {
                        0.0
                    } else {
                        1.0
                    }
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::io::image::pixel_flags::{Flags, PixelFlags};
    use crate::math::size2us::Size2us;

    #[test]
    fn a_pixel_has_no_data_when_any_channel_is_non_finite() {
        // 3x2. Red is null at index 1, blue at index 4, and index 1 is finite in the other two —
        // the union is what the flags must hold, because a pixel missing one channel has no
        // complete measurement.
        let size = Size2us::new(3usize, 2usize);
        let red = [0.0, f32::NAN, 2.0, 3.0, 4.0, 5.0];
        let green = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        let blue = [0.0, 1.0, 2.0, 3.0, f32::INFINITY, 5.0];
        let flags = PixelFlags::of_non_finite(size, &[&red, &green, &blue]).unwrap();

        assert_eq!(flags.count(Flags::NO_DATA), 2);
        let mask = flags.mask_of(Flags::NO_DATA);
        for index in 0..6 {
            let expected = index == 1 || index == 4;
            assert_eq!(flags.at(index) == Flags::NO_DATA, expected, "index {index}");
            assert_eq!(mask.get(index), expected, "index {index}");
        }
        assert_eq!(
            flags.validity_plane().pixels(),
            &[1.0, 0.0, 1.0, 1.0, 0.0, 1.0]
        );

        // The same planes with the nulls removed produce no flags at all, so a caller cannot mistake
        // "nothing missing" for "an empty plane".
        assert!(PixelFlags::of_non_finite(size, &[&green]).is_none());

        // The bytes round-trip with their counts, as a spill reads them back.
        let restored = PixelFlags::from_bytes(size, flags.bytes());
        assert_eq!(restored.bytes(), flags.bytes());
        assert_eq!(restored.count(Flags::NO_DATA), 2);
    }

    #[test]
    fn a_wholly_null_plane_flags_every_pixel() {
        let size = Size2us::new(3usize, 2usize);
        let plane = [f32::NAN; 6];
        let flags = PixelFlags::of_non_finite(size, &[&plane]).unwrap();
        assert_eq!(flags.count(Flags::NO_DATA), size.pixel_count());
        assert!(flags.contains(Flags::NO_DATA));
    }
}
