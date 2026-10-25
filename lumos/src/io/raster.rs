//! The raster formats imaginarium reads — TIFF, PNG, JPEG — decoded into a linear image, for
//! science or for a preview, and a linear image written back as one.
//!
//! FITS and RAW have their own decoders under `fits/` and `raw/`; a raster is read through
//! imaginarium in one call, so what is left here is what each product makes of it.

use std::io::{BufReader, BufWriter, Write};
use std::path::Path;

use arrayvec::ArrayVec;
use common::{Buffer2, Size2us, fs};
use imaginarium::{ChannelCount, InterleavedImage, PlanarImage, RasterFormat};

use crate::image::frame_desc::MAX_SIDE;
use crate::image::linear_image::LinearImage;
use crate::image::metadata::derived_facts::DerivedFacts;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::image_provenance::{
    ColorProvenance, DemosaicProvenance, ImageProvenance, RasterTransfer, RowOrder,
    SourceProvenance,
};
use crate::image::metadata::observation_header::ObservationHeader;
use crate::image::metadata::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::io::error::{ImageError, ScientificRefusal, WriteError};
use crate::io::load_ctx::LoadCtx;
use crate::io::null_policy::NullSamples;

/// The `format` image at `path`, decoded by imaginarium.
fn read(path: &Path, format: RasterFormat) -> Result<InterleavedImage, ImageError> {
    let file = fs::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    InterleavedImage::decode(BufReader::new(file), format).map_err(|source| ImageError::Image {
        path: path.to_path_buf(),
        source,
    })
}

/// What a raster decode of a `format` file records: imaginarium decoded it, it stores its first
/// row at the top, and nothing was clipped or demosaiced.
const fn provenance(
    format: RasterFormat,
    transfer: RasterTransfer,
    color: ColorProvenance,
) -> ImageProvenance {
    ImageProvenance {
        source: SourceProvenance::Raster { format, transfer },
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
    format: RasterFormat,
    ctx: &LoadCtx,
) -> Result<LinearImage, ImageError> {
    if format != RasterFormat::Tiff {
        return Err(ImageError::scientific_rejection(
            path,
            ScientificRefusal::PreviewOnlyRaster { format },
        ));
    }
    let decoded = read(path, format)?;
    ctx.check_cancelled(path)?;
    let desc = decoded.desc();
    check_side(path, desc.size())?;
    if !desc.color_format().sample_type.is_float() {
        return Err(ImageError::scientific_rejection(
            path,
            ScientificRefusal::IntegerRaster,
        ));
    }
    if desc.color_format().channel_count == ChannelCount::Rgba {
        return Err(ImageError::scientific_rejection(
            path,
            ScientificRefusal::AlphaChannel,
        ));
    }

    let color = if desc.color_format().channel_count == ChannelCount::Mono {
        ColorProvenance::Monochrome
    } else {
        ColorProvenance::Unspecified
    };
    let mut image = LinearImage::from(
        PlanarImage::try_from(decoded).expect("a float mono or RGB raster has planes"),
    );
    let size = image.desc().size();
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

/// The `format` raster at `path` as a preview, in `f32` planes, an alpha channel dropped.
pub(crate) fn preview(
    path: &Path,
    format: RasterFormat,
    ctx: &LoadCtx,
) -> Result<LinearImage, ImageError> {
    let decoded = read(path, format)?;
    ctx.check_cancelled(path)?;
    check_side(path, decoded.desc().size())?;
    let alpha_dropped = decoded.desc().color_format().has_alpha();
    let planes =
        PlanarImage::try_from(decoded.without_alpha()).expect("a raster without alpha has planes");
    let mut image = LinearImage::from(planes);
    image.metadata = ImageMetadata {
        header: ObservationHeader::default(),
        derived: DerivedFacts::builder()
            .provenance(provenance(
                format,
                RasterTransfer::Unspecified,
                ColorProvenance::UnmanagedRaster { alpha_dropped },
            ))
            .build(),
    };
    Ok(image)
}

/// A raster of `size` refused when a side is past [`MAX_SIDE`], the longest a frame has.
fn check_side(path: &Path, size: Size2us) -> Result<(), ImageError> {
    if size.width > MAX_SIDE || size.height > MAX_SIDE {
        return Err(ImageError::RasterSideTooLong {
            path: path.to_path_buf(),
            size,
        });
    }
    Ok(())
}

/// Write `image` to `path` as the raster format its extension names: PNG, JPEG or TIFF,
/// published durably as the FITS writers publish, so a reader sees the old file or the new one.
pub(crate) fn save(path: &Path, image: &LinearImage) -> Result<(), WriteError> {
    write_image(path, &InterleavedImage::from(image))
}

/// Write `image` to `path` as the raster format its extension names, published durably.
pub(crate) fn write_image(path: &Path, image: &InterleavedImage) -> Result<(), WriteError> {
    let format = RasterFormat::from_path(path).ok_or_else(|| WriteError::RasterExtension {
        path: path.to_path_buf(),
    })?;
    fs::publish(path, fs::PublicationMode::Durable, |file| {
        let mut out = BufWriter::new(file);
        image.encode(&mut out, format)?;
        Ok::<_, WriteError>(out.flush()?)
    })
}

#[cfg(all(test, feature = "real-data"))]
pub(crate) mod internals {
    use imaginarium::{InterleavedImage, RasterFormat};
    use std::path::Path;

    use crate::io::error::ImageError;
    use crate::io::raster;

    /// The dataset's raster at `path`, in the format its extension names.
    pub(crate) fn read_image(path: &Path) -> Result<InterleavedImage, ImageError> {
        let format = RasterFormat::from_path(path).expect("a dataset raster's extension names it");
        raster::read(path, format)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use common::Size2us;

    use crate::image::frame_desc::MAX_SIDE;
    use crate::io::error::ImageError;
    use crate::io::raster::check_side;

    /// A side of `MAX_SIDE` is a frame's, one more on either side is not, and the error names the
    /// file and its size.
    #[test]
    fn a_raster_side_past_the_frame_limit_is_refused() {
        let path = Path::new("wide.tiff");
        assert!(check_side(path, Size2us::new(MAX_SIDE, MAX_SIDE)).is_ok());
        for size in [Size2us::new(MAX_SIDE + 1, 1), Size2us::new(1, MAX_SIDE + 1)] {
            let error = check_side(path, size).unwrap_err();
            assert!(
                matches!(&error, ImageError::RasterSideTooLong { path: at, size: refused }
                    if at == path && *refused == size),
                "{error:?}"
            );
        }
    }
}
