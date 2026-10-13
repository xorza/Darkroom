//! [`PixelFlags`]: which pixels of an image carry which [`QualityFlags`].

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::image::flag_counts::FlagCounts;
use crate::image::quality_flags::QualityFlags;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

/// Which pixels of an image carry which [`QualityFlags`].
///
/// One byte per pixel, and one per image rather than one per channel: a pixel is flagged when any
/// of its channels is, which is the granularity the combine's per-pixel gate acts on. An image with
/// no flagged pixel carries none of these at all.
#[derive(Debug, Clone)]
pub struct PixelFlags {
    bits: Buffer2<u8>,
    /// Pixels holding each flag. Kept current by every mutation, so whether a flag is present
    /// anywhere is answered without a scan.
    counts: FlagCounts,
}

impl PixelFlags {
    /// The flags `flags_at` gives every pixel, by row-major index; `None` when it gives none.
    pub(crate) fn from_fn(
        size: Size2us,
        flags_at: impl Fn(usize) -> QualityFlags + Sync,
    ) -> Option<Self> {
        let mut bits = Buffer2::new_default(size.width, size.height);
        bits.pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each(|(index, byte)| *byte = flags_at(index).byte());
        Self::from_buffer(bits)
    }

    /// Flags from a plane of their bytes whose `counts` its producer tallied as it wrote them;
    /// `None` when no pixel holds one.
    pub(crate) fn from_counted(bits: Buffer2<u8>, counts: FlagCounts) -> Option<Self> {
        debug_assert_eq!(
            counts,
            FlagCounts::of(bits.pixels()),
            "counts of other bytes"
        );
        counts.any().then_some(Self { bits, counts })
    }

    /// OR `flags` into every pixel whose row-major index satisfies `holds`. The counts take the
    /// bits it sets, row by row, rather than a recount of the plane.
    pub(crate) fn or_where(&mut self, flags: QualityFlags, holds: impl Fn(usize) -> bool + Sync) {
        let width = self.size().width.max(1);
        let added = self
            .bits
            .pixels_mut()
            .par_chunks_mut(width)
            .enumerate()
            .map(|(y, row)| {
                let mut added = FlagCounts::default();
                for (x, byte) in row.iter_mut().enumerate() {
                    if holds(y * width + x) {
                        added.tally(QualityFlags::from_byte(flags.byte() & !*byte));
                        *byte |= flags.byte();
                    }
                }
                added
            })
            .reduce(FlagCounts::default, FlagCounts::merged);
        self.counts = self.counts.merged(added);
    }

    /// OR `flags` into the pixels at `indices`; the counts take the bits it sets.
    pub(crate) fn or_at(&mut self, flags: QualityFlags, indices: &[usize]) {
        let bytes = self.bits.pixels_mut();
        for &index in indices {
            self.counts
                .tally(QualityFlags::from_byte(flags.byte() & !bytes[index]));
            bytes[index] |= flags.byte();
        }
    }

    /// A `size` plane holding `flags` at `indices` alone; `None` when there are none, so no pixel
    /// holds one.
    pub(crate) fn at_indices(
        size: Size2us,
        flags: QualityFlags,
        indices: &[usize],
    ) -> Option<Self> {
        if indices.is_empty() || flags == QualityFlags::default() {
            return None;
        }
        let mut plane = Self {
            bits: Buffer2::new_default(size.width, size.height),
            counts: FlagCounts::default(),
        };
        plane.or_at(flags, indices);
        Some(plane)
    }

    /// [`Self::dilate_window`] over the square `radius` either side of each pixel.
    pub(crate) fn dilate(&mut self, radius: usize, fixed: QualityFlags) {
        self.dilate_window(Reach::symmetric(radius), fixed);
    }

    /// OR every flag but `fixed` over the window `reach` spans around each pixel, clipped to the
    /// image: what an operation whose output at a pixel reads its inputs that far away does to the
    /// facts the inputs carried. `fixed` flags stay where they are.
    ///
    /// Separable, and each pass is van Herk–Gil-Werman: an OR over blocks of the window's length
    /// from both ends, so a pixel costs the same at any radius.
    pub(crate) fn dilate_window(&mut self, reach: Reach, fixed: QualityFlags) {
        if reach.before == 0 && reach.after == 0 {
            return;
        }
        let Size2us { width, height } = self.size();
        let soft = !fixed.byte();
        let source: Vec<u8> = self.bits.pixels().iter().map(|&byte| byte & soft).collect();
        let mut horizontal = vec![0u8; source.len()];
        horizontal
            .par_chunks_mut(width)
            .zip(source.par_chunks(width))
            .for_each_init(BlockOrs::default, |scratch, (out, row)| {
                scratch.or_window(row, out, reach);
            });
        // The vertical pass runs the same window over whole rows at once: a row is one element of
        // the column-wise sequence, and OR is element-wise.
        let mut prefix = horizontal.clone();
        let mut suffix = horizontal.clone();
        let block = reach.len();
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
            let parts = WindowParts::of(reach.window(y, height), block, height);
            let (from_suffix, from_prefix) = match parts {
                WindowParts::Both { suffix, prefix } => (Some(suffix), Some(prefix)),
                WindowParts::Prefix(at) => (None, Some(at)),
                WindowParts::Suffix(at) => (Some(at), None),
            };
            for byte in out.iter_mut() {
                *byte &= fixed.byte();
            }
            if let Some(at) = from_suffix {
                or_into(out, &suffix[at * width..(at + 1) * width]);
            }
            if let Some(at) = from_prefix {
                or_into(out, &prefix[at * width..(at + 1) * width]);
            }
        });
        self.counts = FlagCounts::of(self.bits.pixels());
    }

    /// Flags from a plane of their bytes; `None` when no pixel holds one.
    pub(crate) fn from_buffer(bits: Buffer2<u8>) -> Option<Self> {
        let flags = Self::from_plane(bits);
        flags.counts.any().then_some(flags)
    }

    /// Whether any pixel holds a flag other than those in `except`.
    pub(crate) const fn contains_other_than(&self, except: QualityFlags) -> bool {
        self.contains(QualityFlags::from_byte(!except.byte()))
    }

    /// Flags restored from the bytes [`Self::bytes`] holds, as a spill wrote them.
    pub(crate) fn from_bytes(size: Size2us, bytes: &[u8]) -> Self {
        Self::from_plane(Buffer2::new(size.width, size.height, bytes.to_vec()))
    }

    fn from_plane(bits: Buffer2<u8>) -> Self {
        let counts = FlagCounts::of(bits.pixels());
        Self { bits, counts }
    }

    pub const fn size(&self) -> Size2us {
        Size2us::new(self.bits.width(), self.bits.height())
    }

    /// Each pixel's [`QualityFlags::byte`], row-major.
    pub fn bytes(&self) -> &[u8] {
        self.bits.pixels()
    }

    pub(crate) fn into_buffer(self) -> Buffer2<u8> {
        self.bits
    }

    /// The flags of the pixel at `index`, row-major.
    #[inline]
    pub(crate) fn at(&self, index: usize) -> QualityFlags {
        QualityFlags::from_byte(self.bits.pixels()[index])
    }

    /// The flags of the pixel at `pos`.
    #[inline]
    pub fn at_pos(&self, pos: Vec2us) -> QualityFlags {
        self.at(self.size().index_of(pos))
    }

    /// How many pixels hold `flag`.
    ///
    /// # Panics
    ///
    /// If `flag` is not a single flag.
    pub const fn count(&self, flag: QualityFlags) -> u64 {
        self.counts.count(flag)
    }

    /// Whether any pixel holds a flag of `flags`.
    pub const fn contains(&self, flags: QualityFlags) -> bool {
        self.counts.any_of(flags)
    }

    /// The pixels holding `flag`, as the bit buffer a neighbour search takes, so a pixel
    /// reconstructed from its neighbours never draws on one that has nothing to give.
    pub(crate) fn mask_of(&self, flag: QualityFlags) -> BitBuffer2 {
        let mut mask = BitBuffer2::new_default(self.size());
        let bytes = self.bits.pixels();
        mask.fill_from_predicate(|index| QualityFlags::from_byte(bytes[index]).intersects(flag));
        mask
    }

    /// The pixels as a plane: `0.0` where a pixel holds any of `excluded`, `1.0` elsewhere.
    ///
    /// The form the measurement consumers want it in — the combine gates on a coverage plane, and
    /// the warp resamples this one through the same kernel as the image to find how much real data
    /// backs each output pixel.
    pub(crate) fn validity_plane(&self, excluded: QualityFlags) -> Buffer2<f32> {
        let size = self.size();
        let bytes = self.bits.pixels();
        Buffer2::new(
            size.width,
            size.height,
            bytes
                .par_iter()
                .map(|&byte| {
                    if QualityFlags::from_byte(byte).intersects(excluded) {
                        0.0
                    } else {
                        1.0
                    }
                })
                .collect(),
        )
    }
}

/// OR `source` into `target`, element by element.
fn or_into(target: &mut [u8], source: &[u8]) {
    for (target, &source) in target.iter_mut().zip(source) {
        *target |= source;
    }
}

/// One worker's block ORs of a row, from the start and from the end of each block.
#[derive(Debug, Default)]
struct BlockOrs {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
}

impl BlockOrs {
    /// `out[i]` = OR of `row[i − reach.before ..= i + reach.after]`, clipped to the row, through
    /// these block ORs.
    fn or_window(&mut self, row: &[u8], out: &mut [u8], reach: Reach) {
        let len = row.len();
        let block = reach.len();
        let Self { prefix, suffix } = self;
        prefix.clear();
        prefix.extend_from_slice(row);
        suffix.clear();
        suffix.extend_from_slice(row);
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
            let parts = WindowParts::of(reach.window(i, len), block, len);
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
}

/// How far a window reaches either side of the pixel it is for, along one axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reach {
    pub(crate) before: usize,
    pub(crate) after: usize,
}

impl Reach {
    pub(crate) const fn symmetric(radius: usize) -> Self {
        Self {
            before: radius,
            after: radius,
        }
    }

    const fn len(self) -> usize {
        self.before + self.after + 1
    }

    /// The window for position `i` of a sequence of `len`, clipped to it.
    fn window(self, i: usize, len: usize) -> Span {
        Span {
            first: i.saturating_sub(self.before),
            last: (i + self.after).min(len - 1),
        }
    }
}

/// An inclusive run of indices along one axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    first: usize,
    last: usize,
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
    fn of(Span { first, last }: Span, block: usize, len: usize) -> Self {
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
    use crate::image::pixel_flags::PixelFlags;
    use crate::image::quality_flags::QualityFlags;
    use crate::math::size2us::Size2us;

    impl PixelFlags {
        /// The pixels where any of `planes` holds a non-finite sample, flagged
        /// [`QualityFlags::NO_DATA`], or `None` when none do: the flags a FITS decode of those
        /// samples gives.
        pub(crate) fn of_non_finite(size: Size2us, planes: &[&[f32]]) -> Option<Self> {
            debug_assert!(
                planes
                    .iter()
                    .all(|plane| plane.len() == size.width * size.height),
                "every plane must match the flag geometry"
            );
            Self::from_fn(size, |index| {
                if planes.iter().any(|plane| !plane[index].is_finite()) {
                    QualityFlags::NO_DATA
                } else {
                    QualityFlags::default()
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::image::pixel_flags::{PixelFlags, Reach};
    use crate::image::quality_flags::QualityFlags;
    use crate::internals::test_rng::TestRng;
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

        assert_eq!(flags.count(QualityFlags::NO_DATA), 2);
        // A set is held when any of its flags is; the empty set never is.
        for (set, held) in [
            (QualityFlags::NO_DATA, true),
            (QualityFlags::SATURATED, false),
            (QualityFlags::UNMEASURED, true),
            (QualityFlags::RESAMPLE_EXCLUDED, true),
            (QualityFlags::default(), false),
        ] {
            assert_eq!(flags.contains(set), held, "{set:?}");
        }
        assert!(!flags.contains_other_than(QualityFlags::NO_DATA));
        // Every flag is either left out of a resample or carried through it, never both.
        assert_eq!(
            QualityFlags::RESAMPLE_EXCLUDED.union(QualityFlags::RESAMPLE_CARRIED),
            QualityFlags::KNOWN
        );
        assert_eq!(
            QualityFlags::RESAMPLE_EXCLUDED.intersection(QualityFlags::RESAMPLE_CARRIED),
            QualityFlags::default()
        );
        assert!(flags.contains_other_than(QualityFlags::SATURATED));
        let mask = flags.mask_of(QualityFlags::NO_DATA);
        for index in 0..6 {
            let expected = index == 1 || index == 4;
            assert_eq!(
                flags.at(index) == QualityFlags::NO_DATA,
                expected,
                "index {index}"
            );
            assert_eq!(mask.get(index), expected, "index {index}");
        }
        assert_eq!(
            flags.validity_plane(QualityFlags::NO_DATA).pixels(),
            &[1.0, 0.0, 1.0, 1.0, 0.0, 1.0]
        );

        // The same planes with the nulls removed produce no flags at all, so a caller cannot
        // mistake "nothing missing" for "an empty plane".
        assert!(PixelFlags::of_non_finite(size, &[&green]).is_none());

        // The bytes round-trip with their counts, as a spill reads them back.
        let restored = PixelFlags::from_bytes(size, flags.bytes());
        assert_eq!(restored.bytes(), flags.bytes());
        assert_eq!(restored.count(QualityFlags::NO_DATA), 2);
    }

    #[test]
    fn a_wholly_null_plane_flags_every_pixel() {
        let size = Size2us::new(3usize, 2usize);
        let plane = [f32::NAN; 6];
        let flags = PixelFlags::of_non_finite(size, &[&plane]).unwrap();
        assert_eq!(
            flags.count(QualityFlags::NO_DATA),
            u64::try_from(size.pixel_count()).unwrap()
        );
        assert!(flags.contains(QualityFlags::NO_DATA));
    }

    /// The separable dilation equals the brute-force window OR, for symmetric reaches from 1 to
    /// past the image and for the one-sided kernel reach a warp needs, on widths and heights that
    /// do not divide into blocks; `NO_DATA` does not spread.
    #[test]
    fn dilation_matches_the_brute_force_window() {
        let mut rng = TestRng::new(7);
        for (width, height) in [(1usize, 1usize), (7, 5), (13, 17), (32, 3)] {
            let size = Size2us::new(width, height);
            for reach in [1usize, 2, 3, 6, 40]
                .map(Reach::symmetric)
                .into_iter()
                .chain([
                    Reach {
                        before: 2,
                        after: 3,
                    },
                    Reach {
                        before: 0,
                        after: 1,
                    },
                ])
            {
                let initial: Vec<QualityFlags> = (0..size.pixel_count())
                    .map(|_| match rng.next_u64() % 9 {
                        0 => QualityFlags::SATURATED,
                        1 => QualityFlags::NO_DATA,
                        _ => QualityFlags::default(),
                    })
                    .collect();
                let Some(mut flags) = PixelFlags::from_fn(size, |index| initial[index]) else {
                    continue;
                };
                flags.dilate_window(reach, QualityFlags::NO_DATA);
                for y in 0..height {
                    for x in 0..width {
                        let near = |flag: QualityFlags| {
                            let rows = reach.window(y, height);
                            let columns = reach.window(x, width);
                            (rows.first..=rows.last).any(|sy| {
                                (columns.first..=columns.last)
                                    .any(|sx| initial[sy * width + sx] == flag)
                            })
                        };
                        let index = y * width + x;
                        let expected = QualityFlags::default()
                            .union(if near(QualityFlags::SATURATED) {
                                QualityFlags::SATURATED
                            } else {
                                QualityFlags::default()
                            })
                            .union(if initial[index] == QualityFlags::NO_DATA {
                                QualityFlags::NO_DATA
                            } else {
                                QualityFlags::default()
                            });
                        assert_eq!(
                            flags.at(index),
                            expected,
                            "{width}x{height}, {reach:?}, ({x}, {y})"
                        );
                    }
                }
                let saturated = (0..size.pixel_count())
                    .filter(|&index| flags.at(index).intersects(QualityFlags::SATURATED))
                    .count();
                assert_eq!(
                    flags.count(QualityFlags::SATURATED),
                    u64::try_from(saturated).unwrap()
                );
            }
        }
    }
}
