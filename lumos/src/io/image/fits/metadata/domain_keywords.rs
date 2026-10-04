//! The keywords that carry an image's [`SampleDomain`], quantization σ and saturation record
//! through a lumos-written FITS file.
//!
//! The samples go out already normalized, so the domain they were normalized in is the one set of
//! facts a reader cannot recover from the data. Every field is written, the origin of an assumed
//! scale included, so a reload gives back the same domain and a saved master stays commensurate
//! with the frames it was made from.

use fits_well::FitsError;
use fits_well::header::Header;

use crate::io::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};

/// The scale the samples were normalized by. See `SampleScale` in the decoder for how a reader
/// uses it.
pub(crate) const SAMPLE_SCALE: &str = "LUMSCALE";
const SCALE_ORIGIN: &str = "LUMSORIG";
const PEDESTAL: &str = "LUMPED";
const PEDESTAL_LEVEL: &str = "LUMPEDLV";
/// In the samples' own (normalized) units, so a reader takes it as it stands.
const QUANTIZATION_SIGMA: &str = "LUMQSIG";
/// That every saturated pixel is flagged in the image's flags extension. A calibrated frame has no
/// level left to mark them by, so a reload learns it here; `DATAMAX` says it for a frame as decoded.
const SATURATION_FLAGGED: &str = "LUMSATF";

const DECLARED: &str = "DECLARED";
const ASSUMED: &str = "ASSUMED";
const REMOVED: &str = "REMOVED";
const KEPT: &str = "KEPT";

pub(super) fn write(
    header: &mut Header,
    domain: Option<&SampleDomain>,
    quantization_sigma: Option<f32>,
    saturation_flagged: bool,
) -> fits_well::Result<()> {
    if let Some(domain) = domain {
        header.set(SAMPLE_SCALE, domain.scale)?;
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
    if let Some(sigma) = quantization_sigma {
        if !sigma.is_finite() || sigma < 0.0 {
            return Err(FitsError::KeywordOutOfRange {
                name: QUANTIZATION_SIGMA,
            });
        }
        header.set(QUANTIZATION_SIGMA, f64::from(sigma))?;
    }
    if saturation_flagged {
        header.set(SATURATION_FLAGGED, true)?;
    }
    Ok(())
}

/// Whether the header records that every saturated pixel is flagged.
pub(crate) fn read_saturation_flagged(header: &Header) -> fits_well::Result<bool> {
    Ok(header.get_logical(SATURATION_FLAGGED)?.unwrap_or(false))
}

/// The origin of a recorded [`SAMPLE_SCALE`]. A lumos writer records one with every scale.
pub(crate) fn read_origin(header: &Header) -> fits_well::Result<ScaleOrigin> {
    match header.get_text(SCALE_ORIGIN)?.map(str::trim) {
        Some(DECLARED) => Ok(ScaleOrigin::Declared),
        Some(ASSUMED) => Ok(ScaleOrigin::Assumed),
        _ => Err(FitsError::TypeMismatch {
            name: SCALE_ORIGIN.to_string(),
            expected: "DECLARED or ASSUMED beside LUMSCALE",
        }),
    }
}

/// The recorded pedestal, or `None` when the header records none.
pub(crate) fn read_pedestal(header: &Header) -> fits_well::Result<Option<Pedestal>> {
    match header.get_text(PEDESTAL)?.map(str::trim) {
        None => Ok(None),
        Some(REMOVED) => Ok(Some(Pedestal::Removed)),
        Some(KEPT) => match header.get_real(PEDESTAL_LEVEL)? {
            Some(level) if level.is_finite() => Ok(Some(Pedestal::Kept(level))),
            _ => Err(FitsError::TypeMismatch {
                name: PEDESTAL_LEVEL.to_string(),
                expected: "a finite level beside LUMPED = 'KEPT'",
            }),
        },
        Some(_) => Err(FitsError::TypeMismatch {
            name: PEDESTAL.to_string(),
            expected: "REMOVED or KEPT",
        }),
    }
}

pub(crate) fn read_quantization_sigma(header: &Header) -> fits_well::Result<Option<f32>> {
    header
        .get_real(QUANTIZATION_SIGMA)?
        .map(|value| {
            let value = value as f32;
            if value.is_finite() && value >= 0.0 {
                Ok(value)
            } else {
                Err(FitsError::KeywordOutOfRange {
                    name: QUANTIZATION_SIGMA,
                })
            }
        })
        .transpose()
}
