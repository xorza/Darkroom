//! The raster formats imaginarium reads — TIFF, PNG, JPEG — decoded into a linear image, for
//! science or for a preview, and a linear image written back as one.
//!
//! FITS and RAW have their own decoders under `fits/` and `raw/`; a raster is read through
//! imaginarium in one call, so what is left here is what each product makes of it.

use std::io::{BufRead, BufWriter, Seek, Write};
use std::path::Path;

use arrayvec::ArrayVec;
use common::{Size2us, SystemMemory, fs};
use imaginarium::{ChannelCount, InterleavedImage, PlanarDesc, PlanarImage, RasterFormat};

use crate::error::Cancelled;
use crate::frame::frame_info::DecoderBytes;
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
use crate::io::error::{RasterLoadError, ScientificRefusal, WriteError};
use crate::io::load_ctx::LoadCtx;
use crate::io::null_policy::CountedPlane;

/// The `format` image `source` holds, decoded by imaginarium, refused when its header states more
/// samples than the process can allocate now.
pub(crate) fn read(
    source: impl BufRead + Seek,
    format: RasterFormat,
) -> Result<InterleavedImage, RasterLoadError> {
    Ok(format.decode(source, SystemMemory::available())?)
}

/// What a raster decode of a frame of `desc` holds beside the planes it makes: the interleaved
/// samples the codec decoded, which a mono frame's plane keeps as its own and a color frame's
/// three planes are copied out of.
pub(crate) const fn decoded_beside(desc: PlanarDesc) -> DecoderBytes {
    DecoderBytes {
        source: 0,
        beside: if desc.channel_count() == 1 {
            0
        } else {
            desc.byte_len()
        },
    }
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

/// The `format` raster `source` holds as a linear image: a floating-point TIFF, taken as linear, of
/// no alpha and of sides a frame may have, its nulls resolved by the context's policy. Every other
/// raster is refused, an integer one among them: its samples are display-encoded.
pub(crate) fn linear(
    source: impl BufRead + Seek,
    format: RasterFormat,
    ctx: &LoadCtx,
) -> Result<LinearImage, RasterLoadError> {
    if format != RasterFormat::Tiff {
        return Err(RasterLoadError::Refused(
            ScientificRefusal::PreviewOnlyRaster { format },
        ));
    }
    let decoded = read(source, format)?;
    Cancelled::check(&ctx.cancel)?;
    let desc = decoded.desc();
    check_side(desc.size())?;
    if !desc.color_format().sample_type.is_float() {
        return Err(RasterLoadError::Refused(ScientificRefusal::IntegerRaster));
    }
    if desc.color_format().channel_count == ChannelCount::Rgba {
        return Err(RasterLoadError::Refused(ScientificRefusal::AlphaChannel));
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
            .map(|plane| CountedPlane::count(plane.pixels_mut()))
            .collect::<ArrayVec<CountedPlane<'_>, 3>>();
        ctx.nulls.resolve(&mut planes, size, None)?
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

/// The `format` raster `source` holds as a preview, in `f32` planes, an alpha channel dropped.
pub(crate) fn preview(
    source: impl BufRead + Seek,
    format: RasterFormat,
    ctx: &LoadCtx,
) -> Result<LinearImage, RasterLoadError> {
    let decoded = read(source, format)?;
    Cancelled::check(&ctx.cancel)?;
    check_side(decoded.desc().size())?;
    let alpha_dropped = decoded.desc().color_format().has_alpha();
    let planes = PlanarImage::without_alpha(&decoded);
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
const fn check_side(size: Size2us) -> Result<(), RasterLoadError> {
    if size.width > MAX_SIDE || size.height > MAX_SIDE {
        return Err(RasterLoadError::SideTooLong(size));
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
        format.encode(image, &mut out)?;
        Ok::<_, WriteError>(out.flush()?)
    })
}

#[cfg(test)]
mod tests {
    use common::Size2us;

    use crate::frame::frame_info::DecoderBytes;
    use crate::image::frame_desc::MAX_SIDE;
    use crate::io::error::RasterLoadError;
    use crate::io::raster::{check_side, decoded_beside};
    use imaginarium::{ChannelLayout, PlanarDesc};

    /// A side of `MAX_SIDE` is a frame's, one more on either side is not, and the error names the
    /// size.
    #[test]
    fn a_raster_side_past_the_frame_limit_is_refused() {
        assert!(check_side(Size2us::new(MAX_SIDE, MAX_SIDE)).is_ok());
        for size in [Size2us::new(MAX_SIDE + 1, 1), Size2us::new(1, MAX_SIDE + 1)] {
            let error = check_side(size).unwrap_err();
            assert!(
                matches!(&error, RasterLoadError::SideTooLong(refused) if *refused == size),
                "{error:?}"
            );
        }
    }

    /// A color raster's decode holds its interleaved samples, as many bytes as the frame's planes,
    /// beside them: 4·3·6 = 72 for a 3×2 RGB frame. A mono frame's plane is the decoded buffer.
    #[test]
    fn a_color_raster_holds_its_interleaved_samples_beside_the_planes() {
        let rgb = PlanarDesc::new((3, 2), ChannelLayout::Rgb);
        assert_eq!(
            decoded_beside(rgb),
            DecoderBytes {
                source: 0,
                beside: 72
            }
        );
        let mono = PlanarDesc::new((3, 2), ChannelLayout::Mono);
        assert_eq!(decoded_beside(mono), DecoderBytes::default());
    }
}
