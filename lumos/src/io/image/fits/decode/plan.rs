use std::path::Path;

use fits_well::FitsError;
use fits_well::header::Header;
use fits_well::image::Scaling;
use fits_well::image::{Bitpix as FitsBitpix, ImageMetadata, SampleType};
use fits_well::io::{BLOCK_SIZE, Hdu, HduKind};

use crate::io::image::error::ImageError;

use crate::io::image::fits::metadata;
use crate::io::image::fits::metadata::domain_keywords;
use crate::io::image::fits::options::{FitsCubeInterpretation, FitsFloatScale};
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::sample_domain::ScaleOrigin;

const FITS_DECODE_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Above this, a floating-point HDU's `DATAMAX` is read as declaring an ADU saturation level rather
/// than a normalized one. Siril's threshold, and well clear of both the `[0, 1]` convention and the
/// slight overshoot an interpolated or stacked frame can carry past unity.
const FLOAT_ADU_DATAMAX_MIN: f64 = 10.0;

/// What such a frame is divided by: the 16-bit full scale, which is the depth essentially every
/// camera that writes ADU into a float FITS digitizes at. Siril and PixInsight both use it.
const FLOAT_ADU_DIVISOR: f32 = 65_535.0;

#[derive(Debug, Clone, Copy)]
pub(super) struct FitsHduDescription<'a> {
    header: &'a Header,
    kind: HduKind,
    image: ImageMetadata<'a>,
    source_bytes: u64,
}

impl<'a> FitsHduDescription<'a> {
    pub(super) fn from_hdu(path: &Path, hdu: &'a Hdu) -> Result<Self, ImageError> {
        let image = hdu.image().map_err(|source| match source {
            FitsError::NotAnImage => {
                ImageError::fits_unsupported(path, "selected HDU is not an image")
            }
            source => ImageError::fits(path, source),
        })?;
        Ok(Self {
            header: &hdu.header,
            kind: hdu.kind,
            image,
            source_bytes: padded_data_bytes(path, hdu.data_bytes)?,
        })
    }
}

#[derive(Debug)]
pub(super) struct FitsDecodePlan {
    pub(super) shape: Vec<usize>,
    pub(super) dimensions: ImageDimensions,
    pub(super) sample_type: SampleType,
    pub(super) scaling: Scaling,
    /// How the stored samples reach the pipeline's `[0, 1]` domain. See [`sample_scale`].
    pub(super) sample_scale: SampleScale,
    /// The header's `DATAMAX`, in the file's sample units: the decode flags saturation against it.
    pub(super) data_max: Option<f64>,
    pub(super) source_bytes: u64,
    pub(super) decoded_bytes: u64,
    /// The flag plane the decode builds when the header allows a null or declares a `DATAMAX`.
    flag_plane_bytes: u64,
    pub(super) peak_bytes: u64,
    pub(super) rows_per_chunk: usize,
}

impl FitsDecodePlan {
    /// Refuse a flags extension the memory limit cannot hold beside the decoded image: its bytes
    /// as read, and the flag plane the decode may hold until the two are joined.
    pub(super) fn admit_flags_extension(
        &self,
        path: &Path,
        memory_limit_bytes: u64,
    ) -> Result<(), ImageError> {
        let extension_bytes = checked_size_bytes(
            path,
            self.dimensions.pixel_count(),
            1,
            "FITS flags extension",
        )?;
        let required = self
            .decoded_bytes
            .checked_add(extension_bytes)
            .and_then(|bytes| bytes.checked_add(self.flag_plane_bytes))
            .ok_or_else(|| {
                ImageError::fits_unsupported(path, "FITS flags memory size overflows u64")
            })?;
        enforce_fits_budget(
            path,
            "decoded output with its flags",
            required,
            memory_limit_bytes,
        )
    }
}

/// Whether a decode of samples stored as `sample_type` under `scaling` could produce pixels with no
/// measurement, settled from the header alone.
///
/// An integer `BITPIX` produces a null only where a stored sample equals `BLANK`, so a header
/// carrying no such keyword *proves* there are none — and that is every frame a camera writes. A
/// floating-point one carries its nulls in-band as IEEE NaN, with nothing in the header to announce
/// them, so it answers `true` whether or not any are actually there. Wrong only in the direction
/// that over-reserves.
const fn may_carry_nulls(sample_type: SampleType, scaling: &Scaling) -> bool {
    !sample_type.is_integer() || scaling.blank.is_some()
}

/// What one full-scale span of the stored integer type measures, in physical units.
///
/// The pipeline's linear domain is `[0, 1]`, so an integer FITS is divided by the span its own
/// `BITPIX` and `BSCALE` declare: `|BSCALE| × (2^bits − 1)`. That maps the FITS unsigned
/// convention (`BITPIX = 16`, `BZERO = 2¹⁵`) exactly onto `[0, 1]`, and puts a signed frame on
/// `[-0.5, 0.5]` around its own zero.
///
/// Only the scale is applied — never an offset, and never a clamp. Offsetting would move a signed
/// frame's zero point, and clamping would cut the sub-pedestal noise tail that the calibration
/// path depends on (see [`crate::io::raw`]'s unclamped normalization).
///
/// A floating-point `BITPIX` declares no full scale, so the only evidence available is `DATAMAX`: a
/// declared saturation level above [`FLOAT_ADU_DATAMAX_MIN`] means the samples are ADU rather than
/// `[0, 1]` and they are divided by [`FLOAT_ADU_DIVISOR`], the threshold and divisor Siril uses.
/// Anything else — a `DATAMAX` of about 1, or none at all — is taken as already normalized, which
/// is PixInsight's default for a float FITS and what keeps a Lumos-written master round-tripping.
///
/// The divisor comes from the *header*, never from the pixels, and that is where this departs from
/// Siril: with `DATAMAX` absent it scans the data instead (three sampled pixels on the partial-read
/// path, which is why its full and partial reads can disagree about the same file). A divisor read
/// off each frame's own extrema differs frame to frame, which is exactly what [`crate::combine`]
/// rejects a frame set for. The scan still decides one thing: a frame taken as normalized whose
/// samples reach past Siril's threshold is refused rather than loaded as it stands
/// ([`SampleScale::verify_normalized`]).
fn sample_scale(
    path: &Path,
    header: &Header,
    stored: FitsBitpix,
    scaling: &Scaling,
    float_scale: FitsFloatScale,
) -> Result<SampleScale, ImageError> {
    let divided_by = |divisor: f32, origin| SampleScale {
        divisor,
        physical: f64::from(divisor),
        origin,
        verify_normalized: false,
    };
    let steps = match stored {
        FitsBitpix::U8 => f64::from(u8::MAX),
        FitsBitpix::I16 => f64::from(u16::MAX),
        FitsBitpix::I32 => f64::from(u32::MAX),
        FitsBitpix::I64 => u64::MAX as f64,
        FitsBitpix::F32 | FitsBitpix::F64 => {
            // A lumos-written file stores its samples already normalized and records the scale
            // they were normalized by; that record beats every guess below.
            if let Some(recorded) = header
                .get_real(domain_keywords::SAMPLE_SCALE)
                .map_err(|source| ImageError::fits(path, source))?
            {
                if !recorded.is_finite() || recorded <= 0.0 {
                    return Err(ImageError::fits_unsupported(
                        path,
                        format!(
                            "{} {recorded} must be finite and positive",
                            domain_keywords::SAMPLE_SCALE
                        ),
                    ));
                }
                return Ok(SampleScale {
                    divisor: 1.0,
                    physical: recorded,
                    origin: domain_keywords::read_origin(header)
                        .map_err(|source| ImageError::fits(path, source))?,
                    verify_normalized: false,
                });
            }
            return match float_scale {
                // "Already normalized" says where the samples sit, not what a unit of them is
                // worth in the source's own terms, so the scale stays a guess.
                FitsFloatScale::Normalized => Ok(divided_by(1.0, ScaleOrigin::Assumed)),
                FitsFloatScale::FullScale(scale) => {
                    // The caller's own figure, so it is checked here rather than trusted: a
                    // non-positive one would invert or erase the samples.
                    if !scale.is_finite() || scale <= 0.0 {
                        return Err(ImageError::fits_unsupported(
                            path,
                            format!("declared floating-point full scale {scale} must be positive"),
                        ));
                    }
                    Ok(divided_by(scale, ScaleOrigin::Declared))
                }
                FitsFloatScale::Auto => {
                    let data_max = header
                        .get_real("DATAMAX")
                        .map_err(|source| ImageError::fits(path, source))?;
                    Ok(match data_max {
                        Some(max) if max > FLOAT_ADU_DATAMAX_MIN => {
                            divided_by(FLOAT_ADU_DIVISOR, ScaleOrigin::Assumed)
                        }
                        _ => SampleScale {
                            verify_normalized: true,
                            ..divided_by(1.0, ScaleOrigin::Assumed)
                        },
                    })
                }
            };
        }
    };
    // File-derived metadata: a corrupt or hand-edited header can carry any BSCALE, and a zero or
    // non-finite one leaves no span to normalize into. Reject rather than emit infinities.
    let bscale = scaling.bscale;
    if !bscale.is_finite() || bscale == 0.0 {
        return Err(ImageError::fits_unsupported(
            path,
            format!("BSCALE {bscale} leaves no scale to normalize integer samples by"),
        ));
    }
    let divisor = bscale.abs() * steps;
    if !divisor.is_finite() || divisor <= 0.0 {
        return Err(ImageError::fits_unsupported(
            path,
            format!("BSCALE {bscale} overflows the normalization scale for {stored:?} samples"),
        ));
    }
    Ok(divided_by(divisor as f32, ScaleOrigin::Declared))
}

/// How one HDU's stored samples reach the pipeline's `[0, 1]` domain, and what a decoded unit is
/// worth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SampleScale {
    /// The decoder divides every stored (physical) sample by this.
    pub(super) divisor: f32,
    /// Multiply a decoded sample by this to recover the file's physical value. Equal to `divisor`
    /// except for a lumos-written file, whose samples were stored already divided.
    pub(super) physical: f64,
    pub(super) origin: ScaleOrigin,
    /// The samples were taken as already normalized because nothing declared otherwise, so the
    /// decode checks that they are: a maximum past [`FLOAT_ADU_DATAMAX_MIN`] is ADU, and loading it
    /// as normalized would put the saturation level at 0.95 ADU.
    pub(super) verify_normalized: bool,
}

impl SampleScale {
    /// Whether a frame whose samples reach `maximum` after the division is what this scale says it
    /// is. Only an unchecked "already normalized" can be wrong this way.
    pub(super) fn accepts_maximum(&self, maximum: f32) -> bool {
        !self.verify_normalized || f64::from(maximum) <= FLOAT_ADU_DATAMAX_MIN
    }
}

pub(super) fn preflight_fits_image(
    path: &Path,
    hdu: FitsHduDescription<'_>,
    cube: FitsCubeInterpretation,
    float_scale: FitsFloatScale,
    memory_limit_bytes: u64,
) -> Result<FitsDecodePlan, ImageError> {
    let dimensions = dimensions_from_shape(path, hdu.image.shape, cube)?;
    let stored_bitpix = hdu.image.bitpix;
    let scaling = hdu.image.scaling;
    let sample_type = SampleType::from_scaling(stored_bitpix, &scaling);
    let sample_scale = sample_scale(path, hdu.header, stored_bitpix, &scaling, float_scale)?;
    let decoded_bytes = checked_size_bytes(
        path,
        dimensions.sample_count(),
        size_of::<f32>(),
        "decoded FITS output",
    )?;
    let row_samples = dimensions.width();
    let row_f32_bytes = row_samples.checked_mul(size_of::<f32>()).ok_or_else(|| {
        ImageError::fits_unsupported(path, "FITS output row size overflows usize")
    })?;
    let rows_per_chunk = (FITS_DECODE_CHUNK_BYTES / row_f32_bytes.max(1))
        .max(1)
        .min(dimensions.height());
    let chunk_samples = row_samples.checked_mul(rows_per_chunk).ok_or_else(|| {
        ImageError::fits_unsupported(path, "FITS decode chunk size overflows usize")
    })?;
    let native_chunk_bytes = checked_size_bytes(
        path,
        chunk_samples,
        stored_bitpix.elem_size(),
        "FITS native decode chunk",
    )?;
    let data_max = metadata::read_data_max(hdu.header);
    // A byte per pixel, which the decode builds beside the planes once it meets a null or a
    // saturation level to flag against.
    let flag_plane_bytes = if may_carry_nulls(sample_type, &scaling) || data_max.is_some() {
        checked_size_bytes(path, dimensions.pixel_count(), 1, "FITS flag plane")?
    } else {
        0
    };
    // The reader's copy of a chunk as stored, and the scratch it byte-swaps or decodes it into,
    // which the conversion writes straight into the output and which is still held while the flag
    // plane is built.
    let peak_bytes = decoded_bytes
        .checked_add(native_chunk_bytes)
        .and_then(|bytes| bytes.checked_add(native_chunk_bytes))
        .and_then(|bytes| bytes.checked_add(flag_plane_bytes))
        .and_then(|bytes| {
            if hdu.kind == HduKind::CompressedImage {
                bytes.checked_add(hdu.source_bytes.checked_mul(2)?)
            } else {
                Some(bytes)
            }
        })
        .ok_or_else(|| ImageError::fits_unsupported(path, "FITS peak memory size overflows u64"))?;

    enforce_fits_budget(
        path,
        "stored data unit",
        hdu.source_bytes,
        memory_limit_bytes,
    )?;
    enforce_fits_budget(path, "decoded output", decoded_bytes, memory_limit_bytes)?;
    enforce_fits_budget(
        path,
        "estimated peak memory",
        peak_bytes,
        memory_limit_bytes,
    )?;

    Ok(FitsDecodePlan {
        shape: hdu.image.shape.to_vec(),
        dimensions,
        sample_type,
        scaling,
        sample_scale,
        data_max,
        source_bytes: hdu.source_bytes,
        decoded_bytes,
        flag_plane_bytes,
        peak_bytes,
        rows_per_chunk,
    })
}

fn checked_size_bytes(
    path: &Path,
    elements: usize,
    element_bytes: usize,
    name: &str,
) -> Result<u64, ImageError> {
    elements
        .checked_mul(element_bytes)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| ImageError::fits_unsupported(path, format!("{name} size overflows usize")))
}

fn enforce_fits_budget(
    path: &Path,
    name: &str,
    required: u64,
    memory_limit_bytes: u64,
) -> Result<(), ImageError> {
    if required > memory_limit_bytes {
        return Err(ImageError::fits_unsupported(
            path,
            format!(
                "{name} requires {required} bytes, exceeding the FITS load budget of {memory_limit_bytes} bytes"
            ),
        ));
    }
    Ok(())
}

fn padded_data_bytes(path: &Path, bytes: u64) -> Result<u64, ImageError> {
    if bytes == 0 {
        return Ok(0);
    }
    bytes
        .checked_add(BLOCK_SIZE as u64 - 1)
        .map(|padded| padded / BLOCK_SIZE as u64 * BLOCK_SIZE as u64)
        .ok_or_else(|| {
            ImageError::fits_unsupported(path, "FITS padded data-unit size overflows u64")
        })
}

pub(super) fn dimensions_from_shape(
    path: &Path,
    shape: &[usize],
    cube: FitsCubeInterpretation,
) -> Result<ImageDimensions, ImageError> {
    if shape.contains(&0) {
        return Err(ImageError::fits_unsupported(
            path,
            format!("FITS image axes must be nonzero, got {shape:?}"),
        ));
    }
    let (width, height, channels) = match shape {
        [width, height] | [width, height, 1] => (*width, *height, 1),
        [width, height, 3] if cube == FitsCubeInterpretation::Rgb => (*width, *height, 3),
        [_, _, 3] => Err(ImageError::fits_unsupported(
            path,
            "three-plane FITS cube requires FitsCubeInterpretation::Rgb",
        ))?,
        [_, _, channels] => Err(ImageError::fits_unsupported(
            path,
            format!("Unsupported channel count (NAXIS3): {channels}"),
        ))?,
        _ => {
            return Err(ImageError::fits_unsupported(
                path,
                format!("Unsupported number of dimensions: {}", shape.len()),
            ));
        }
    };
    if width > ImageDimensions::MAX_SIDE || height > ImageDimensions::MAX_SIDE {
        return Err(ImageError::fits_unsupported(
            path,
            format!(
                "FITS image {width}x{height} has a side past {} px",
                ImageDimensions::MAX_SIDE
            ),
        ));
    }
    let pixel_count = width.checked_mul(height).ok_or_else(|| {
        ImageError::fits_unsupported(path, format!("FITS pixel count overflows: {shape:?}"))
    })?;
    pixel_count.checked_mul(channels).ok_or_else(|| {
        ImageError::fits_unsupported(path, format!("FITS sample count overflows: {shape:?}"))
    })?;
    Ok(ImageDimensions::new((width, height), channels))
}

#[cfg(test)]
pub(super) mod internals {
    use fits_well::header::Header;
    use fits_well::image::{Bitpix, ImageMetadata};
    use fits_well::io::HduKind;

    use crate::io::image::fits::decode::plan::FitsHduDescription;

    /// An HDU of `kind` whose image is `shape` samples of `bitpix`, scaled as `header`
    /// declares.
    pub(crate) fn description<'a>(
        header: &'a Header,
        kind: HduKind,
        shape: &'a [usize],
        bitpix: Bitpix,
        source_bytes: u64,
    ) -> FitsHduDescription<'a> {
        FitsHduDescription {
            header,
            kind,
            image: ImageMetadata {
                shape,
                bitpix,
                scaling: header.scaling().unwrap(),
            },
            source_bytes,
        }
    }
}
