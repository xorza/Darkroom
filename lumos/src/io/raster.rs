//! The raster formats imaginarium reads — TIFF, PNG, JPEG — decoded into a linear or a preview
//! image, and a linear image written back as one.
//!
//! FITS and RAW have their own decoders under `fits/` and `raw/`; a raster is read through
//! imaginarium in one call, so what is left here is what each product makes of it.

use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use arrayvec::ArrayVec;
use common::{FixedFile, fs};
use imaginarium::{AdmittedDecodeError, Buffer2, ChannelCount, FileFormat, Image, ImageDesc};

use crate::image::channel_layout::ChannelLayout;
use crate::image::derived_facts::DerivedFacts;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::image_provenance::{
    ColorProvenance, DemosaicProvenance, ImageProvenance, RasterFormat, RasterTransfer, RowOrder,
    SourceProvenance,
};
use crate::image::linear_image::{self, LinearImage};
use crate::image::observation_header::ObservationHeader;
use crate::image::preview_image::PreviewImage;
use crate::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::io::error::{ImageError, OverBudget, ScientificRefusal, WriteError};
use crate::io::load_ctx::LoadCtx;
use crate::io::null_policy::NullSamples;
use crate::math::size2us::Size2us;

/// The `format` image at `path`, decoded by imaginarium once `admit` accepts the description its
/// header gives and the input bytes the decoder holds beside the image, before any pixel is
/// allocated. A format that reads its whole input first holds the file as long as it was at open,
/// refused past `ctx`'s memory limit before it is read.
fn read(
    path: &Path,
    format: FileFormat,
    ctx: &LoadCtx,
    admit: impl FnOnce(&ImageDesc, u64) -> Result<(), ImageError>,
) -> Result<Image, ImageError> {
    let io_error = |source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = FixedFile::open(path).map_err(io_error)?;
    let held = if format.reads_whole_input() {
        file.len()
    } else {
        0
    };
    admit_peak(path, held, ctx)?;
    Image::decode_admitted(BufReader::new(file), format, |desc| admit(desc, held)).map_err(
        |error| match error {
            AdmittedDecodeError::Refused(error) => error,
            AdmittedDecodeError::Decode(source) => ImageError::Image {
                path: path.to_path_buf(),
                source,
            },
        },
    )
}

/// Refuse a decode of `path` that holds `peak` bytes, past `ctx`'s memory limit.
fn admit_peak(path: &Path, peak: u64, ctx: &LoadCtx) -> Result<(), ImageError> {
    OverBudget::check(peak, ctx.memory_limit_bytes).map_err(|budget| ImageError::ImageOverBudget {
        path: path.to_path_buf(),
        budget,
    })
}

/// What a raster decode of a `format` file records: imaginarium decoded it, it stores its first
/// row at the top, and nothing was clipped or demosaiced.
fn provenance(
    format: FileFormat,
    transfer: RasterTransfer,
    color: ColorProvenance,
) -> ImageProvenance {
    ImageProvenance {
        source: SourceProvenance::Raster {
            format: RasterFormat::from(format),
            transfer,
        },
        color,
        clipped: false,
        demosaic: DemosaicProvenance::None,
        // Every raster format imaginarium reads stores its first row at the top.
        row_order: RowOrder::TopDown,
    }
}

/// The `format` raster at `path` as a linear image: a floating-point TIFF, taken as linear, of no
/// alpha and of sides a frame may have, its nulls resolved by the context's policy. Every other
/// raster is refused, an integer one among them: its samples are display-encoded. Each refusal,
/// and one past the context's memory limit, comes from the header, before the decode.
pub(crate) fn linear(
    path: &Path,
    format: FileFormat,
    ctx: &LoadCtx,
) -> Result<LinearImage, ImageError> {
    if format != FileFormat::Tiff {
        return Err(ImageError::scientific_rejection(
            path,
            ScientificRefusal::PreviewOnlyRaster { format },
        ));
    }
    let decoded = read(path, format, ctx, |desc, _| {
        if let Some(refusal) = linear_refusal(desc) {
            return Err(ImageError::scientific_rejection(path, refusal));
        }
        admit_peak(path, linear_peak(linear_dimensions(desc)), ctx)
    })?;
    ctx.check_cancelled(path)?;

    let color = if decoded.desc().color_format.channel_count == ChannelCount::L {
        ColorProvenance::Monochrome
    } else {
        ColorProvenance::Unspecified
    };
    let mut image = LinearImage::from(decoded);
    let size = image.dimensions().size();
    let flags = {
        let mut planes = image
            .planes_mut()
            .map(Buffer2::pixels_mut)
            .collect::<ArrayVec<&mut [f32], 3>>();
        ctx.nulls
            .resolve(&mut planes, size)
            .map_err(|NullSamples { count, first_idx }| {
                ImageError::null_samples(path, count, first_idx)
            })?
    };
    image.set_flags(flags);
    // A float raster is taken as it stands, so one sample is one unit of whatever the file already
    // held — which it does not name, and whose span and zero point it does not state.
    image.metadata.derived = DerivedFacts::builder()
        .provenance(provenance(format, RasterTransfer::Float, color))
        .domain(SampleDomain {
            scale: 1.0,
            origin: ScaleOrigin::Assumed,
            pedestal: Pedestal::Unknown,
            unit: None,
        })
        .build();
    Ok(image)
}

/// The `format` raster at `path` as a preview, in `f32` and interleaved as it decoded, an alpha
/// channel dropped.
pub(crate) fn preview(
    path: &Path,
    format: FileFormat,
    ctx: &LoadCtx,
) -> Result<PreviewImage, ImageError> {
    let decoded = read(path, format, ctx, |desc, held| {
        admit_peak(path, preview_peak(*desc, held), ctx)
    })?;
    ctx.check_cancelled(path)?;
    let alpha_dropped = decoded.desc().color_format.channel_count == ChannelCount::Rgba;
    let target = linear_image::f32_target_format(decoded.desc().color_format);
    let metadata = ImageMetadata {
        header: ObservationHeader::default(),
        derived: DerivedFacts::builder()
            .provenance(provenance(
                format,
                RasterTransfer::Unspecified,
                ColorProvenance::UnmanagedRaster { alpha_dropped },
            ))
            .build(),
    };
    Ok(PreviewImage::interleaved(decoded.convert(target), metadata))
}

/// Write `image` to `path` as the raster format its extension names: PNG, JPEG or TIFF,
/// published durably as the FITS writers publish, so a reader sees the old file or the new one.
pub(crate) fn save(path: &Path, image: &LinearImage) -> Result<(), WriteError> {
    write_image(path, &Image::from(image))
}

/// Why a TIFF of `desc` is no linear image; `None` for one it is.
fn linear_refusal(desc: &ImageDesc) -> Option<ScientificRefusal> {
    if desc.width > ImageDimensions::MAX_SIDE || desc.height > ImageDimensions::MAX_SIDE {
        return Some(ScientificRefusal::RasterSideTooLong {
            width: desc.width,
            height: desc.height,
        });
    }
    if !desc.color_format.sample_type.is_float() {
        return Some(ScientificRefusal::IntegerRaster);
    }
    (desc.color_format.channel_count == ChannelCount::Rgba)
        .then_some(ScientificRefusal::AlphaChannel)
}

/// The shape of the linear image a float raster of `desc` decodes to, admitted as a frame's.
fn linear_dimensions(desc: &ImageDesc) -> ImageDimensions {
    let layout = if desc.color_format.channel_count == ChannelCount::L {
        ChannelLayout::Mono
    } else {
        ChannelLayout::Rgb
    };
    ImageDimensions::checked(Size2us::new(desc.width, desc.height), layout)
        .expect("a raster of sides a frame may have, its bytes counted in a usize")
}

/// What a linear decode of `dimensions` holds at its peak: the decoded image beside the planes an
/// RGB one splits into, a grayscale one moving into its plane, then the planes beside the flag
/// plane its nulls may need. A TIFF holds no input beside the image.
fn linear_peak(dimensions: ImageDimensions) -> u64 {
    let frame = dimensions.frame_bytes();
    let split = if dimensions.is_grayscale() {
        frame
    } else {
        2 * frame
    };
    bytes(split.max(dimensions.decoded_bytes()))
}

/// What a preview decode of `desc` holds at its peak: the decoded image beside the `held` input
/// bytes, then beside its `f32` copy, unless it decodes to `f32` already.
fn preview_peak(desc: ImageDesc, held: u64) -> u64 {
    let decoded = bytes(desc.size_in_bytes());
    let target = linear_image::f32_target_format(desc.color_format);
    let converted = if target == desc.color_format {
        0
    } else {
        bytes(ImageDesc::new(desc.width, desc.height, target).size_in_bytes())
    };
    decoded + held.max(converted)
}

/// `count` bytes as a `u64`.
fn bytes(count: usize) -> u64 {
    u64::try_from(count).expect("a byte count fits u64")
}

/// Write `image` to `path` as the raster format its extension names, published durably.
pub(crate) fn write_image(path: &Path, image: &Image) -> Result<(), WriteError> {
    let format = FileFormat::from_path(path)?;
    fs::publish(path, fs::PublicationMode::Durable, |file| {
        let mut out = BufWriter::new(file);
        image.encode(&mut out, format)?;
        Ok::<_, WriteError>(out.flush()?)
    })
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use std::io::BufReader;
    use std::path::Path;

    use common::fs;
    use imaginarium::{FileFormat, Image};

    /// The image in the file at `path`, in the format its extension names, decoded unbounded: the
    /// dataset's own files.
    pub(crate) fn read_image(path: &Path) -> Result<Image, imaginarium::Error> {
        let format = FileFormat::from_path(path)?;
        Image::decode(BufReader::new(fs::open(path)?), format)
    }
}
