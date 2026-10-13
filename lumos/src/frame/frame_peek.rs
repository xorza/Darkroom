//! What one frame is worth to a memory estimate.

use std::fmt::Debug;

use crate::frame::frame_stats::FrameStats;
use crate::frame::stackable_image::StackableImage;
use crate::frame_store::stored_layout::StoredLayout;
use crate::image::cfa::cfa_frame_info::CfaFrameInfo;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::image_dimensions::ImageDimensions;
use crate::memory::footprint::Footprint;

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
        StoredLayout::decoded(self.dimensions).heap_bytes(self.dimensions)
    }

    /// What one such frame's way into a stack holds on `workers`: its decode, the decoder's own
    /// bytes beside the frame it is making, then its statistics beside the frame.
    pub(crate) fn footprint(self, workers: usize) -> Footprint {
        Footprint {
            kept: self.resident_bytes(),
            peak: self.resident_bytes().saturating_add(self.decoder_bytes),
        }
        .beside(FrameStats::heap_bytes(
            self.dimensions,
            self.mosaic.as_ref(),
            workers,
        ))
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
    use crate::image::cfa::cfa_pattern::CfaPattern;

    /// A 100 × 100 one-channel frame resides as 40 000 B of samples and 10 000 B of flags.
    ///
    /// As a mosaic of RGGB its statistics hold its pairs, 98 a row over 100 rows at 4 bytes and a
    /// bin byte, 49 000, more than its 40 000 B of level samples: its decode peaks at 99 000 B,
    /// and a decoder holding 50 000 B beside the frame raises that to 100 000 B. As a plain frame
    /// its noise is measured by MRS over one tile that is the frame, 10 000 coefficients of 33
    /// bytes and a region of 28 bytes a pixel, 610 000, the tile's 136 bytes of bookkeeping, and a
    /// row of 100 pixels, 400 B, for each worker to smooth through: a peak of 660 536 B on one
    /// worker, and 2800 B more on eight.
    #[test]
    fn the_statistics_and_the_decoders_bytes_set_the_decode_peak() {
        let rggb = Some(CfaType::Bayer(CfaPattern::Rggb));
        let peek = |mosaic, decoder_bytes| FramePeek {
            dimensions: ImageDimensions::new((100, 100), 1),
            decoder_bytes,
            mosaic,
        };
        for (mosaic, decoder_bytes, peaks) in [
            (rggb, 0, [99_000; 2]),
            (rggb, 50_000, [100_000; 2]),
            (None, 0, [660_536, 663_336]),
        ] {
            for (workers, peak) in [1, 8].into_iter().zip(peaks) {
                assert_eq!(
                    peek(mosaic, decoder_bytes).footprint(workers),
                    Footprint { kept: 50_000, peak },
                    "{mosaic:?}, {decoder_bytes} B, {workers} workers"
                );
            }
        }
    }
}
