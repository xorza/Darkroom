//! [`PixelFlags`]: which pixels of an image carry which [`QualityFlags`].

use std::ops::Range;
use std::slice;

use common::par;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::bit_buffer2::BitBuffer2;
use crate::image::flag_counts::FlagCounts;
use crate::image::quality_flags::QualityFlags;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::scratch_pool::{ScratchPool, Shapeless};
use crate::unsafe_send_ptr::UnsafeSendPtr;

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
        par::iter_mut(bits.pixels_mut())
            .enumerate()
            .for_each(|(idx, byte)| *byte = flags_at(idx).byte());
        Self::from_bytes(bits)
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
    pub(super) fn or_where(&mut self, flags: QualityFlags, holds: impl Fn(usize) -> bool + Sync) {
        let width = self.size().width.max(1);
        let added = par::chunks_mut(self.bits.pixels_mut(), width)
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

    /// OR `flags` into the pixels at `idxs`; the counts take the bits it sets.
    pub(super) fn or_at(&mut self, flags: QualityFlags, idxs: &[usize]) {
        let bytes = self.bits.pixels_mut();
        for &idx in idxs {
            self.counts
                .tally(QualityFlags::from_byte(flags.byte() & !bytes[idx]));
            bytes[idx] |= flags.byte();
        }
    }

    /// A `size` plane holding `flags` at `idxs` alone; `None` when there are none, so no pixel
    /// holds one.
    pub(super) fn at_idxs(size: Size2us, flags: QualityFlags, idxs: &[usize]) -> Option<Self> {
        if idxs.is_empty() || flags == QualityFlags::default() {
            return None;
        }
        let mut plane = Self {
            bits: Buffer2::new_default(size.width, size.height),
            counts: FlagCounts::default(),
        };
        plane.or_at(flags, idxs);
        Some(plane)
    }

    /// [`Self::dilate_window`] over the square `radius` either side of each pixel.
    pub(crate) fn dilate(&mut self, radius: usize, fixed: QualityFlags) {
        self.dilate_window(Reach::symmetric(radius), fixed);
    }

    /// OR every flag but `fixed` over the window `reach` spans around each pixel, clipped to the
    /// image: what an operation whose output at a pixel reads its inputs that far away does to the
    /// facts the inputs carried. `fixed` flags stay where they are.
    fn dilate_window(&mut self, reach: Reach, fixed: QualityFlags) {
        let size = self.size();
        dilate_bytes(
            self.bits.pixels_mut(),
            size,
            DilationSpec { reach, fixed },
            &DilationScratch::default(),
            STRIP_WIDTH,
        );
        self.counts = FlagCounts::of(self.bits.pixels());
    }

    /// These flags grown as [`Self::dilate_window`] grows them, as bytes into `out`, working in
    /// `scratch`: for an owner that grows a frame's flags frame after frame.
    pub(crate) fn dilated_into(
        &self,
        spec: DilationSpec,
        out: &mut Buffer2<u8>,
        scratch: &DilationScratch,
    ) {
        debug_assert_eq!(Size2us::from(&*out), self.size());
        out.pixels_mut().copy_from_slice(self.bits.pixels());
        dilate_bytes(out.pixels_mut(), self.size(), spec, scratch, STRIP_WIDTH);
    }

    /// Whether any pixel holds a flag other than those in `except`.
    pub(crate) const fn contains_other_than(&self, except: QualityFlags) -> bool {
        self.counts.any_other_than(except)
    }

    /// Flags from a plane of their bytes, as [`Self::bytes`] holds them; `None` when no pixel holds
    /// one.
    pub(crate) fn from_bytes(bits: Buffer2<u8>) -> Option<Self> {
        let counts = FlagCounts::of(bits.pixels());
        counts.any().then_some(Self { bits, counts })
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

    /// Whether the pixel at `idx` carries any flag: a stage that measures only the pixels of a
    /// plain measurement leaves it out.
    #[inline]
    pub(crate) fn is_flagged(&self, idx: usize) -> bool {
        self.bits.pixels()[idx] != 0
    }

    /// The flags of the pixel at `idx`, row-major.
    #[inline]
    pub(crate) fn at(&self, idx: usize) -> QualityFlags {
        QualityFlags::from_byte(self.bits.pixels()[idx])
    }

    /// The flags of the pixel at `pos`.
    #[inline]
    pub fn at_pos(&self, pos: Vec2us) -> QualityFlags {
        self.at(self.size().idx_of(pos))
    }

    /// How many pixels hold `flag`.
    ///
    /// # Panics
    ///
    /// If `flag` is not a single flag.
    pub const fn count(&self, flag: QualityFlags) -> u64 {
        self.counts.count(flag)
    }

    /// How many pixels hold each flag.
    pub(crate) const fn counts(&self) -> &FlagCounts {
        &self.counts
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
        mask.fill_from_predicate(|idx| QualityFlags::from_byte(bytes[idx]).intersects(flag));
        mask
    }
}

/// How far a dilation grows the flags, and which of them stay where they are.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DilationSpec {
    pub(crate) reach: Reach,
    pub(crate) fixed: QualityFlags,
}

/// The block ORs a dilation works in, one set for each worker, kept by an owner that dilates
/// frame after frame.
#[derive(Debug, Default)]
pub(crate) struct DilationScratch {
    ors: ScratchPool<BlockOrs>,
}

/// The columns a task of a dilation's vertical pass takes: a cache line of flag bytes, so a
/// strip's block ORs over the whole height stay a small part of the plane.
const STRIP_WIDTH: usize = 64;

impl DilationScratch {
    /// The bytes a dilation of a `size` image on `workers` holds here: each worker's block ORs, of
    /// a row or of a strip's columns down the whole height, whichever is larger, at any reach.
    pub(crate) const fn heap_bytes(size: Size2us, workers: usize) -> usize {
        let strip = if size.width < STRIP_WIDTH {
            size.width
        } else {
            STRIP_WIDTH
        };
        let elements = if size.width > size.height * strip {
            size.width
        } else {
            size.height * strip
        };
        workers * 2 * elements
    }
}

/// [`PixelFlags::dilate_window`] on the bytes `bits` of a `size` image, in `scratch`, the
/// vertical pass in strips of `strip` columns.
///
/// Separable, and each pass is van Herk–Gil-Werman: an OR over blocks of the window's length from
/// both ends, so a pixel costs the same at any radius. The horizontal pass takes a row a task, the
/// vertical pass a strip of columns down the whole height a task, each in place, so a worker holds
/// a strip's block ORs, not a plane's.
fn dilate_bytes(
    bits: &mut [u8],
    size: Size2us,
    spec: DilationSpec,
    scratch: &DilationScratch,
    strip: usize,
) {
    let DilationSpec { reach, fixed } = spec;
    if reach.before == 0 && reach.after == 0 {
        return;
    }
    let width = size.width;
    let soft = !fixed.byte();
    par::chunks_mut(bits, width).for_each(|row| {
        let ors = &mut scratch.ors.acquire();
        ors.or_row(row, soft, reach);
    });
    let plane = UnsafeSendPtr::new(bits.as_mut_ptr());
    par::into_iter(0..width.div_ceil(strip)).for_each(|idx| {
        let ors = &mut scratch.ors.acquire();
        let strip = Strip {
            plane,
            size,
            columns: idx * strip..((idx + 1) * strip).min(width),
        };
        ors.or_columns(&strip, soft, reach);
    });
}

/// A strip of a plane's columns, its rows reached through the plane's pointer. Each task of
/// [`dilate_bytes`]' vertical pass holds a strip of its own, and the plane is not touched while
/// they live, so a row of one strip never aliases another strip's.
#[derive(Debug)]
struct Strip {
    plane: UnsafeSendPtr<u8>,
    size: Size2us,
    columns: Range<usize>,
}

impl Strip {
    /// The strip's part of row `y`.
    ///
    /// # Safety
    ///
    /// No other borrow of this strip's row `y` lives while the one returned does.
    #[expect(
        clippy::mut_from_ref,
        reason = "a strip's rows live in the plane its pointer reaches; the caller keeps them apart"
    )]
    unsafe fn row(&self, y: usize) -> &mut [u8] {
        debug_assert!(y < self.size.height && self.columns.end <= self.size.width);
        unsafe {
            slice::from_raw_parts_mut(
                self.plane
                    .get()
                    .add(y * self.size.width + self.columns.start),
                self.columns.len(),
            )
        }
    }
}

/// One worker's block ORs of a sequence, from the start and from the end of each block: of a
/// row's bytes, or of a strip's rows.
#[derive(Debug, Default)]
struct BlockOrs {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
}

impl Shapeless for BlockOrs {}

impl BlockOrs {
    /// Grow the bits `soft` keeps of `row` over the window `reach` spans around each byte,
    /// clipped to the row, in place; the other bits stay as they are.
    fn or_row(&mut self, row: &mut [u8], soft: u8, reach: Reach) {
        let len = row.len();
        let block = reach.len();
        let Self { prefix, suffix } = self;
        prefix.clear();
        prefix.reserve_exact(len);
        prefix.extend(row.iter().map(|&byte| byte & soft));
        suffix.clear();
        suffix.reserve_exact(len);
        suffix.extend_from_slice(prefix);
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
        for (i, byte) in row.iter_mut().enumerate() {
            let grown = match WindowParts::of(reach.window(i, len), block, len) {
                WindowParts::Both {
                    suffix: s,
                    prefix: p,
                } => suffix[s] | prefix[p],
                WindowParts::Prefix(at) => prefix[at],
                WindowParts::Suffix(at) => suffix[at],
            };
            *byte = (*byte & !soft) | grown;
        }
    }

    /// [`Self::or_row`] down the columns of `strip`: the same window over whole rows at once, a
    /// row being one element of the column-wise sequence, and OR element-wise.
    fn or_columns(&mut self, strip: &Strip, soft: u8, reach: Reach) {
        let (width, height) = (strip.columns.len(), strip.size.height);
        // Each row borrowed below is dropped before the next is.
        let row = |y: usize| unsafe { strip.row(y) };
        let block = reach.len();
        let Self { prefix, suffix } = self;
        prefix.clear();
        prefix.reserve_exact(height * width);
        for y in 0..height {
            prefix.extend(row(y).iter().map(|&byte| byte & soft));
        }
        suffix.clear();
        suffix.reserve_exact(height * width);
        suffix.extend_from_slice(prefix);
        for y in 1..height {
            if y % block != 0 {
                let (before, cur) = prefix.split_at_mut(y * width);
                or_into(&mut cur[..width], &before[(y - 1) * width..]);
            }
        }
        for y in (0..height.saturating_sub(1)).rev() {
            if (y + 1) % block != 0 {
                let (cur, after) = suffix.split_at_mut((y + 1) * width);
                or_into(&mut cur[y * width..], &after[..width]);
            }
        }
        let span = |y: usize| y * width..(y + 1) * width;
        for y in 0..height {
            let out = row(y);
            for byte in out.iter_mut() {
                *byte &= !soft;
            }
            match WindowParts::of(reach.window(y, height), block, height) {
                WindowParts::Both {
                    suffix: s,
                    prefix: p,
                } => {
                    or_into(out, &suffix[span(s)]);
                    or_into(out, &prefix[span(p)]);
                }
                WindowParts::Prefix(p) => or_into(out, &prefix[span(p)]),
                WindowParts::Suffix(s) => or_into(out, &suffix[span(s)]),
            }
        }
    }
}

/// OR `source` into `target`, element by element.
fn or_into(target: &mut [u8], source: &[u8]) {
    for (target, &source) in target.iter_mut().zip(source) {
        *target |= source;
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
            Self::from_fn(size, |idx| {
                if planes.iter().any(|plane| !plane[idx].is_finite()) {
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
    use common::par;
    use imaginarium::Buffer2;

    use crate::image::pixel_flags::{
        DilationScratch, DilationSpec, PixelFlags, Reach, dilate_bytes,
    };
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
        for idx in 0..6 {
            let expected = idx == 1 || idx == 4;
            assert_eq!(
                flags.at(idx) == QualityFlags::NO_DATA,
                expected,
                "index {idx}"
            );
            assert_eq!(mask.get(idx), expected, "index {idx}");
            assert_eq!(flags.is_flagged(idx), expected, "index {idx}");
        }

        // The same planes with the nulls removed produce no flags at all, so a caller cannot
        // mistake "nothing missing" for "an empty plane".
        assert!(PixelFlags::of_non_finite(size, &[&green]).is_none());

        // The bytes round-trip with their counts, as a spill reads them back.
        let restored = PixelFlags::from_bytes(Buffer2::new(
            size.width,
            size.height,
            flags.bytes().to_vec(),
        ))
        .unwrap();
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

    /// A dilation's block ORs hold no more than their price: each set at most one worker's share,
    /// for an image wider than a strip is tall, one taller, and one narrower than a strip, and no
    /// more sets than workers.
    #[test]
    fn a_dilation_holds_no_more_than_its_price() {
        for (width, height) in [(1000usize, 3usize), (200, 9), (9, 200)] {
            let size = Size2us::new(width, height);
            let flags = PixelFlags::from_fn(size, |idx| {
                if idx % 7 == 0 {
                    QualityFlags::SATURATED
                } else {
                    QualityFlags::default()
                }
            })
            .unwrap();
            let scratch = DilationScratch::default();
            let mut out = Buffer2::new_default(width, height);
            for reach in [1, 40].map(Reach::symmetric) {
                let spec = DilationSpec {
                    reach,
                    fixed: QualityFlags::default(),
                };
                flags.dilated_into(spec, &mut out, &scratch);
            }
            let share = DilationScratch::heap_bytes(size, 1);
            assert!(
                scratch
                    .ors
                    .all_free(|ors| ors.prefix.capacity() + ors.suffix.capacity() <= share),
                "{width}x{height}"
            );
            assert!(scratch.ors.pooled() <= par::threads());
        }
    }

    /// The separable dilation equals the brute-force window OR, for symmetric reaches from 1 to
    /// past the image and for the one-sided kernel reach a warp needs, on widths and heights that
    /// do not divide into blocks, and in strips of any width, one that does not divide the image
    /// and one wider than it among them; `NO_DATA` does not spread.
    #[test]
    fn dilation_matches_the_brute_force_window() {
        let mut rng = TestRng::new(7);
        for (width, height) in [(1usize, 1usize), (7, 5), (13, 17), (32, 3), (70, 9)] {
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
                let Some(mut flags) = PixelFlags::from_fn(size, |idx| initial[idx]) else {
                    continue;
                };
                let in_strips = [1, 3].map(|strip| {
                    let mut bytes = flags.bytes().to_vec();
                    dilate_bytes(
                        &mut bytes,
                        size,
                        DilationSpec {
                            reach,
                            fixed: QualityFlags::NO_DATA,
                        },
                        &DilationScratch::default(),
                        strip,
                    );
                    bytes
                });
                flags.dilate_window(reach, QualityFlags::NO_DATA);
                for bytes in &in_strips {
                    assert_eq!(bytes, flags.bytes(), "{width}x{height}, {reach:?}");
                }
                for y in 0..height {
                    for x in 0..width {
                        let near = |flag: QualityFlags| {
                            let rows = reach.window(y, height);
                            let cols = reach.window(x, width);
                            (rows.first..=rows.last).any(|sy| {
                                (cols.first..=cols.last).any(|sx| initial[sy * width + sx] == flag)
                            })
                        };
                        let idx = y * width + x;
                        let expected = QualityFlags::default()
                            .union(if near(QualityFlags::SATURATED) {
                                QualityFlags::SATURATED
                            } else {
                                QualityFlags::default()
                            })
                            .union(if initial[idx] == QualityFlags::NO_DATA {
                                QualityFlags::NO_DATA
                            } else {
                                QualityFlags::default()
                            });
                        assert_eq!(
                            flags.at(idx),
                            expected,
                            "{width}x{height}, {reach:?}, ({x}, {y})"
                        );
                    }
                }
                let saturated = (0..size.pixel_count())
                    .filter(|&idx| flags.at(idx).intersects(QualityFlags::SATURATED))
                    .count();
                assert_eq!(
                    flags.count(QualityFlags::SATURATED),
                    u64::try_from(saturated).unwrap()
                );
            }
        }
    }
}
