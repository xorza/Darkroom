//! A [`StackProduct`] as one FITS file, in the layout astropy's `CCDData` reads: the image as the
//! primary HDU, `MASK` and `UNCERT` beside it, and `COVERAGE`, `WEIGHT` and `DISPERSION` after.
//!
//! `CCDData.read` takes the image in its `BUNIT`, the mask, and the inverse variance as an
//! `InverseVariance` uncertainty, each plane's `BSCALE` putting it in the units of the image as
//! astropy scales it. Lumos reads every plane back as it wrote it, bit for bit.

pub(crate) mod error;
mod report_keywords;

use std::io::{Read, Seek, Write};
use std::path::Path;

use arrayvec::ArrayVec;
use common::{Buffer2, fs};
use fits_well::header::{Header, Keyword};
use fits_well::io::StreamReader;
use fits_well::{FitsReader, FitsWriter};

use crate::error::Cancelled;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::flags::quality_map::QualityMap;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::linear_image::LinearImage;
use crate::io::error::{ImageError, WriteError};
use crate::io::fits::decode::selected_fits::{self, Checksum, SelectedFits};
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::extension_claims::{ExtensionClaims, IMAGE_KEYWORD, PRIMARY};
use crate::io::fits::flags_extension;
use crate::io::fits::gain_extension;
use crate::io::fits::linear;
use crate::io::fits::lumos_extension::{
    DescribedImage, LumosExtension, QUALITY_FORMAT, QUALITY_VERSION,
};
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};
use crate::io::fits::plane_hdu::{FloatPlanes, NoDataMask, PlaneHdu, PlaneSource};
use crate::io::load_ctx::LoadCtx;
use crate::stack_product::StackProduct;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::fits::error::{ProductError, ProductRefusal};
use crate::stack_product::weight_kind::WeightKind;
use common::Size2us;

/// 1: the report in the primary header, the quality planes in `CCDData`'s layout. 2: what the
/// frame weights of the weight plane were, which a plane of inverse variances is scaled by.
const PRODUCT_VERSION: i64 = 2;
/// The `UTYPE` astropy names an inverse-variance uncertainty by: its class's name.
const INVERSE_VARIANCE: &str = "InverseVariance";

/// The primary header's keyword that names a stack product, and states its version.
const PRODUCT_KEYWORD: Keyword = Keyword::new("LUMPRODV");
/// The keyword astropy's `CCDData` reads an uncertainty's kind from.
const UNCERTAINTY_TYPE: Keyword = Keyword::new("UTYPE");
/// The primary header's keyword for a coverage of one value at every pixel, which no plane holds.
const UNIFORM_COVERAGE: Keyword = Keyword::new("LUMCOVU");
/// The primary header's keyword for what the frame weights the `WEIGHT` plane sums were.
const WEIGHT_KIND: Keyword = Keyword::new("LUMWGTK");
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

/// Write `product` to `path` as one checksummed FITS file — see [`write`](fn@write).
pub(crate) fn save(path: &Path, product: &StackProduct) -> Result<(), WriteError> {
    fs::publish(path, fs::PublicationMode::Durable, |file| {
        write(product, file)
    })
}

/// Write `product` to `out` as one checksummed FITS file, each HDU streamed from the planes the
/// product holds.
pub(crate) fn write(product: &StackProduct, out: impl Write + Seek) -> Result<(), WriteError> {
    let image = &product.image;
    let mut primary = linear::hdu(image, product.cfa_type)?;
    primary.header.set(PRODUCT_KEYWORD, PRODUCT_VERSION)?;
    report_keywords::write(&mut primary.header, &product.report)?;
    primary
        .header
        .set(WEIGHT_KIND, weight_kind_name(product.weight_kind))?;
    if let Some(Coverage::Uniform { value, .. }) = &product.coverage {
        primary.header.set(UNIFORM_COVERAGE, f64::from(*value))?;
    }
    let flags = flags_extension::encode(image.flags(), None, 1)?;
    let gain = gain_extension::encode(image.flat_gain().map(|gain| &**gain), None)?;
    let described = DescribedImage {
        size: image.desc().size(),
        channels: image.channel_count(),
        scale: image_scale(image),
        weight_units: product.weight_kind.units(),
    };
    let mask = image
        .flags()
        .filter(|flags| flags.contains(QualityFlags::NO_DATA))
        .map(|flags| {
            plane_hdu(
                LumosExtension::Mask,
                image_shape(described.size, described.channels),
                NoDataMask(flags),
                &described,
            )
        })
        .transpose()?;
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
    ]
    .into_iter()
    .filter_map(|(extension, map)| map.map(|map| map_hdu(extension, map, &described)))
    .collect::<Result<ArrayVec<_, 4>, _>>()?;
    let mut writer = FitsWriter::new(out).with_checksums();
    primary.write(&mut writer)?;
    if let Some(flags) = &flags {
        flags.write(&mut writer)?;
    }
    if let Some(gain) = &gain {
        gain.write(&mut writer)?;
    }
    if let Some(mask) = &mask {
        mask.write(&mut writer)?;
    }
    for map in &maps {
        map.write(&mut writer)?;
    }
    Ok(())
}

/// Read the stack product at `path`, under `ctx`'s options.
pub(crate) fn load(path: &Path, ctx: &LoadCtx) -> Result<StackProduct, ProductError> {
    fs::open(path)
        .map_err(ReadFailure::from)
        .and_then(|file| read(file, ctx))
        .map_err(|failure| match failure {
            ReadFailure::Fits(error) => ImageError::fits_load(path, error).into(),
            ReadFailure::Refused(refusal) => ProductError::Refused {
                path: path.to_path_buf(),
                refusal,
            },
        })
}

/// Why [`read`] stopped, before [`load`] names the file.
#[derive(Debug)]
pub(crate) enum ReadFailure {
    Fits(FitsLoadError),
    Refused(ProductRefusal),
}

impl<E: Into<FitsLoadError>> From<E> for ReadFailure {
    fn from(error: E) -> Self {
        Self::Fits(error.into())
    }
}

/// The stack product `source` holds, under `ctx`'s options.
pub(crate) fn read(source: impl Read + Seek, ctx: &LoadCtx) -> Result<StackProduct, ReadFailure> {
    Cancelled::check(&ctx.cancel)?;
    let mut reader = FitsReader::open(source)?;
    Cancelled::check(&ctx.cancel)?;
    let primary = reader.hdus()[0].header.clone();
    if !linear::validate_image_header(&primary)? {
        return Err(ReadFailure::Refused(ProductRefusal::NotAProduct));
    }
    let Some(version) = primary.get_integer(PRODUCT_KEYWORD)? else {
        return Err(ReadFailure::Refused(ProductRefusal::NotAProduct));
    };
    if version != PRODUCT_VERSION {
        return Err(ReadFailure::Refused(ProductRefusal::Version {
            version,
            expected: PRODUCT_VERSION,
        }));
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
                    keyword: UNIFORM_COVERAGE.to_string(),
                })
            }
        })
        .transpose()?;

    let selected = SelectedFits::primary(&mut reader, ctx)?;
    let cfa_type = selected.cfa_type();
    let image = selected
        .read(&mut reader, ctx, Checksum::Judged, &SharedGains::default())?
        .into_linear()?;

    let mut found = Vec::with_capacity(PLANES.len());
    for extension in PLANES {
        let hdus = reader.hdus();
        if let Some(idx) = ExtensionClaims::of(hdus, extension)?.for_image(hdus, 0)? {
            found.push(Found { extension, idx });
        }
    }
    let described = DescribedImage {
        size: image.desc().size(),
        channels: image.channel_count(),
        scale: image_scale(&image),
        weight_units: weight_kind.units(),
    };
    let mut planes = QualityPlanesRead::default();
    let mut mask = None;
    for Found { extension, idx } in found {
        Cancelled::check(&ctx.cancel)?;
        check_plane(&reader, idx, extension, &described)?;
        if extension == LumosExtension::Mask {
            mask = Some(selected_fits::read_extension::<u8, _>(&mut reader, idx)?);
            continue;
        }
        let stored = Some(selected_fits::read_extension::<f32, _>(&mut reader, idx)?);
        match extension {
            LumosExtension::Uncertainty => planes.inverse_variance = stored,
            LumosExtension::Coverage => planes.coverage = stored,
            LumosExtension::Weight => planes.weight = stored,
            LumosExtension::Dispersion => planes.dispersion = stored,
            LumosExtension::Mask | LumosExtension::Flags | LumosExtension::Gain => {
                unreachable!("`PLANES` holds the quality planes alone, the mask read above")
            }
        }
    }
    check_mask(mask, &image).map_err(ReadFailure::Refused)?;
    let size = image.desc().size();
    let map = |stored: Option<StoredPlanes<f32>>| stored.map(|stored| quality_map(stored, size));
    let coverage = match (uniform_coverage, map(planes.coverage)) {
        (Some(_), Some(_)) => return Err(ReadFailure::Refused(ProductRefusal::CoverageTwice)),
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
    idx: usize,
}

/// An extension's samples as [`selected_fits::read_extension`] reads them: a plane, or one per
/// channel.
type StoredPlanes<T> = ArrayVec<Vec<T>, 3>;

/// The samples of each quality plane the file holds, as stored.
#[derive(Debug, Default)]
struct QualityPlanesRead {
    coverage: Option<StoredPlanes<f32>>,
    weight: Option<StoredPlanes<f32>>,
    inverse_variance: Option<StoredPlanes<f32>>,
    dispersion: Option<StoredPlanes<f32>>,
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
            keyword: WEIGHT_KIND.to_string(),
        })?;
    WEIGHT_KINDS
        .into_iter()
        .find_map(|(kind, named)| (named == name).then_some(kind))
        .ok_or_else(|| {
            FitsRefusal::KeywordOutOfRange {
                keyword: WEIGHT_KIND.to_string(),
            }
            .into()
        })
}

/// Multiply a sample of the image by this to recover the source's value: its domain's scale.
fn image_scale(image: &LinearImage) -> Option<f64> {
    image.metadata.derived.domain().map(|domain| domain.scale)
}

/// The HDU of `map`, the `extension` plane of `image`: one plane shared by every channel, or one
/// per channel.
fn map_hdu<'a>(
    extension: LumosExtension,
    map: &'a QualityMap,
    image: &DescribedImage,
) -> Result<PlaneHdu<FloatPlanes<'a>>, WriteError> {
    let planes: ArrayVec<&[f32], 3> = match map {
        QualityMap::Shared(plane) => [plane.pixels()].into_iter().collect(),
        QualityMap::PerChannel(planes) => planes.iter().map(Buffer2::pixels).collect(),
    };
    plane_hdu(
        extension,
        image_shape(image.size, planes.len()),
        FloatPlanes::new(planes),
        image,
    )
}

/// An image's width and height, then its `channels` when it has more than one.
fn image_shape(size: Size2us, channels: usize) -> ArrayVec<usize, 3> {
    [size.width, size.height]
        .into_iter()
        .chain((channels > 1).then_some(channels))
        .collect()
}

/// The `extension` plane of `image` in `shape`, its samples from `source`.
fn plane_hdu<S: PlaneSource>(
    extension: LumosExtension,
    shape: ArrayVec<usize, 3>,
    source: S,
    image: &DescribedImage,
) -> Result<PlaneHdu<S>, WriteError> {
    let mut header = Header::new();
    header
        .set("EXTNAME", extension.extname())?
        .set(IMAGE_KEYWORD, PRIMARY)?
        .set(FORMAT_KEYWORD, QUALITY_FORMAT)?
        .set(VERSION_KEYWORD, QUALITY_VERSION)?;
    if extension == LumosExtension::Uncertainty {
        header.set(UNCERTAINTY_TYPE, INVERSE_VARIANCE)?;
    }
    Ok(PlaneHdu {
        header,
        shape,
        scaling: extension.scaling(image),
        source,
    })
}

/// Refuse the `extension` plane at HDU `idx` unless it is its format's for `image`
/// ([`LumosExtension::check`]), an uncertainty an inverse variance.
fn check_plane<R: Read + Seek>(
    reader: &StreamReader<R>,
    idx: usize,
    extension: LumosExtension,
    image: &DescribedImage,
) -> Result<(), ReadFailure> {
    let hdu = &reader.hdus()[idx];
    extension.check(hdu, image)?;
    if extension == LumosExtension::Uncertainty {
        let utype = hdu.header.get_text(UNCERTAINTY_TYPE)?;
        if utype != Some(INVERSE_VARIANCE) {
            return Err(ReadFailure::Refused(ProductRefusal::UncertaintyType {
                utype: utype.map(str::to_owned),
                expected: INVERSE_VARIANCE,
            }));
        }
    }
    Ok(())
}

/// Refuse a `mask` that is not the image's `NO_DATA` at each of its pixels, in every channel, or
/// none where the image has such pixels.
fn check_mask(mask: Option<StoredPlanes<u8>>, image: &LinearImage) -> Result<(), ProductRefusal> {
    let no_data = |pixel: usize| {
        image
            .flags()
            .is_some_and(|flags| flags.at(pixel).intersects(QualityFlags::NO_DATA))
    };
    let pixel_count = image.pixel_count();
    let Some(mask) = mask else {
        return match (0..pixel_count).find(|&pixel| no_data(pixel)) {
            Some(idx) => Err(ProductRefusal::MaskDisagrees { idx }),
            None => Ok(()),
        };
    };
    match mask
        .iter()
        .flatten()
        .enumerate()
        .position(|(idx, &byte)| byte != u8::from(no_data(idx % pixel_count)))
    {
        Some(idx) => Err(ProductRefusal::MaskDisagrees { idx }),
        None => Ok(()),
    }
}

/// The planes `stored` holds, one or three of `size`: [`check_plane`] admitted only those.
fn quality_map(stored: StoredPlanes<f32>, size: Size2us) -> QualityMap {
    let mut planes = stored
        .into_iter()
        .map(|samples| Buffer2::new(size, samples));
    match (planes.next(), planes.next(), planes.next()) {
        (Some(shared), None, None) => QualityMap::Shared(shared),
        (Some(red), Some(green), Some(blue)) => QualityMap::PerChannel([red, green, blue]),
        _ => unreachable!("`check_plane` admitted one plane or three"),
    }
}

#[cfg(test)]
mod tests;
