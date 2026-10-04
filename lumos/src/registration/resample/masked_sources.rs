//! [`MaskedSources`]: a frame whose source declared pixels with no measurement, as the warp reads
//! it.
//!
//! An output pixel draws on a whole kernel footprint of source pixels, so one null under it
//! reaches every output pixel whose window covers it — up to 8×8 for Lanczos4. Letting the fill
//! under a null into the interpolation would put a fabricated sample into the result at nearly
//! full weight. The warp instead samples over the surviving taps alone, normalized by their weight
//! (normalized convolution), and the share of the kernel's magnitude they hold is the pixel's
//! coverage. With every null set to zero in the channels, a window's sums over the data already
//! leave the nulls out, and the validity plane gives the weight sums over the taps that remain.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::pixel_flags::{Flags, PixelFlags, Reach};
use crate::math::vec2us::Vec2us;
use crate::registration::resample::source_image::{SourceImage, SourcePlane};
use crate::registration::resample::source_position::SourcePosition;

/// The sources a null-aware warp samples, prepared once per masked frame: every channel with its
/// nulls set to zero, the validity plane, and where a window can reach a null at all.
///
/// Built per masked frame rather than held in [`WarpBuffers`](crate::registration::resample::WarpBuffers):
/// a frame with no mask must not pay planes it will never touch, and a frame with one is already
/// the exception.
#[derive(Debug)]
pub(crate) struct MaskedSources {
    validity: Buffer2<f32>,
    zeroed: ArrayVec<Buffer2<f32>, 3>,
    /// `NO_DATA` grown by the kernel's window: set at a source cell when a window sampling from it
    /// can read a null. A window that cannot is a product of its axes like any other.
    null_near: PixelFlags,
}

impl MaskedSources {
    /// `image`'s sources under `flags`, which hold `NO_DATA`, for a kernel whose windows read
    /// `reach` around their cell.
    pub(crate) fn new(image: &SourceImage<'_>, flags: &PixelFlags, reach: Reach) -> Self {
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
                            *value = if flags.at_pos(Vec2us::new(x, y)).intersects(Flags::NO_DATA) {
                                0.0
                            } else {
                                sample
                            };
                        }
                    });
                zeroed
            })
            .collect();
        let mut null_near = flags
            .without(Flags::from_byte(!Flags::NO_DATA.byte()))
            .expect("a masked frame holds NO_DATA");
        null_near.dilate_window(reach, Flags::default());
        Self {
            validity: flags.validity_plane(),
            zeroed,
            null_near,
        }
    }

    /// The channels with their nulls at zero.
    pub(crate) fn planes(&self) -> impl Iterator<Item = SourcePlane<'_>> {
        self.zeroed.iter().map(Self::plane)
    }

    /// 1 where the source holds a measurement, 0 at a null.
    pub(crate) fn validity(&self) -> SourcePlane<'_> {
        Self::plane(&self.validity)
    }

    fn plane(buffer: &Buffer2<f32>) -> SourcePlane<'_> {
        SourcePlane {
            pixels: buffer.pixels(),
            width: buffer.width(),
        }
    }

    /// Whether a window sampling `position` can read a null. A position in the footprint's rim
    /// takes the edge cell, whose grown flags cover every in-bounds tap such a window reads.
    #[expect(
        clippy::cast_sign_loss,
        reason = "each cell is clamped to the image before the conversion"
    )]
    pub(crate) fn null_near(&self, position: SourcePosition) -> bool {
        let size = self.null_near.size();
        let x = (position.cell_x.max(0) as usize).min(size.width - 1);
        let y = (position.cell_y.max(0) as usize).min(size.height - 1);
        self.null_near.byte(y * size.width + x) != 0
    }
}
