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
//!   its sums by the validity plane, lane by lane, so the value under an excluded pixel, a NaN
//!   fill among them, never enters a sum, and the validity plane's own sums are the weights of the
//!   taps that remain.
//! - A bound, at saturation or the flat's floor ([`QualityFlags::RESAMPLE_CARRIED`]), is still
//!   the best figure for its pixel, and where every frame clips a star's core the only one. The
//!   warp samples it like any other and carries its flag to every output pixel whose sample gave
//!   it weight, for the combine to leave out while enough clean samples remain.

use imaginarium::Buffer2;

use crate::image::flag_counts::FlagCounts;
use crate::image::pixel_flags::{DilationScratch, DilationSpec, PixelFlags, Reach};
use crate::image::quality_flags::QualityFlags;
use crate::math::size2us::Size2us;
use crate::registration::resample::source_image::SourcePlane;
use crate::registration::resample::source_position::SourcePosition;
use crate::registration::resample::warp_kernel::TapAxis;
use crate::scratch_pool::Shapeless;

/// A flagged frame's sources as the warp samples them, prepared once per frame in a
/// [`FlagScratch`].
#[derive(Debug)]
pub(crate) struct FlaggedSources<'a> {
    flags: &'a PixelFlags,
    /// 1 where the source holds a measurement, 0 at an excluded pixel; for a frame that excludes
    /// any.
    validity: Option<&'a Buffer2<f32>>,
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
    validity: Option<Buffer2<f32>>,
    near: Option<Buffer2<u8>>,
    dilation: DilationScratch,
    /// A plane for the warped flags, kept from a frame that carried none.
    output: Option<Buffer2<u8>>,
}

impl FlagScratch {
    /// The bytes the scratch holds beside a `size` frame's warp at the most: its validity plane,
    /// its flags grown by the kernel's window, that growth's planes, and the plane for the warped
    /// flags, which it keeps from a frame whose warp carried none.
    pub(crate) const fn heap_bytes(size: Size2us) -> usize {
        size.pixel_count() * (size_of::<f32>() + 2 * size_of::<u8>())
            + DilationScratch::heap_bytes(size)
    }

    /// `flags`' sources prepared in this scratch, for windows that read `reach` around their
    /// cell, or for the nearest pixel when `reach` is `None`.
    pub(crate) fn prepare<'a>(
        &'a mut self,
        flags: &'a PixelFlags,
        reach: Option<Reach>,
    ) -> FlaggedSources<'a> {
        let size = flags.size();
        if flags.contains(QualityFlags::RESAMPLE_EXCLUDED) {
            flags.validity_into(
                QualityFlags::RESAMPLE_EXCLUDED,
                sized(&mut self.validity, size),
            );
        }
        if let Some(reach) = reach {
            flags.dilated_into(
                DilationSpec {
                    reach,
                    fixed: QualityFlags::default(),
                },
                sized(&mut self.near, size),
                &mut self.dilation,
            );
        }
        FlaggedSources {
            flags,
            validity: self
                .validity
                .as_ref()
                .filter(|_| flags.contains(QualityFlags::RESAMPLE_EXCLUDED)),
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
            Some(plane) if Size2us::from(&plane) == size => plane,
            _ => Buffer2::new_default(size.width, size.height),
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
fn sized<T: Default + Clone>(slot: &mut Option<Buffer2<T>>, size: Size2us) -> &mut Buffer2<T> {
    if slot
        .as_ref()
        .is_none_or(|plane| Size2us::from(plane) != size)
    {
        *slot = Some(Buffer2::new_default(size.width, size.height));
    }
    slot.as_mut().expect("the slot was filled above")
}

impl FlaggedSources<'_> {
    /// The validity plane, when the frame excludes any pixel.
    pub(crate) fn validity(&self) -> Option<SourcePlane<'_>> {
        self.validity.map(plane)
    }

    /// The flags a window sampling `position` can read. A position in the footprint's rim takes
    /// the edge cell, whose grown flags cover every in-bounds tap such a window reads.
    #[expect(
        clippy::cast_sign_loss,
        reason = "each cell is clamped to the image before the conversion"
    )]
    pub(crate) fn near(&self, position: SourcePosition) -> QualityFlags {
        let near = self
            .near
            .expect("a filter's sources are grown by its window");
        let x = (position.cell_x.max(0) as usize).min(near.width() - 1);
        let y = (position.cell_y.max(0) as usize).min(near.height() - 1);
        QualityFlags::from_byte(near.pixels()[y * near.width() + x])
    }

    /// The carried flags of source pixel `index`, none when it is excluded: an excluded pixel is
    /// never sampled.
    pub(crate) fn carried_at(&self, index: usize) -> QualityFlags {
        carried(self.flags.at(index))
    }

    /// The carried flags of every in-bounds source pixel the window of `x` and `y` gives a weight
    /// other than zero. A carried value has no limit on how far it is from the truth — a clipped
    /// core can be any amount brighter — so any weight on it can move the sample by any amount.
    pub(crate) fn carried_in(&self, x: &TapAxis, y: &TapAxis) -> QualityFlags {
        let size = self.flags.size();
        let mut carried = QualityFlags::default();
        for row in weighed_taps(y, size.height) {
            for column in weighed_taps(x, size.width) {
                carried = carried.union(self.carried_at(row * size.width + column));
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

fn plane(buffer: &Buffer2<f32>) -> SourcePlane<'_> {
    SourcePlane {
        pixels: buffer.pixels(),
        width: buffer.width(),
    }
}
