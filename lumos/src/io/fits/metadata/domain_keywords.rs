//! The keywords that carry an image's [`SampleDomain`](crate::SampleDomain), quantization σ and
//! saturation record through a lumos-written FITS file.
//!
//! The samples go out already normalized, with the scale they were normalized by as `BSCALE`, so
//! every FITS reader gets the source's values back in the stated `BUNIT`. The rest of the domain is
//! what no reader can recover from the data. Every field is written, the origin of an assumed
//! scale included, so a reload gives back the same domain and a saved master stays commensurate
//! with the frames it was made from.

use fits_well::header::{Header, Keyword};
use fits_well::image::Scaling;

use crate::image::image_metadata::ImageMetadata;
use crate::image::sample_domain::{Pedestal, ScaleOrigin};
use crate::io::fits::error::{FitsLoadError, FitsRefusal};

/// Whether the `BSCALE` the samples carry was declared or assumed. Its presence marks a
/// lumos-written scale; see `SampleScale` in the decoder for how a reader uses it.
pub(crate) const SCALE_ORIGIN: Keyword = Keyword::new("LUMSORIG");
pub(crate) const PEDESTAL: Keyword = Keyword::new("LUMPED");
pub(crate) const PEDESTAL_LEVEL: Keyword = Keyword::new("LUMPEDLV");
/// In the samples' own (normalized) units, so a reader takes it as it stands.
const QUANTIZATION_SIGMA: Keyword = Keyword::new("LUMQSIG");
/// That every saturated pixel is flagged in the image's flags extension. A calibrated frame has no
/// level left to mark them by, so a reload learns it here; `DATAMAX` says it for a frame as decoded.
const SATURATION_FLAGGED: Keyword = Keyword::new("LUMSATF");

const DECLARED: &str = "DECLARED";
const ASSUMED: &str = "ASSUMED";
const REMOVED: &str = "REMOVED";
const KEPT: &str = "KEPT";

/// The scaling normalized samples are written with: `BSCALE` the domain's scale, so the physical
/// values are the source's own, in its `BUNIT`.
pub(crate) fn scaling(metadata: &ImageMetadata) -> Scaling {
    metadata
        .derived
        .domain()
        .map_or(Scaling::IDENTITY, |domain| Scaling {
            bscale: domain.scale,
            ..Scaling::IDENTITY
        })
}

/// Write what `metadata` says its samples are worth beside the `BSCALE` of [`scaling`]: the rest
/// of the domain, the quantization σ, and whether every saturated pixel is flagged.
pub(super) fn write(header: &mut Header, metadata: &ImageMetadata) -> fits_well::Result<()> {
    if let Some(domain) = metadata.derived.domain() {
        header.set(
            SCALE_ORIGIN,
            match domain.origin {
                ScaleOrigin::Declared => DECLARED,
                ScaleOrigin::Assumed => ASSUMED,
            },
        )?;
        match domain.pedestal {
            Pedestal::Removed => {
                header.set(PEDESTAL, REMOVED)?;
            }
            Pedestal::Kept(level) => {
                header.set(PEDESTAL, KEPT)?;
                header.set(PEDESTAL_LEVEL, level)?;
            }
            // Absent is how a reader learns that nothing is known.
            Pedestal::Unknown => {}
        }
        if let Some(unit) = &domain.unit {
            header.set("BUNIT", unit.as_str())?;
        }
    }
    if let Some(sigma) = metadata.derived.quantization_sigma() {
        debug_assert!(
            sigma.is_finite() && sigma >= 0.0,
            "a quantization sigma is a finite step: {sigma}"
        );
        header.set(QUANTIZATION_SIGMA, f64::from(sigma))?;
    }
    if metadata.derived.saturation_flagged() {
        header.set(SATURATION_FLAGGED, true)?;
    }
    Ok(())
}

/// Whether the header records that every saturated pixel is flagged.
pub(crate) fn read_saturation_flagged(header: &Header) -> Result<bool, FitsLoadError> {
    Ok(header.get_logical(SATURATION_FLAGGED)?.unwrap_or(false))
}

/// The origin of a lumos-written `BSCALE`, or `None` when the header records none.
pub(crate) fn read_origin(header: &Header) -> Result<Option<ScaleOrigin>, FitsLoadError> {
    match header.get_text(SCALE_ORIGIN)?.map(str::trim) {
        None => Ok(None),
        Some(DECLARED) => Ok(Some(ScaleOrigin::Declared)),
        Some(ASSUMED) => Ok(Some(ScaleOrigin::Assumed)),
        Some(value) => Err(FitsRefusal::ScaleOrigin {
            value: value.to_owned(),
        }
        .into()),
    }
}

/// The recorded pedestal, or `None` when the header records none.
pub(crate) fn read_pedestal(header: &Header) -> Result<Option<Pedestal>, FitsLoadError> {
    match header.get_text(PEDESTAL)?.map(str::trim) {
        None => Ok(None),
        Some(REMOVED) => Ok(Some(Pedestal::Removed)),
        Some(KEPT) => match header.get_real(PEDESTAL_LEVEL)? {
            Some(level) if level.is_finite() => Ok(Some(Pedestal::Kept(level))),
            _ => Err(FitsRefusal::PedestalLevel.into()),
        },
        Some(value) => Err(FitsRefusal::Pedestal {
            value: value.to_owned(),
        }
        .into()),
    }
}

pub(crate) fn read_quantization_sigma(header: &Header) -> Result<Option<f32>, FitsLoadError> {
    header
        .get_real(QUANTIZATION_SIGMA)?
        .map(|value| {
            let value = value as f32;
            if value.is_finite() && value >= 0.0 {
                Ok(value)
            } else {
                Err(FitsRefusal::KeywordOutOfRange {
                    keyword: QUANTIZATION_SIGMA.to_string(),
                }
                .into())
            }
        })
        .transpose()
}
