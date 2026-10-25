//! What a decoder decided, recorded alongside the samples it produced.
//!
//! [`ImageProvenance`] is the record; the enums below are its fields, each naming one axis of the
//! decision — which source and how it became samples, what color interpretation, which demosaic.

use std::fmt;
use std::fmt::{Display, Formatter};

use imaginarium::RasterFormat;
use serde::{Deserialize, Serialize};

use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::metadata::fits_transfer_provenance::FitsTransferProvenance;

/// The source the samples came from, and how its decoder made samples of what it stored. Each
/// container has one decoder, so a pairing no load makes cannot be stated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SourceProvenance {
    /// A FITS file read by fits-well, its samples divided into the pipeline's `[0, 1]` domain. The
    /// physical values the file declared are recoverable through the image's
    /// [`SampleDomain`](crate::SampleDomain).
    Fits(FitsTransferProvenance),
    /// A camera RAW read by LibRaw, its sensor samples above black divided into the same domain by
    /// `maximum − black`.
    CameraRaw,
    /// A raster read by imaginarium.
    Raster {
        format: RasterFormat,
        transfer: RasterTransfer,
    },
}

impl SourceProvenance {
    /// The decoder that read the source.
    pub const fn decoder(&self) -> DecoderProvenance {
        match self {
            Self::Fits(_) => DecoderProvenance::FitsWell,
            Self::CameraRaw => DecoderProvenance::LibRaw,
            Self::Raster { .. } => DecoderProvenance::Imaginarium,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecoderProvenance {
    FitsWell,
    LibRaw,
    Imaginarium,
}

/// How a raster's samples relate to light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RasterTransfer {
    /// Floating-point samples, taken as linear: no TIFF tag lumos reads states a transfer
    /// function, and float samples are how linear data is stored.
    Float,
    /// Samples decoded for display only, whose transfer function is not read.
    Unspecified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorProvenance {
    SensorCfa,
    SensorRgb,
    Monochrome,
    Unspecified,
    UnmanagedRaster { alpha_dropped: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DemosaicProvenance {
    None,
    LumosRcd,
    LumosMarkesteijn {
        passes: MarkesteijnPasses,
    },
    LibRaw,
    /// No demosaic: a CFA drizzle put each photosite into the channel of its color alone.
    CfaDrizzle,
    /// No demosaic: the green photosites as they are, the others filled from their green
    /// neighbours — a plane to register on, not a color image.
    GreenProxy,
}

/// Which end of the image the first stored row belongs to.
///
/// FITS declares this with `ROWORDER`, and the rows are decoded in file order whatever it says —
/// Siril's rule that "`ROWORDER` shall not be used to unflip the image data for stacking", which
/// keeps a frame's samples where the file put them. Only the Bayer phase is corrected for it, in
/// `Bayerpat::read_cfa`.
///
/// The consequence is that two frames of one target declaring different orders load as vertically
/// mirrored images. Registration cannot reconcile that — triangle matching rejects a mirrored field
/// outright by default, and a similarity transform could not express the reflection anyway — so the
/// order is recorded here and frames are held to agreeing on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowOrder {
    /// The first stored row is the top of the image.
    TopDown,
    /// The first stored row is the bottom — the image is stored upside-down relative to display.
    BottomUp,
}

impl RowOrder {
    /// The FITS `ROWORDER` value for this order.
    ///
    /// The one spelling: what the writer emits, what the reader compares against, and what an error
    /// message prints. Three copies of a format string is three chances for one of them to drift
    /// from the file format.
    pub(crate) const fn keyword(self) -> &'static str {
        match self {
            Self::TopDown => "TOP-DOWN",
            Self::BottomUp => "BOTTOM-UP",
        }
    }
}

impl Display for RowOrder {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.keyword())
    }
}

/// Decoder decisions that affect the meaning of the returned samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageProvenance {
    pub source: SourceProvenance,
    pub color: ColorProvenance,
    /// Whether this load path itself clipped samples.
    pub clipped: bool,
    pub demosaic: DemosaicProvenance,
    /// Which end of the image the first stored row belongs to, as the source declared it. The rows
    /// were not reordered to match — see [`RowOrder`].
    pub row_order: RowOrder,
}

#[cfg(test)]
mod tests {
    use common::SerdeFormat;
    use imaginarium::RasterFormat;

    use crate::image::metadata::image_provenance::{RasterTransfer, SourceProvenance};

    /// imaginarium's format is the provenance's own field: it writes by its variant name and
    /// reads back as itself.
    #[test]
    fn a_raster_source_round_trips_through_ron() {
        let source = SourceProvenance::Raster {
            format: RasterFormat::Jpeg,
            transfer: RasterTransfer::Unspecified,
        };
        let text = SerdeFormat::Ron.serialize(&source).unwrap();
        assert_eq!(
            String::from_utf8(text.clone()).unwrap(),
            "Raster(\n    format: Jpeg,\n    transfer: Unspecified,\n)"
        );
        assert_eq!(
            SerdeFormat::Ron
                .deserialize::<SourceProvenance>(&text)
                .unwrap(),
            source
        );
    }
}
