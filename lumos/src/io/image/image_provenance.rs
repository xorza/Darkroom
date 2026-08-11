//! What a decoder decided, recorded alongside the samples it produced.
//!
//! [`ImageProvenance`] is the record; the enums below are its fields, each naming one axis of the
//! decision — which container, which decoder, what transfer function, what colour interpretation,
//! which demosaic.

use crate::io::image::fits::provenance::FitsTransferProvenance;

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
    RawNormalized,
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
}
