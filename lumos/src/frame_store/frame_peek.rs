//! What one frame is worth to a memory estimate.

use std::fmt::Debug;

use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::CfaFrameInfo;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::pixel_flags::Flags;
use crate::memory;

/// What one frame is worth to a memory estimate, before the rest of the set is read.
///
/// Sizing a run needs the geometry and whether the frame carries the two quality planes a masked
/// one does. A header answers the first exactly and the second only sometimes, so the second is
/// deliberately allowed to over-report: reserving planes a frame turns out not to carry costs a run
/// some concurrency, while missing planes it does carry overcommits the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FramePeek {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) may_carry_nulls: bool,
}

impl FramePeek {
    /// What a frame already in hand settles — exactly, since its mask is right there.
    pub(crate) fn of_decoded(image: &impl StackableImage) -> Self {
        Self {
            dimensions: image.dimensions(),
            may_carry_nulls: image
                .flags()
                .is_some_and(|flags| flags.contains(Flags::NO_DATA)),
        }
    }

    /// Bytes one such frame occupies once resident: its own pixels and its flag plane, plus the
    /// quality planes if it may carry them.
    pub(crate) const fn resident_bytes(self) -> usize {
        let quality = if self.may_carry_nulls {
            memory::quality_plane_bytes(self.dimensions)
        } else {
            0
        };
        memory::frame_bytes(self.dimensions) + memory::flag_plane_bytes(self.dimensions) + quality
    }
}

impl From<CfaFrameInfo> for FramePeek {
    /// A CFA peek answers everything this needs and the demosaic kind besides, which no memory
    /// estimate reads.
    fn from(info: CfaFrameInfo) -> Self {
        Self {
            dimensions: info.dimensions,
            may_carry_nulls: info.may_carry_nulls,
        }
    }
}
