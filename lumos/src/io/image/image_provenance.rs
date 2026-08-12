//! What a decoder decided, recorded alongside the samples it produced.
//!
//! [`ImageProvenance`] is the record; the enums below are its fields, each naming one axis of the
//! decision — which container, which decoder, what transfer function, what colour interpretation,
//! which demosaic.

use serde::{Deserialize, Serialize};

use crate::io::image::fits::provenance::FitsTransferProvenance;
use crate::io::image::sample_domain::SampleDomain;
use crate::io::raw::provenance::RawTransferProvenance;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceContainer {
    Fits,
    CameraRaw,
    Tiff,
    Png,
    Jpeg,
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
    /// declared are recoverable through [`FitsTransferProvenance::physical_scale`].
    FitsNormalized(FitsTransferProvenance),
    /// Sensor samples divided into the same domain by `maximum − black` — see
    /// [`RawTransferProvenance::physical_scale`].
    RawNormalized(RawTransferProvenance),
    DeclaredLinearRaster,
    UnspecifiedRaster,
}

impl TransferProvenance {
    /// The FITS transfer record, or `None` for samples that did not come from a FITS HDU.
    pub(crate) fn fits(&self) -> Option<&FitsTransferProvenance> {
        match self {
            TransferProvenance::FitsNormalized(transfer) => Some(transfer),
            _ => None,
        }
    }

    /// What one decoded sample is worth in the source's own terms, whichever decoder produced it.
    ///
    /// The span each path divided by to reach `[0, 1]`, paired with the unit that span was
    /// expressed in. `None` means the samples carry no declared domain to compare — a preview
    /// raster, or an image this crate synthesized rather than decoded — and a caller that needs to
    /// know two frames agree has to treat that as "cannot tell", not as "the same".
    pub fn sample_domain(&self) -> Option<SampleDomain> {
        match self {
            TransferProvenance::FitsNormalized(transfer) => Some(SampleDomain {
                scale: transfer.physical_scale,
                unit: transfer.unit.clone(),
            }),
            TransferProvenance::RawNormalized(transfer) => Some(SampleDomain {
                scale: transfer.physical_scale,
                // Sensor counts above black. No RAW format states a unit for them, and inventing
                // one here would make a RAW frame disagree with a FITS frame that spells the same
                // thing differently.
                unit: None,
            }),
            // A float raster declared linear is taken as it stands, so one sample is one unit of
            // whatever the file already held — which it does not name.
            TransferProvenance::DeclaredLinearRaster => Some(SampleDomain {
                scale: 1.0,
                unit: None,
            }),
            TransferProvenance::UnspecifiedRaster => None,
        }
    }
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
    LumosMarkesteijn,
    LibRaw,
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

impl std::fmt::Display for RowOrder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
