//! [`FlaggedSources`]: a frame whose source flags pixels, as the warp reads it.
//!
//! An output pixel draws on a whole kernel footprint of source pixels, so one flagged value under
//! it reaches every output pixel whose window covers it — up to 8×8 for Lanczos4 — at whatever
//! weight its tap has. What the warp does with it depends on what the flag says of the value.
//!
//! - A fill, a hit, a defect or its repair ([`QualityFlags::RESAMPLE_EXCLUDED`]) is no measurement
//!   at all. The warp samples over the other taps alone, normalized by their weight (normalized
//!   convolution), and the share of the kernel's magnitude they hold is the pixel's coverage: the
//!   space-telescope pipelines' rule, which gives such a pixel no weight. With every excluded pixel
//!   set to zero in the channels, a window's sums over the data already leave them out, and the
//!   validity plane gives the weight sums over the taps that remain.
//! - A bound, at saturation or the flat's floor ([`QualityFlags::RESAMPLE_CARRIED`]), is still
//!   the best figure for its pixel, and where every frame clips a star's core the only one. The
//!   warp samples it like any other and carries its flag to every output pixel whose sample gave
//!   it weight, for the combine to leave out while enough clean samples remain.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::pixel_flags::{PixelFlags, QualityFlags, Reach};
use crate::math::vec2us::Vec2us;
use crate::registration::resample::kernel::warp_kernel::TapAxis;
use crate::registration::resample::source_image::{SourceImage, SourcePlane};
use crate::registration::resample::source_position::SourcePosition;

/// A flagged frame's sources as the warp samples them, prepared once per frame.
///
/// Built per flagged frame rather than held in
/// [`WarpBuffers`](crate::registration::resample::WarpBuffers): a frame with no flags must not pay
/// planes it will never touch, and a frame with some is already the exception.
#[derive(Debug)]
pub(crate) struct FlaggedSources<'a> {
    flags: &'a PixelFlags,
    excluded: Option<ExcludedSources>,
    /// The flags grown by the kernel's window: a source cell holds every flag a window sampling
    /// from it can read. A window near none is a product of its axes like any other.
    near: PixelFlags,
}

/// The channels with every excluded pixel set to zero, and the validity plane: 1 where the source
/// holds a measurement, 0 at an excluded pixel.
#[derive(Debug)]
struct ExcludedSources {
    validity: Buffer2<f32>,
    zeroed: ArrayVec<Buffer2<f32>, 3>,
}

impl<'a> FlaggedSources<'a> {
    /// `image`'s sources under `flags`, for a kernel whose windows read `reach` around their
    /// cell.
    pub(crate) fn new(image: &SourceImage<'_>, flags: &'a PixelFlags, reach: Reach) -> Self {
        let excluded = flags
            .contains(QualityFlags::RESAMPLE_EXCLUDED)
            .then(|| ExcludedSources::new(image, flags));
        let mut near = flags.clone();
        near.dilate_window(reach, QualityFlags::default());
        Self {
            flags,
            excluded,
            near,
        }
    }

    /// The channels with their excluded pixels at zero, when the frame excludes any.
    pub(crate) fn zeroed_planes(&self) -> Option<impl Iterator<Item = SourcePlane<'_>>> {
        self.excluded
            .as_ref()
            .map(|excluded| excluded.zeroed.iter().map(plane))
    }

    /// The validity plane, when the frame excludes any pixel.
    pub(crate) fn validity(&self) -> Option<SourcePlane<'_>> {
        self.excluded
            .as_ref()
            .map(|excluded| plane(&excluded.validity))
    }

    /// The flags a window sampling `position` can read. A position in the footprint's rim takes
    /// the edge cell, whose grown flags cover every in-bounds tap such a window reads.
    #[expect(
        clippy::cast_sign_loss,
        reason = "each cell is clamped to the image before the conversion"
    )]
    pub(crate) fn near(&self, position: SourcePosition) -> QualityFlags {
        let size = self.near.size();
        let x = (position.cell_x.max(0) as usize).min(size.width - 1);
        let y = (position.cell_y.max(0) as usize).min(size.height - 1);
        self.near.at(y * size.width + x)
    }

    /// The carried flags of source pixel `index`, none when it is excluded: an excluded pixel is
    /// never sampled.
    pub(crate) fn carried_at(&self, index: usize) -> QualityFlags {
        Self::carried(self.flags.at(index))
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

    fn carried(flags: QualityFlags) -> QualityFlags {
        if flags.intersects(QualityFlags::RESAMPLE_EXCLUDED) {
            QualityFlags::default()
        } else {
            flags.intersection(QualityFlags::RESAMPLE_CARRIED)
        }
    }
}

impl ExcludedSources {
    fn new(image: &SourceImage<'_>, flags: &PixelFlags) -> Self {
        let size = image.size();
        let zeroed = image
            .planes
            .iter()
            .map(|source| {
                let mut zeroed = Buffer2::new_default(size.width, size.height);
                let width = size.width;
                zeroed
                    .pixels_mut()
                    .par_chunks_mut(width)
                    .zip(source.pixels.par_chunks(width))
                    .enumerate()
                    .for_each(|(y, (row, source_row))| {
                        for (x, (value, &sample)) in row.iter_mut().zip(source_row).enumerate() {
                            *value = if flags
                                .at_pos(Vec2us::new(x, y))
                                .intersects(QualityFlags::RESAMPLE_EXCLUDED)
                            {
                                0.0
                            } else {
                                sample
                            };
                        }
                    });
                zeroed
            })
            .collect();
        Self {
            validity: flags.validity_plane(QualityFlags::RESAMPLE_EXCLUDED),
            zeroed,
        }
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
