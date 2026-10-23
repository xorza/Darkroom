//! The raster formats imaginarium reads — TIFF, PNG, JPEG — decoded into a linear or a preview
//! image, and a linear image written back as one.
//!
//! FITS and RAW have their own decoders under `fits/` and `raw/`; a raster is read through
//! imaginarium in one call, so what is left here is what each product makes of it.

use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use arrayvec::ArrayVec;
use common::fs;
use imaginarium::{Buffer2, ChannelCount, FileFormat, Image};

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
use crate::io::error::{ImageError, ScientificRefusal, WriteError};
use crate::io::load_ctx::LoadCtx;
use crate::io::null_policy::NullSamples;

/// The `format` image at `path`, decoded by imaginarium.
fn read(path: &Path, format: FileFormat) -> Result<Image, ImageError> {
    let file = fs::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Image::decode(BufReader::new(file), format).map_err(|source| ImageError::Image {
        path: path.to_path_buf(),
        source,
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
/// raster is refused, an integer one among them: its samples are display-encoded.
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
    let decoded = read(path, format)?;
    ctx.check_cancelled(path)?;
    let desc = decoded.desc();
    if desc.width > ImageDimensions::MAX_SIDE || desc.height > ImageDimensions::MAX_SIDE {
        return Err(ImageError::scientific_rejection(
            path,
            ScientificRefusal::RasterSideTooLong {
                width: desc.width,
                height: desc.height,
            },
        ));
    }
    if !desc.color_format.sample_type.is_float() {
        return Err(ImageError::scientific_rejection(
            path,
            ScientificRefusal::IntegerRaster,
        ));
    }
    if desc.color_format.channel_count == ChannelCount::Rgba {
        return Err(ImageError::scientific_rejection(
            path,
            ScientificRefusal::AlphaChannel,
        ));
    }

    let color = if desc.color_format.channel_count == ChannelCount::L {
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
    let decoded = read(path, format)?;
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
    use std::path::Path;

    use imaginarium::{FileFormat, Image};

    use crate::io::error::ImageError;
    use crate::io::raster;

    /// The dataset's raster at `path`, in the format its extension names.
    pub(crate) fn read_image(path: &Path) -> Result<Image, ImageError> {
        let format = FileFormat::from_path(path).expect("a dataset raster's extension names it");
        raster::read(path, format)
    }
}
