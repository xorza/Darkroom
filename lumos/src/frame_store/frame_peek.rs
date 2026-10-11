//! What one frame is worth to a memory estimate.

use std::fmt::Debug;

use crate::combine::cache::slots::Slots;
use crate::combine::config::Normalization;
use crate::combine::normalization::{FrameNorm, SetCoverage};
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stackable_image::StackableImage;
use crate::io::image::cfa::{CfaFrameInfo, CfaType};
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
    /// The pattern of a mosaic frame, whose statistics measure its noise per colour; `None` for
    /// any other frame.
    pub(crate) mosaic: Option<CfaType>,
}

impl FramePeek {
    /// What a frame already in hand settles. Its decode is over, so what its decoder held is not
    /// known.
    pub(crate) fn of_decoded(image: &impl StackableImage) -> Self {
        Self {
            dimensions: image.dimensions(),
            decoder_bytes: 0,
            mosaic: image.cfa_type().filter(CfaType::is_mosaic),
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
    /// `output_bytes` beside them, normalized by `normalization`, with `threads` workers: each
    /// decode peaks at what its statistics hold beside the frame, or at the decoder's own bytes
    /// beside the frame it is making, whichever is larger. A frame may carry a mask its decode
    /// alone reveals, so the normalization is charged as over a set that shares only some pixels, which holds more than one every frame covers.
    pub(crate) fn run_shape(
        self,
        frame_count: usize,
        output_bytes: usize,
        normalization: Normalization,
        threads: usize,
    ) -> RunShape {
        let mut shape = RunShape::decoded_stack(
            frame_count,
            self.resident_bytes(),
            FrameStats::scratch_bytes(self.dimensions, self.mosaic.as_ref(), threads),
            output_bytes,
            FrameNorm::working_bytes(
                frame_count,
                self.dimensions,
                Slots::new(self.mosaic, self.dimensions.channels()),
                normalization,
                SetCoverage::Common,
                threads,
            ),
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
            mosaic: Some(info.cfa_type).filter(CfaType::is_mosaic),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::image::cfa::cfa_pattern::CfaPattern;

    /// A 100 × 100 one-channel frame resides as 40 000 B of samples and 10 000 B of flags.
    ///
    /// As a mosaic of RGGB its statistics hold its pairs, 98 a row over 100 rows at 4 bytes and a
    /// bin byte, 49 000, more than its 40 000 B of level samples: its decode peaks at 99 000 B,
    /// and a decoder holding 50 000 B beside the frame raises that to 100 000 B. As a plain frame
    /// its noise is measured by MRS over one tile that is the frame, 10 000 coefficients of 33
    /// bytes and a region of 32 bytes a pixel, 650 000, and the tile's 88 bytes of bookkeeping: a
    /// peak of 700 088 B, whatever the workers, one tile having nothing to share.
    #[test]
    fn the_statistics_and_the_decoders_bytes_set_the_decode_peak() {
        let rggb = Some(CfaType::Bayer(CfaPattern::Rggb));
        for (mosaic, decoder_bytes, peak) in [
            (rggb, 0, 99_000),
            (rggb, 50_000, 100_000),
            (None, 0, 700_088),
        ] {
            let peek = FramePeek {
                dimensions: ImageDimensions::new((100, 100), 1),
                decoder_bytes,
                mosaic,
            };
            for threads in [1, 8] {
                let shape = peek.run_shape(3, 0, Normalization::None, threads);
                assert_eq!(shape.decode.output_bytes, 50_000);
                assert_eq!(
                    shape.decode.peak_bytes, peak,
                    "{mosaic:?}, {decoder_bytes} B, {threads} threads"
                );
            }
        }
    }
}
