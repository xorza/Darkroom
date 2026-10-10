//! What a decoder decided, recorded alongside the samples it produced.
//!
//! [`ImageProvenance`] is the record; the enums below are its fields, each naming one axis of the
//! decision — which container, which decoder, what transfer function, what colour interpretation,
//! which demosaic.

use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use imaginarium::FileFormat;
use serde::{Deserialize, Serialize};

use crate::io::image::fits::provenance::FitsTransferProvenance;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceContainer {
    Fits,
    CameraRaw,
    Tiff,
    Png,
    Jpeg,
}

impl From<FileFormat> for SourceContainer {
    fn from(format: FileFormat) -> Self {
        match format {
            FileFormat::Png => Self::Png,
            FileFormat::Jpeg => Self::Jpeg,
            FileFormat::Tiff => Self::Tiff,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderProvenance {
    FitsWell,
    LibRaw,
    Imaginarium,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransferProvenance {
    /// FITS samples divided into the pipeline's `[0, 1]` domain. The physical values the file
    /// declared are recoverable through the image's [`SampleDomain`](crate::SampleDomain).
    FitsNormalized(FitsTransferProvenance),
    /// Sensor samples above black, divided into the same domain by `maximum − black`.
    RawNormalized,
    /// A floating-point raster, taken as linear: no TIFF tag lumos reads states a transfer
    /// function, and float samples are how linear data is stored.
    FloatRaster,
    /// A raster decoded for display only, whose transfer function is not read.
    UnspecifiedRaster,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorProvenance {
    SensorCfa,
    SensorRgb,
    Monochrome,
    Unspecified,
    UnmanagedRaster { alpha_dropped: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemosaicProvenance {
    None,
    LumosRcd,
    LumosMarkesteijn {
        passes: MarkesteijnPasses,
    },
    LibRaw,
    /// No demosaic: a CFA drizzle put each photosite into the channel of its colour alone.
    CfaDrizzle,
    /// No demosaic: the green photosites as they are, the others filled from their green
    /// neighbours — a plane to register on, not a colour image.
    GreenProxy,
}

/// Which end of the image the first stored row belongs to.
///
/// FITS declares this with `ROWORDER`, and the rows are decoded in file order whatever it says —
/// Siril's rule that "`ROWORDER` shall not be used to unflip the image data for stacking", which
/// keeps a frame's samples where the file put them. Only the Bayer phase is corrected for it, in
/// `read_bayer_cfa`.
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
#[derive(Debug, Clone, PartialEq)]
pub struct ImageProvenance {
    pub container: SourceContainer,
    pub decoder: DecoderProvenance,
    pub transfer: TransferProvenance,
    pub color: ColorProvenance,
    /// Whether this load path itself clipped samples.
    pub clipped: bool,
    pub demosaic: DemosaicProvenance,
    /// Which end of the image the first stored row belongs to, as the source declared it. The rows
    /// were not reordered to match — see [`RowOrder`].
    pub row_order: RowOrder,
}
