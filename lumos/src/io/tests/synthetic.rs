//! Load/decode round-trip tests on synthetic frames.
//!
//! `fits-well` ships a `FitsWriter`, so a synthetic FITS can be written and read back through the
//! real `decode::load_linear` path — exercising sample-type selection, the unsigned-via-BZERO
//! convention, the division of integer samples into the `[0, 1]` domain (and the float path's
//! exemption from it), and both halves of the null convention. The demosaic path is exercised by
//! building mosaics from known colors and demosaicing them back.

use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::combine::normalization::Normalization;
use crate::combine::stack_config::StackConfig;
use crate::combine::stack_config::weighting::Weighting;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::channel_layout::ChannelLayout;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::image::quality_flags::QualityFlags;
use crate::ingest::error::AdmissionError;
use crate::internals::assertions::assert_close;
use crate::io::decoder::Decoder;
use crate::io::decoder::cfa_decoder::CfaDecoder;
use crate::io::frame_info::FrameInfo;
use crate::io::writer;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use std::convert::Infallible;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::combine::error::StackError;
use crate::combine::frame_cache::frame_step::SteppedPaths;
use crate::combine::stack::planned_combine::PlannedCombine;
use crate::drizzle::drizzle_config::DrizzleConfig;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::drizzle::error::DrizzleError;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::sample_domain::{DomainMap, ScaleOrigin};
use crate::ingest::ingest_config::IngestConfig;
use crate::ingest::ingest_run::IngestRun;
use crate::io::error::{ImageError, ScientificRefusal};
use crate::io::fits::decode;
use crate::io::fits::error::FitsRefusal;
use crate::io::fits::fits_load_options::{FitsCubeInterpretation, FitsFloatScale, FitsLoadOptions};
use crate::io::load_context::LoadContext;
use crate::io::null_policy::NullPolicy;
use crate::memory::run_memory::RunMemory;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;

use crate::internals::cfa::{XTRANS_PATTERN, make_cfa};
use crate::internals::fits::{fits_refusal, write_fits};
use crate::{CalibrationMasters, CalibrationSet, CfaType, ColorProvenance, PreviewPixels};
use common::{CancelToken, TempDir};
use fits_well::header::Header;
use fits_well::image::{Image, Scaling};
use fits_well::{FitsError, FitsWriter};
use imaginarium::ColorFormat;

/// Write `image` to a temp FITS file via `FitsWriter`, then load it through `decode::load_linear`.
fn write_and_load(name: &str, image: &Image) -> Result<LinearImage, ImageError> {
    let dir = TempDir::new("lumos-fits-roundtrip");
    let path = dir.join(format!("{name}.fits"));
    write_fits(&path, image, None);
    decode::load_linear(&path, &LoadContext::default())
}

/// Write `image` with `header`'s cards to `path`.
fn write_with_header(path: PathBuf, image: &Image, header: &Header) -> PathBuf {
    write_fits(&path, image, Some(header));
    path
}

fn write_header_and_load(name: &str, header: &Header) -> Result<LinearImage, ImageError> {
    let dir = TempDir::new("lumos-fits-roundtrip");
    let path = dir.join(format!("{name}.fits"));
    let mut writer = FitsWriter::new(File::create(&path).unwrap());
    writer.write_raw_hdu(header, &0.0f32.to_be_bytes()).unwrap();
    writer.into_inner().sync_all().unwrap();
    decode::load_linear(&path, &LoadContext::default())
}

fn one_pixel_header() -> Header {
    let mut header = Header::new();
    header.set("SIMPLE", true).unwrap();
    header.set("BITPIX", -32).unwrap();
    header.set("NAXIS", 2).unwrap();
    header.set("NAXIS1", 1).unwrap();
    header.set("NAXIS2", 1).unwrap();
    header
}

/// An optional keyword the header gives with the wrong type or value is absent from the metadata
/// and the frame loads; a keyword that decides how the samples are read still fails the load.
#[test]
fn optional_metadata_degrades_and_load_deciding_keywords_fail() {
    // An integer image, whose scale its BITPIX settles, so DATAMAX is metadata only.
    let mut mistyped = one_pixel_header();
    mistyped.set("BITPIX", 32).unwrap();
    mistyped.set("DATAMAX", "not a real").unwrap();
    mistyped.set("ISOSPEED", -1).unwrap();
    let image = write_header_and_load("mistyped_metadata", &mistyped).unwrap();
    assert_eq!(image.metadata.header.data_max, None);
    assert_eq!(image.metadata.header.iso, None);

    // A float image's DATAMAX is how its scale is decided, so there it fails the load.
    let mut mistyped_float_scale = one_pixel_header();
    mistyped_float_scale.set("DATAMAX", "not a real").unwrap();
    assert!(matches!(
        write_header_and_load("mistyped_float_scale", &mistyped_float_scale),
        Err(ImageError::Fits {
            source: FitsError::TypeMismatch { name, .. },
            ..
        }) if name == "DATAMAX"
    ));

    let mut mistyped_row_order = one_pixel_header();
    mistyped_row_order.set("ROWORDER", 1).unwrap();
    assert!(matches!(
        write_header_and_load("mistyped_row_order", &mistyped_row_order),
        Err(ImageError::Fits {
            source: FitsError::TypeMismatch { name, .. },
            ..
        }) if name == "ROWORDER"
    ));
}

#[test]
fn fits_float32_round_trips_pixels_and_order() {
    let size = Size2us::new(32usize, 24usize);
    // The asymmetric physical scale catches both transposition and accidental frame-max scaling.
    // Divided by 8 to stay below the maximum an undeclared float scale may reach; exact, since 8 is
    // a power of two.
    let pixels: Vec<f32> = (0..size.height)
        .flat_map(|y| {
            (0..size.width).map(move |x| (-12.0 + y as f32 * 3.0 + x as f32 * 0.25) / 8.0)
        })
        .collect();
    let image = Image::new(
        vec![size.width, size.height], // fits-well is NAXIS1-first: [width, height]
        pixels.clone(),
    )
    .unwrap();

    let loaded = write_and_load("float32", &image).unwrap();
    assert_eq!(loaded.width(), size.width);
    assert_eq!(loaded.height(), size.height);
    assert_eq!(loaded.channels(), 1);
    assert_eq!(loaded.channel(0).pixels(), pixels);
}

#[test]
fn fits_integer_samples_are_divided_by_the_span_their_header_declares() {
    // Every case divides by |BSCALE| × (2^bits − 1) — the span the header itself declares — so
    // frames from different integer widths and BSCALEs land in one comparable domain. The scale
    // is applied without an offset, so a signed frame keeps its own zero.

    // BITPIX = 16, BSCALE = 1: divisor 65535, physical -32768..=32767 → about [-0.5, 0.5].
    let signed = Image::new(vec![4, 1], vec![-32_768i16, -3, 0, 32_767]).unwrap();
    let signed_loaded = write_and_load("int16", &signed).unwrap();
    let pixels = signed_loaded.channel(0).pixels();
    assert_close!(pixels[0], -0.500_007_6, 1e-7, "{pixels:?}");
    assert_close!(pixels[1], -4.577_636_7e-5, 1e-9, "{pixels:?}");
    assert_eq!(pixels[2], 0.0);
    assert_close!(pixels[3], 0.499_992_37, 1e-7, "{pixels:?}");

    // BSCALE = -2.5 widens the declared span to 2.5 × 65535 = 163837.5, and the physical values
    // 17.5, 10, 0 divide by it. A negative BSCALE scales by its magnitude: the sign already lives
    // in the physical value fits-well produced.
    let scaled = Image::new_scaled(
        vec![3, 1],
        vec![-3i16, 0, 4],
        Scaling {
            bscale: -2.5,
            bzero: 10.0,
            blank: None,
        },
    )
    .unwrap();
    let scaled_loaded = write_and_load("negative_bscale", &scaled).unwrap();
    let pixels = scaled_loaded.channel(0).pixels();
    // The 2.5 cancels: 17.5 / (2.5 × 65535) = 7/65535 = 1.0681315e-4, and
    // 10 / (2.5 × 65535) = 4/65535 = 6.1036087e-5.
    assert_close!(pixels[0], 1.068_131_5e-4, 1e-9, "{pixels:?}");
    assert_close!(pixels[1], 6.103_609e-5, 1e-9, "{pixels:?}");
    assert_eq!(pixels[2], 0.0);

    // The headline case: the FITS unsigned convention (BZERO = 2¹⁵) lands exactly on [0, 1].
    let size = Size2us::new(5usize, 1usize);
    let raw = [0u16, 16384, 32768, 49152, 65535];
    let image = Image::from_u16(vec![size.width, size.height], &raw).unwrap();

    let loaded = write_and_load("uint16", &image).unwrap();
    // |BSCALE| × (2¹⁶ − 1) = 65535, recorded so a later stage can ask what one sample is worth.
    assert_eq!(loaded.metadata.derived.domain().unwrap().scale, 65_535.0);
    let pixels = loaded.channel(0).pixels();
    assert_eq!(pixels[0], 0.0);
    assert_eq!(pixels[4], 1.0);
    for (index, &value) in raw.iter().enumerate() {
        let expected = f32::from(value) / 65_535.0;
        assert_close!(pixels[index], expected, 1e-7, "sample {index}: {pixels:?}");
    }
}

/// A caller can state the scale of a float FITS the header does not describe.
///
/// The one input whose domain the header may not settle. `Auto` reads `DATAMAX` and is right when
/// the file declares one; `FullScale` is the answer for an ADU frame that does not, which no header
/// rule can identify and which the decoder must not guess at from the pixels — a divisor read off
/// each frame's own content differs between frames, and the combine rejects a set for that.
#[test]
fn a_float_fits_scale_can_be_declared_when_the_header_does_not() {
    let pixels = vec![0.0f32, 16_384.0, 32_768.0, 65_535.0];
    let image = Image::new(vec![4, 1], pixels.clone()).unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let path = write_with_header(
        dir.join("float32_declared_scale.fits"),
        &image,
        &Header::new(),
    );

    let with_scale = |scale| LoadContext {
        fits: FitsLoadOptions {
            float_scale: scale,
            ..Default::default()
        },
        ..LoadContext::default()
    };

    // No DATAMAX, so `Auto` takes the samples as normalized, and they reach 65535: refused, which is
    // the case the declaration exists for.
    assert_eq!(
        fits_refusal(decode::load_linear(&path, &with_scale(FitsFloatScale::Auto)).unwrap_err()),
        FitsRefusal::UndeclaredFloatScale { maximum: 65_535.0 }
    );

    // Declared: divided, and the span recorded so a later stage can compare it against another
    // frame's.
    let declared =
        decode::load_linear(&path, &with_scale(FitsFloatScale::FullScale(65_535.0))).unwrap();
    assert_eq!(
        declared.channel(0).pixels(),
        &[0.0, 0.250_003_8, 0.500_007_6, 1.0]
    );
    assert_eq!(declared.metadata.derived.domain().unwrap().scale, 65_535.0);

    // `Normalized` refuses the header's evidence, for a DATAMAX that describes the sensor rather
    // than the samples.
    let mut declaring = Header::new();
    declaring.set("DATAMAX", 65_535.0).unwrap();
    let declaring_path = write_with_header(
        dir.join("float32_datamax_overridden.fits"),
        &image,
        &declaring,
    );
    let forced =
        decode::load_linear(&declaring_path, &with_scale(FitsFloatScale::Normalized)).unwrap();
    assert_eq!(forced.channel(0).pixels(), &*pixels);

    // A scale that would invert or erase the samples is rejected rather than applied.
    for bad in [0.0, -1.0, f32::NAN] {
        let reason = fits_refusal(
            decode::load_linear(&path, &with_scale(FitsFloatScale::FullScale(bad))).unwrap_err(),
        );
        assert!(
            matches!(reason, FitsRefusal::DeclaredFullScale { scale } if scale.to_bits() == bad.to_bits()),
            "full scale {bad}: {reason:?}"
        );
    }
}

/// σ describes one ADC step, so it is only comparable to the samples once it is divided by the same
/// span they were. The step is read off the stored integers: 12-bit data written left-justified
/// into BITPIX 16 steps by 16 stored units, and the container's step of one
/// would understate σ 16×.
#[test]
fn fits_quantization_sigma_follows_the_adc_step_into_the_normalized_domain() {
    let mut header = Header::new();
    header.set("BAYERPAT", "RGGB").unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let sigma_of = |name: &str, raw: &[u16]| {
        let image = Image::from_u16(vec![raw.len(), 1], raw).unwrap();
        let path = write_with_header(dir.join(format!("{name}.fits")), &image, &header);
        decode::load_cfa(&path, &LoadContext::default())
            .unwrap()
            .metadata
            .derived
            .quantization_sigma()
            .unwrap()
    };
    // Full 16-bit data: 32767 is odd, so no trailing zero is shared and the step is one ADU. The
    // divisor is 65535, so σ = (1/√12) / 65535 = 0.28867513 / 65535 = 4.4049001e-6.
    let full = sigma_of("cfa_uint16_full", &[0, 16_384, 32_768, 65_535]);
    assert_close!(full, 4.404_9e-6, 1e-11, "{full}");
    // 12-bit samples shifted left by four: every stored value (v − 32768 under BZERO) is a
    // multiple of 16, so the step is 16 ADU and σ = 16 × 4.4049001e-6 = 7.0478402e-5.
    let shifted = sigma_of("cfa_uint16_12bit", &[0, 16, 65_280, 32_768]);
    assert_close!(shifted, 7.047_840_2e-5, 1e-10, "{shifted}");
    // A frame of zeros shares every trailing zero, which says nothing: the container's step stands.
    let zeros = sigma_of("cfa_uint16_zeros", &[32_768; 4]);
    assert_close!(zeros, 4.404_9e-6, 1e-11, "{zeros}");
}

#[test]
fn fits_float_samples_are_normalized_only_when_datamax_declares_them_adu() {
    // A float BITPIX declares no full scale, so DATAMAX is the only evidence. The decision is
    // taken from the header alone — never from the pixels, which would give each frame its own
    // divisor and break the commensurability the division exists to establish. These three files
    // hold *identical* samples and differ only in their header.
    let pixels = vec![-5.0f32, 0.0, 0.5, 65_535.0];
    let image = Image::new(vec![4, 1], pixels.clone()).unwrap();

    // No DATAMAX: taken as already normalized, which these samples are not. The decode scans them,
    // and a maximum of 65535 past Siril's threshold of 10 is refused rather than loaded with its
    // saturation level at 0.95 ADU. The caller can still say they are normalized.
    assert_eq!(
        fits_refusal(write_and_load("float32_no_datamax", &image).unwrap_err()),
        FitsRefusal::UndeclaredFloatScale { maximum: 65_535.0 }
    );
    let dir = TempDir::new("lumos-fits-roundtrip");
    let bare_path = write_with_header(dir.join("float32_bare.fits"), &image, &Header::new());
    let normalized_options = LoadContext {
        fits: FitsLoadOptions {
            float_scale: FitsFloatScale::Normalized,
            ..Default::default()
        },
        ..LoadContext::default()
    };
    let bare = decode::load_linear(&bare_path, &normalized_options).unwrap();
    assert_eq!(bare.channel(0).pixels(), &*pixels);

    // DATAMAX ≈ 1: a normalized frame saying so. Left alone — and refused when its samples
    // contradict it.
    let mut normalized_header = Header::new();
    normalized_header.set("DATAMAX", 1.0).unwrap();
    let contradicted = write_with_header(
        dir.join("float32_datamax_1_adu.fits"),
        &image,
        &normalized_header,
    );
    assert!(decode::load_linear(&contradicted, &LoadContext::default()).is_err());
    let unit_pixels = vec![-0.25f32, 0.0, 0.5, 1.0];
    let unit_image = Image::new(vec![4, 1], unit_pixels.clone()).unwrap();
    let normalized_path = write_with_header(
        dir.join("float32_datamax_1.fits"),
        &unit_image,
        &normalized_header,
    );
    let normalized = decode::load_linear(&normalized_path, &LoadContext::default()).unwrap();
    assert_eq!(normalized.channel(0).pixels(), &*unit_pixels);
    assert_eq!(normalized.metadata.header.data_max, Some(1.0));

    // DATAMAX = 65535: a saturation level far above unity, so the samples are ADU and divide by
    // the 16-bit full scale. -5/65535 = -7.629511e-5, 0.5/65535 = 7.629511e-6, 65535/65535 = 1.
    let mut adu_header = Header::new();
    adu_header.set("DATAMAX", 65_535.0).unwrap();
    let adu_path = write_with_header(dir.join("float32_datamax_adu.fits"), &image, &adu_header);
    let adu = decode::load_linear(&adu_path, &LoadContext::default()).unwrap();
    let decoded = adu.channel(0).pixels();
    assert_close!(decoded[0], -7.629_511e-5, 1e-9, "{decoded:?}");
    assert_eq!(decoded[1], 0.0);
    assert_close!(decoded[2], 7.629_511e-6, 1e-10, "{decoded:?}");
    assert_eq!(decoded[3], 1.0);
    // DATAMAX follows the samples, so the round-trip is stable: saving and reloading this frame
    // sees DATAMAX = 1 and leaves it alone rather than dividing a second time.
    assert_eq!(adu.metadata.header.data_max, Some(1.0));

    // The two frames hold the same ADU data and were divided by spans 65535 apart, which is exactly
    // the mismatch a stack has to be able to detect. The sample domain is what makes it detectable
    // — both frames otherwise present as `FitsNormalized` and compare equal on every other axis.
    let bare_domain = bare.metadata.derived.domain().unwrap();
    let adu_domain = adu.metadata.derived.domain().cloned().unwrap();
    assert_eq!(bare_domain.scale, 1.0);
    assert_eq!(adu_domain.scale, 65_535.0);
    assert_eq!(bare_domain.origin, ScaleOrigin::Assumed);
    assert_eq!(bare_domain.conversion_to(&adu_domain), None);

    // The path loaders refuse the set too, before combining a byte; the same two files on one
    // span stack.
    let stack_paths = |paths: &[&Path]| {
        (StackConfig {
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        })
        .stack_paths(
            paths,
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        )
    };
    assert!(matches!(
        stack_paths(&[&normalized_path, &adu_path]),
        Err(StackError::Admission(
            AdmissionError::SampleDomainMismatch {
                index: 1,
                reference_index: 0,
                ..
            }
        ))
    ));
    assert!(stack_paths(&[&normalized_path, &normalized_path]).is_ok());
}

/// A set whose second frame disagrees with the first — on sample domain, row order or CFA
/// pattern — is refused by both load tiers before the third frame decodes: the third path names
/// no file, so reaching it would report a load error instead. One worker thread keeps the order
/// fixed.
#[test]
fn a_mismatched_frame_set_stops_before_the_third_frame_decodes() {
    use crate::combine::error::StackError;
    use crate::combine::frame_cache::FrameCache;
    use crate::combine::normalization::Normalization;
    use crate::ingest::ingest_config::IngestConfig;

    let dir = TempDir::new("lumos-frame-set");
    let image = Image::new(vec![4, 1], vec![0.0f32, 0.25, 0.5, 1.0]).unwrap();
    let linear = |name: &str, keywords: &[(&str, &str)], datamax: f64| {
        let mut header = Header::new();
        header.set("DATAMAX", datamax).unwrap();
        for (keyword, value) in keywords {
            header.set(keyword, *value).unwrap();
        }
        write_with_header(dir.join(format!("{name}.fits")), &image, &header)
    };
    let mosaic = |name: &str, pattern: CfaPattern| {
        let path = dir.join(format!("{name}.fits"));
        writer::cfa_fits(
            &make_cfa(Size2us::new(4, 2), vec![0.5; 8], CfaType::Bayer(pattern)),
            &path,
        )
        .unwrap();
        path
    };
    let reference = linear("reference", &[], 1.0);
    let other_span = linear("other_span", &[], 65_535.0);
    let mirrored = linear("mirrored", &[("ROWORDER", "BOTTOM-UP")], 1.0);
    let rggb = mosaic("rggb", CfaPattern::Rggb);
    let bggr = mosaic("bggr", CfaPattern::Bggr);
    let never_decoded = dir.join("never_decoded.fits");

    let single_thread = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    for available_memory in [1 << 30, 1] {
        let ingest = IngestConfig::with_cache_dir(dir.join("cache"));
        let config = StackConfig {
            normalization: Normalization::None,
            weighting: Weighting::Equal,
            ..StackConfig::light()
        };
        let memory = RunMemory::new(1 << 30, Some(available_memory));
        let linear_set = |second: &Path| {
            single_thread.install(|| {
                FrameCache::from_paths(
                    &[reference.as_path(), second, never_decoded.as_path()],
                    PlannedCombine {
                        normalization: config.normalization,
                        quality: config.quality,
                    },
                    IngestRun::planned(memory, &ingest),
                )
            })
        };
        let tier = if available_memory == 1 {
            "disk"
        } else {
            "memory"
        };
        assert!(
            matches!(
                linear_set(&other_span),
                Err(StackError::Admission(
                    AdmissionError::SampleDomainMismatch {
                        index: 1,
                        reference_index: 0,
                        ..
                    }
                ))
            ),
            "{tier}"
        );
        assert!(
            matches!(
                linear_set(&mirrored),
                Err(StackError::Admission(AdmissionError::RowOrderMismatch {
                    index: 1,
                    reference_index: 0,
                    ..
                }))
            ),
            "{tier}"
        );
        let cfa_set = single_thread.install(|| {
            FrameCache::from_cfa_paths(
                SteppedPaths {
                    paths: &[rggb.as_path(), bggr.as_path(), never_decoded.as_path()],
                    step: &None::<Infallible>,
                },
                PlannedCombine {
                    normalization: config.normalization,
                    quality: config.quality,
                },
                IngestRun::planned(memory, &ingest),
            )
        });
        assert!(
            matches!(
                cfa_set,
                Err(StackError::Admission(AdmissionError::CfaPatternMismatch {
                    index: 1,
                    reference_index: 0,
                    actual: Some(CfaType::Bayer(CfaPattern::Bggr)),
                    expected: Some(CfaType::Bayer(CfaPattern::Rggb)),
                }))
            ),
            "{tier}"
        );
        // The same set with a matching second frame does reach the third, which fails: the disk
        // tier reads the file's identity before decoding it, the memory tier decodes it.
        let reached = linear_set(&reference);
        assert!(
            matches!(
                reached,
                Err(StackError::ImageLoad(_) | StackError::FrameStore(_))
            ),
            "{tier}: {reached:?}"
        );
    }
}

#[test]
fn fits_bunit_travels_with_the_samples_and_separates_frames_the_span_cannot() {
    // Two float frames holding identical samples, divided by the same span, differing only in the
    // quantity BUNIT says those samples measure. Nothing else in the pipeline can tell them apart:
    // both are `FitsNormalized` at scale 1, so without the unit they would stack as if a surface
    // brightness and a count rate were the same measurement.
    let image = Image::new(vec![2, 1], vec![0.25f32, 0.5]).unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let load = |name: &str, bunit: &str| {
        let mut header = Header::new();
        header.set("BUNIT", bunit).unwrap();
        let path = write_with_header(dir.join(format!("{name}.fits")), &image, &header);
        decode::load_linear(&path, &LoadContext::default())
            .unwrap()
            .metadata
            .derived
            .domain()
            .cloned()
            .unwrap()
    };

    let jansky = load("float32_bunit_jy", "Jy/beam");
    let counts = load("float32_bunit_counts", "count/s");
    assert_eq!(jansky.scale, counts.scale, "the span must not be the axis");
    assert_eq!(jansky.unit.as_deref(), Some("Jy/beam"));
    assert_eq!(counts.unit.as_deref(), Some("count/s"));
    assert_eq!(jansky.conversion_to(&counts), None);

    // Trailing blanks are not significant in a FITS string, so padding is not a mismatch...
    let padded = load("float32_bunit_padded", "Jy/beam   ");
    assert_eq!(padded.unit.as_deref(), Some("Jy/beam"));
    assert_eq!(padded.conversion_to(&jansky), Some(DomainMap::IDENTITY));

    // ...and an all-blank BUNIT states no unit rather than an empty one, which leaves the frame
    // comparable to anything on the same span instead of disagreeing with all of them.
    let blank = load("float32_bunit_blank", "   ");
    assert_eq!(blank.unit, None);
    assert_eq!(blank.conversion_to(&jansky), Some(DomainMap::IDENTITY));
    assert_eq!(blank.conversion_to(&counts), Some(DomainMap::IDENTITY));

    // Case is part of the unit: mega- and milli-jansky per steradian are 10^9 apart, and folding
    // case here would hide exactly the mismatch this check exists to catch.
    let mega = load("float32_bunit_mega", "MJy/sr");
    let milli = load("float32_bunit_milli", "mJy/sr");
    assert_eq!(mega.conversion_to(&milli), None);
}

#[test]
fn fits_nulls_are_carried_as_a_mask_rather_than_failing_the_load() {
    // The standard's own undefined-value flag, both halves of it. A float BITPIX declares no BLANK
    // — IEEE NaN *is* the blank (FITS 4.0 §6.3, NOST agreement) — and an integer one names the
    // stored value that means "no measurement". Both must open.
    //
    // The same logical image either way: finite samples 1, 2, 4, 8, 16 ADU with the third pixel
    // undefined. Written once as float and once as uint16, they must produce the same mask.
    let float_pixels = vec![1.0f32, 2.0, f32::NAN, 4.0, 8.0, 16.0];
    let float_image = Image::new(vec![3, 2], float_pixels).unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let float_path = write_with_header(dir.join("float32_null.fits"), &float_image, &Header::new());
    // ADU with no declared scale, which `Auto` refuses; the caller says how to read them.
    let float_context = LoadContext {
        fits: FitsLoadOptions {
            float_scale: FitsFloatScale::Normalized,
            ..Default::default()
        },
        ..LoadContext::default()
    };
    let float = decode::load_linear(&float_path, &float_context).unwrap();

    // BITPIX = 16 with BSCALE/BZERO identity, and -32768 declared as BLANK. fits-well maps that
    // stored value to NaN, which is the same thing the float file handed over.
    let integer_image = Image::new_scaled(
        vec![3, 2],
        vec![1i16, 2, -32_768, 4, 8, 16],
        Scaling {
            bscale: 1.0,
            bzero: 0.0,
            blank: Some(-32_768),
        },
    )
    .unwrap();
    let integer_path =
        write_with_header(dir.join("int16_blank.fits"), &integer_image, &Header::new());
    let integer = decode::load_linear(&integer_path, &LoadContext::default()).unwrap();

    for (name, image) in [("float", &float), ("integer", &integer)] {
        let nulls = image
            .flags()
            .unwrap_or_else(|| panic!("{name} frame must carry a mask"));
        assert_eq!(nulls.count(QualityFlags::NO_DATA), 1, "{name}");
        for index in 0..6 {
            assert_eq!(
                nulls.mask_of(QualityFlags::NO_DATA).get(index),
                index == 2,
                "{name} index {index}"
            );
        }
    }

    // The null's sample is the median of the finite ones, not a hole. The float file is taken as
    // already normalized (no DATAMAX), so its finite samples are 1, 2, 4, 8, 16 and the median is
    // the middle one, 4.
    let filled = float.channel(0).pixels();
    assert_eq!(filled, &[1.0, 2.0, 4.0, 4.0, 8.0, 16.0]);
    // Every other sample is untouched, which is what makes the fill a substitution rather than a
    // rescale of the frame.
    assert_eq!(filled[0], 1.0);
    assert_eq!(filled[5], 16.0);

    // The integer file divides by |1| × (2¹⁶ − 1), so its median is 4/65535 — the same fill rule
    // applied after the same division the samples took, not before it.
    let integer_filled = integer.channel(0).pixels();
    let expected = 4.0 / 65_535.0;
    assert_close!(integer_filled[2], expected, 1e-9, "{integer_filled:?}");

    // And the policy that refuses them still does, naming how many and where the first one is.
    let strict = LoadContext {
        nulls: NullPolicy::Reject,
        ..float_context.clone()
    };
    let refused = decode::load_linear(&float_path, &strict).unwrap_err();
    assert!(
        matches!(
            refused,
            ImageError::NullSamples {
                count: 1,
                first_index: 2,
                ..
            }
        ),
        "{refused:?}"
    );

    // Every entry that reads files takes its FITS policy from its config: the strict one refuses
    // the frame in a stack and in a drizzle, and the default one stacks it.
    let paths = [float_path.clone(), float_path.clone()];
    let config = StackConfig {
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    let ingest = |context: &LoadContext| IngestConfig {
        fits: context.fits.clone(),
        nulls: context.nulls,
        ..IngestConfig::with_cache_dir(dir.join("cache"))
    };
    let stacked = config.stack_paths(
        &paths,
        &ingest(&float_context),
        ProgressMonitor::unattended(),
    );
    assert!(stacked.is_ok(), "{stacked:?}");
    let refused = config.stack_paths(&paths, &ingest(&strict), ProgressMonitor::unattended());
    assert!(
        matches!(
            refused,
            Err(StackError::ImageLoad(ImageError::NullSamples {
                count: 1,
                first_index: 2,
                ..
            }))
        ),
        "{refused:?}"
    );
    let refused = DrizzleConfig::default().drizzle_paths(
        vec![DrizzleFrame::new(
            float_path,
            WarpTransform::new(Transform::identity()),
        )],
        &ingest(&strict),
        ProgressMonitor::unattended(),
    );
    assert!(
        matches!(
            refused,
            Err(DrizzleError::Stack(StackError::ImageLoad(
                ImageError::NullSamples {
                    count: 1,
                    first_index: 2,
                    ..
                }
            )))
        ),
        "{refused:?}"
    );
}

#[test]
fn a_wholly_null_fits_image_loads_as_zero_with_every_pixel_masked() {
    // The degenerate end of the same rule: no finite sample to take a level from. The frame still
    // opens, and the mask — not the samples — is what says none of it is data.
    let image = Image::new(vec![2, 2], vec![f32::NAN; 4]).unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let path = write_with_header(dir.join("float32_all_null.fits"), &image, &Header::new());
    let loaded = decode::load_linear(&path, &LoadContext::default()).unwrap();

    assert_eq!(loaded.channel(0).pixels(), &[0.0; 4]);
    let nulls = loaded.flags().unwrap();
    assert_eq!(nulls.count(QualityFlags::NO_DATA), 4);
    assert!((0..4).all(|index| nulls.mask_of(QualityFlags::NO_DATA).get(index)));
}

#[test]
fn fits_datamax_follows_the_samples_into_the_normalized_domain() {
    // DATAMAX is a saturation level in the file's sample units, so it is divided by the same span
    // the samples were: it stays a threshold the decoded samples can be compared against, and it
    // still does not influence what those samples decode to.
    let image = Image::new(vec![4, 1], vec![-7i16, 0, 41, 300]).unwrap();
    let mut low_header = Header::new();
    low_header.set("DATAMAX", 100.0).unwrap();
    let mut high_header = Header::new();
    high_header.set("DATAMAX", 65_535.0).unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let low_path = write_with_header(dir.join("datamax_100.fits"), &image, &low_header);
    let high_path = write_with_header(dir.join("datamax_65535.fits"), &image, &high_header);

    let low = decode::load_linear(&low_path, &LoadContext::default()).unwrap();
    let high = decode::load_linear(&high_path, &LoadContext::default()).unwrap();

    // BITPIX = 16, BSCALE = 1 → divisor 65535, and the samples are identical either way.
    let pixels = low.channel(0).pixels();
    assert_eq!(high.channel(0).pixels(), pixels);
    assert_eq!(pixels[1], 0.0);
    for (index, physical) in [-7.0f32, 0.0, 41.0, 300.0].into_iter().enumerate() {
        let expected = physical / 65_535.0;
        assert_close!(pixels[index], expected, 1e-9, "sample {index}: {pixels:?}");
    }

    // 100 / 65535 = 1.5259e-3; 65535 / 65535 = 1 exactly.
    let low_max = low.metadata.header.data_max.unwrap();
    assert_close!(low_max, 1.525_902_2e-3, 1e-9, "{low_max}");
    assert_eq!(high.metadata.header.data_max, Some(1.0));
}

/// A mono sensor frame (`CFATYPE = 'MONO'`, which this crate writes) has no mosaic, so it loads as
/// linear data. A three-plane cube keeps the `BAYERPAT` of the sensor it was demosaiced from,
/// which describes none of its planes, so it loads too.
#[test]
fn mono_sensor_frames_and_stale_cube_patterns_load_as_linear() {
    let dir = TempDir::new("lumos-fits-roundtrip");
    let mut mono_header = Header::new();
    mono_header.set("CFATYPE", "MONO").unwrap();
    let mono = Image::new(vec![2, 2], vec![0.125f32, 0.25, 0.375, 0.5]).unwrap();
    let path = write_with_header(dir.join("mono_sensor.fits"), &mono, &mono_header);
    let loaded = LoadContext::default().linear(&path).unwrap();
    assert_eq!(loaded.channel(0).pixels(), &[0.125, 0.25, 0.375, 0.5]);
    assert_eq!(
        loaded.metadata.derived.provenance().unwrap().color,
        ColorProvenance::Monochrome
    );

    let mut stale = Header::new();
    stale.set("BAYERPAT", "RGGB").unwrap();
    let samples: Vec<f32> = (0..12).map(|i| i as f32 / 16.0).collect();
    let cube = Image::new(vec![2, 2, 3], samples.clone()).unwrap();
    let path = write_with_header(dir.join("stale_cube.fits"), &cube, &stale);
    let context = LoadContext {
        fits: FitsLoadOptions {
            cube: FitsCubeInterpretation::Rgb,
            ..FitsLoadOptions::default()
        },
        ..LoadContext::default()
    };
    let loaded = context.linear(&path).unwrap();
    for channel in 0..3 {
        assert_eq!(
            loaded.channel(channel).pixels(),
            &samples[channel * 4..(channel + 1) * 4]
        );
    }
}

#[test]
fn mosaic_fits_uses_the_cfa_calibration_route() {
    let size = Size2us::new(32usize, 32usize);
    let pattern = CfaType::Bayer(CfaPattern::Rggb);
    let target = [0.8f32, 0.5, 0.2];
    let dark_value = 0.1f32;
    let pixels: Vec<f32> = (0..size.height)
        .flat_map(|y| {
            (0..size.width).map(move |x| target[pattern.slot_at(Vec2us::new(x, y))] + dark_value)
        })
        .collect();
    let image = Image::new(vec![size.width, size.height], pixels.clone()).unwrap();
    let mut header = Header::new();
    header.set("BAYERPAT", "RGGB").unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let path = write_with_header(dir.join("bayer_cfa.fits"), &image, &header);

    assert!(matches!(
        LoadContext::default().linear(&path),
        Err(ImageError::ScientificInputRejected {
            reason: ScientificRefusal::MosaicFits,
            ..
        })
    ));
    let mut loaded = LoadContext::default().cfa(&path).unwrap();
    assert_eq!(loaded.data().pixels(), pixels);
    assert_eq!(loaded.cfa_type(), pattern);
    let cache_loaded = CfaDecoder::load(&path, &LoadContext::default()).unwrap();
    assert_eq!(cache_loaded.data(), loaded.data());
    assert_eq!(cache_loaded.cfa_type(), loaded.cfa_type());
    // The reader holds the whole frame as one chunk beside it: 1024 samples of 4 bytes as
    // stored, and as many swapped into words.
    assert_eq!(
        CfaDecoder::peek(&path, &LoadContext::default()),
        Some(FrameInfo {
            dimensions: ImageDimensions::new((size.width, size.height), ChannelLayout::Mono),
            cfa_type: pattern,
            decoder_bytes: 8192,
        })
    );

    let preview = LoadContext::default().preview(&path).unwrap();
    assert!(matches!(
        &preview.metadata.derived.provenance(),
        Some(crate::ImageProvenance {
            color: ColorProvenance::SensorRgb,
            demosaic: crate::DemosaicProvenance::LumosRcd,
            ..
        })
    ));
    assert!(
        matches!(
            LoadContext::default().preview(&path).unwrap().into_pixels(),
            PreviewPixels::Planes(_)
        ),
        "a demosaiced decode hands back its planes"
    );
    let preview: imaginarium::Image = preview.into();
    assert_eq!(preview.desc().color_format, ColorFormat::RGB_F32);
    let preview_pixels = bytemuck::cast_slice::<u8, f32>(preview.bytes());
    for y in 6..size.height - 6 {
        for x in 6..size.width - 6 {
            let channel = pattern.slot_at(Vec2us::new(x, y));
            assert_eq!(
                preview_pixels[(size.index_of(Vec2us::new(x, y))) * 3 + channel],
                target[channel] + dark_value
            );
        }
    }

    let dark = make_cfa(size, vec![dark_value; size.pixel_count()], pattern);
    let masters = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark),
            flat: None,
            bias: None,
            flat_dark: None,
        },
        SigmaThreshold::new(5.0).unwrap(),
        &CancelToken::never(),
    )
    .unwrap();
    let mut equivalent = make_cfa(size, pixels, pattern);
    masters.calibrate(&mut loaded).unwrap();
    masters.calibrate(&mut equivalent).unwrap();
    assert_eq!(loaded.data(), equivalent.data());

    let demosaiced = loaded.demosaic_measured(MarkesteijnPasses::One);
    let equivalent_demosaiced = equivalent.demosaic_measured(MarkesteijnPasses::One);
    for channel in 0..3 {
        assert_eq!(
            demosaiced.channel(channel),
            equivalent_demosaiced.channel(channel)
        );
    }
    assert!(matches!(
        demosaiced.metadata.derived.provenance(),
        Some(crate::ImageProvenance {
            source: crate::SourceProvenance::Fits(crate::FitsTransferProvenance {
                bscale: 1.0,
                bzero: 0.0,
                ..
            },),
            color: ColorProvenance::SensorRgb,
            demosaic: crate::DemosaicProvenance::LumosRcd,
            clipped: false,
            ..
        })
    ));
    // A float FITS carries no declared full scale, so it is taken as already normalized and its
    // samples pass through undivided.
    assert_eq!(demosaiced.metadata.derived.domain().unwrap().scale, 1.0);
    for y in 6..size.height - 6 {
        for x in 6..size.width - 6 {
            let channel = pattern.slot_at(Vec2us::new(x, y));
            let expected = (target[channel] + dark_value) - dark_value;
            assert_eq!(
                demosaiced.channel(channel)[size.index_of(Vec2us::new(x, y))],
                expected
            );
        }
    }
}

#[test]
fn fits_nulls_of_every_non_finite_kind_are_summarized_together() {
    // NaN is the standard's flag, but ±inf reaches the same place: a sample the file declares that
    // no arithmetic downstream can use. All three count, and all three are masked.
    let size = Size2us::new(4usize, 4usize);
    let mut pixels = vec![0.3f32; size.pixel_count()];
    pixels[0] = f32::NAN;
    pixels[5] = f32::INFINITY;
    pixels[10] = f32::NEG_INFINITY;
    let image = Image::new(vec![size.width, size.height], pixels).unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let path = write_with_header(dir.join("nan_inf.fits"), &image, &Header::new());

    let masked = decode::load_linear(&path, &LoadContext::default()).unwrap();
    let nulls = masked.flags().unwrap();
    assert_eq!(nulls.count(QualityFlags::NO_DATA), 3);
    for index in 0..size.pixel_count() {
        assert_eq!(
            nulls.mask_of(QualityFlags::NO_DATA).get(index),
            matches!(index, 0 | 5 | 10),
            "index {index}"
        );
    }
    // Thirteen finite samples, all 0.3, so the median they fill with is 0.3 and the frame comes out
    // uniform — the fill is invisible in the samples, which is exactly why the mask has to exist.
    assert_eq!(masked.channel(0).pixels(), &[0.3f32; 16]);
}

#[test]
fn demosaic_uniform_bayer_recovers_color() {
    let size = Size2us::new(32usize, 32usize);
    let rgb = [0.8f32, 0.5, 0.2]; // R, G, B
    let cfa = CfaType::Bayer(CfaPattern::Rggb);

    // Sample each Bayer site from the (uniform) true color.
    let mut mosaic = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        for x in 0..size.width {
            mosaic[size.index_of(Vec2us::new(x, y))] = rgb[cfa.slot_at(Vec2us::new(x, y))];
        }
    }
    let image = make_cfa(size, mosaic, cfa).demosaic_measured(MarkesteijnPasses::One);

    // A uniform color must demosaic back to that color. RCD is gradient-based, so a perfectly
    // flat field is a degenerate (zero-gradient) input with a few ratio artifacts — but recovery
    // must be *unbiased*: the interior mean of every channel matches the true color, and the
    // typical pixel is close (median deviation small).
    let channels = [
        image.channel(0).pixels(),
        image.channel(1).pixels(),
        image.channel(2).pixels(),
    ];
    for (ch, &true_c) in channels.iter().zip(&rgb) {
        let mut devs: Vec<f32> = Vec::new();
        let mut sum = 0.0f64;
        for y in 6..size.height - 6 {
            for x in 6..size.width - 6 {
                let v = ch[size.index_of(Vec2us::new(x, y))];
                sum += f64::from(v);
                devs.push((v - true_c).abs());
            }
        }
        let mean = (sum / devs.len() as f64) as f32;
        devs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median_dev = devs[devs.len() / 2];
        assert_close!(
            mean,
            true_c,
            0.01,
            "interior mean {mean} should recover channel color {true_c}"
        );
        assert!(
            median_dev < 0.01,
            "the typical interior pixel should match {true_c}, median deviation {median_dev}"
        );
    }
}

#[test]
fn calibrated_demosaic_preserves_out_of_range_samples() {
    let size = Size2us::new(48usize, 48usize);

    for cfa in [
        CfaType::Bayer(CfaPattern::Rggb),
        CfaType::XTrans(XTRANS_PATTERN),
    ] {
        for expected in [-0.25f32, 1.25] {
            let image = make_cfa(size, vec![expected; size.pixel_count()], cfa)
                .demosaic_measured(MarkesteijnPasses::One);

            for channel in 0..3 {
                let pixels = image.channel(channel).pixels();
                assert!(pixels.iter().all(|sample| sample.is_finite()));
                for y in 8..size.height - 8 {
                    for x in 8..size.width - 8 {
                        let actual = pixels[size.index_of(Vec2us::new(x, y))];
                        assert_close!(
                            actual,
                            expected,
                            1e-4,
                            "{cfa:?} channel {channel} at ({x},{y}) changed uniform {expected} to {actual}"
                        );
                    }
                }
            }
        }
    }
}

/// A FITS `DATAMAX` lets the decoder flag saturation on the samples as decoded, before any
/// calibration moves them. DATAMAX = 4000 ADU on a uint16 frame: the level is 0.95 × 4000 = 3800,
/// so 3800 and 4000 are flagged and 3799 is not. Without DATAMAX nothing is flagged, and the image
/// says so, which leaves detection to test the samples itself.
#[test]
fn a_fits_datamax_flags_saturation_at_decode() {
    let dir = TempDir::new("lumos-fits-roundtrip");
    let image = Image::from_u16(vec![4, 1], &[100, 3799, 3800, 4000]).unwrap();
    let mut header = Header::new();
    header.set("DATAMAX", 4000.0).unwrap();
    let path = write_with_header(dir.join("uint16_datamax.fits"), &image, &header);
    let loaded = decode::load_linear(&path, &LoadContext::default()).unwrap();
    assert!(loaded.metadata.derived.saturation_flagged());
    let flags = loaded.flags().unwrap();
    let saturated: Vec<bool> = (0..4)
        .map(|index| flags.at(index).intersects(QualityFlags::SATURATED))
        .collect();
    assert_eq!(saturated, [false, false, true, true]);

    let path = write_with_header(dir.join("uint16_bare.fits"), &image, &Header::new());
    let bare = decode::load_linear(&path, &LoadContext::default()).unwrap();
    assert!(!bare.metadata.derived.saturation_flagged());
    assert!(bare.flags().is_none());
}
