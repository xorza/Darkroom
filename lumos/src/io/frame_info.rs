//! [`FrameInfo`]: what a frame is, read from its file's header without decoding it.

use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::Demosaic;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::image_dimensions::ImageDimensions;
use crate::io::error::OverBudget;
use crate::io::load_context::LoadContext;
use crate::memory::footprint::Footprint;
use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;

/// What a frame is, read from its header: its shape, its sensor's pattern — [`CfaType::Mono`] for
/// a frame of no filter, or of one already demosaiced — and what its decoder holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameInfo {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) cfa_type: CfaType,
    /// What the decoder holds beside the frame while it makes it: a camera RAW's whole file, which
    /// LibRaw parses in place, and the raw buffer it unpacks into; nothing for a FITS file, which
    /// is streamed.
    pub(crate) decoder_bytes: usize,
}

impl FrameInfo {
    /// The frame's decode: its samples and its flag plane kept, which a decoder that flags
    /// saturation gives most frames, and the decoder's bytes beside them on the way.
    pub(crate) const fn decoded(self) -> Footprint {
        let frame = self.dimensions.frame_bytes() + self.dimensions.flag_plane_bytes();
        Footprint {
            kept: frame,
            peak: frame + self.decoder_bytes,
        }
    }

    /// `way`, which ends on the frame's decoded mosaic, then the demosaic on `workers`, an X-Trans
    /// one in `passes`: the noise the demosaic measures on the mosaic beside it, then the demosaic,
    /// which carries the flags to the frame it makes.
    pub(crate) fn demosaiced(
        self,
        way: Footprint,
        passes: MarkesteijnPasses,
        workers: usize,
    ) -> Footprint {
        let noise = if self.cfa_type.is_mosaic() {
            MosaicNoiseMeasure::heap_bytes(self.dimensions.size(), &self.cfa_type)
        } else {
            0
        };
        let flags = self.dimensions.flag_plane_bytes();
        let demosaic = Demosaic::of(self.cfa_type, passes).footprint(self.dimensions, workers);
        way.beside(noise).then(Footprint {
            kept: demosaic.kept + flags,
            peak: demosaic.peak + flags,
        })
    }

    /// Refuse a decode of the frame and its demosaic past `context`'s memory limit, from the
    /// header before either runs: the way a preview of a mosaic takes, its demosaic on every
    /// thread of the pool it runs on.
    pub(crate) fn admit_demosaic(self, context: &LoadContext) -> Result<(), OverBudget> {
        let way = self.demosaiced(
            self.decoded(),
            context.xtrans_passes,
            rayon::current_num_threads(),
        );
        OverBudget::check(
            u64::try_from(way.peak).expect("a byte count fits u64"),
            context.memory_limit_bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::image::cfa::demosaic::bayer::rcd;
    use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::io::frame_info::FrameInfo;
    use crate::memory::footprint::Footprint;
    use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;

    /// A 10 × 8 frame decodes to 320 B of samples and 80 of flags, 400 B kept, 500 at the peak with
    /// a decoder of 100 B. A Bayer demosaic on 4 workers then holds the mosaic, three planes and its
    /// tiles, 4 · 320 B and RCD's workspace, and keeps the three planes, 960 B, with the 80 flag
    /// bytes on both. A mono frame's demosaic keeps its one plane: 320 + 80 = 400 B, and its way
    /// peaks at the decoder's 500 B.
    #[test]
    fn the_way_to_a_demosaic_holds_the_decode_the_noise_then_the_planes() {
        let dimensions = ImageDimensions::new((10, 8), ChannelLayout::Mono);
        let info = |cfa_type| FrameInfo {
            dimensions,
            cfa_type,
            decoder_bytes: 100,
        };
        let rggb = CfaType::Bayer(CfaPattern::Rggb);
        assert_eq!(
            info(rggb).decoded(),
            Footprint {
                kept: 400,
                peak: 500,
            }
        );
        let noise = MosaicNoiseMeasure::heap_bytes(dimensions.size(), &rggb);
        let demosaic_peak = 4 * 320 + rcd::internals::workspace_bytes(4) + 80;
        assert_eq!(
            info(rggb).demosaiced(info(rggb).decoded(), MarkesteijnPasses::One, 4),
            Footprint {
                kept: 960 + 80,
                peak: demosaic_peak.max(400 + noise).max(500),
            }
        );
        let mono = info(CfaType::Mono);
        assert_eq!(
            mono.demosaiced(mono.decoded(), MarkesteijnPasses::One, 4),
            Footprint {
                kept: 400,
                peak: 500,
            }
        );
    }
}
