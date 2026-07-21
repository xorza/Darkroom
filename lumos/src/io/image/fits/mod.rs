use std::fs::File;
use std::io::{Error as IoError, ErrorKind};
use std::mem::size_of;
use std::ops::Range;
use std::path::Path;

use fits_well::header::Header;
use fits_well::image::{Bitpix as FitsBitpix, Image, SampleType};
use fits_well::io::{BLOCK_SIZE, ChecksumStatus, Hdu, HduKind, SliceReader, StreamReader};
use fits_well::{FitsReader, FitsWriter};
use rayon::prelude::*;

use common::{CancelToken, file_utils};

use crate::io::image::cfa::{CfaFrameInfo, CfaImage, CfaType, QUANTIZATION_SIGMA_PER_STEP};
use crate::io::image::error::ImageError;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::{
    BitPix, ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageDimensions, ImageMetadata,
    ImageProvenance, SourceContainer, TransferProvenance, scientific_rejection,
};
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::xtrans::XTransPattern;
use crate::resources;

pub(crate) const CFA_FITS_FORMAT: &str = "CFAIMAGE";
pub(crate) const CFA_FITS_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy)]
struct FitsLoadBudget {
    bytes: u64,
}

impl FitsLoadBudget {
    fn from_system() -> Self {
        Self {
            bytes: resources::memory_budget(resources::available_memory()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FitsHduDescription<'a> {
    header: &'a Header,
    kind: HduKind,
    source_bytes: u64,
}

impl<'a> FitsHduDescription<'a> {
    fn from_hdu(path: &Path, hdu: &'a Hdu) -> Result<Self, ImageError> {
        let source_bytes = padded_data_bytes(path, hdu.data_bytes)?;
        Ok(Self {
            header: &hdu.header,
            kind: hdu.kind,
            source_bytes,
        })
    }
}

#[derive(Debug)]
struct FitsDecodePlan {
    shape: Vec<usize>,
    dimensions: ImageDimensions,
    bitpix: BitPix,
    scaling: fits_well::image::Scaling,
    source_bytes: u64,
    decoded_bytes: u64,
    peak_bytes: u64,
}

#[derive(Debug)]
enum FitsReadSelection {
    Whole,
    Section(Vec<Range<usize>>),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CfaFitsHduMetadata<'a> {
    pub(crate) extname: Option<&'a str>,
    pub(crate) image_type: Option<&'a str>,
    pub(crate) prepared: bool,
}

#[derive(Debug)]
pub(crate) struct CfaFitsHdu {
    pub(crate) image: Image,
    pub(crate) header: Header,
}

#[derive(Debug)]
struct DecodedFitsImage {
    dimensions: ImageDimensions,
    metadata: ImageMetadata,
    pixels: LinearPixels,
}

impl DecodedFitsImage {
    fn into_linear(self, path: &Path) -> Result<LinearImage, ImageError> {
        if self.metadata.cfa_type.is_some() {
            return Err(scientific_rejection(
                path,
                "mosaic FITS must be loaded as CfaImage and calibrated before demosaicing",
            ));
        }

        Ok(LinearImage {
            metadata: self.metadata,
            pixels: self.pixels,
        })
    }

    fn into_cfa(
        self,
        path: &Path,
        declared_quantization_sigma: Option<f32>,
    ) -> Result<CfaImage, ImageError> {
        if !self.dimensions.is_grayscale() {
            return Err(fits_unsupported(
                path,
                "scientific CFA input must have exactly one image plane",
            ));
        }
        if self.metadata.cfa_type.is_none() {
            return Err(fits_unsupported(
                path,
                "scientific CFA FITS input is missing validated CFA pattern metadata",
            ));
        }

        let quantization_sigma = declared_quantization_sigma.or_else(|| {
            match (&self.metadata.bitpix, &self.metadata.provenance) {
                (
                    BitPix::UInt8
                    | BitPix::Int16
                    | BitPix::UInt16
                    | BitPix::Int32
                    | BitPix::UInt32
                    | BitPix::Int64,
                    Some(ImageProvenance {
                        transfer: TransferProvenance::FitsPhysical { bscale, .. },
                        ..
                    }),
                ) => Some(bscale.abs() as f32 * QUANTIZATION_SIGMA_PER_STEP),
                _ => None,
            }
        });
        let Self {
            mut metadata,
            pixels,
            ..
        } = self;
        if let Some(provenance) = &mut metadata.provenance {
            provenance.color = ColorProvenance::SensorCfa;
        }
        Ok(CfaImage {
            data: pixels.into_l(),
            metadata,
            quantization_sigma,
        })
    }
}

fn fits_err(path: &Path, source: fits_well::FitsError) -> ImageError {
    ImageError::Fits {
        path: path.to_path_buf(),
        source,
    }
}

fn fits_unsupported(path: &Path, reason: impl Into<String>) -> ImageError {
    ImageError::FitsUnsupported {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn validate_cfa_container_headers(
    path: &Path,
    primary: Option<&Header>,
    image: &Header,
) -> Result<bool, ImageError> {
    if let Some(primary) = primary
        && let Some(format) = primary
            .get_text("LUMOSFMT")
            .map_err(|source| fits_err(path, source))?
        && format != CFA_FITS_FORMAT
    {
        return Err(fits_unsupported(
            path,
            format!("Lumos {format} FITS is not a standalone {CFA_FITS_FORMAT} image"),
        ));
    }

    let is_lumos_cfa = image
        .get_text("LUMOSFMT")
        .map_err(|source| fits_err(path, source))?
        .is_some_and(|format| format == CFA_FITS_FORMAT);
    if is_lumos_cfa {
        let version = image
            .get_integer("LUMOSVER")
            .map_err(|source| fits_err(path, source))?;
        if version != Some(CFA_FITS_VERSION) {
            return Err(fits_unsupported(
                path,
                format!(
                    "unsupported Lumos CFA FITS version {version:?}; expected {CFA_FITS_VERSION}"
                ),
            ));
        }
    }
    Ok(is_lumos_cfa)
}

/// Load an already-linear astronomical image from a FITS file.
pub(crate) fn load_linear_fits(path: &Path) -> Result<LinearImage, ImageError> {
    read_first_image(path, FitsLoadBudget::from_system())?.into_linear(path)
}

pub(crate) fn load_preview_fits(path: &Path) -> Result<LinearImage, ImageError> {
    let decoded = read_first_image(path, FitsLoadBudget::from_system())?;
    if decoded.metadata.cfa_type.is_some() {
        Ok(decoded
            .into_cfa(path, None)?
            .demosaic(&CancelToken::never())
            .expect("validated CFA FITS preview demosaic cannot fail"))
    } else {
        decoded.into_linear(path)
    }
}

fn read_first_image(path: &Path, budget: FitsLoadBudget) -> Result<DecodedFitsImage, ImageError> {
    let file = File::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = FitsReader::open(file).map_err(|source| fits_err(path, source))?;

    let index = *reader
        .image_indices()
        .first()
        .ok_or_else(|| fits_unsupported(path, "no image HDU found"))?;
    let hdu = &reader.hdus()[index];
    let plan = preflight_fits_image(path, FitsHduDescription::from_hdu(path, hdu)?, budget)?;

    read_stream_hdu(&mut reader, index, path, plan)
}

pub(crate) fn load_cfa_fits(path: &Path) -> Result<CfaImage, ImageError> {
    let file = File::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = FitsReader::open(file).map_err(|source| fits_err(path, source))?;
    let index = *reader
        .image_indices()
        .first()
        .ok_or_else(|| fits_unsupported(path, "no image HDU found"))?;
    let hdu = &reader.hdus()[index];
    let plan = preflight_fits_image(
        path,
        FitsHduDescription::from_hdu(path, hdu)?,
        FitsLoadBudget::from_system(),
    )?;
    let is_lumos_cfa = validate_cfa_container_headers(
        path,
        reader.hdus().first().map(|hdu| &hdu.header),
        &reader.hdus()[index].header,
    )?;
    if is_lumos_cfa {
        let report = reader
            .verify_checksum(index)
            .map_err(|source| fits_err(path, source))?;
        if report.datasum != ChecksumStatus::Valid || report.checksum != ChecksumStatus::Valid {
            return Err(fits_unsupported(path, "Lumos CFA FITS checksum mismatch"));
        }
    }
    let quantization_sigma = read_quantization_sigma(&reader.hdus()[index].header)
        .map_err(|source| fits_err(path, source))?;
    read_stream_hdu(&mut reader, index, path, plan)?.into_cfa(path, quantization_sigma)
}

pub(crate) fn read_cfa_hdu(
    reader: &mut SliceReader<'_>,
    index: usize,
    path: &Path,
) -> Result<CfaImage, ImageError> {
    let hdu = &reader.hdus()[index];
    let plan = preflight_fits_image(
        path,
        FitsHduDescription::from_hdu(path, hdu)?,
        FitsLoadBudget::from_system(),
    )?;
    let quantization_sigma = read_quantization_sigma(&reader.hdus()[index].header)
        .map_err(|source| fits_err(path, source))?;
    let header = reader.hdus()[index].header.clone();
    read_decoded_hdu(&header, plan, path, |selection| match selection {
        FitsReadSelection::Whole => reader.read_image(index).map(|image| image.physical_f32()),
        FitsReadSelection::Section(ranges) => reader
            .read_image_section(index, &ranges)
            .map(|image| image.physical_f32()),
    })?
    .into_cfa(path, quantization_sigma)
}

fn read_stream_hdu(
    reader: &mut StreamReader<File>,
    index: usize,
    path: &Path,
    plan: FitsDecodePlan,
) -> Result<DecodedFitsImage, ImageError> {
    let header = reader.hdus()[index].header.clone();
    read_decoded_hdu(&header, plan, path, |selection| match selection {
        FitsReadSelection::Whole => reader.read_image(index).map(|image| image.physical_f32()),
        FitsReadSelection::Section(ranges) => reader
            .read_image_section(index, &ranges)
            .map(|image| image.physical_f32()),
    })
}

fn read_decoded_hdu(
    header: &Header,
    plan: FitsDecodePlan,
    path: &Path,
    mut read_pixels: impl FnMut(FitsReadSelection) -> fits_well::Result<Vec<f32>>,
) -> Result<DecodedFitsImage, ImageError> {
    tracing::debug!(
        source_bytes = plan.source_bytes,
        decoded_bytes = plan.decoded_bytes,
        peak_bytes = plan.peak_bytes,
        "FITS image passed header-first memory preflight"
    );
    let plane_size = plan.dimensions.pixel_count();
    let pixels = if plan.dimensions.is_rgb() {
        let red = read_fits_plane(
            path,
            FitsReadSelection::Section(channel_ranges(&plan, 0)),
            0,
            plane_size,
            &mut read_pixels,
        )?;
        let green = read_fits_plane(
            path,
            FitsReadSelection::Section(channel_ranges(&plan, 1)),
            1,
            plane_size,
            &mut read_pixels,
        )?;
        let blue = read_fits_plane(
            path,
            FitsReadSelection::Section(channel_ranges(&plan, 2)),
            2,
            plane_size,
            &mut read_pixels,
        )?;
        LinearPixels::from_planar_channels(plan.dimensions, [red, green, blue])
    } else {
        LinearPixels::from_planar_channels(
            plan.dimensions,
            [read_fits_plane(
                path,
                FitsReadSelection::Whole,
                0,
                plane_size,
                &mut read_pixels,
            )?],
        )
    };

    let mut metadata =
        read_metadata(header, plan.shape, plan.bitpix).map_err(|source| fits_err(path, source))?;
    metadata.provenance = Some(ImageProvenance {
        container: SourceContainer::Fits,
        decoder: DecoderProvenance::FitsWell,
        transfer: TransferProvenance::FitsPhysical {
            bscale: plan.scaling.bscale,
            bzero: plan.scaling.bzero,
            unit: read_text(header, "BUNIT").map_err(|source| fits_err(path, source))?,
        },
        color: if metadata.cfa_type.is_some() {
            ColorProvenance::SensorCfa
        } else if plan.dimensions.is_grayscale() {
            ColorProvenance::Monochrome
        } else {
            ColorProvenance::Unspecified
        },
        clipped: false,
        demosaic: DemosaicProvenance::None,
    });

    Ok(DecodedFitsImage {
        dimensions: plan.dimensions,
        metadata,
        pixels,
    })
}

fn channel_ranges(plan: &FitsDecodePlan, channel: usize) -> Vec<Range<usize>> {
    let mut ranges = vec![0..plan.dimensions.width(), 0..plan.dimensions.height()];
    if plan.shape.len() == 3 {
        ranges.push(channel..channel + 1);
    }
    ranges
}

fn read_fits_plane(
    path: &Path,
    selection: FitsReadSelection,
    channel: usize,
    expected_pixels: usize,
    read_pixels: &mut impl FnMut(FitsReadSelection) -> fits_well::Result<Vec<f32>>,
) -> Result<Vec<f32>, ImageError> {
    let pixels = read_pixels(selection).map_err(|source| fits_err(path, source))?;
    if pixels.len() != expected_pixels {
        return Err(fits_unsupported(
            path,
            format!(
                "channel {channel} has {} pixels; expected {expected_pixels}",
                pixels.len()
            ),
        ));
    }
    validate_fits_pixels(pixels).map_err(|summary| {
        fits_unsupported(
            path,
            format!(
                "image contains {} null/non-finite pixels; first at linear index {}",
                summary.count,
                channel * expected_pixels + summary.first_index
            ),
        )
    })
}

pub(crate) fn save_cfa_fits(path: &Path, image: &CfaImage) -> std::io::Result<()> {
    let encoded = encode_cfa_hdu(
        image,
        CfaFitsHduMetadata {
            extname: None,
            image_type: image.metadata.image_type.as_deref(),
            prepared: false,
        },
    )?;
    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        FitsWriter::new(&mut *file)
            .with_checksums()
            .write_image_with_header(&encoded.image, &encoded.header)
            .map_err(fits_to_io)
    })
}

pub(crate) fn encode_cfa_hdu(
    cfa: &CfaImage,
    hdu_metadata: CfaFitsHduMetadata<'_>,
) -> std::io::Result<CfaFitsHdu> {
    let mut header = Header::new();
    header
        .set("LUMOSFMT", CFA_FITS_FORMAT)
        .and_then(|header| header.set("LUMOSVER", CFA_FITS_VERSION))
        .map_err(fits_to_io)?;
    if let Some(extname) = hdu_metadata.extname {
        header.set("EXTNAME", extname).map_err(fits_to_io)?;
        header.set("LUMROLE", extname).map_err(fits_to_io)?;
    }
    if hdu_metadata.prepared {
        header.set("LUMPREP", true).map_err(fits_to_io)?;
    }
    write_image_metadata(&mut header, &cfa.metadata, hdu_metadata.image_type)
        .map_err(fits_to_io)?;
    write_cfa_metadata(&mut header, cfa).map_err(fits_to_io)?;

    let image = Image::new(
        [cfa.data.width(), cfa.data.height()],
        cfa.data.pixels().to_vec(),
    )
    .map_err(fits_to_io)?;
    Ok(CfaFitsHdu { image, header })
}

pub(crate) fn fits_to_io(source: fits_well::FitsError) -> IoError {
    match source {
        fits_well::FitsError::Io(source) => source,
        source => IoError::new(ErrorKind::InvalidData, source),
    }
}

fn preflight_fits_image(
    path: &Path,
    hdu: FitsHduDescription<'_>,
    budget: FitsLoadBudget,
) -> Result<FitsDecodePlan, ImageError> {
    if !matches!(
        hdu.kind,
        HduKind::Primary | HduKind::Image | HduKind::CompressedImage
    ) {
        return Err(fits_unsupported(path, "selected HDU is not an image"));
    }

    let shape = if hdu.kind == HduKind::CompressedImage {
        compressed_shape(hdu.header).map_err(|source| fits_err(path, source))?
    } else {
        hdu.header.axes().map_err(|source| fits_err(path, source))?
    };
    let dimensions = dimensions_from_shape(path, &shape)?;
    let stored_bitpix = if hdu.kind == HduKind::CompressedImage {
        let code = hdu
            .header
            .get_integer("ZBITPIX")
            .map_err(|source| fits_err(path, source))?
            .ok_or_else(|| fits_unsupported(path, "compressed image is missing ZBITPIX"))?;
        FitsBitpix::from_code(code).map_err(|source| fits_err(path, source))?
    } else {
        hdu.header
            .bitpix()
            .map_err(|source| fits_err(path, source))?
    };
    let scaling = hdu
        .header
        .scaling()
        .map_err(|source| fits_err(path, source))?;
    let bitpix = map_bitpix(SampleType::from_scaling(stored_bitpix, &scaling));
    let decoded_bytes = checked_size_bytes(
        path,
        dimensions.sample_count(),
        size_of::<f32>(),
        "decoded FITS output",
    )?;
    let native_plane_bytes = checked_size_bytes(
        path,
        dimensions.pixel_count(),
        stored_bitpix.elem_size(),
        "FITS decode plane",
    )?;
    let peak_bytes = if hdu.kind == HduKind::CompressedImage {
        decoded_bytes
            .checked_add(native_plane_bytes)
            .and_then(|bytes| bytes.checked_add(hdu.source_bytes.checked_mul(2)?))
    } else if dimensions.is_rgb() {
        native_plane_bytes
            .checked_mul(3)
            .and_then(|native_bytes| decoded_bytes.checked_add(native_bytes))
    } else {
        decoded_bytes.checked_add(hdu.source_bytes)
    }
    .ok_or_else(|| fits_unsupported(path, "FITS peak memory size overflows u64"))?;

    enforce_fits_budget(path, "stored data unit", hdu.source_bytes, budget)?;
    enforce_fits_budget(path, "decoded output", decoded_bytes, budget)?;
    enforce_fits_budget(path, "estimated peak memory", peak_bytes, budget)?;

    Ok(FitsDecodePlan {
        shape,
        dimensions,
        bitpix,
        scaling,
        source_bytes: hdu.source_bytes,
        decoded_bytes,
        peak_bytes,
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
        .ok_or_else(|| fits_unsupported(path, format!("{name} size overflows usize")))
}

fn enforce_fits_budget(
    path: &Path,
    name: &str,
    required: u64,
    budget: FitsLoadBudget,
) -> Result<(), ImageError> {
    if required > budget.bytes {
        return Err(fits_unsupported(
            path,
            format!(
                "{name} requires {required} bytes, exceeding the FITS load budget of {} bytes",
                budget.bytes
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
        .ok_or_else(|| fits_unsupported(path, "FITS padded data-unit size overflows u64"))
}

pub(crate) fn fits_cfa_frame_info(path: &Path) -> Result<CfaFrameInfo, ImageError> {
    let file = File::open(path).map_err(|source| ImageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let reader = FitsReader::open(file).map_err(|source| fits_err(path, source))?;
    let index = *reader
        .image_indices()
        .first()
        .ok_or_else(|| fits_unsupported(path, "no image HDU found"))?;
    let hdu = &reader.hdus()[index];
    validate_cfa_container_headers(
        path,
        reader.hdus().first().map(|primary| &primary.header),
        &hdu.header,
    )?;
    let shape = match hdu.kind {
        HduKind::CompressedImage => {
            compressed_shape(&hdu.header).map_err(|source| fits_err(path, source))?
        }
        _ => hdu.header.axes().map_err(|source| fits_err(path, source))?,
    };
    let dimensions = dimensions_from_shape(path, &shape)?;
    if !dimensions.is_grayscale() {
        return Err(fits_unsupported(
            path,
            "scientific CFA input must have exactly one image plane",
        ));
    }
    let cfa_type = read_cfa_from_headers(&hdu.header)
        .map_err(|source| fits_err(path, source))?
        .ok_or_else(|| {
            fits_unsupported(
                path,
                "scientific CFA FITS input is missing validated CFA pattern metadata",
            )
        })?;
    Ok(CfaFrameInfo {
        dimensions,
        demosaic: cfa_type.demosaic_kind(),
    })
}

fn compressed_shape(header: &Header) -> fits_well::Result<Vec<usize>> {
    let rank = header
        .get_integer("ZNAXIS")?
        .ok_or(fits_well::FitsError::MissingKeyword { name: "ZNAXIS" })?;
    let rank = usize::try_from(rank)
        .ok()
        .filter(|rank| *rank <= 999)
        .ok_or(fits_well::FitsError::KeywordOutOfRange { name: "ZNAXIS" })?;
    (1..=rank)
        .map(|axis| {
            let key = format!("ZNAXIS{axis}");
            let value = header
                .get_integer(&key)?
                .ok_or(fits_well::FitsError::MissingKeyword { name: "ZNAXISn" })?;
            usize::try_from(value)
                .map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: "ZNAXISn" })
        })
        .collect()
}

fn dimensions_from_shape(path: &Path, shape: &[usize]) -> Result<ImageDimensions, ImageError> {
    if shape.contains(&0) {
        return Err(fits_unsupported(
            path,
            format!("FITS image axes must be nonzero, got {shape:?}"),
        ));
    }
    let (width, height, channels) = match shape {
        [width, height] => (*width, *height, 1),
        [width, height, channels] if *channels == 1 || *channels == 3 => {
            (*width, *height, *channels)
        }
        [_, _, channels] => Err(fits_unsupported(
            path,
            format!("Unsupported channel count (NAXIS3): {channels}"),
        ))?,
        _ => {
            return Err(fits_unsupported(
                path,
                format!("Unsupported number of dimensions: {}", shape.len()),
            ));
        }
    };
    let pixel_count = width
        .checked_mul(height)
        .ok_or_else(|| fits_unsupported(path, format!("FITS pixel count overflows: {shape:?}")))?;
    pixel_count
        .checked_mul(channels)
        .ok_or_else(|| fits_unsupported(path, format!("FITS sample count overflows: {shape:?}")))?;
    Ok(ImageDimensions::new((width, height), channels))
}

fn read_metadata(
    header: &Header,
    header_dimensions: Vec<usize>,
    bitpix: BitPix,
) -> fits_well::Result<ImageMetadata> {
    Ok(ImageMetadata {
        object: read_text(header, "OBJECT")?,
        instrument: read_text(header, "INSTRUME")?,
        telescope: read_text(header, "TELESCOP")?,
        date_obs: read_text(header, "DATE-OBS")?,
        exposure_time: header.get_real("EXPTIME")?,
        iso: read_u32(header, "ISOSPEED")?,
        bitpix,
        header_dimensions,
        cfa_type: read_cfa_from_headers(header)?,
        camera_white_balance: read_camera_white_balance(header)?,
        filter: read_text(header, "FILTER")?,
        gain: header.get_real("GAIN")?,
        egain: header.get_real("EGAIN")?,
        ccd_temp: first_real(header, "CCD-TEMP", "CCDTEMP")?,
        image_type: first_text(header, "IMAGETYP", "FRAME")?,
        xbinning: read_i32(header, "XBINNING")?,
        ybinning: read_i32(header, "YBINNING")?,
        set_temp: header.get_real("SET-TEMP")?,
        offset: read_i32(header, "OFFSET")?,
        focal_length: header.get_real("FOCALLEN")?,
        airmass: header.get_real("AIRMASS")?,
        ra_deg: read_ra_deg(header)?,
        dec_deg: read_dec_deg(header)?,
        pixel_size_x: header.get_real("XPIXSZ")?,
        pixel_size_y: header.get_real("YPIXSZ")?,
        data_max: header.get_real("DATAMAX")?,
        provenance: None,
        calibrated: header.get_logical("LUMCAL")?.unwrap_or(false),
    })
}

fn write_image_metadata(
    header: &mut Header,
    metadata: &ImageMetadata,
    image_type: Option<&str>,
) -> fits_well::Result<()> {
    set_optional_text(header, "OBJECT", metadata.object.as_deref())?;
    set_optional_text(header, "INSTRUME", metadata.instrument.as_deref())?;
    set_optional_text(header, "TELESCOP", metadata.telescope.as_deref())?;
    set_optional_text(header, "DATE-OBS", metadata.date_obs.as_deref())?;
    set_optional_real(header, "EXPTIME", metadata.exposure_time)?;
    set_optional_integer(header, "ISOSPEED", metadata.iso.map(i64::from))?;
    set_optional_text(header, "FILTER", metadata.filter.as_deref())?;
    set_optional_real(header, "GAIN", metadata.gain)?;
    set_optional_real(header, "EGAIN", metadata.egain)?;
    set_optional_real(header, "CCD-TEMP", metadata.ccd_temp)?;
    set_optional_text(
        header,
        "IMAGETYP",
        image_type.or(metadata.image_type.as_deref()),
    )?;
    set_optional_integer(header, "XBINNING", metadata.xbinning.map(i64::from))?;
    set_optional_integer(header, "YBINNING", metadata.ybinning.map(i64::from))?;
    set_optional_real(header, "SET-TEMP", metadata.set_temp)?;
    set_optional_integer(header, "OFFSET", metadata.offset.map(i64::from))?;
    set_optional_real(header, "FOCALLEN", metadata.focal_length)?;
    set_optional_real(header, "AIRMASS", metadata.airmass)?;
    set_optional_real(header, "RA", metadata.ra_deg)?;
    set_optional_real(header, "DEC", metadata.dec_deg)?;
    set_optional_real(header, "XPIXSZ", metadata.pixel_size_x)?;
    set_optional_real(header, "YPIXSZ", metadata.pixel_size_y)?;
    set_optional_real(header, "DATAMAX", metadata.data_max)?;
    if metadata.calibrated {
        header.set("LUMCAL", true)?;
    }
    if let Some(ImageProvenance {
        transfer: TransferProvenance::FitsPhysical {
            unit: Some(unit), ..
        },
        ..
    }) = &metadata.provenance
    {
        header.set("BUNIT", unit.as_str())?;
    }
    Ok(())
}

fn write_cfa_metadata(header: &mut Header, cfa: &CfaImage) -> fits_well::Result<()> {
    match cfa.metadata.cfa_type.as_ref() {
        Some(CfaType::Mono) => {
            header.set("CFATYPE", "MONO")?;
        }
        Some(CfaType::Bayer(pattern)) => {
            header.set("CFATYPE", "BAYER")?;
            header.set("BAYERPAT", bayerpat(*pattern))?;
            header.set("ROWORDER", "TOP-DOWN")?;
        }
        Some(CfaType::XTrans(pattern)) => {
            XTransPattern::new(*pattern).map_err(|_| fits_well::FitsError::TypeMismatch {
                name: "CFATYPE".to_string(),
                expected: "valid X-Trans pattern",
            })?;
            header.set("CFATYPE", "XTRANS")?;
            header.set("ROWORDER", "TOP-DOWN")?;
            for (row, values) in pattern.iter().enumerate() {
                let keyword = format!("XTRNROW{row}");
                let value = values
                    .iter()
                    .map(|value| char::from(b'0' + *value))
                    .collect::<String>();
                header.set(&keyword, value)?;
            }
        }
        None => {
            return Err(fits_well::FitsError::TypeMismatch {
                name: "CFATYPE".to_string(),
                expected: "declared CFA sensor type",
            });
        }
    }

    if let Some([red, green_1, blue, green_2]) = cfa.metadata.camera_white_balance {
        header.set("LUMWBR", f64::from(red))?;
        header.set("LUMWBG1", f64::from(green_1))?;
        header.set("LUMWBB", f64::from(blue))?;
        header.set("LUMWBG2", f64::from(green_2))?;
    }
    if let Some(sigma) = cfa.quantization_sigma {
        if !sigma.is_finite() || sigma < 0.0 {
            return Err(fits_well::FitsError::KeywordOutOfRange { name: "QNTZSIG" });
        }
        header.set("QNTZSIG", f64::from(sigma))?;
    }
    Ok(())
}

fn set_optional_text(
    header: &mut Header,
    keyword: &str,
    value: Option<&str>,
) -> fits_well::Result<()> {
    if let Some(value) = value {
        header.set(keyword, value)?;
    }
    Ok(())
}

fn set_optional_real(
    header: &mut Header,
    keyword: &str,
    value: Option<f64>,
) -> fits_well::Result<()> {
    if let Some(value) = value {
        header.set(keyword, value)?;
    }
    Ok(())
}

fn set_optional_integer(
    header: &mut Header,
    keyword: &str,
    value: Option<i64>,
) -> fits_well::Result<()> {
    if let Some(value) = value {
        header.set(keyword, value)?;
    }
    Ok(())
}

fn bayerpat(pattern: CfaPattern) -> &'static str {
    match pattern {
        CfaPattern::Rggb => "RGGB",
        CfaPattern::Bggr => "BGGR",
        CfaPattern::Grbg => "GRBG",
        CfaPattern::Gbrg => "GBRG",
    }
}

/// Map fits-well's effective `SampleType` to lumos's `BitPix`.
///
/// `SampleType` already resolves the FITS unsigned/signed-byte BZERO conventions. Lumos has no
/// signed-byte or unsigned-64 metadata variant, so `I8` folds into `UInt8` and `U64` into `Int64`.
fn map_bitpix(sample_type: SampleType) -> BitPix {
    match sample_type {
        SampleType::I8 | SampleType::U8 => BitPix::UInt8,
        SampleType::I16 => BitPix::Int16,
        SampleType::U16 => BitPix::UInt16,
        SampleType::I32 => BitPix::Int32,
        SampleType::U32 => BitPix::UInt32,
        SampleType::I64 | SampleType::U64 => BitPix::Int64,
        SampleType::F32 => BitPix::Float32,
        SampleType::F64 => BitPix::Float64,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NullSummary {
    count: usize,
    first_index: usize,
}

/// `physical_f32` represents integer `BLANK` and floating nulls as non-finite values. Until
/// `LinearImage` carries a validity plane, accepting those values would either poison arithmetic or
/// invent valid zero-valued samples, so the entire image is rejected with a summary.
fn validate_fits_pixels(pixels: Vec<f32>) -> Result<Vec<f32>, NullSummary> {
    let nulls = pixels
        .par_iter()
        .enumerate()
        .filter_map(|(index, pixel)| {
            (!pixel.is_finite()).then_some(NullSummary {
                count: 1,
                first_index: index,
            })
        })
        .reduce_with(|left, right| NullSummary {
            count: left.count + right.count,
            first_index: left.first_index.min(right.first_index),
        });
    if let Some(summary) = nulls {
        return Err(summary);
    }

    Ok(pixels)
}

/// Read CFA pattern from BAYERPAT header, adjusting for ROWORDER if present.
///
/// BAYERPAT values: "RGGB", "BGGR", "GRBG", "GBRG", or "TRUE" (= RGGB).
/// ROWORDER: "TOP-DOWN" (default) or "BOTTOM-UP" (flips pattern vertically).
/// XBAYROFF/YBAYROFF: integer offsets into the Bayer matrix (shifts pattern).
fn read_cfa_from_headers(header: &Header) -> fits_well::Result<Option<CfaType>> {
    match header.get_text("CFATYPE")? {
        Some(value) if value.eq_ignore_ascii_case("MONO") => return Ok(Some(CfaType::Mono)),
        Some(value) if value.eq_ignore_ascii_case("BAYER") => {
            return read_bayer_cfa(header, true);
        }
        Some(value) if value.eq_ignore_ascii_case("XTRANS") => {
            return Ok(Some(CfaType::XTrans(read_xtrans_pattern(header)?)));
        }
        Some(_) => {
            return Err(fits_well::FitsError::TypeMismatch {
                name: "CFATYPE".to_string(),
                expected: "MONO, BAYER, or XTRANS",
            });
        }
        None => {}
    }
    read_bayer_cfa(header, false)
}

fn read_bayer_cfa(header: &Header, required: bool) -> fits_well::Result<Option<CfaType>> {
    let Some(bayerpat) = header.get_text("BAYERPAT")? else {
        if required {
            return Err(fits_well::FitsError::MissingKeyword { name: "BAYERPAT" });
        }
        return Ok(None);
    };
    let Some(mut pattern) = CfaPattern::from_bayerpat(bayerpat) else {
        return Err(fits_well::FitsError::TypeMismatch {
            name: "BAYERPAT".to_string(),
            expected: "RGGB, BGGR, GRBG, or GBRG",
        });
    };

    // ROWORDER: if BOTTOM-UP, the first row in memory is the bottom of the image,
    // so the Bayer pattern needs to be flipped vertically.
    if let Some(roworder) = header.get_text("ROWORDER")?
        && roworder.trim().eq_ignore_ascii_case("BOTTOM-UP")
    {
        pattern = pattern.flip_vertical();
    }

    // XBAYROFF/YBAYROFF: offset into the Bayer matrix.
    // An odd Y offset flips rows, an odd X offset flips columns.
    let xoff = header.get_integer("XBAYROFF")?.unwrap_or(0);
    let yoff = header.get_integer("YBAYROFF")?.unwrap_or(0);
    if yoff & 1 != 0 {
        pattern = pattern.flip_vertical();
    }
    if xoff & 1 != 0 {
        pattern = pattern.flip_horizontal();
    }

    Ok(Some(CfaType::Bayer(pattern)))
}

fn read_xtrans_pattern(header: &Header) -> fits_well::Result<[[u8; 6]; 6]> {
    let mut pattern = [[0u8; 6]; 6];
    for (row, values) in pattern.iter_mut().enumerate() {
        let keyword = format!("XTRNROW{row}");
        let value =
            header
                .get_text(&keyword)?
                .ok_or_else(|| fits_well::FitsError::TypeMismatch {
                    name: keyword.clone(),
                    expected: "six X-Trans color digits",
                })?;
        if value.len() != 6 {
            return Err(fits_well::FitsError::TypeMismatch {
                name: keyword,
                expected: "six X-Trans color digits",
            });
        }
        for (column, byte) in value.bytes().enumerate() {
            values[column] = match byte {
                b'0'..=b'2' => byte - b'0',
                _ => {
                    return Err(fits_well::FitsError::TypeMismatch {
                        name: keyword,
                        expected: "X-Trans digits in the range 0..=2",
                    });
                }
            };
        }
    }
    XTransPattern::new(pattern).map_err(|_| fits_well::FitsError::TypeMismatch {
        name: "CFATYPE".to_string(),
        expected: "valid X-Trans pattern",
    })?;
    Ok(pattern)
}

fn read_camera_white_balance(header: &Header) -> fits_well::Result<Option<[f32; 4]>> {
    let values = [
        header.get_real("LUMWBR")?,
        header.get_real("LUMWBG1")?,
        header.get_real("LUMWBB")?,
        header.get_real("LUMWBG2")?,
    ];
    match values {
        [None, None, None, None] => Ok(None),
        [Some(red), Some(green_1), Some(blue), Some(green_2)] => {
            let values = [red as f32, green_1 as f32, blue as f32, green_2 as f32];
            if values.iter().all(|value| value.is_finite() && *value > 0.0) {
                Ok(Some(values))
            } else {
                Err(fits_well::FitsError::KeywordOutOfRange { name: "LUMWB*" })
            }
        }
        _ => Err(fits_well::FitsError::TypeMismatch {
            name: "LUMWB*".to_string(),
            expected: "all four white-balance multipliers or none",
        }),
    }
}

fn read_quantization_sigma(header: &Header) -> fits_well::Result<Option<f32>> {
    header
        .get_real("QNTZSIG")?
        .map(|value| {
            let value = value as f32;
            if value.is_finite() && value >= 0.0 {
                Ok(value)
            } else {
                Err(fits_well::FitsError::KeywordOutOfRange { name: "QNTZSIG" })
            }
        })
        .transpose()
}

/// Read RA in degrees from FITS headers.
///
/// Tries: `RA` (degrees, NINA/SGP), `OBJCTRA` (HMS string, MaximDL/ASCOM),
/// `CRVAL1` (WCS reference point, plate-solved images).
fn read_ra_deg(header: &Header) -> fits_well::Result<Option<f64>> {
    if let Some(ra) = header.get_real("RA")? {
        return Ok(Some(ra));
    }
    if let Some(s) = header.get_text("OBJCTRA")? {
        // OBJCTRA is in hours; scale the sexagesimal value to degrees.
        return Ok(parse_sexagesimal(s).map(|h| h * 15.0));
    }
    header.get_real("CRVAL1")
}

/// Read DEC in degrees from FITS headers.
///
/// Tries: `DEC` (degrees), `OBJCTDEC` (DMS string), `CRVAL2` (WCS).
fn read_dec_deg(header: &Header) -> fits_well::Result<Option<f64>> {
    if let Some(dec) = header.get_real("DEC")? {
        return Ok(Some(dec));
    }
    if let Some(s) = header.get_text("OBJCTDEC")? {
        return Ok(parse_sexagesimal(s));
    }
    header.get_real("CRVAL2")
}

/// Parse a sexagesimal triple "±AA BB CC.cc" to its signed decimal value
/// `AA + BB/60 + CC/3600`. Accepts space- or colon-delimited fields.
///
/// DEC (degrees) uses the result directly; RA (hours) scales it by 15 at the call
/// site. The sign is taken from the first field, so "-00 30 00" is -0.5 — a bare
/// `-AA.abs()` would otherwise drop the sign on a zero-degree/hour field.
fn parse_sexagesimal(s: &str) -> Option<f64> {
    let parts: Vec<f64> = s
        .split([' ', ':'])
        .filter(|p| !p.is_empty())
        .map(|p| p.trim().parse().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() != 3 {
        return None;
    }
    let sign = if parts[0].is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    Some(sign * (parts[0].abs() + parts[1] / 60.0 + parts[2] / 3600.0))
}

/// Read an optional string-valued header key as an owned `String`.
fn read_text(header: &Header, key: &str) -> fits_well::Result<Option<String>> {
    Ok(header.get_text(key)?.map(str::to_owned))
}

fn first_text(header: &Header, first: &str, second: &str) -> fits_well::Result<Option<String>> {
    match read_text(header, first)? {
        Some(value) => Ok(Some(value)),
        None => read_text(header, second),
    }
}

fn first_real(header: &Header, first: &str, second: &str) -> fits_well::Result<Option<f64>> {
    match header.get_real(first)? {
        Some(value) => Ok(Some(value)),
        None => header.get_real(second),
    }
}

fn read_u32(header: &Header, key: &'static str) -> fits_well::Result<Option<u32>> {
    header
        .get_integer(key)?
        .map(|value| {
            u32::try_from(value).map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: key })
        })
        .transpose()
}

fn read_i32(header: &Header, key: &'static str) -> fits_well::Result<Option<i32>> {
    header
        .get_integer(key)?
        .map(|value| {
            i32::try_from(value).map_err(|_| fits_well::FitsError::KeywordOutOfRange { name: key })
        })
        .transpose()
}

#[cfg(test)]
mod tests;
