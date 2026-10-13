use std::path::Path;

use fits_well::FitsError;
use fits_well::header::Header;
use fits_well::image::Scaling;
use fits_well::image::{Bitpix as FitsBitpix, Metadata as FitsMetadata, SampleType};
use fits_well::io::{BLOCK_SIZE, Hdu, HduKind};

use crate::io::error::{FitsAllocation, FitsRefusal, ImageError};

use crate::image::image_dimensions::ImageDimensions;
use crate::image::sample_domain::ScaleOrigin;
use crate::io::fits::fits_load_options::{FitsCubeInterpretation, FitsFloatScale};
use crate::io::fits::metadata;
use crate::io::fits::metadata::domain_keywords;

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
    image: FitsMetadata<'a>,
    source_bytes: u64,
}

impl<'a> FitsHduDescription<'a> {
    /// The image at HDU `index`, `hdu`.
    pub(super) fn from_hdu(path: &Path, hdu: &'a Hdu, index: usize) -> Result<Self, ImageError> {
        let image = hdu.image().map_err(|source| match source {
            FitsError::NotAnImage => {
                ImageError::fits_unsupported(path, FitsRefusal::NotAnImage { hdu: index })
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

    /// How the image decodes under `cube` and `float_scale`, within `limit`: its shape, its
    /// scale, and the memory each step holds, refused when any would exceed the limit.
    pub(super) fn plan(
        self,
        cube: FitsCubeInterpretation,
        float_scale: FitsFloatScale,
        limit: DecodeLimit<'_>,
    ) -> Result<FitsDecodePlan, ImageError> {
        let path = limit.path;
        let dimensions = dimensions_from_shape(path, self.image.shape, cube)?;
        let stored_bitpix = self.image.bitpix;
        let scaling = self.image.scaling;
        let sample_type = SampleType::from_scaling(stored_bitpix, &scaling);
        let sample_scale = self.sample_scale(path, float_scale)?;
        let decoded_bytes = limit.checked_bytes(
            dimensions.sample_count(),
            size_of::<f32>(),
            FitsAllocation::DecodedOutput,
        )?;
        let row_samples = dimensions.width();
        let row_f32_bytes = row_samples
            .checked_mul(size_of::<f32>())
            .ok_or_else(|| overflow(path, FitsAllocation::OutputRow))?;
        let rows_per_chunk = (FITS_DECODE_CHUNK_BYTES / row_f32_bytes.max(1))
            .max(1)
            .min(dimensions.height());
        let chunk_samples = row_samples
            .checked_mul(rows_per_chunk)
            .ok_or_else(|| overflow(path, FitsAllocation::DecodeChunk))?;
        let native_chunk_bytes = limit.checked_bytes(
            chunk_samples,
            stored_bitpix.elem_size(),
            FitsAllocation::NativeDecodeChunk,
        )?;
        let data_max = metadata::read_data_max(self.header);
        // A byte per pixel, which the decode builds beside the planes once it meets a null or a
        // saturation level to flag against.
        let flag_plane_bytes = if may_carry_nulls(sample_type, &scaling) || data_max.is_some() {
            limit.checked_bytes(dimensions.pixel_count(), 1, FitsAllocation::FlagPlane)?
        } else {
            0
        };
        // The reader's copy of a chunk as stored, and the words it byte-swaps it into — none for
        // bytes, which it hands over as read — which the conversion writes straight into the output
        // and which are still held while the flag plane is built.
        let swapped_chunk_bytes = if stored_bitpix == FitsBitpix::U8 {
            0
        } else {
            native_chunk_bytes.next_multiple_of(size_of::<u64>() as u64)
        };
        let peak_bytes = decoded_bytes
            .checked_add(native_chunk_bytes)
            .and_then(|bytes| bytes.checked_add(swapped_chunk_bytes))
            .and_then(|bytes| bytes.checked_add(flag_plane_bytes))
            .and_then(|bytes| {
                if self.kind == HduKind::CompressedImage {
                    bytes.checked_add(self.source_bytes.checked_mul(2)?)
                } else {
                    Some(bytes)
                }
            })
            .ok_or_else(|| overflow(path, FitsAllocation::PeakMemory))?;

        limit.admit(FitsAllocation::StoredDataUnit, self.source_bytes)?;
        limit.admit(FitsAllocation::DecodedOutput, decoded_bytes)?;
        limit.admit(FitsAllocation::PeakMemory, peak_bytes)?;

        Ok(FitsDecodePlan {
            shape: self.image.shape.to_vec(),
            dimensions,
            sample_type,
            scaling,
            sample_scale,
            data_max,
            source_bytes: self.source_bytes,
            decoded_bytes,
            flag_plane_bytes,
            peak_bytes,
            native_chunk_bytes,
            rows_per_chunk,
        })
    }

    /// What one full-scale span of the stored integer type measures, in physical units.
    ///
    /// The pipeline's linear domain is `[0, 1]`, so an integer FITS is divided by the span its own
    /// `BITPIX` and `BSCALE` declare: `|BSCALE| × (2^bits − 1)`. That maps the FITS unsigned
    /// convention (`BITPIX = 16`, `BZERO = 2¹⁵`) exactly onto `[0, 1]`, and puts a signed frame on
    /// `[-0.5, 0.5]` around its own zero.
    ///
    /// Only the scale is applied — never an offset, and never a clamp. Offsetting would move a
    /// signed frame's zero point, and clamping would cut the sub-pedestal noise tail that the
    /// calibration path depends on (see [`crate::io::raw`]'s unclamped normalization).
    ///
    /// A floating-point `BITPIX` declares no full scale, so the only evidence available is
    /// `DATAMAX`: a declared saturation level above [`FLOAT_ADU_DATAMAX_MIN`] means the samples are
    /// ADU rather than `[0, 1]` and they are divided by [`FLOAT_ADU_DIVISOR`], the threshold and
    /// divisor Siril uses. Anything else — a `DATAMAX` of about 1, or none at all — is taken as
    /// already normalized, which is PixInsight's default for a float FITS and what keeps a
    /// Lumos-written master round-tripping.
    ///
    /// The divisor comes from the *header*, never from the pixels, and that is where this departs
    /// from Siril: with `DATAMAX` absent it scans the data instead (three sampled pixels on the
    /// partial-read path, which is why its full and partial reads can disagree about the same
    /// file). A divisor read off each frame's own extrema differs frame to frame, which is exactly
    /// what [`crate::combine`] rejects a frame set for. The scan still decides one thing: a frame
    /// taken as normalized whose samples reach past Siril's threshold is refused rather than loaded
    /// as it stands ([`SampleScale::verify_normalized`]).
    fn sample_scale(
        &self,
        path: &Path,
        float_scale: FitsFloatScale,
    ) -> Result<SampleScale, ImageError> {
        let (header, stored, scaling) = (self.header, self.image.bitpix, &self.image.scaling);
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
                            FitsRefusal::RecordedScale { scale: recorded },
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
                                FitsRefusal::DeclaredFullScale { scale },
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
        // File-derived metadata: a corrupt or hand-edited header can carry any BSCALE, and a zero
        // or non-finite one leaves no span to normalize into. Reject rather than emit infinities.
        let bscale = scaling.bscale;
        if !bscale.is_finite() || bscale == 0.0 {
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::UnusableBscale { bscale },
            ));
        }
        let divisor = bscale.abs() * steps;
        if !divisor.is_finite() || divisor <= 0.0 {
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::BscaleOverflow {
                    bscale,
                    bitpix: stored,
                },
            ));
        }
        Ok(divided_by(divisor as f32, ScaleOrigin::Declared))
    }
}

#[derive(Debug)]
pub(super) struct FitsDecodePlan {
    pub(super) shape: Vec<usize>,
    pub(super) dimensions: ImageDimensions,
    pub(super) sample_type: SampleType,
    pub(super) scaling: Scaling,
    /// How the stored samples reach the pipeline's `[0, 1]` domain. See
    /// [`FitsHduDescription::sample_scale`].
    pub(super) sample_scale: SampleScale,
    /// The header's `DATAMAX`, in the file's sample units: the decode flags saturation against it.
    pub(super) data_max: Option<f64>,
    pub(super) source_bytes: u64,
    pub(super) decoded_bytes: u64,
    /// The flag plane the decode builds when the header allows a null or declares a `DATAMAX`.
    flag_plane_bytes: u64,
    pub(super) peak_bytes: u64,
    /// One chunk of the image as stored, which the reader's buffer holds.
    native_chunk_bytes: u64,
    pub(super) rows_per_chunk: usize,
}

impl FitsDecodePlan {
    /// What the decode holds beside the frame it makes, its pixels and its flags: the reader's
    /// chunk buffers, and a compressed image's tiles, while it reads the image; then, while it
    /// reads each extension of `extensions` bytes in turn, the reader's buffer — which it keeps,
    /// grown to the largest read so far — and the extension's own copy.
    pub(super) fn beside_frame_bytes(&self, extensions: &[u64]) -> u64 {
        let mut buffer = self.native_chunk_bytes;
        extensions.iter().fold(
            self.peak_bytes - self.decoded_bytes - self.flag_plane_bytes,
            |beside, &bytes| {
                buffer = buffer.max(bytes);
                beside.max(buffer + bytes)
            },
        )
    }

    /// Refuse a flags extension the memory limit cannot hold beside the decoded image: its bytes
    /// as read, and the flag plane the decode may hold until the two are joined.
    pub(super) fn admit_flags_extension(&self, limit: DecodeLimit<'_>) -> Result<(), ImageError> {
        let extension_bytes = limit.checked_bytes(
            self.dimensions.pixel_count(),
            1,
            FitsAllocation::FlagsExtension,
        )?;
        let required = self
            .decoded_bytes
            .checked_add(extension_bytes)
            .and_then(|bytes| bytes.checked_add(self.flag_plane_bytes))
            .ok_or_else(|| overflow(limit.path, FitsAllocation::DecodedOutputWithFlags))?;
        limit.admit(FitsAllocation::DecodedOutputWithFlags, required)
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

fn overflow(path: &Path, allocation: FitsAllocation) -> ImageError {
    ImageError::fits_unsupported(path, FitsRefusal::SizeOverflow { allocation })
}

/// The memory a decode of the file at `path` may hold, and the file its refusals name.
#[derive(Debug, Clone, Copy)]
pub(super) struct DecodeLimit<'a> {
    pub(super) path: &'a Path,
    pub(super) bytes: u64,
}

impl DecodeLimit<'_> {
    /// The bytes of `elements` of `element_bytes` each, refused as `allocation` when they overflow.
    fn checked_bytes(
        self,
        elements: usize,
        element_bytes: usize,
        allocation: FitsAllocation,
    ) -> Result<u64, ImageError> {
        elements
            .checked_mul(element_bytes)
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or_else(|| overflow(self.path, allocation))
    }

    /// `required` bytes of `allocation`, refused when they exceed the limit.
    fn admit(self, allocation: FitsAllocation, required: u64) -> Result<(), ImageError> {
        if required > self.bytes {
            return Err(ImageError::fits_unsupported(
                self.path,
                FitsRefusal::OverBudget {
                    allocation,
                    required,
                    limit: self.bytes,
                },
            ));
        }
        Ok(())
    }
}

fn padded_data_bytes(path: &Path, bytes: u64) -> Result<u64, ImageError> {
    if bytes == 0 {
        return Ok(0);
    }
    bytes
        .checked_add(BLOCK_SIZE as u64 - 1)
        .map(|padded| padded / BLOCK_SIZE as u64 * BLOCK_SIZE as u64)
        .ok_or_else(|| overflow(path, FitsAllocation::StoredDataUnit))
}

pub(super) fn dimensions_from_shape(
    path: &Path,
    shape: &[usize],
    cube: FitsCubeInterpretation,
) -> Result<ImageDimensions, ImageError> {
    if shape.contains(&0) {
        return Err(ImageError::fits_unsupported(
            path,
            FitsRefusal::ZeroAxis {
                shape: shape.to_vec(),
            },
        ));
    }
    let (width, height, channels) = match shape {
        [width, height] | [width, height, 1] => (*width, *height, 1),
        [width, height, 3] if cube == FitsCubeInterpretation::Rgb => (*width, *height, 3),
        [_, _, 3] => {
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::CubeNeedsRgb,
            ));
        }
        [_, _, channels] => {
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::PlaneCount { planes: *channels },
            ));
        }
        _ => {
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::AxisCount { axes: shape.len() },
            ));
        }
    };
    if width > ImageDimensions::MAX_SIDE || height > ImageDimensions::MAX_SIDE {
        return Err(ImageError::fits_unsupported(
            path,
            FitsRefusal::SideTooLong { width, height },
        ));
    }
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| {
            ImageError::fits_unsupported(
                path,
                FitsRefusal::SampleCountOverflow {
                    shape: shape.to_vec(),
                },
            )
        })?;
    Ok(ImageDimensions::new((width, height), channels))
}

#[cfg(test)]
pub(super) mod internals {
    use fits_well::header::Header;
    use fits_well::image::{Bitpix, Metadata as FitsMetadata};
    use fits_well::io::HduKind;

    use crate::io::fits::decode::plan::FitsHduDescription;

    /// An HDU of `kind` whose image is `shape` samples of `bitpix`, scaled as `header` declares,
    /// its data unit `source_bytes` as stored.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct HduSpec<'a> {
        pub(crate) header: &'a Header,
        pub(crate) kind: HduKind,
        pub(crate) shape: &'a [usize],
        pub(crate) bitpix: Bitpix,
        pub(crate) source_bytes: u64,
    }

    impl<'a> HduSpec<'a> {
        pub(crate) fn description(self) -> FitsHduDescription<'a> {
            FitsHduDescription {
                header: self.header,
                kind: self.kind,
                image: FitsMetadata {
                    shape: self.shape,
                    bitpix: self.bitpix,
                    scaling: self.header.scaling().unwrap(),
                },
                source_bytes: self.source_bytes,
            }
        }
    }
}
