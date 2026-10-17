//! A [`StackProduct`] as one FITS file, in the layout astropy's `CCDData` reads: the image as the
//! primary HDU, `MASK` and `UNCERT` beside it, and `COVERAGE`, `WEIGHT` and `DISPERSION` after.
//!
//! `CCDData.read` takes the image in its `BUNIT`, the mask, and the inverse variance as an
//! `InverseVariance` uncertainty, each plane's `BSCALE` putting it in the units of the image as
//! astropy scales it. Lumos reads every plane back as it wrote it, bit for bit.

mod report_keywords;

use std::array;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use common::file_utils;
use fits_well::FitsWriter;
use fits_well::header::Header;
use fits_well::image::{Data as FitsData, Image};
use fits_well::io::StreamReader;
use imaginarium::Buffer2;

use crate::error::Cancelled;
use crate::image::linear_image::LinearImage;
use crate::image::quality_flags::QualityFlags;
use crate::image::quality_map::QualityMap;
use crate::io::error::{ImageError, OverBudget, WriteError};
use crate::io::fits::decode::selected_fits::{self, Checksum, SelectedFits};
use crate::io::fits::error::{FitsAllocation, FitsLoadError, FitsRefusal};
use crate::io::fits::extension_claims::{ExtensionClaims, IMAGE_KEYWORD, PRIMARY};
use crate::io::fits::flags_extension::FlagsExtension;
use crate::io::fits::gain_extension::GainExtension;
use crate::io::fits::linear::{self, LinearFitsHdu};
use crate::io::fits::lumos_extension::{DescribedImage, LumosExtension};
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};
use crate::io::load_context::LoadContext;
use crate::math::size2us::Size2us;
use crate::stack_product::StackProduct;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::weight_kind::WeightKind;

/// 1: the report in the primary header, the quality planes in `CCDData`'s layout. 2: what the
/// frame weights of the weight plane were, which a plane of inverse variances is scaled by.
pub(crate) const PRODUCT_VERSION: i64 = 2;
pub(crate) const QUALITY_FORMAT: &str = "QUALPLAN";
pub(crate) const QUALITY_VERSION: i64 = 1;
/// The `UTYPE` astropy names an inverse-variance uncertainty by: its class's name.
pub(crate) const INVERSE_VARIANCE: &str = "InverseVariance";

/// The primary header's keyword that names a stack product, and states its version.
const PRODUCT_KEYWORD: &str = "LUMPRODV";
/// The keyword astropy's `CCDData` reads an uncertainty's kind from.
const UNCERTAINTY_TYPE: &str = "UTYPE";
/// The primary header's keyword for a coverage of one value at every pixel, which no plane holds.
const UNIFORM_COVERAGE: &str = "LUMCOVU";
/// The primary header's keyword for what the frame weights the `WEIGHT` plane sums were.
const WEIGHT_KIND: &str = "LUMWGTK";
/// Each [`WeightKind`] by the value [`WEIGHT_KIND`] names it with.
const WEIGHT_KINDS: [(WeightKind, &str); 3] = [
    (WeightKind::Count, "COUNT"),
    (WeightKind::InverseVariance, "INVERSE_VARIANCE"),
    (WeightKind::Relative, "RELATIVE"),
];

/// The planes after the flags and gain, in the order the file holds them: `CCDData`'s two first.
const PLANES: [LumosExtension; 5] = [
    LumosExtension::Mask,
    LumosExtension::Uncertainty,
    LumosExtension::Coverage,
    LumosExtension::Weight,
    LumosExtension::Dispersion,
];

/// Write `product` to `path` as one checksummed FITS file.
pub(crate) fn save(path: &Path, product: &StackProduct) -> Result<(), WriteError> {
    let image = &product.image;
    let mut primary = LinearFitsHdu::encode(image, product.cfa_type)?;
    primary.header.set(PRODUCT_KEYWORD, PRODUCT_VERSION)?;
    report_keywords::write(&mut primary.header, &product.report)?;
    primary
        .header
        .set(WEIGHT_KIND, weight_kind_name(product.weight_kind))?;
    if let Some(Coverage::Uniform { value, .. }) = &product.coverage {
        primary.header.set(UNIFORM_COVERAGE, f64::from(*value))?;
    }
    let flags = FlagsExtension::encode(image.flags(), None, 1)?;
    let gain = GainExtension::encode(image.flat_gain().map(Arc::as_ref), None)?;
    let described = DescribedImage {
        size: image.dimensions().size(),
        channels: image.channels(),
        scale: image_scale(image),
        weight_kind: Some(product.weight_kind),
    };
    let mut planes = Vec::with_capacity(PLANES.len());
    if let Some(mask) = mask(image) {
        planes.push(encode_plane(LumosExtension::Mask, mask, &described)?);
    }
    let maps = [
        (
            LumosExtension::Uncertainty,
            product.inverse_variance.as_ref(),
        ),
        (
            LumosExtension::Coverage,
            match &product.coverage {
                Some(Coverage::PerPixel(map)) => Some(map),
                Some(Coverage::Uniform { .. }) | None => None,
            },
        ),
        (LumosExtension::Weight, product.weight.as_ref()),
        (LumosExtension::Dispersion, product.dispersion.as_ref()),
    ];
    for (extension, map) in maps {
        if let Some(map) = map {
            planes.push(encode_plane(extension, map_samples(map), &described)?);
        }
    }
    file_utils::publish(path, file_utils::PublicationMode::Durable, |file| {
        let mut writer = FitsWriter::new(&mut *file).with_checksums();
        writer.write_image(&primary.image, Some(&primary.header))?;
        if let Some(flags) = &flags {
            writer.write_image(&flags.image, Some(&flags.header))?;
        }
        if let Some(gain) = &gain {
            writer.write_image(&gain.image, Some(&gain.header))?;
        }
        for plane in &planes {
            writer.write_image(&plane.image, Some(&plane.header))?;
        }
        Ok(())
    })
}

/// Read the stack product at `path`, under `context`'s options and memory limit.
pub(crate) fn load(path: &Path, context: &LoadContext) -> Result<StackProduct, ImageError> {
    read(path, context).map_err(|error| ImageError::fits_load(path, error))
}

fn read(path: &Path, context: &LoadContext) -> Result<StackProduct, FitsLoadError> {
    let mut reader = selected_fits::open(path, context)?;
    let primary = reader.hdus()[0].header.clone();
    if !linear::validate_image_header(&primary)? {
        return Err(FitsRefusal::NotAProduct.into());
    }
    let version = primary
        .get_integer(PRODUCT_KEYWORD)?
        .ok_or(FitsRefusal::NotAProduct)?;
    if version != PRODUCT_VERSION {
        return Err(FitsRefusal::ProductVersion { version }.into());
    }
    let report = report_keywords::read(&primary)?;
    let weight_kind = read_weight_kind(&primary)?;
    let uniform_coverage = primary
        .get_real(UNIFORM_COVERAGE)?
        .map(|value| {
            let coverage = value as f32;
            if f64::from(coverage) == value && (0.0..=1.0).contains(&coverage) {
                Ok(coverage)
            } else {
                Err(FitsRefusal::KeywordOutOfRange {
                    keyword: UNIFORM_COVERAGE.to_owned(),
                })
            }
        })
        .transpose()?;

    let selected = SelectedFits::primary(&mut reader, context)?;
    let cfa_type = selected.cfa_type();
    let image = selected.read(context, Checksum::Judged)?.into_linear()?;

    let mut found = Vec::with_capacity(PLANES.len());
    for extension in PLANES {
        let hdus = reader.hdus();
        if let Some(index) = ExtensionClaims::of(hdus, extension)?.for_image(hdus, 0)? {
            found.push(Found { extension, index });
        }
    }
    let plane_bytes = found.iter().try_fold(0, |bytes: u64, plane| {
        let image = reader.hdus()[plane.index].image()?;
        let samples = image.shape.iter().product::<usize>() * image.bitpix.elem_size();
        Ok::<_, FitsLoadError>(bytes + u64::try_from(samples).expect("a size fits u64"))
    })?;
    let image_bytes = u64::try_from(image.sample_count() * size_of::<f32>() + image.pixel_count())
        .expect("a size fits u64");
    OverBudget::check(image_bytes + plane_bytes, context.memory_limit_bytes).map_err(|budget| {
        FitsRefusal::OverBudget {
            allocation: FitsAllocation::QualityPlanes,
            budget,
        }
    })?;

    let described = DescribedImage {
        size: image.dimensions().size(),
        channels: image.channels(),
        scale: image_scale(&image),
        weight_kind: Some(weight_kind),
    };
    let mut planes = QualityPlanesRead::default();
    let mut mask = None;
    for Found { extension, index } in found {
        Cancelled::check(&context.cancel)?;
        let stored = read_plane(&mut reader, index, extension, &described)?;
        match extension {
            LumosExtension::Mask => mask = Some(stored),
            LumosExtension::Uncertainty => planes.inverse_variance = Some(stored),
            LumosExtension::Coverage => planes.coverage = Some(stored),
            LumosExtension::Weight => planes.weight = Some(stored),
            LumosExtension::Dispersion => planes.dispersion = Some(stored),
            LumosExtension::Flags | LumosExtension::Gain => {
                unreachable!("`PLANES` holds the quality planes alone")
            }
        }
    }
    check_mask(mask, &image)?;
    let size = image.dimensions().size();
    let map = |stored: Option<FitsData>| stored.map(|stored| quality_map(stored, size));
    let coverage = match (uniform_coverage, map(planes.coverage)) {
        (Some(_), Some(_)) => return Err(FitsRefusal::CoverageTwice.into()),
        (Some(value), None) => Some(Coverage::Uniform { value, size }),
        (None, plane) => plane.map(Coverage::PerPixel),
    };
    Ok(StackProduct {
        image,
        coverage,
        weight: map(planes.weight),
        weight_kind,
        inverse_variance: map(planes.inverse_variance),
        dispersion: map(planes.dispersion),
        cfa_type,
        report,
    })
}

/// A quality plane's HDU in the file being read.
#[derive(Debug)]
struct Found {
    extension: LumosExtension,
    index: usize,
}

/// The samples of each quality plane the file holds, as stored.
#[derive(Debug, Default)]
struct QualityPlanesRead {
    coverage: Option<FitsData>,
    weight: Option<FitsData>,
    inverse_variance: Option<FitsData>,
    dispersion: Option<FitsData>,
}

/// One quality plane's HDU, as written.
#[derive(Debug)]
struct EncodedPlane {
    image: Image,
    header: Header,
}

/// A plane's samples and shape: the image's width and height, then its channels when it has more
/// than one.
#[derive(Debug)]
struct PlaneSamples<T> {
    shape: Vec<usize>,
    samples: Vec<T>,
}

/// The value [`WEIGHT_KIND`] names `kind` with.
fn weight_kind_name(kind: WeightKind) -> &'static str {
    WEIGHT_KINDS
        .into_iter()
        .find_map(|(named, name)| (named == kind).then_some(name))
        .expect("every weight kind has a name")
}

/// The weight kind `header` states, refused when it states none or one no product names.
fn read_weight_kind(header: &Header) -> Result<WeightKind, FitsLoadError> {
    let name = header
        .get_text(WEIGHT_KIND)?
        .ok_or_else(|| FitsRefusal::MissingKeyword {
            keyword: WEIGHT_KIND.to_owned(),
        })?;
    WEIGHT_KINDS
        .into_iter()
        .find_map(|(kind, named)| (named == name).then_some(kind))
        .ok_or_else(|| {
            FitsRefusal::KeywordOutOfRange {
                keyword: WEIGHT_KIND.to_owned(),
            }
            .into()
        })
}

/// Multiply a sample of the image by this to recover the source's value: its domain's scale.
fn image_scale(image: &LinearImage) -> Option<f64> {
    image.metadata.derived.domain().map(|domain| domain.scale)
}

/// The image's `NO_DATA` pixels as the mask astropy reads, a byte each, 1 where the pixel holds
/// no measurement, in every channel: `CCDData` takes a mask of the image's own shape. `None` when
/// every pixel holds one.
fn mask(image: &LinearImage) -> Option<PlaneSamples<u8>> {
    let flags = image
        .flags()
        .filter(|flags| flags.contains(QualityFlags::NO_DATA))?;
    let plane: Vec<u8> = flags
        .bytes()
        .iter()
        .map(|&byte| u8::from(QualityFlags::from_byte(byte).intersects(QualityFlags::NO_DATA)))
        .collect();
    let mut samples = Vec::with_capacity(image.sample_count());
    for _ in 0..image.channels() {
        samples.extend_from_slice(&plane);
    }
    Some(PlaneSamples {
        shape: image_shape(image.dimensions().size(), image.channels()),
        samples,
    })
}

fn map_samples(map: &QualityMap) -> PlaneSamples<f32> {
    let (planes, channels) = match map {
        QualityMap::Shared(plane) => (std::slice::from_ref(plane), 1),
        QualityMap::PerChannel(planes) => (planes.as_slice(), 3),
    };
    let size = Size2us::new(planes[0].width(), planes[0].height());
    let mut samples = Vec::with_capacity(size.pixel_count() * channels);
    for plane in planes {
        samples.extend_from_slice(plane.pixels());
    }
    PlaneSamples {
        shape: image_shape(size, channels),
        samples,
    }
}

fn image_shape(size: Size2us, channels: usize) -> Vec<usize> {
    if channels == 1 {
        vec![size.width, size.height]
    } else {
        vec![size.width, size.height, channels]
    }
}

fn encode_plane<T>(
    extension: LumosExtension,
    plane: PlaneSamples<T>,
    image: &DescribedImage,
) -> Result<EncodedPlane, WriteError>
where
    Vec<T>: Into<FitsData>,
{
    let mut header = Header::new();
    header
        .set("EXTNAME", extension.extname())
        .and_then(|header| header.set(IMAGE_KEYWORD, PRIMARY))
        .and_then(|header| header.set(FORMAT_KEYWORD, QUALITY_FORMAT))
        .and_then(|header| header.set(VERSION_KEYWORD, QUALITY_VERSION))?;
    if extension == LumosExtension::Uncertainty {
        header.set(UNCERTAINTY_TYPE, INVERSE_VARIANCE)?;
    }
    let image = Image::new_scaled(plane.shape, plane.samples, extension.scaling(image))?;
    Ok(EncodedPlane { image, header })
}

/// The samples of the `extension` plane at HDU `index`, refused unless it is its format's for
/// `image` ([`LumosExtension::check`]), an uncertainty an inverse variance.
fn read_plane(
    reader: &mut StreamReader<File>,
    index: usize,
    extension: LumosExtension,
    image: &DescribedImage,
) -> Result<FitsData, FitsLoadError> {
    let hdu = &reader.hdus()[index];
    extension.check(hdu, image)?;
    if extension == LumosExtension::Uncertainty {
        let utype = hdu.header.get_text(UNCERTAINTY_TYPE)?;
        if utype != Some(INVERSE_VARIANCE) {
            return Err(FitsRefusal::UncertaintyType {
                utype: utype.map(str::to_owned),
            }
            .into());
        }
    }
    let stored = hdu.image()?;
    let section: Vec<_> = stored.shape.iter().map(|&extent| 0..extent).collect();
    selected_fits::read_extension(reader, index, &section)
}

/// Refuse a `mask` that is not the image's `NO_DATA` at each of its pixels, in every channel, or
/// none where the image has such pixels.
fn check_mask(mask: Option<FitsData>, image: &LinearImage) -> Result<(), FitsLoadError> {
    let no_data = |pixel: usize| {
        image
            .flags()
            .is_some_and(|flags| flags.at(pixel).intersects(QualityFlags::NO_DATA))
    };
    let pixel_count = image.pixel_count();
    let Some(mask) = mask else {
        return match (0..pixel_count).find(|&pixel| no_data(pixel)) {
            Some(index) => Err(FitsRefusal::MaskDisagrees { index }.into()),
            None => Ok(()),
        };
    };
    let FitsData::U8(mask) = mask else {
        unreachable!("`read_plane` admitted only BITPIX = 8");
    };
    match mask
        .iter()
        .enumerate()
        .position(|(index, &byte)| byte != u8::from(no_data(index % pixel_count)))
    {
        Some(index) => Err(FitsRefusal::MaskDisagrees { index }.into()),
        None => Ok(()),
    }
}

/// The plane `stored` holds, one or three of `size`: [`read_plane`] admitted only those.
fn quality_map(stored: FitsData, size: Size2us) -> QualityMap {
    let FitsData::F32(samples) = stored else {
        unreachable!("`read_plane` admitted only BITPIX = -32");
    };
    let pixel_count = size.pixel_count();
    if samples.len() == pixel_count {
        return QualityMap::Shared(Buffer2::new(size.width, size.height, samples));
    }
    debug_assert_eq!(samples.len(), 3 * pixel_count);
    QualityMap::PerChannel(array::from_fn(|channel| {
        let plane = &samples[channel * pixel_count..(channel + 1) * pixel_count];
        Buffer2::new(size.width, size.height, plane.to_vec())
    }))
}

#[cfg(test)]
mod tests;
