//! [`FrameInfo`]: what a frame is, read from its file's header without decoding it.

use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::Demosaic;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::pixel_flags::DilationScratch;
use crate::image::stackable_image::StackableImage;
use crate::io::error::OverBudget;
use crate::io::load_ctx::LoadCtx;
use crate::memory::footprint::Footprint;
use crate::noise::mosaic_noise_measure::MosaicNoiseMeasure;

/// What a frame is, read from its header: its shape, its sensor's pattern — [`CfaType::Mono`] for
/// a frame of no filter, or of one already demosaiced — what its decoder holds, and the bytes of
/// the flat gain its file stores, none for a file with none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameInfo {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) cfa_type: CfaType,
    pub(crate) decoder: DecoderBytes,
    pub(crate) gain_bytes: usize,
}

/// What a decoder holds while it makes a frame, beside the frame's own bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DecoderBytes {
    /// Held until the frame's samples are allocated, then freed: a camera RAW's whole file, which
    /// LibRaw parses in place until it unpacked the sensor data.
    pub(crate) source: usize,
    /// Held beside the frame while it is made: the raw buffer LibRaw unpacks into, a FITS file's
    /// chunk buffers.
    pub(crate) beside: usize,
}

impl DecoderBytes {
    /// The peak of a decode that makes a frame of `frame` bytes: what stays beside, with the source
    /// before the frame and the frame after it.
    pub(crate) const fn peak(self, frame: usize) -> usize {
        self.beside
            + if self.source > frame {
                self.source
            } else {
                frame
            }
    }
}

impl FrameInfo {
    /// What `image`, decoded already, settles: its decoder is done, so what it held is not known.
    pub(crate) fn of_decoded(image: &impl StackableImage) -> Self {
        Self {
            dimensions: image.dimensions(),
            cfa_type: image.cfa_type().unwrap_or(CfaType::Mono),
            decoder: DecoderBytes::default(),
            gain_bytes: image.flat_gain().map_or(0, |gain| gain.ram_bytes()),
        }
    }

    /// The frame's decode: its samples and its flag plane kept, which a decoder that flags
    /// saturation gives most frames, and the decoder's bytes on the way.
    pub(crate) const fn decoded(self) -> Footprint {
        let frame = self.dimensions.decoded_bytes();
        Footprint {
            kept: frame,
            peak: self.decoder.peak(frame),
        }
    }

    /// `way`, which ends on the frame's decoded mosaic, then the demosaic on `workers`, an X-Trans
    /// one in `passes`: the noise the demosaic measures on the mosaic beside it, the block ORs its
    /// flags grow by to the demosaic's support, then the demosaic, which carries the flags to the
    /// frame it makes.
    pub(crate) fn demosaiced(
        self,
        way: Footprint,
        passes: MarkesteijnPasses,
        workers: usize,
    ) -> Footprint {
        let size = self.dimensions.size();
        let noise = self
            .cfa_type
            .mosaic()
            .map_or(0, |mosaic| MosaicNoiseMeasure::heap_bytes(size, mosaic));
        let dilation = self
            .cfa_type
            .mosaic()
            .map_or(0, |_| DilationScratch::heap_bytes(size, workers));
        let flags = self.dimensions.flag_plane_bytes();
        let demosaic = Demosaic::of(self.cfa_type, passes).footprint(self.dimensions, workers);
        way.beside(noise).beside(dilation).then(Footprint {
            kept: demosaic.kept + flags,
            peak: demosaic.peak + flags,
        })
    }

    /// Refuse a decode of the frame and its demosaic past `ctx`'s memory limit, from the
    /// header before either runs: the way a preview of a mosaic takes, its demosaic on every
    /// thread of the pool it runs on.
    pub(super) fn admit_demosaic(self, ctx: &LoadCtx) -> Result<(), OverBudget> {
        let way = self.demosaiced(
            self.decoded(),
            ctx.xtrans_passes,
            rayon::current_num_threads(),
        );
        OverBudget::check(
            u64::try_from(way.peak).expect("a byte count fits u64"),
            ctx.memory_limit_bytes,
        )
    }
}

#[cfg(test)]
mod tests;
