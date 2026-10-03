use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// A sample at or past this fraction of its channel's span above black is flagged
/// [`Flags::SATURATED`]. Many sensors clip a little below their nominal white level, which LibRaw's
/// own `adjust_maximum` allows for down to 75% of it; 95% catches those clips and stays above any
/// unsaturated star core.
pub(crate) const SATURATION_FRACTION: f32 = 0.95;

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
    /// The raw value reached the sensor's linear limit, so the sample is a lower bound, not a
    /// measurement.
    pub(crate) const SATURATED: Self = Self(1 << 1);
    /// Hot or cold in the defect map.
    pub(crate) const DEFECT: Self = Self(1 << 2);
    /// Found by L.A.Cosmic.
    pub(crate) const COSMIC_RAY: Self = Self(1 << 3);
    /// The value is an interpolation from neighbours, not the photosite's own.
    pub(crate) const REPAIRED: Self = Self(1 << 4);
    /// The flat divisor was clamped at its floor, so the value is corrected by less than its
    /// vignetting asks.
    pub(crate) const FLAT_FLOOR: Self = Self(1 << 5);

    #[inline]
    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

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
    /// The flags `flags_at` gives every pixel, by row-major index; `None` when it gives none.
    pub(crate) fn from_fn(size: Size2us, flags_at: impl Fn(usize) -> Flags + Sync) -> Option<Self> {
        let mut bits = Buffer2::new_default(size.width, size.height);
        bits.pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each(|(index, byte)| *byte = flags_at(index).0);
        let flags = Self::from_plane(bits);
        flags.counts.iter().any(|&count| count > 0).then_some(flags)
    }

    /// OR `flags` into every pixel whose row-major index satisfies `holds`, creating the plane in
    /// `slot` when it has none and some pixel does.
    pub(crate) fn add_where(
        slot: &mut Option<Self>,
        size: Size2us,
        flags: Flags,
        holds: impl Fn(usize) -> bool + Sync,
    ) {
        match slot {
            Some(existing) => {
                debug_assert_eq!(existing.size(), size, "flags for another geometry");
                existing
                    .bits
                    .pixels_mut()
                    .par_iter_mut()
                    .enumerate()
                    .for_each(|(index, byte)| {
                        if holds(index) {
                            *byte |= flags.0;
                        }
                    });
                existing.counts = counts_of(existing.bits.pixels());
            }
            None => {
                *slot = Self::from_fn(size, |index| {
                    if holds(index) {
                        flags
                    } else {
                        Flags::default()
                    }
                });
            }
        }
    }

    /// OR every flag but `fixed` over the `(2·radius + 1)²` square around each pixel, clipped to the
    /// image: what an operation whose output at a pixel reads its inputs that far away does to the
    /// facts the inputs carried. `fixed` flags stay where they are.
    ///
    /// Separable, and each pass is van Herk–Gil-Werman: an OR over blocks of the window's length
    /// from both ends, so a pixel costs the same at any radius.
    pub(crate) fn dilate(&mut self, radius: usize, fixed: Flags) {
        if radius == 0 {
            return;
        }
        let Size2us { width, height } = self.size();
        let soft = !fixed.0;
        let source: Vec<u8> = self.bits.pixels().iter().map(|&byte| byte & soft).collect();
        let mut horizontal = vec![0u8; source.len()];
        horizontal
            .par_chunks_mut(width)
            .zip(source.par_chunks(width))
            .for_each(|(out, row)| or_window(row, out, radius));
        // The vertical pass runs the same window over whole rows at once: a row is one element of
        // the column-wise sequence, and OR is element-wise.
        let mut prefix = horizontal.clone();
        let mut suffix = horizontal.clone();
        let block = 2 * radius + 1;
        for y in 1..height {
            if y % block != 0 {
                let (before, current) = prefix.split_at_mut(y * width);
                or_into(&mut current[..width], &before[(y - 1) * width..]);
            }
        }
        for y in (0..height.saturating_sub(1)).rev() {
            if (y + 1) % block != 0 {
                let (current, after) = suffix.split_at_mut((y + 1) * width);
                or_into(&mut current[y * width..], &after[..width]);
            }
        }
        let bits = self.bits.pixels_mut();
        bits.par_chunks_mut(width).enumerate().for_each(|(y, out)| {
            let parts = WindowParts::of(
                y.saturating_sub(radius),
                (y + radius).min(height - 1),
                block,
                height,
            );
            let (from_suffix, from_prefix) = match parts {
                WindowParts::Both { suffix, prefix } => (Some(suffix), Some(prefix)),
                WindowParts::Prefix(at) => (None, Some(at)),
                WindowParts::Suffix(at) => (Some(at), None),
            };
            for byte in out.iter_mut() {
                *byte &= fixed.0;
            }
            if let Some(at) = from_suffix {
                or_into(out, &suffix[at * width..(at + 1) * width]);
            }
            if let Some(at) = from_prefix {
                or_into(out, &prefix[at * width..(at + 1) * width]);
            }
        });
        self.counts = counts_of(self.bits.pixels());
    }

    /// Flags restored from the bytes [`Self::bytes`] holds, as a spill wrote them.
    pub(crate) fn from_bytes(size: Size2us, bytes: &[u8]) -> Self {
        Self::from_plane(Buffer2::new(size.width, size.height, bytes.to_vec()))
    }

    fn from_plane(bits: Buffer2<u8>) -> Self {
        let counts = counts_of(bits.pixels());
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

/// How many of `bytes` hold each flag, by bit position.
fn counts_of(bytes: &[u8]) -> [usize; 8] {
    let mut counts = [0; 8];
    for &byte in bytes {
        let mut remaining = byte;
        while remaining != 0 {
            counts[remaining.trailing_zeros() as usize] += 1;
            remaining &= remaining - 1;
        }
    }
    counts
}

/// OR `source` into `target`, element by element.
fn or_into(target: &mut [u8], source: &[u8]) {
    for (target, &source) in target.iter_mut().zip(source) {
        *target |= source;
    }
}

/// `out[i]` = OR of `row[i − radius ..= i + radius]`, clipped to the row.
fn or_window(row: &[u8], out: &mut [u8], radius: usize) {
    let len = row.len();
    let block = 2 * radius + 1;
    let mut prefix = row.to_vec();
    let mut suffix = row.to_vec();
    for i in 1..len {
        if i % block != 0 {
            prefix[i] |= prefix[i - 1];
        }
    }
    for i in (0..len.saturating_sub(1)).rev() {
        if (i + 1) % block != 0 {
            suffix[i] |= suffix[i + 1];
        }
    }
    for (i, value) in out.iter_mut().enumerate() {
        let parts = WindowParts::of(
            i.saturating_sub(radius),
            (i + radius).min(len - 1),
            block,
            len,
        );
        *value = match parts {
            WindowParts::Both {
                suffix: s,
                prefix: p,
            } => suffix[s] | prefix[p],
            WindowParts::Prefix(at) => prefix[at],
            WindowParts::Suffix(at) => suffix[at],
        };
    }
}

/// Which block ORs make up the OR over a window `[first, last]` of a sequence of `len`, where
/// `last − first < block`: a suffix OR runs from an index to its block's end (clipped to the
/// sequence), a prefix OR from its block's start to an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowParts {
    /// The window spans two blocks: the first's suffix and the second's prefix.
    Both {
        suffix: usize,
        prefix: usize,
    },
    Prefix(usize),
    Suffix(usize),
}

impl WindowParts {
    /// One block holds a shorter window only at the sequence's ends: one that starts at its
    /// block's start is a prefix, and one that ends at its block's clipped end is a suffix.
    fn of(first: usize, last: usize, block: usize, len: usize) -> Self {
        debug_assert!(last >= first && last - first < block && last < len);
        if first / block != last / block {
            return Self::Both {
                suffix: first,
                prefix: last,
            };
        }
        if first.is_multiple_of(block) {
            Self::Prefix(last)
        } else {
            debug_assert_eq!(
                last,
                ((first / block + 1) * block - 1).min(len - 1),
                "a window inside one block ends at the block's end"
            );
            Self::Suffix(first)
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::io::image::pixel_flags::{Flags, PixelFlags};
    use crate::math::size2us::Size2us;

    impl PixelFlags {
        /// The pixels where any of `planes` holds a non-finite sample, flagged [`Flags::NO_DATA`],
        /// or `None` when none do: the flags a FITS decode of those samples gives.
        pub(crate) fn of_non_finite(size: Size2us, planes: &[&[f32]]) -> Option<Self> {
            debug_assert!(
                planes
                    .iter()
                    .all(|plane| plane.len() == size.width * size.height),
                "every plane must match the flag geometry"
            );
            Self::from_fn(size, |index| {
                if planes.iter().any(|plane| !plane[index].is_finite()) {
                    Flags::NO_DATA
                } else {
                    Flags::default()
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::internals::test_rng::TestRng;
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

    /// The separable dilation equals the brute-force square OR, for every radius from 1 to past the
    /// image and for widths and heights that do not divide into blocks; `NO_DATA` does not spread.
    #[test]
    fn dilation_matches_the_brute_force_square() {
        let mut rng = TestRng::new(7);
        for (width, height) in [(1usize, 1usize), (7, 5), (13, 17), (32, 3)] {
            let size = Size2us::new(width, height);
            for radius in [1usize, 2, 3, 6, 40] {
                let initial: Vec<Flags> = (0..size.pixel_count())
                    .map(|_| match rng.next_u64() % 9 {
                        0 => Flags::SATURATED,
                        1 => Flags::NO_DATA,
                        _ => Flags::default(),
                    })
                    .collect();
                let Some(mut flags) = PixelFlags::from_fn(size, |index| initial[index]) else {
                    continue;
                };
                flags.dilate(radius, Flags::NO_DATA);
                for y in 0..height {
                    for x in 0..width {
                        let near = |flag: Flags| {
                            (y.saturating_sub(radius)..=(y + radius).min(height - 1)).any(|sy| {
                                (x.saturating_sub(radius)..=(x + radius).min(width - 1))
                                    .any(|sx| initial[sy * width + sx] == flag)
                            })
                        };
                        let index = y * width + x;
                        let expected = Flags::default()
                            .union(if near(Flags::SATURATED) {
                                Flags::SATURATED
                            } else {
                                Flags::default()
                            })
                            .union(if initial[index] == Flags::NO_DATA {
                                Flags::NO_DATA
                            } else {
                                Flags::default()
                            });
                        assert_eq!(
                            flags.at(index),
                            expected,
                            "{width}x{height}, radius {radius}, ({x}, {y})"
                        );
                    }
                }
                let saturated = (0..size.pixel_count())
                    .filter(|&index| flags.at(index).intersects(Flags::SATURATED))
                    .count();
                assert_eq!(flags.count(Flags::SATURATED), saturated);
            }
        }
    }
}
