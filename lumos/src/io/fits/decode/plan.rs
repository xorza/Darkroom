use fits_well::FitsError;
use fits_well::header::Header;
use fits_well::image::Scaling;
use fits_well::image::{Bitpix as FitsBitpix, Metadata as FitsMetadata, SampleType};
use fits_well::io::{BLOCK_SIZE, Hdu, HduKind};

use crate::io::error::OverBudget;
use crate::io::fits::error::FitsLoadError;
use crate::io::fits::error::{FitsAllocation, FitsRefusal};

use crate::image::channel_layout::ChannelLayout;
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
const FLOAT_ADU_DIVISOR: f64 = 65_535.0;

#[derive(Debug, Clone, Copy)]
pub(super) struct FitsHduDescription<'a> {
    header: &'a Header,
    kind: HduKind,
    image: FitsMetadata<'a>,
    source_bytes: u64,
}

impl<'a> FitsHduDescription<'a> {
    /// The image at HDU `index`, `hdu`.
    pub(super) fn from_hdu(hdu: &'a Hdu, index: usize) -> Result<Self, FitsLoadError> {
        let image = hdu.image().map_err(|source| match source {
            FitsError::NotAnImage => FitsLoadError::from(FitsRefusal::NotAnImage { hdu: index }),
            source => FitsLoadError::from(source),
        })?;
        Ok(Self {
            header: &hdu.header,
            kind: hdu.kind,
            image,
            source_bytes: padded_data_bytes(hdu.data_bytes)?,
        })
    }

    /// How the image decodes under `cube` and `float_scale`, within `limit`: its shape, its
    /// scale, and the memory each step holds, refused when any would exceed the limit.
    pub(super) fn plan(
        self,
        cube: FitsCubeInterpretation,
        float_scale: FitsFloatScale,
        limit: u64,
    ) -> Result<FitsDecodePlan, FitsLoadError> {
        let dimensions = dimensions_from_shape(self.image.shape, cube)?;
        let stored_bitpix = self.image.bitpix;
        let scaling = self.image.scaling;
        let sample_type = SampleType::from_scaling(stored_bitpix, &scaling);
        let data_max = self.data_max(float_scale)?;
        let sample_scale = self.sample_scale(float_scale, data_max)?;
        let decoded_bytes = checked_bytes(
            dimensions.sample_count(),
            size_of::<f32>(),
            FitsAllocation::DecodedOutput,
        )?;
        let row_samples = dimensions.width();
        let row_f32_bytes = row_samples
            .checked_mul(size_of::<f32>())
            .ok_or_else(|| overflow(FitsAllocation::OutputRow))?;
        let rows_per_chunk = (FITS_DECODE_CHUNK_BYTES / row_f32_bytes.max(1))
            .max(1)
            .min(dimensions.height());
        let chunk_samples = row_samples
            .checked_mul(rows_per_chunk)
            .ok_or_else(|| overflow(FitsAllocation::DecodeChunk))?;
        let native_chunk_bytes = checked_bytes(
            chunk_samples,
            stored_bitpix.elem_size(),
            FitsAllocation::NativeDecodeChunk,
        )?;
        // A byte per pixel, which the decode builds beside the planes once it meets a null or a
        // saturation level to flag against.
        let flag_plane_bytes = if may_carry_nulls(sample_type, &scaling) || data_max.is_some() {
            checked_bytes(dimensions.pixel_count(), 1, FitsAllocation::FlagPlane)?
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
            .ok_or_else(|| overflow(FitsAllocation::PeakMemory))?;

        admit(limit, FitsAllocation::StoredDataUnit, self.source_bytes)?;
        admit(limit, FitsAllocation::DecodedOutput, decoded_bytes)?;
        admit(limit, FitsAllocation::PeakMemory, peak_bytes)?;

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

    /// The header's `DATAMAX`, a positive saturation level in the file's sample units.
    ///
    /// Where it decides the scale — a float HDU read with [`FitsFloatScale::Auto`] that records no
    /// scale of its own — a card of the wrong type fails the load, as every keyword that decides
    /// how the samples are read does. Elsewhere it is metadata, skipped with the reason logged. A
    /// non-positive level is no level either way: "not computed" is how a header writes it.
    fn data_max(&self, float_scale: FitsFloatScale) -> Result<Option<f64>, FitsLoadError> {
        let decides_scale = matches!(self.image.bitpix, FitsBitpix::F32 | FitsBitpix::F64)
            && float_scale == FitsFloatScale::Auto
            && self.header.get(domain_keywords::SCALE_ORIGIN).is_none();
        if !decides_scale {
            return Ok(metadata::read_data_max(self.header));
        }
        let level = self.header.get_real("DATAMAX")?;
        Ok(level.filter(|&level| metadata::is_saturation_level(level)))
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
    /// `data_max`, the header's `DATAMAX` as [`metadata::read_data_max`] reads it: a declared saturation level above [`FLOAT_ADU_DATAMAX_MIN`] means the samples are
    /// ADU rather than `[0, 1]` and they are divided by [`FLOAT_ADU_DIVISOR`], the threshold and
    /// divisor Siril uses. Anything else — a `DATAMAX` of about 1, or none at all — is taken as
    /// already normalized, which is PixInsight's default for a float FITS and what keeps a
    /// Lumos-written master round-tripping.
    ///
    /// The divisor comes from the *header*, never from the pixels, and that is where this departs
    /// from Siril: with `DATAMAX` absent it scans the data instead (three sampled pixels on the
    /// partial-read path, which is why its full and partial reads can disagree about the same
    /// file). A divisor read off each frame's own extrema differs frame to frame, which is exactly
    /// what `combine` rejects a frame set for. The scan still decides one thing: a frame
    /// taken as normalized whose samples reach past Siril's threshold is refused rather than loaded
    /// as it stands ([`SampleScale::verify_normalized`]).
    fn sample_scale(
        &self,
        float_scale: FitsFloatScale,
        data_max: Option<f64>,
    ) -> Result<SampleScale, FitsLoadError> {
        let (header, stored, scaling) = (self.header, self.image.bitpix, &self.image.scaling);
        let divided_by = |divisor: f64, origin| SampleScale {
            divisor,
            origin,
            verify_normalized: false,
        };
        let steps = match stored {
            FitsBitpix::U8 => f64::from(u8::MAX),
            FitsBitpix::I16 => f64::from(u16::MAX),
            FitsBitpix::I32 => f64::from(u32::MAX),
            FitsBitpix::I64 => u64::MAX as f64,
            FitsBitpix::F32 | FitsBitpix::F64 => {
                // A lumos-written file stores its samples already normalized, with the scale they
                // were normalized by as `BSCALE`; that record beats every guess below.
                if let Some(origin) = domain_keywords::read_origin(header)? {
                    let recorded = scaling.bscale;
                    if !recorded.is_finite() || recorded <= 0.0 || scaling.bzero != 0.0 {
                        return Err(FitsLoadError::from(FitsRefusal::RecordedScale {
                            bscale: recorded,
                            bzero: scaling.bzero,
                        }));
                    }
                    return Ok(divided_by(recorded, origin));
                }
                return match float_scale {
                    // "Already normalized" says where the samples sit, not what a unit of them is
                    // worth in the source's own terms, so the scale stays a guess.
                    FitsFloatScale::Normalized => Ok(divided_by(1.0, ScaleOrigin::Assumed)),
                    FitsFloatScale::FullScale(scale) => {
                        // The caller's own figure, so it is checked here rather than trusted: a
                        // non-positive one would invert or erase the samples.
                        if !scale.is_finite() || scale <= 0.0 {
                            return Err(FitsLoadError::from(FitsRefusal::DeclaredFullScale {
                                scale,
                            }));
                        }
                        Ok(divided_by(f64::from(scale), ScaleOrigin::Declared))
                    }
                    FitsFloatScale::Auto => Ok(match data_max {
                        Some(max) if max > FLOAT_ADU_DATAMAX_MIN => {
                            divided_by(FLOAT_ADU_DIVISOR, ScaleOrigin::Assumed)
                        }
                        _ => SampleScale {
                            verify_normalized: true,
                            ..divided_by(1.0, ScaleOrigin::Assumed)
                        },
                    }),
                };
            }
        };
        // File-derived metadata: a corrupt or hand-edited header can carry any BSCALE, and a zero
        // or non-finite one leaves no span to normalize into. Reject rather than emit infinities.
        let bscale = scaling.bscale;
        if !bscale.is_finite() || bscale == 0.0 {
            return Err(FitsLoadError::from(FitsRefusal::UnusableBscale { bscale }));
        }
        let divisor = bscale.abs() * steps;
        if !divisor.is_finite() || divisor <= 0.0 {
            return Err(FitsLoadError::from(FitsRefusal::BscaleOverflow {
                bscale,
                bitpix: stored,
            }));
        }
        Ok(divided_by(divisor, ScaleOrigin::Declared))
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
    pub(super) fn admit_flags_extension(&self, limit: u64) -> Result<(), FitsLoadError> {
        let extension_bytes = checked_bytes(
            self.dimensions.pixel_count(),
            1,
            FitsAllocation::FlagsExtension,
        )?;
        let required = self
            .decoded_bytes
            .checked_add(extension_bytes)
            .and_then(|bytes| bytes.checked_add(self.flag_plane_bytes))
            .ok_or_else(|| overflow(FitsAllocation::DecodedOutputWithFlags))?;
        admit(limit, FitsAllocation::DecodedOutputWithFlags, required)
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
    /// The decoder divides every physical sample by this, and a decoded sample times this is the
    /// file's physical value again.
    pub(super) divisor: f64,
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

fn overflow(allocation: FitsAllocation) -> FitsLoadError {
    FitsLoadError::from(FitsRefusal::SizeOverflow { allocation })
}

/// The bytes of `elements` of `element_bytes` each, refused as `allocation` when they overflow.
fn checked_bytes(
    elements: usize,
    element_bytes: usize,
    allocation: FitsAllocation,
) -> Result<u64, FitsLoadError> {
    elements
        .checked_mul(element_bytes)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or_else(|| overflow(allocation))
}

/// `required` bytes of `allocation`, refused when they exceed `limit`, the bytes a decode may
/// hold.
fn admit(limit: u64, allocation: FitsAllocation, required: u64) -> Result<(), FitsLoadError> {
    OverBudget::check(required, limit)
        .map_err(|budget| FitsLoadError::from(FitsRefusal::OverBudget { allocation, budget }))
}

fn padded_data_bytes(bytes: u64) -> Result<u64, FitsLoadError> {
    if bytes == 0 {
        return Ok(0);
    }
    bytes
        .checked_add(BLOCK_SIZE as u64 - 1)
        .map(|padded| padded / BLOCK_SIZE as u64 * BLOCK_SIZE as u64)
        .ok_or_else(|| overflow(FitsAllocation::StoredDataUnit))
}

pub(super) fn dimensions_from_shape(
    shape: &[usize],
    cube: FitsCubeInterpretation,
) -> Result<ImageDimensions, FitsLoadError> {
    if shape.contains(&0) {
        return Err(FitsLoadError::from(FitsRefusal::ZeroAxis {
            shape: shape.to_vec(),
        }));
    }
    let (width, height, channels) = match shape {
        [width, height] | [width, height, 1] => (*width, *height, ChannelLayout::Mono),
        [width, height, 3] if cube == FitsCubeInterpretation::Rgb => {
            (*width, *height, ChannelLayout::Rgb)
        }
        [_, _, 3] => {
            return Err(FitsLoadError::from(FitsRefusal::CubeNeedsRgb));
        }
        [_, _, channels] => {
            return Err(FitsLoadError::from(FitsRefusal::PlaneCount {
                planes: *channels,
            }));
        }
        _ => {
            return Err(FitsLoadError::from(FitsRefusal::AxisCount {
                axes: shape.len(),
            }));
        }
    };
    if width > ImageDimensions::MAX_SIDE || height > ImageDimensions::MAX_SIDE {
        return Err(FitsLoadError::from(FitsRefusal::SideTooLong {
            width,
            height,
        }));
    }
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(channels.count()))
        .ok_or_else(|| {
            FitsLoadError::from(FitsRefusal::SampleCountOverflow {
                shape: shape.to_vec(),
            })
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
