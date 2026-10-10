//! What one frame is worth to a memory estimate.

use std::fmt::Debug;

use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::CfaFrameInfo;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::memory::memory_plan::RunShape;

/// What one frame is worth to a memory estimate, before the rest of the set is read: its geometry,
/// and what its decoder holds while it makes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FramePeek {
    pub(crate) dimensions: ImageDimensions,
    /// What the decoder holds beside the frame while it makes it — see
    /// [`CfaFrameInfo::decoder_bytes`].
    pub(crate) decoder_bytes: usize,
}

impl FramePeek {
    /// What a frame already in hand settles. Its decode is over, so what its decoder held is not
    /// known.
    pub(crate) fn of_decoded(image: &impl StackableImage) -> Self {
        Self {
            dimensions: image.dimensions(),
            decoder_bytes: 0,
        }
    }

    /// Bytes one such frame occupies once resident: its own pixels and its flag plane, which also
    /// holds the mask of a frame with pixels it has no measurement for.
    pub(crate) const fn resident_bytes(self) -> usize {
        self.dimensions.frame_bytes() + self.dimensions.flag_plane_bytes()
    }
}

impl FramePeek {
    /// The shape of a run that decodes `frame_count` such frames straight into a combine holding
    /// `output_bytes` beside them: each decode peaks at its statistics copy, or at the decoder's
    /// own bytes beside the frame it is making, whichever is larger.
    pub(crate) const fn run_shape(self, frame_count: usize, output_bytes: usize) -> RunShape {
        let mut shape = RunShape::decoded_stack(
            frame_count,
            self.resident_bytes(),
            self.dimensions.frame_bytes(),
            output_bytes,
        );
        shape.decode = shape
            .decode
            .with_peak_at_least(self.resident_bytes().saturating_add(self.decoder_bytes));
        shape
    }
}

impl From<CfaFrameInfo> for FramePeek {
    /// A CFA peek answers everything this needs and the demosaic kind besides, which no memory
    /// estimate reads.
    fn from(info: CfaFrameInfo) -> Self {
        Self {
            dimensions: info.dimensions,
            decoder_bytes: info.decoder_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100 × 100 mono frame resides as 40 000 B of samples and 10 000 B of flags; its decode
    /// peaks at those 50 000 B plus a 40 000 B statistics copy, 90 000 B. A decoder holding 50 000 B
    /// beside the frame raises the peak to 100 000 B; one holding nothing leaves it.
    #[test]
    fn the_decoders_bytes_raise_the_decode_peak() {
        let peek = |decoder_bytes| FramePeek {
            dimensions: ImageDimensions::new((100, 100), 1),
            decoder_bytes,
        };
        for (decoder_bytes, peak) in [(0, 90_000), (50_000, 100_000)] {
            let shape = peek(decoder_bytes).run_shape(3, 0);
            assert_eq!(shape.decode.output_bytes, 50_000);
            assert_eq!(shape.decode.peak_bytes, peak, "{decoder_bytes} B");
        }
    }
}
