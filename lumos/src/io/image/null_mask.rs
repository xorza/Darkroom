use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Which of an image's pixels carry no measurement.
///
/// FITS marks an undefined sample with IEEE NaN for a floating-point `BITPIX` and with the `BLANK`
/// keyword's value for an integer one (FITS 4.0 §6.3 and the NOST floating-point agreement);
/// `fits_well::Scaling::scale_integer` maps the second onto the first, so both arrive as NaN. Their
/// positions are recorded here rather than left in the samples: nothing downstream tolerates a
/// non-finite sample — an interpolating stage spreads one across its whole kernel footprint, and
/// `median_f32_fast` documents a NaN-free contract — so this is the record of where the data is
/// missing and the samples themselves hold a finite fill.
///
/// One mask per image rather than one per channel: a pixel is null when *any* of its channels is,
/// which is the granularity the combine's per-pixel coverage gate can act on.
#[derive(Debug, Clone)]
pub(crate) struct NullMask {
    nulls: BitBuffer2,
    count: usize,
}

impl NullMask {
    /// The pixels where any of `planes` holds a non-finite sample, or `None` when none do.
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
            "every plane must match the mask geometry"
        );
        let mut nulls = BitBuffer2::new_default(size);
        let mut count = 0;
        let mut index = 0;
        for y in 0..size.height {
            for x in 0..size.width {
                // Walked by position rather than by linear index: `BitBuffer2::set` recovers the
                // point with a divide per pixel, and the row loop already knows it.
                if planes.iter().any(|plane| !plane[index].is_finite()) {
                    nulls.set_at(Vec2us::new(x, y), true);
                    count += 1;
                }
                index += 1;
            }
        }
        (count > 0).then_some(Self { nulls, count })
    }

    /// How many pixels carry no measurement. Never zero — a mask with nothing in it is `None`.
    pub(crate) fn count(&self) -> usize {
        self.count
    }

    /// Whether the pixel at this row-major index carries no measurement.
    ///
    /// Kept rather than deferred to its first production caller — the combine's coverage gate, which
    /// does not read the mask yet — because without it the decoder's output cannot be checked, and
    /// the tests below assert exact null positions through it.
    #[allow(
        dead_code,
        reason = "read by this file's tests until the combine gates on the mask"
    )]
    pub(crate) fn is_null(&self, index: usize) -> bool {
        self.nulls.get(index)
    }

    /// Whether every pixel is null, which makes the frame's samples entirely fill.
    #[allow(
        dead_code,
        reason = "read by this file's tests until the combine gates on the mask"
    )]
    pub(crate) fn covers_everything(&self) -> bool {
        self.count == self.nulls.len
    }
}

#[cfg(test)]
mod tests {
    use crate::io::image::null_mask::NullMask;
    use crate::math::size2us::Size2us;

    #[test]
    fn a_pixel_is_null_when_any_channel_is() {
        // 3x2. Red is null at index 1, blue at index 4, and index 1 is finite in the other two —
        // the union is what the mask must hold, because a pixel missing one channel has no
        // complete measurement.
        let size = Size2us::new(3usize, 2usize);
        let red = [0.0, f32::NAN, 2.0, 3.0, 4.0, 5.0];
        let green = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        let blue = [0.0, 1.0, 2.0, 3.0, f32::INFINITY, 5.0];
        let mask = NullMask::of_non_finite(size, &[&red, &green, &blue]).unwrap();

        assert_eq!(mask.count(), 2);
        for (index, expected) in [
            (0, false),
            (1, true),
            (2, false),
            (3, false),
            (4, true),
            (5, false),
        ] {
            assert_eq!(mask.is_null(index), expected, "index {index}");
        }
        assert!(!mask.covers_everything());

        // The same planes with the nulls removed produce no mask at all, so a caller cannot mistake
        // "nothing missing" for "an empty mask".
        assert!(NullMask::of_non_finite(size, &[&green]).is_none());
    }

    #[test]
    fn a_wholly_null_plane_is_reported_as_such() {
        // Width 3 is deliberately not a multiple of the 64-bit word: `count_ones` masks padding
        // bits, and a fully-set row would over-count if this walked words rather than positions.
        let size = Size2us::new(3usize, 2usize);
        let plane = [f32::NAN; 6];
        let mask = NullMask::of_non_finite(size, &[&plane]).unwrap();
        assert_eq!(mask.count(), 6);
        assert!(mask.covers_everything());
        assert!((0..6).all(|index| mask.is_null(index)));
    }
}
