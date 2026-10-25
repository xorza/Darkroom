//! [`FlaggedSources`]: a frame whose source flags pixels, as the warp reads it.
//!
//! An output pixel draws on a whole kernel footprint of source pixels, so one flagged value under
//! it reaches every output pixel whose window covers it — up to 8×8 for Lanczos4 — at whatever
//! weight its tap has. What the warp does with it depends on what the flag says of the value.
//!
//! - A fill, a hit, a defect or its repair ([`QualityFlags::RESAMPLE_EXCLUDED`]) is no measurement
//!   at all. The warp samples over the other taps alone, normalized by their weight (normalized
//!   convolution), and the share of the kernel's magnitude they hold is the pixel's coverage: the
//!   space-telescope pipelines' rule, which gives such a pixel no weight. A window near one masks
//!   its sums lane by lane by the [`Validity`] of its taps, read off their flag bytes, so the value
//!   under an excluded pixel, a NaN fill among them, never enters a sum, and the weights of the
//!   taps that remain are the window's.
//! - A bound, at saturation or the flat's floor ([`QualityFlags::RESAMPLE_CARRIED`]), is still
//!   the best figure for its pixel, and where every frame clips a star's core the only one. The
//!   warp samples it like any other and carries its flag to every output pixel whose sample gave
//!   it weight, for the combine to leave out while enough clean samples remain.

use common::Buffer2;

use crate::image::flags::flag_counts::FlagCounts;
use crate::image::flags::pixel_flags::{DilationScratch, DilationSpec, PixelFlags, Reach};
use crate::image::flags::quality_flags::QualityFlags;
use crate::registration::resample::source_pos::SourcePos;
use crate::registration::resample::warp_kernel::TapAxis;
use crate::scratch_pool::Shapeless;
use crate::simd::{F32_LANES, Isa};
use common::Size2us;

/// A flagged frame's sources as the warp samples them, prepared once per frame in a
/// [`FlagScratch`].
#[derive(Debug)]
pub(crate) struct FlaggedSources<'a> {
    flags: &'a PixelFlags,
    /// The flags grown by the kernel's window: a source cell holds every flag a window sampling
    /// from it can read. A window near none is a product of its axes like any other. For a filter
    /// alone: the nearest pixel reads its own flags.
    near: Option<&'a Buffer2<u8>>,
}

/// The planes a flagged frame's warp prepares beside the frame, kept in a pool from one frame to
/// the next. Every calibrated light flags pixels — its saturation at least — so a new set each
/// frame would pay their whole area in allocation and first-touch page faults each time.
#[derive(Debug, Default)]
pub(crate) struct FlagScratch {
    near: Option<Buffer2<u8>>,
    dilation: DilationScratch,
    /// A plane for the warped flags, kept from a frame that carried none.
    output: Option<Buffer2<u8>>,
}

impl FlagScratch {
    /// The bytes the scratch holds beside a `size` frame's warp on `workers` at the most: its
    /// flags grown by the kernel's window, that growth's block ORs, and the plane for the warped
    /// flags, which it keeps from a frame whose warp carried none.
    pub(crate) const fn heap_bytes(size: Size2us, workers: usize) -> usize {
        2 * size.pixel_count() * size_of::<u8>() + DilationScratch::heap_bytes(size, workers)
    }

    /// `flags`' sources prepared in this scratch, for windows that read `reach` around their
    /// cell, or for the nearest pixel when `reach` is `None`.
    pub(crate) fn prepare<'a>(
        &'a mut self,
        flags: &'a PixelFlags,
        reach: Option<Reach>,
    ) -> FlaggedSources<'a> {
        if let Some(reach) = reach {
            flags.dilated_into(
                DilationSpec {
                    reach,
                    fixed: QualityFlags::default(),
                },
                sized(&mut self.near, flags.size()),
                &self.dilation,
            );
        }
        FlaggedSources {
            flags,
            near: self.near.as_ref().filter(|_| reach.is_some()),
        }
    }

    /// Keep the plane of `flags`, the last frame's warped flags, for the next frame's.
    pub(crate) fn keep(&mut self, flags: Option<PixelFlags>) {
        if let Some(flags) = flags {
            self.output = Some(flags.into_buffer());
        }
    }

    /// A plane for the warped flags of a frame of `size`: the one kept here, or a new one. Every
    /// pixel is written, so none is cleared.
    pub(crate) fn output(&mut self, size: Size2us) -> Buffer2<u8> {
        match self.output.take() {
            Some(plane) if plane.size() == size => plane,
            _ => Buffer2::new_default(size),
        }
    }

    /// The warped flags in `plane`; the plane kept here when it holds none.
    pub(crate) fn finish(&mut self, plane: Buffer2<u8>) -> Option<PixelFlags> {
        let counts = FlagCounts::of(plane.pixels());
        if counts.any() {
            PixelFlags::from_counted(plane, counts)
        } else {
            self.output = Some(plane);
            None
        }
    }
}

impl Shapeless for FlagScratch {}

/// The plane in `slot`, made for `size` when it holds none of that size.
fn sized(slot: &mut Option<Buffer2<u8>>, size: Size2us) -> &mut Buffer2<u8> {
    if slot.as_ref().is_none_or(|plane| plane.size() != size) {
        *slot = Some(Buffer2::new_default(size));
    }
    slot.as_mut().expect("the slot was filled above")
}

impl FlaggedSources<'_> {
    /// Which sources hold a measurement, when the frame excludes any pixel.
    pub(super) fn validity(&self) -> Option<Validity<'_>> {
        self.flags
            .contains(QualityFlags::RESAMPLE_EXCLUDED)
            .then(|| Validity {
                flags: self.flags.bytes(),
                width: self.flags.size().width,
            })
    }

    /// The flags a window sampling `pos` can read. A position in the footprint's rim takes
    /// the edge cell, whose grown flags cover every in-bounds tap such a window reads.
    #[expect(
        clippy::cast_sign_loss,
        reason = "each cell is clamped to the image before the conversion"
    )]
    pub(crate) fn near(&self, pos: SourcePos) -> QualityFlags {
        let near = self
            .near
            .expect("a filter's sources are grown by its window");
        let x = (pos.cell_x.max(0) as usize).min(near.width() - 1);
        let y = (pos.cell_y.max(0) as usize).min(near.height() - 1);
        QualityFlags::from_byte(near.pixels()[y * near.width() + x])
    }

    /// The carried flags of source pixel `idx`, none when it is excluded: an excluded pixel is
    /// never sampled.
    pub(super) fn carried_at(&self, idx: usize) -> QualityFlags {
        carried(self.flags.at(idx))
    }

    /// The carried flags of every in-bounds source pixel the window of `x` and `y` gives a weight
    /// other than zero. A carried value has no limit on how far it is from the truth — a clipped
    /// core can be any amount brighter — so any weight on it can move the sample by any amount.
    pub(super) fn carried_in(&self, x: &TapAxis, y: &TapAxis) -> QualityFlags {
        let size = self.flags.size();
        let mut carried = QualityFlags::default();
        for row in weighed_taps(y, size.height) {
            for col in weighed_taps(x, size.width) {
                carried = carried.union(self.carried_at(row * size.width + col));
            }
        }
        carried
    }
}

/// The carried part of `flags`, none when they exclude the pixel.
fn carried(flags: QualityFlags) -> QualityFlags {
    if flags.intersects(QualityFlags::RESAMPLE_EXCLUDED) {
        QualityFlags::default()
    } else {
        flags.intersection(QualityFlags::RESAMPLE_CARRIED)
    }
}

/// The taps of `axis` within an axis of `length` pixels whose weight is not zero.
#[expect(
    clippy::cast_sign_loss,
    reason = "each tap is checked against the image before the conversion"
)]
fn weighed_taps(axis: &TapAxis, length: usize) -> impl Iterator<Item = usize> + '_ {
    (axis.start..)
        .zip(axis.weights())
        .filter(move |&(tap, &weight)| weight != 0.0 && (0..length as i32).contains(&tap))
        .map(|(tap, _)| tap as usize)
}

/// Which source pixels hold a measurement, read off their flag bytes: none of
/// [`QualityFlags::RESAMPLE_EXCLUDED`].
#[derive(Debug, Clone, Copy)]
pub(super) struct Validity<'a> {
    flags: &'a [u8],
    pub(super) width: usize,
}

impl Validity<'_> {
    /// Whether source pixel `idx` holds a measurement.
    #[inline]
    pub(super) const fn holds(self, idx: usize) -> bool {
        !QualityFlags::from_byte(self.flags[idx]).intersects(QualityFlags::RESAMPLE_EXCLUDED)
    }

    /// One lane for each of the `count` source pixels from `idx`, at most a vector's: 1 where it
    /// holds a measurement, 0 where it does not and past them. The factor the masked weight sums
    /// take, and the mask the values take.
    #[inline(always)]
    pub(super) fn lanes<S: Isa>(self, isa: S, idx: usize, count: usize) -> S::F32 {
        debug_assert!(count <= F32_LANES, "{count} taps in a vector");
        let mut lanes = [0.0; F32_LANES];
        for (lane, offset) in lanes.iter_mut().zip(0..count) {
            if self.holds(idx + offset) {
                *lane = 1.0;
            }
        }
        isa.load_f32(&lanes)
    }
}
