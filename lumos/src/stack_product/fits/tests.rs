use common::{Buffer2, TempDir, fs};
use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::image::{Bitpix, Scaling};
use imaginarium::PlanarImage;

use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::flags::flag_counts::FlagCounts;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::flags::quality_map::QualityMap;
use crate::image::linear_image::LinearImage;
use crate::image::metadata::dark_comparison::DarkComparison;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::image::metadata::temperature_difference::TemperatureDifference;
use crate::image::metadata::temperature_source::TemperatureSource;
use crate::internals::fits::{fits_refusal, rewrite_fits};
use crate::io::fits::error::FitsRefusal;
use crate::io::fits::lumos_extension::LumosExtension;
use crate::io::load_ctx::LoadCtx;
use crate::stack_product::StackProduct;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::fits::error::{ProductError, ProductRefusal};
use crate::stack_product::run_report::RunReport;
use crate::stack_product::weight_kind::WeightKind;
use common::Size2us;

/// An edit of one HDU of a file, by index: its header and data, `false` to drop it.
type Edit = Box<dyn FnMut(usize, &mut Header, &mut Vec<u8>) -> bool>;

const SIZE: Size2us = Size2us::new(4, 3);
/// The pixel that holds no measurement, and the one that saturated.
const NO_DATA_PIXEL: usize = 5;
const SATURATED_PIXEL: usize = 2;
const SCALE: f64 = 1_234.567_8;

/// A plane of `SIZE` whose samples share no bit pattern with another plane's: `seed` and the
/// index through a sevenths ramp, so each sample is a full-mantissa float.
fn plane(seed: f32) -> Buffer2<f32> {
    Buffer2::from_fn(SIZE, |x, y| (seed + (y * SIZE.width + x) as f32) / 7.0)
}

fn domain() -> SampleDomain {
    SampleDomain {
        scale: SCALE,
        origin: ScaleOrigin::Declared,
        pedestal: Pedestal::Kept(2048.0),
        unit: Some("ADU".to_owned()),
    }
}

fn image(pixels: PlanarImage, flags: Option<PixelFlags>) -> LinearImage {
    let mut image = LinearImage::new(pixels, flags, ImageMetadata::default());
    image.metadata.derived.set_domain(Some(domain()));
    image
}

/// An RGB product with every plane, per channel, and a pixel of no data and one saturated. The
/// inverse variance holds 0 (no information) and +∞ (a noise model of 0), the dispersion NaN
/// (fewer than two samples), and the report a value in every field.
fn rgb_product() -> StackProduct {
    let flags = PixelFlags::from_fn(SIZE, |pixel| match pixel {
        NO_DATA_PIXEL => QualityFlags::NO_DATA,
        SATURATED_PIXEL => QualityFlags::SATURATED,
        _ => QualityFlags::default(),
    });
    let mut inverse_variance = [plane(10.0), plane(11.0), plane(12.0)];
    inverse_variance[0][0] = 0.0;
    inverse_variance[1][1] = f32::INFINITY;
    let mut dispersion = [plane(20.0), plane(21.0), plane(22.0)];
    dispersion[2][3] = f32::NAN;
    let mut excluded_samples = [0; 8];
    excluded_samples[QualityFlags::SATURATED.idx()] = 7;
    let mut kept_flagged_samples = [0; 8];
    kept_flagged_samples[QualityFlags::DEFECT.idx()] = 3;
    StackProduct {
        image: image(
            PlanarImage::from([plane(0.0), plane(1.0), plane(2.0)]),
            flags,
        ),
        coverage: Some(Coverage::PerPixel(QualityMap::Shared(plane(30.0)))),
        weight: Some(QualityMap::PerChannel([
            plane(40.0),
            plane(41.0),
            plane(42.0),
        ])),
        weight_kind: WeightKind::InverseVariance,
        inverse_variance: Some(QualityMap::PerChannel(inverse_variance)),
        dispersion: Some(QualityMap::PerChannel(dispersion)),
        cfa_type: None,
        report: RunReport {
            excluded_samples: FlagCounts::from_per_bit(excluded_samples),
            kept_flagged_samples: FlagCounts::from_per_bit(kept_flagged_samples),
            variance_bg_only: true,
            unverified_dark_exposures: 1,
            unverified_dark_temperatures: 2,
            camera_temperature_darks: 3,
            dark_temperature_differences: 4,
            scaled_darks: 5,
            fitted_dark_scales: 6,
            flat_dark: Some(DarkComparison {
                exposure_compared: false,
                temperature: Some(TemperatureSource::Sensor),
                temperature_difference: Some(TemperatureDifference {
                    reading: TemperatureSource::Sensor,
                    frame: -10.0,
                    dark: -8.5,
                }),
            }),
            floored_flat_pixels: 8,
            spilled_frames: 9,
            small_n_fallback_pixels: 10,
            overcommit_bytes: 11,
            planned_peak_bytes: Some(14),
            parked_lights: 12,
            unconverged_points: 13,
        },
    }
}

fn bits(plane: &Buffer2<f32>) -> Vec<u32> {
    plane
        .pixels()
        .iter()
        .map(|sample| sample.to_bits())
        .collect()
}

fn assert_map_eq(read: Option<&QualityMap>, written: Option<&QualityMap>, name: &str) {
    match (read, written) {
        (Some(QualityMap::Shared(read)), Some(QualityMap::Shared(written))) => {
            assert_eq!(bits(read), bits(written), "{name}");
        }
        (Some(QualityMap::PerChannel(read)), Some(QualityMap::PerChannel(written))) => {
            for (read, written) in read.iter().zip(written) {
                assert_eq!(bits(read), bits(written), "{name}");
            }
        }
        (None, None) => {}
        (read, written) => panic!("{name}: read {read:?}, written {written:?}"),
    }
}

/// Every plane comes back bit for bit, its zeros, infinity and NaN included, as does the report;
/// the image too, but under its pixel of no data, which holds no measurement and reads back as
/// the fill. The file is in `CCDData`'s layout: `UNCERT` names its kind astropy's way and carries
/// `BSCALE = 1/scale²`, so astropy's scaling puts it in the image's physical units, as
/// `DISPERSION`'s `scale²` does and `WEIGHT`'s `1/scale²`, its weights inverse variances; `MASK` is
/// a byte per sample of the image's own shape, 1 under
/// the pixel of no data in every channel. The image alone still loads as a linear image: the
/// quality planes describe it, so the file holds one image.
#[test]
fn a_product_round_trips_every_plane_bit_for_bit() {
    let dir = TempDir::new("product-round-trip");
    let path = dir.join("product.fits");
    let written = rgb_product();
    written.write_fits(&path).unwrap();

    let read = StackProduct::read_fits(&path, &LoadCtx::default()).unwrap();
    for channel in 0..3 {
        let (read, written) = (read.image.channel(channel), written.image.channel(channel));
        for (pixel, (read, written)) in read.pixels().iter().zip(written.pixels()).enumerate() {
            if pixel != NO_DATA_PIXEL {
                assert_eq!(read.to_bits(), written.to_bits(), "{channel} {pixel}");
            }
        }
    }
    let flags = read.image.flags().unwrap();
    assert_eq!(flags.at(NO_DATA_PIXEL), QualityFlags::NO_DATA);
    assert_eq!(flags.at(SATURATED_PIXEL), QualityFlags::SATURATED);
    assert_eq!(flags.count(QualityFlags::NO_DATA), 1);
    assert_eq!(read.image.metadata.derived.domain(), Some(&domain()));
    let Some(Coverage::PerPixel(coverage)) = &read.coverage else {
        panic!("{:?}", read.coverage);
    };
    let Some(Coverage::PerPixel(written_coverage)) = &written.coverage else {
        unreachable!()
    };
    assert_map_eq(Some(coverage), Some(written_coverage), "coverage");
    assert_map_eq(read.weight.as_ref(), written.weight.as_ref(), "weight");
    assert_map_eq(
        read.inverse_variance.as_ref(),
        written.inverse_variance.as_ref(),
        "inverse variance",
    );
    assert_map_eq(
        read.dispersion.as_ref(),
        written.dispersion.as_ref(),
        "dispersion",
    );
    assert_eq!(read.cfa_type, None);
    assert_eq!(read.report, written.report);

    let bytes = fs::read(&path).unwrap();
    let file = FitsReader::from_bytes(&bytes).unwrap();
    let names: Vec<_> = file.hdus()[1..]
        .iter()
        .map(|hdu| hdu.header.get_text("EXTNAME").unwrap().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "LUMFLAGS",
            "MASK",
            "UNCERT",
            "COVERAGE",
            "WEIGHT",
            "DISPERSION"
        ]
    );
    let scaling = |idx: usize| file.hdus()[idx].image_metadata().unwrap().scaling;
    assert_eq!(scaling(0).bscale, SCALE);
    assert_eq!(scaling(2), Scaling::IDENTITY);
    assert_eq!(scaling(3).bscale, 1.0 / (SCALE * SCALE));
    assert_eq!(scaling(4), Scaling::IDENTITY);
    assert_eq!(scaling(5).bscale, 1.0 / (SCALE * SCALE));
    assert_eq!(scaling(6).bscale, SCALE * SCALE);
    assert_eq!(read.weight_kind, WeightKind::InverseVariance);
    assert_eq!(
        file.hdus()[0].header.get_text("LUMWGTK").unwrap(),
        Some("INVERSE_VARIANCE")
    );
    assert_eq!(
        file.hdus()[3].header.get_text("UTYPE").unwrap(),
        Some("InverseVariance")
    );
    let mask = file.hdus()[2].image_metadata().unwrap();
    assert_eq!(
        (mask.bitpix, mask.shape.to_vec()),
        (Bitpix::U8, vec![4, 3, 3])
    );
    let mut reader = FitsReader::from_bytes(&bytes).unwrap();
    let mask = reader.read_data_unit(2).unwrap();
    let pixel_count = SIZE.pixel_count();
    let set: Vec<_> = (0..mask.len()).filter(|&idx| mask[idx] == 1).collect();
    assert_eq!(
        set,
        [
            NO_DATA_PIXEL,
            pixel_count + NO_DATA_PIXEL,
            2 * pixel_count + NO_DATA_PIXEL
        ]
    );

    let alone = LoadCtx::default().linear(&path).unwrap();
    assert_eq!(bits(alone.channel(1)), bits(read.image.channel(1)));
}

/// A monochrome product of undemosaiced Bayer frames: its pattern comes back, its coverage of one
/// value too, held as a keyword rather than a plane of that value, and of its planes only the
/// weight it holds, a count, which no scale changes. With no pixel of no data it writes no `MASK`. A linear image written alone
/// reloads as itself, an RGB one as a cube no load option has to name.
#[test]
fn a_mosaic_product_keeps_its_pattern_and_its_uniform_coverage() {
    let dir = TempDir::new("product-mosaic");
    let path = dir.join("product.fits");
    let cfa_type = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Gbrg));
    let written = StackProduct {
        image: image(PlanarImage::from(plane(0.0)), None),
        coverage: Some(Coverage::Uniform {
            value: 0.75,
            size: SIZE,
        }),
        weight: Some(QualityMap::Shared(plane(1.0))),
        weight_kind: WeightKind::Count,
        inverse_variance: None,
        dispersion: None,
        cfa_type: Some(cfa_type),
        report: RunReport::default(),
    };
    written.write_fits(&path).unwrap();
    let read = StackProduct::read_fits(&path, &LoadCtx::default()).unwrap();
    assert_eq!(read.cfa_type, Some(cfa_type));
    assert!(matches!(
        read.coverage,
        Some(Coverage::Uniform {
            value: 0.75,
            size: SIZE
        })
    ));
    assert_map_eq(read.weight.as_ref(), written.weight.as_ref(), "weight");
    assert!(read.inverse_variance.is_none() && read.dispersion.is_none());
    assert_eq!(bits(read.image.channel(0)), bits(written.image.channel(0)));
    assert_eq!(read.report, RunReport::default());
    let bytes = fs::read(&path).unwrap();
    let file = FitsReader::from_bytes(&bytes).unwrap();
    assert_eq!(file.hdus().len(), 2, "the primary and WEIGHT");
    assert_eq!(
        file.hdus()[1].image_metadata().unwrap().scaling,
        Scaling::IDENTITY
    );
    assert_eq!(read.weight_kind, WeightKind::Count);

    let rgb = rgb_product().image;
    let linear_path = dir.join("linear.fits");
    rgb.write_fits(&linear_path).unwrap();
    let reloaded = LoadCtx::default().linear(&linear_path).unwrap();
    assert!(reloaded.is_rgb());
    assert_eq!(bits(reloaded.channel(2)).len(), SIZE.pixel_count());
    for channel in 0..3 {
        let (read, written) = (reloaded.channel(channel), rgb.channel(channel));
        for (pixel, (read, written)) in read.pixels().iter().zip(written.pixels()).enumerate() {
            if pixel != NO_DATA_PIXEL {
                assert_eq!(read.to_bits(), written.to_bits(), "{channel} {pixel}");
            }
        }
    }
    assert_eq!(reloaded.metadata.derived.domain(), Some(&domain()));
}

/// What a product read refused: the frame layer's FITS format, or the product's own.
#[derive(Debug, PartialEq)]
enum Refused {
    Fits(FitsRefusal),
    Product(ProductRefusal),
}

impl Refused {
    /// The refusal `error` holds; panics on any other error.
    fn of(error: ProductError) -> Self {
        match error {
            ProductError::Refused { refusal, .. } => Self::Product(refusal),
            ProductError::Image(error) => Self::Fits(fits_refusal(error)),
        }
    }
}

impl From<FitsRefusal> for Refused {
    fn from(refusal: FitsRefusal) -> Self {
        Self::Fits(refusal)
    }
}

impl From<ProductRefusal> for Refused {
    fn from(refusal: ProductRefusal) -> Self {
        Self::Product(refusal)
    }
}

/// What the reader refuses of a product file a hand or a tool edited: each edit, its checksums
/// made valid again, against the refusal it meets.
#[test]
fn a_product_edited_out_of_its_format_is_refused() {
    let dir = TempDir::new("product-refusals");
    let source = dir.join("product.fits");
    rgb_product().write_fits(&source).unwrap();
    let pixel_count = SIZE.pixel_count();
    let cases: Vec<(&str, Edit, Refused)> = vec![
        (
            "no product version",
            Box::new(|idx, header, _| {
                if idx == 0 {
                    header.remove("LUMPRODV").unwrap();
                }
                true
            }),
            Refused::from(ProductRefusal::NotAProduct),
        ),
        (
            "another product version",
            Box::new(|idx, header, _| {
                if idx == 0 {
                    header.set("LUMPRODV", 3).unwrap();
                }
                true
            }),
            Refused::from(ProductRefusal::Version {
                version: 3,
                expected: 2,
            }),
        ),
        (
            "no weight kind",
            Box::new(|idx, header, _| {
                if idx == 0 {
                    header.remove("LUMWGTK").unwrap();
                }
                true
            }),
            Refused::from(FitsRefusal::MissingKeyword {
                keyword: "LUMWGTK".to_owned(),
            }),
        ),
        (
            "a weight kind no product names",
            Box::new(|idx, header, _| {
                if idx == 0 {
                    header.set("LUMWGTK", "EXPOSURE").unwrap();
                }
                true
            }),
            Refused::from(FitsRefusal::KeywordOutOfRange {
                keyword: "LUMWGTK".to_owned(),
            }),
        ),
        (
            "inverse-variance weights named a count",
            Box::new(|idx, header, _| {
                if idx == 0 {
                    header.set("LUMWGTK", "COUNT").unwrap();
                }
                true
            }),
            Refused::from(FitsRefusal::ExtensionSamples {
                extension: LumosExtension::Weight,
                bitpix: Bitpix::F32,
                scaling: Scaling {
                    bscale: 1.0 / (SCALE * SCALE),
                    ..Scaling::IDENTITY
                },
            }),
        ),
        (
            "a report field missing",
            Box::new(|idx, header, _| {
                if idx == 0 {
                    header.remove("LUMSPIL").unwrap();
                }
                true
            }),
            Refused::from(FitsRefusal::MissingKeyword {
                keyword: "LUMSPIL".to_owned(),
            }),
        ),
        (
            "a negative count",
            Box::new(|idx, header, _| {
                if idx == 0 {
                    header.set("LUMEXC1", -1).unwrap();
                }
                true
            }),
            Refused::from(FitsRefusal::KeywordOutOfRange {
                keyword: "LUMEXC1".to_owned(),
            }),
        ),
        (
            "an uncertainty of another kind",
            Box::new(|idx, header, _| {
                if idx == 3 {
                    header.set("UTYPE", "StdDevUncertainty").unwrap();
                }
                true
            }),
            Refused::from(ProductRefusal::UncertaintyType {
                utype: Some("StdDevUncertainty".to_owned()),
                expected: "InverseVariance",
            }),
        ),
        (
            "a mask set where the image holds data",
            Box::new(move |idx, _, data| {
                if idx == 2 {
                    data[pixel_count] = 1;
                }
                true
            }),
            Refused::from(ProductRefusal::MaskDisagrees { idx: pixel_count }),
        ),
        (
            "no mask for a pixel of no data",
            Box::new(|idx, _, _| idx != 2),
            Refused::from(ProductRefusal::MaskDisagrees { idx: NO_DATA_PIXEL }),
        ),
        (
            "a uniform coverage beside its plane",
            Box::new(|idx, header, _| {
                if idx == 0 {
                    header.set("LUMCOVU", 1.0).unwrap();
                }
                true
            }),
            Refused::from(ProductRefusal::CoverageTwice),
        ),
    ];
    for (name, edit, refusal) in cases {
        let path = dir.join("edited.fits");
        fs::copy(&source, &path).unwrap();
        rewrite_fits(&path, edit);
        let error = StackProduct::read_fits(&path, &LoadCtx::default()).unwrap_err();
        assert_eq!(Refused::of(error), refusal, "{name}");
    }

    let linear = dir.join("linear.fits");
    (rgb_product().image).write_fits(&linear).unwrap();
    let error = StackProduct::read_fits(&linear, &LoadCtx::default()).unwrap_err();
    assert_eq!(Refused::of(error), ProductRefusal::NotAProduct.into());
}
