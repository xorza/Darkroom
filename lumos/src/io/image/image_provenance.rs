//! What a decoder decided, recorded alongside the samples it produced.
//!
//! [`ImageProvenance`] is the record; the enums below are its fields, each naming one axis of the
//! decision — which container, which decoder, what transfer function, what colour interpretation,
//! which demosaic.

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
