use std::io::{Read, Seek};
use std::ops::Range;

use arrayvec::ArrayVec;
use common::Size2us;
use fits_well::header::Header;
use fits_well::image::{
    Bitpix as FitsBitpix, Metadata as FitsMetadata, Region, SampleType, Scaling,
};
use fits_well::io::StreamSource;
use fits_well::{FitsError, ImageReader};
use imaginarium::{ChannelLayout, PlanarDesc};

use crate::image::frame_desc::MAX_SIDE;
use crate::image::metadata::sample_domain::ScaleOrigin;
use crate::io::fits::CHUNK_BYTES;
use crate::io::fits::error::{FitsAllocation, FitsLoadError, FitsRefusal};
use crate::io::fits::fits_load_config::{FitsCubeInterpretation, FitsFloatScale};
use crate::io::fits::metadata;
use crate::io::fits::metadata::domain_keywords;

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
    image: FitsMetadata<'a>,
    source_bytes: u64,
}

/// What fits-well's reader of `image` holds viewing `regions` in turn and finishing the
/// checksum: its bound, saturated at `u64::MAX`, which the plan then refuses as an overflow.
pub(super) fn reader_bytes<R: Read + Seek>(
    image: &ImageReader<'_, StreamSource<R>>,
    regions: &[ArrayVec<Range<usize>, 3>],
) -> Result<u64, FitsError> {
    let views = image.view_bytes(regions.iter().map(|ranges| Region::from(ranges.as_slice())))?;
    Ok(views
        .kept
        .saturating_add(views.handle)
        .saturating_add(image.finish_bytes()))
}

impl<'a> FitsHduDescription<'a> {
    /// The image `image` opens, its data unit `source_bytes` as stored.
    pub(super) const fn of_image<R: Read + Seek>(
        image: &ImageReader<'a, StreamSource<R>>,
        source_bytes: u64,
    ) -> Self {
        Self {
            header: image.header(),
            image: image.metadata(),
            source_bytes,
        }
    }

    /// How the image decodes under `cube` and `float_scale`: its shape, its scale, the rows a chunk
    /// reads, and the memory each step holds, refused when a size overflows. A chunk spans every
    /// plane, and an image tiled in `tiles` reads whole rows of its tiles, so each tile decodes
    /// once; `reader` gives what fits-well's reader holds reading the chunks.
    pub(super) fn plan(
        self,
        cube: FitsCubeInterpretation,
        float_scale: FitsFloatScale,
        tiles: Option<&[usize]>,
        reader: impl FnOnce(&[ArrayVec<Range<usize>, 3>]) -> Result<u64, FitsError>,
    ) -> Result<FitsDecodePlan, FitsLoadError> {
        let desc = desc_from_shape(self.image.shape, cube)?;
        let stored_bitpix = self.image.bitpix;
        let scaling = self.image.scaling;
        let sample_type = SampleType::from_scaling(stored_bitpix, scaling);
        let data_max = self.data_max(float_scale)?;
        let sample_scale = self.sample_scale(float_scale, data_max)?;
        let decoded_bytes = checked_bytes(
            desc.sample_count(),
            size_of::<f32>(),
            FitsAllocation::DecodedOutput,
        )?;
        let row_samples = desc
            .width()
            .checked_mul(desc.channel_count())
            .ok_or_else(|| overflow(FitsAllocation::OutputRow))?;
        let row_f32_bytes = row_samples
            .checked_mul(size_of::<f32>())
            .ok_or_else(|| overflow(FitsAllocation::OutputRow))?;
        let rows_per_chunk = chunk_rows(
            CHUNK_BYTES / row_f32_bytes.max(1),
            desc.height(),
            tiles.map(|tiles| tiles[1]),
        );
        let chunk_samples = row_samples
            .checked_mul(rows_per_chunk)
            .ok_or_else(|| overflow(FitsAllocation::DecodeChunk))?;
        // The words a chunk's samples land in, which the conversion writes straight into the
        // planes and which are still held while the flag plane is built.
        let words_bytes = checked_bytes(
            chunk_samples,
            stored_bitpix.elem_size(),
            FitsAllocation::NativeDecodeChunk,
        )?
        .next_multiple_of(size_of::<u64>() as u64);
        // A byte per pixel, which the decode builds beside the planes once it meets a null or a
        // saturation level to flag against.
        let flag_plane_bytes = if may_carry_nulls(sample_type, &scaling) || data_max.is_some() {
            checked_bytes(desc.pixel_count(), 1, FitsAllocation::FlagPlane)?
        } else {
            0
        };
        let regions: Vec<_> = chunk_regions(self.image.shape, rows_per_chunk).collect();
        let peak_bytes = reader(&regions)?
            .checked_add(decoded_bytes)
            .and_then(|bytes| bytes.checked_add(words_bytes))
            .and_then(|bytes| bytes.checked_add(flag_plane_bytes))
            .ok_or_else(|| overflow(FitsAllocation::PeakMemory))?;

        Ok(FitsDecodePlan {
            shape: self.image.shape.to_vec(),
            desc,
            sample_type,
            scaling,
            sample_scale,
            data_max,
            source_bytes: self.source_bytes,
            decoded_bytes,
            flag_plane_bytes,
            peak_bytes,
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
            && self.header.get(domain_keywords::SCALE_ORIGIN)?.is_none();
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
    /// `data_max`, the header's `DATAMAX` as [`metadata::read_data_max`] reads it: a declared
    /// saturation level above [`FLOAT_ADU_DATAMAX_MIN`] means the samples are ADU rather than
    /// `[0, 1]` and they are divided by [`FLOAT_ADU_DIVISOR`], the threshold and divisor Siril
    /// uses. Anything else — a `DATAMAX` of about 1, or none at all — is taken as already
    /// normalized, which is PixInsight's default for a float FITS and what keeps a Lumos-written
    /// master round-tripping.
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
    pub(super) desc: PlanarDesc,
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
    pub(super) rows_per_chunk: usize,
}

impl FitsDecodePlan {
    /// What the decode holds beside the frame it makes, its pixels and its flags: a chunk's words
    /// and what the reader holds while it reads the image, and the largest of `extensions`, the
    /// reads of the extensions after it. The sum, not the larger, as what the reader keeps of the
    /// image while it reads an extension is not tracked. The image's part, counted again, is a
    /// chunk and the reader's buffers, small beside the frame, and it covers what the reader holds
    /// for a plain extension, which [`ExtensionChunks::read_bytes`] leaves out.
    ///
    /// [`ExtensionChunks::read_bytes`]: crate::io::fits::decode::extension_chunks::ExtensionChunks::read_bytes
    pub(super) fn beside_frame_bytes(&self, extensions: &[u64]) -> u64 {
        let image = self.peak_bytes - self.decoded_bytes - self.flag_plane_bytes;
        image.saturating_add(extensions.iter().copied().max().unwrap_or(0))
    }
}

/// The rows a chunk reads: `rows` of an image `height` rows high, or, for one whose tiles are
/// `tile_rows` high, the whole rows of tiles they hold, one at least, so a chunk boundary never
/// cuts a tile and each tile decodes once.
fn chunk_rows(rows: usize, height: usize, tile_rows: Option<usize>) -> usize {
    let rows = match tile_rows {
        Some(tile_rows) => (rows / tile_rows).max(1) * tile_rows,
        None => rows.max(1),
    };
    rows.min(height)
}

/// The regions of an image of `shape` that a read views in turn: its chunks of `rows_per_chunk`
/// rows, each across every plane. A section is plane-major, so each plane's samples are one run
/// of it.
pub(super) fn chunk_regions(
    shape: &[usize],
    rows_per_chunk: usize,
) -> impl Iterator<Item = ArrayVec<Range<usize>, 3>> + '_ {
    let height = shape[1];
    (0..height).step_by(rows_per_chunk).map(move |start| {
        let mut ranges: ArrayVec<Range<usize>, 3> =
            [0..shape[0], start..(start + rows_per_chunk).min(height)]
                .into_iter()
                .collect();
        if let Some(&planes) = shape.get(2) {
            ranges.push(0..planes);
        }
        ranges
    })
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

pub(super) fn overflow(allocation: FitsAllocation) -> FitsLoadError {
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

pub(super) fn desc_from_shape(
    shape: &[usize],
    cube: FitsCubeInterpretation,
) -> Result<PlanarDesc, FitsLoadError> {
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
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(FitsLoadError::from(FitsRefusal::SideTooLong {
            size: Size2us::new(width, height),
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
    Ok(PlanarDesc::new((width, height), channels))
}

#[cfg(test)]
pub(super) mod internals {
    use fits_well::header::Header;
    use fits_well::image::{Bitpix, Metadata as FitsMetadata, Scaling};

    use crate::io::fits::decode::plan::FitsHduDescription;

    /// An HDU whose image is `shape` unscaled samples of `bitpix`, its data unit `source_bytes` as
    /// stored.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct HduSpec<'a> {
        pub(crate) header: &'a Header,
        pub(crate) shape: &'a [usize],
        pub(crate) bitpix: Bitpix,
        pub(crate) source_bytes: u64,
    }

    impl<'a> HduSpec<'a> {
        pub(crate) fn description(self) -> FitsHduDescription<'a> {
            FitsHduDescription {
                header: self.header,
                image: FitsMetadata {
                    shape: self.shape,
                    bitpix: self.bitpix,
                    scaling: Scaling::IDENTITY,
                },
                source_bytes: self.source_bytes,
            }
        }
    }
}
