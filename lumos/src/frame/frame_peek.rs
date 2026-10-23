//! What one frame is worth to a memory estimate.

use std::fmt::Debug;

use crate::frame::frame_stats::FrameStats;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::image_dimensions::ImageDimensions;
use crate::io::frame_info::{DecoderBytes, FrameInfo};
use crate::memory::footprint::Footprint;

/// What one frame is worth to a memory estimate, before the rest of the set is read: its geometry,
/// and what its decoder holds while it makes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FramePeek {
    pub(crate) dimensions: ImageDimensions,
    /// What the decoder holds while it makes the frame.
    pub(crate) decoder: DecoderBytes,
    /// The pattern of a mosaic frame, whose statistics measure its noise per color; `None` for
    /// any other frame.
    pub(crate) mosaic: Option<Mosaic>,
}

impl FramePeek {
    /// Bytes one such frame occupies once resident: its own pixels and its flag plane, charged to
    /// every frame, since a decoder that flags saturation gives one to most, and which also holds
    /// the mask of a frame with pixels it has no measurement for.
    pub(crate) const fn resident_bytes(self) -> usize {
        self.dimensions.decoded_bytes()
    }

    /// What one such frame's way into a stack holds on `workers`: its decode, the decoder's own
    /// bytes on the way, then its statistics beside the frame.
    pub(crate) fn footprint(self, workers: usize) -> Footprint {
        Footprint {
            kept: self.resident_bytes(),
            peak: self.decoder.peak(self.resident_bytes()),
        }
        .beside(FrameStats::heap_bytes(
            self.dimensions,
            self.mosaic,
            workers,
        ))
    }
}

impl From<FrameInfo> for FramePeek {
    /// A CFA peek answers everything this needs and the demosaic kind besides, which no memory
    /// estimate reads.
    fn from(info: FrameInfo) -> Self {
        Self {
            dimensions: info.dimensions,
            decoder: info.decoder,
            mosaic: info.cfa_type.mosaic(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::channel_layout::ChannelLayout;

    /// A 100 × 100 one-channel frame resides as 40 000 B of samples and 10 000 B of flags.
    ///
    /// As a mosaic of RGGB its statistics hold its pairs, 98 a row over 100 rows at 4 bytes and a
    /// bin byte, 49 000, more than its 40 000 B of level samples: its decode peaks at 99 000 B,
    /// and a decoder holding 50 000 B beside the frame raises that to 100 000 B. A source of
    /// 120 000 B freed before the frame is made sets the peak alone; one of 30 000 B beside the
    /// 50 000 is gone by then, and leaves it at 100 000 B. As a plain frame
    /// its noise is measured by MRS over one tile that is the frame, 10 000 coefficients of 33
    /// bytes and a region of 28 bytes a pixel, 610 000, the tile's 136 bytes of bookkeeping, and a
    /// row of 100 pixels, 400 B, for each worker to smooth through: a peak of 660 536 B on one
    /// worker, and 2800 B more on eight.
    #[test]
    fn the_statistics_and_the_decoders_bytes_set_the_decode_peak() {
        let rggb = Some(Mosaic::Bayer(CfaPattern::Rggb));
        let peek = |mosaic, decoder| FramePeek {
            dimensions: ImageDimensions::new((100, 100), ChannelLayout::Mono),
            decoder,
            mosaic,
        };
        let bytes = |source, beside| DecoderBytes { source, beside };
        for (mosaic, decoder, peaks) in [
            (rggb, bytes(0, 0), [99_000; 2]),
            (rggb, bytes(0, 50_000), [100_000; 2]),
            (rggb, bytes(120_000, 0), [120_000; 2]),
            (rggb, bytes(30_000, 50_000), [100_000; 2]),
            (None, bytes(0, 0), [660_536, 663_336]),
        ] {
            for (workers, peak) in [1, 8].into_iter().zip(peaks) {
                assert_eq!(
                    peek(mosaic, decoder).footprint(workers),
                    Footprint { kept: 50_000, peak },
                    "{mosaic:?}, {decoder:?}, {workers} workers"
                );
            }
        }
    }
}
