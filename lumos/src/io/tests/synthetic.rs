//! Load/decode round-trip tests on synthetic frames.
//!
//! `fits-well` ships a `FitsWriter`, so a synthetic FITS can be written to bytes and read back
//! through the real FITS decode — exercising sample-type selection, the unsigned-via-BZERO
//! convention, the division of integer samples into the `[0, 1]` domain (and the float path's
//! exemption from it), and both halves of the null convention. The demosaic path is exercised by
//! building mosaics from known colors and demosaicing them back.

use std::path::Path;

use common::{CancelToken, TempDir, fs};
use fits_well::error::ValueKind;
use fits_well::header::{Header, RecordName, keywords};
use fits_well::image::{Image, Scaling};
use fits_well::{FitsError, FitsWriter};
use imaginarium::{ChannelLayout, PlanarDesc};

use crate::calibration::calibration_masters::CalibrationMasters;
use crate::calibration::calibration_set::CalibrationSet;
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::combine::error::StackError;
use crate::combine::normalization::Normalization;
use crate::combine::stack_config::StackConfig;
use crate::combine::stack_config::weighting::Weighting;
use crate::drizzle::drizzle_config::DrizzleConfig;
use crate::drizzle::drizzle_frame::DrizzleFrame;
use crate::drizzle::error::DrizzleError;
use crate::frame::frame_info::{DecoderBytes, FrameInfo};
use crate::image::cfa::bayer_pattern::BayerPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::demosaic::xtrans::markesteijn::MarkesteijnPasses;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::flat_gain::shared_gains::SharedGains;
use crate::image::linear_image::LinearImage;
use crate::image::metadata::image_provenance::ColorProvenance;
use crate::image::metadata::sample_domain::{DomainMap, ScaleOrigin};
use crate::ingest::error::AdmissionError;
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::assertions::assert_close;
use crate::internals::cfa::make_cfa;
use crate::internals::fits::{fits_bytes, fits_refusal, load_cfa, load_linear, write_fits};
use crate::io::decoder::cfa_decoder::{CfaDecoder, CfaRead};
use crate::io::decoder::{Decoder, Peek};
use crate::io::error::{ImageError, ScientificRefusal};
use crate::io::fits::error::FitsLoadError;
use crate::io::fits::error::FitsRefusal;
use crate::io::fits::fits_load_config::{FitsCubeInterpretation, FitsFloatScale, FitsLoadConfig};
use crate::io::load_ctx::LoadCtx;
use crate::io::null_policy::{NullPolicy, NullSamples};
use crate::progress::progress_monitor::ProgressMonitor;
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;
use common::USize2;
use glam::USizeVec2;

/// `image` written as a FITS file and loaded back as a linear image.
fn write_and_load(image: &Image) -> Result<LinearImage, FitsLoadError> {
    load_linear(&fits_bytes(image.view(), None), &LoadCtx::default())
}

/// One sample of 0 under `header`, loaded as a linear image.
fn header_and_load(header: &Header) -> Result<LinearImage, FitsLoadError> {
    let mut writer = FitsWriter::new(Vec::new());
    writer.write_raw_hdu(header, &0.0f32.to_be_bytes()).unwrap();
    load_linear(&writer.into_inner(), &LoadCtx::default())
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
    let image = header_and_load(&mistyped).unwrap();
    assert_eq!(image.metadata.header.data_max, None);
    assert_eq!(image.metadata.header.iso, None);

    // A float image's DATAMAX is how its scale is decided, so there it fails the load.
    let mut mistyped_float_scale = one_pixel_header();
    mistyped_float_scale.set("DATAMAX", "not a real").unwrap();
    assert!(matches!(
        header_and_load(&mistyped_float_scale),
        Err(FitsLoadError::Fits(FitsError::ValueTypeMismatch {
            name,
            expected: ValueKind::Real,
        })) if name == RecordName::from(keywords::DATAMAX)
    ));

    let mut mistyped_row_order = one_pixel_header();
    mistyped_row_order.set("ROWORDER", 1).unwrap();
    assert!(matches!(
        header_and_load(&mistyped_row_order),
        Err(FitsLoadError::Fits(FitsError::ValueTypeMismatch {
            name,
            expected: ValueKind::Text,
        })) if name == RecordName::try_from("ROWORDER").unwrap()
    ));
}

#[test]
fn fits_float32_round_trips_pixels_and_order() {
    let size = USize2::new(32usize, 24usize);
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
        Scaling::IDENTITY,
    )
    .unwrap();

    let loaded = write_and_load(&image).unwrap();
    assert_eq!(loaded.width(), size.width);
    assert_eq!(loaded.height(), size.height);
    assert_eq!(loaded.channel_count(), 1);
    assert_eq!(loaded.channel(0).pixels(), pixels);
}

#[test]
fn fits_integer_samples_are_divided_by_the_span_their_header_declares() {
    // Every case divides by |BSCALE| × (2^bits − 1) — the span the header itself declares — so
    // frames from different integer widths and BSCALEs land in one comparable domain. The scale
    // is applied without an offset, so a signed frame keeps its own zero.

    // BITPIX = 16, BSCALE = 1: divisor 65535, physical -32768..=32767 → about [-0.5, 0.5].
    let signed = Image::new(
        vec![4, 1],
        vec![-32_768i16, -3, 0, 32_767],
        Scaling::IDENTITY,
    )
    .unwrap();
    let signed_loaded = write_and_load(&signed).unwrap();
    let pixels = signed_loaded.channel(0).pixels();
    assert_close!(pixels[0], -0.500_007_6, 1e-7, "{pixels:?}");
    assert_close!(pixels[1], -4.577_636_7e-5, 1e-9, "{pixels:?}");
    assert_eq!(pixels[2], 0.0);
    assert_close!(pixels[3], 0.499_992_37, 1e-7, "{pixels:?}");

    // BSCALE = -2.5 widens the declared span to 2.5 × 65535 = 163837.5, and the physical values
    // 17.5, 10, 0 divide by it. A negative BSCALE scales by its magnitude: the sign already lives
    // in the physical value fits-well produced.
    let scaled = Image::new(
        vec![3, 1],
        vec![-3i16, 0, 4],
        Scaling {
            bscale: -2.5,
            bzero: 10.0,
            blank: None,
        },
    )
    .unwrap();
    let scaled_loaded = write_and_load(&scaled).unwrap();
    let pixels = scaled_loaded.channel(0).pixels();
    // The 2.5 cancels: 17.5 / (2.5 × 65535) = 7/65535 = 1.0681315e-4, and
    // 10 / (2.5 × 65535) = 4/65535 = 6.1036087e-5.
    assert_close!(pixels[0], 1.068_131_5e-4, 1e-9, "{pixels:?}");
    assert_close!(pixels[1], 6.103_609e-5, 1e-9, "{pixels:?}");
    assert_eq!(pixels[2], 0.0);

    // The headline case: the FITS unsigned convention (BZERO = 2¹⁵) lands exactly on [0, 1].
    let size = USize2::new(5usize, 1usize);
    let raw = [0u16, 16384, 32768, 49152, 65535];
    let image = Image::from_u16(vec![size.width, size.height], &raw).unwrap();

    let loaded = write_and_load(&image).unwrap();
    // |BSCALE| × (2¹⁶ − 1) = 65535, recorded so a later stage can ask what one sample is worth.
    assert_eq!(loaded.metadata.derived.domain().unwrap().scale, 65_535.0);
    let pixels = loaded.channel(0).pixels();
    assert_eq!(pixels[0], 0.0);
    assert_eq!(pixels[4], 1.0);
    for (idx, &value) in raw.iter().enumerate() {
        let expected = f32::from(value) / 65_535.0;
        assert_close!(pixels[idx], expected, 1e-7, "sample {idx}: {pixels:?}");
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
    let image = Image::new(vec![4, 1], pixels.clone(), Scaling::IDENTITY).unwrap();
    let fits = fits_bytes(image.view(), Some(&Header::new()));

    let with_scale = |scale| LoadCtx {
        fits: FitsLoadConfig {
            float_scale: scale,
            ..Default::default()
        },
        ..LoadCtx::default()
    };

    // No DATAMAX, so `Auto` takes the samples as normalized, and they reach 65535: refused, which is
    // the case the declaration exists for.
    assert_eq!(
        fits_refusal(load_linear(&fits, &with_scale(FitsFloatScale::Auto)).unwrap_err()),
        FitsRefusal::UndeclaredFloatScale { maximum: 65_535.0 }
    );

    // Declared: divided, and the span recorded so a later stage can compare it against another
    // frame's.
    let declared = load_linear(&fits, &with_scale(FitsFloatScale::FullScale(65_535.0))).unwrap();
    assert_eq!(
        declared.channel(0).pixels(),
        &[0.0, 0.250_003_8, 0.500_007_6, 1.0]
    );
    assert_eq!(declared.metadata.derived.domain().unwrap().scale, 65_535.0);

    // `Normalized` refuses the header's evidence, for a DATAMAX that describes the sensor rather
    // than the samples.
    let mut declaring = Header::new();
    declaring.set("DATAMAX", 65_535.0).unwrap();
    let declaring_fits = fits_bytes(image.view(), Some(&declaring));
    let forced = load_linear(&declaring_fits, &with_scale(FitsFloatScale::Normalized)).unwrap();
    assert_eq!(forced.channel(0).pixels(), &*pixels);

    // A scale that would invert or erase the samples is rejected rather than applied.
    for bad in [0.0, -1.0, f32::NAN] {
        let reason = fits_refusal(
            load_linear(&fits, &with_scale(FitsFloatScale::FullScale(bad))).unwrap_err(),
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
    let sigma_of = |raw: &[u16]| {
        let image = Image::from_u16(vec![raw.len(), 1], raw).unwrap();
        let fits = fits_bytes(image.view(), Some(&header));
        load_cfa(&fits, &LoadCtx::default())
            .unwrap()
            .metadata
            .derived
            .quantization_sigma()
            .unwrap()
    };
    // Full 16-bit data: 32767 is odd, so no trailing zero is shared and the step is one ADU. The
    // divisor is 65535, so σ = (1/√12) / 65535 = 0.28867513 / 65535 = 4.4049001e-6.
    let full = sigma_of(&[0, 16_384, 32_768, 65_535]);
    assert_close!(full, 4.404_9e-6, 1e-11, "{full}");
    // 12-bit samples shifted left by four: every stored value (v − 32768 under BZERO) is a
    // multiple of 16, so the step is 16 ADU and σ = 16 × 4.4049001e-6 = 7.0478402e-5.
    let shifted = sigma_of(&[0, 16, 65_280, 32_768]);
    assert_close!(shifted, 7.047_840_2e-5, 1e-10, "{shifted}");
    // A frame of zeros shares every trailing zero, which says nothing: the container's step stands.
    let zeros = sigma_of(&[32_768; 4]);
    assert_close!(zeros, 4.404_9e-6, 1e-11, "{zeros}");
}

#[test]
fn fits_float_samples_are_normalized_only_when_datamax_declares_them_adu() {
    // A float BITPIX declares no full scale, so DATAMAX is the only evidence. The decision is
    // taken from the header alone — never from the pixels, which would give each frame its own
    // divisor and break the commensurability the division exists to establish. These three files
    // hold *identical* samples and differ only in their header.
    let pixels = vec![-5.0f32, 0.0, 0.5, 65_535.0];
    let image = Image::new(vec![4, 1], pixels.clone(), Scaling::IDENTITY).unwrap();

    // No DATAMAX: taken as already normalized, which these samples are not. The decode scans them,
    // and a maximum of 65535 past Siril's threshold of 10 is refused rather than loaded with its
    // saturation level at 0.95 ADU. The caller can still say they are normalized.
    assert_eq!(
        fits_refusal(write_and_load(&image).unwrap_err()),
        FitsRefusal::UndeclaredFloatScale { maximum: 65_535.0 }
    );
    let bare_fits = fits_bytes(image.view(), Some(&Header::new()));
    let normalized_options = LoadCtx {
        fits: FitsLoadConfig {
            float_scale: FitsFloatScale::Normalized,
            ..Default::default()
        },
        ..LoadCtx::default()
    };
    let bare = load_linear(&bare_fits, &normalized_options).unwrap();
    assert_eq!(bare.channel(0).pixels(), &*pixels);

    // DATAMAX ≈ 1: a normalized frame saying so. Left alone — and refused when its samples
    // contradict it.
    let mut normalized_header = Header::new();
    normalized_header.set("DATAMAX", 1.0).unwrap();
    let contradicted = fits_bytes(image.view(), Some(&normalized_header));
    assert!(load_linear(&contradicted, &LoadCtx::default()).is_err());
    let unit_pixels = vec![-0.25f32, 0.0, 0.5, 1.0];
    let unit_image = Image::new(vec![4, 1], unit_pixels.clone(), Scaling::IDENTITY).unwrap();
    let normalized_fits = fits_bytes(unit_image.view(), Some(&normalized_header));
    let normalized = load_linear(&normalized_fits, &LoadCtx::default()).unwrap();
    assert_eq!(normalized.channel(0).pixels(), &*unit_pixels);
    assert_eq!(normalized.metadata.header.data_max, Some(1.0));

    // DATAMAX = 65535: a saturation level far above unity, so the samples are ADU and divide by
    // the 16-bit full scale. -5/65535 = -7.629511e-5, 0.5/65535 = 7.629511e-6, 65535/65535 = 1.
    let mut adu_header = Header::new();
    adu_header.set("DATAMAX", 65_535.0).unwrap();
    let adu_fits = fits_bytes(image.view(), Some(&adu_header));
    let adu = load_linear(&adu_fits, &LoadCtx::default()).unwrap();
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
    let dir = TempDir::new("lumos-fits-roundtrip");
    let [normalized_path, adu_path] =
        [("normalized", &normalized_fits), ("adu", &adu_fits)].map(|(name, bytes)| {
            let path = dir.join(format!("{name}.fits"));
            fs::write(&path, bytes).unwrap();
            path
        });
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
                idx: 1,
                reference_idx: 0,
                ..
            }
        ))
    ));
    assert!(stack_paths(&[&normalized_path, &normalized_path]).is_ok());
}

#[test]
fn fits_bunit_travels_with_the_samples_and_separates_frames_the_span_cannot() {
    // Two float frames holding identical samples, divided by the same span, differing only in the
    // quantity BUNIT says those samples measure. Nothing else in the pipeline can tell them apart:
    // both are `FitsNormalized` at scale 1, so without the unit they would stack as if a surface
    // brightness and a count rate were the same measurement.
    let image = Image::new(vec![2, 1], vec![0.25f32, 0.5], Scaling::IDENTITY).unwrap();
    let load = |bunit: &str| {
        let mut header = Header::new();
        header.set("BUNIT", bunit).unwrap();
        let fits = fits_bytes(image.view(), Some(&header));
        load_linear(&fits, &LoadCtx::default())
            .unwrap()
            .metadata
            .derived
            .domain()
            .cloned()
            .unwrap()
    };

    let jansky = load("Jy/beam");
    let counts = load("count/s");
    assert_eq!(jansky.scale, counts.scale, "the span must not be the axis");
    assert_eq!(jansky.unit.as_deref(), Some("Jy/beam"));
    assert_eq!(counts.unit.as_deref(), Some("count/s"));
    assert_eq!(jansky.conversion_to(&counts), None);

    // Trailing blanks are not significant in a FITS string, so padding is not a mismatch...
    let padded = load("Jy/beam   ");
    assert_eq!(padded.unit.as_deref(), Some("Jy/beam"));
    assert_eq!(padded.conversion_to(&jansky), Some(DomainMap::IDENTITY));

    // ...and an all-blank BUNIT states no unit rather than an empty one, which leaves the frame
    // comparable to anything on the same span instead of disagreeing with all of them.
    let blank = load("   ");
    assert_eq!(blank.unit, None);
    assert_eq!(blank.conversion_to(&jansky), Some(DomainMap::IDENTITY));
    assert_eq!(blank.conversion_to(&counts), Some(DomainMap::IDENTITY));

    // Case is part of the unit: mega- and milli-jansky per steradian are 10^9 apart, and folding
    // case here would hide exactly the mismatch this check exists to catch.
    let mega = load("MJy/sr");
    let milli = load("mJy/sr");
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
    let float_image = Image::new(vec![3, 2], float_pixels, Scaling::IDENTITY).unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let float_fits = fits_bytes(float_image.view(), Some(&Header::new()));
    // ADU with no declared scale, which `Auto` refuses; the caller says how to read them.
    let float_ctx = LoadCtx {
        fits: FitsLoadConfig {
            float_scale: FitsFloatScale::Normalized,
            ..Default::default()
        },
        ..LoadCtx::default()
    };
    let float = load_linear(&float_fits, &float_ctx).unwrap();

    // BITPIX = 16 with BSCALE/BZERO identity, and -32768 declared as BLANK. fits-well maps that
    // stored value to NaN, which is the same thing the float file handed over.
    let integer_image = Image::new(
        vec![3, 2],
        vec![1i16, 2, -32_768, 4, 8, 16],
        Scaling {
            bscale: 1.0,
            bzero: 0.0,
            blank: Some(-32_768),
        },
    )
    .unwrap();
    let integer_fits = fits_bytes(integer_image.view(), Some(&Header::new()));
    let integer = load_linear(&integer_fits, &LoadCtx::default()).unwrap();

    for (name, image) in [("float", &float), ("integer", &integer)] {
        let nulls = image
            .flags()
            .unwrap_or_else(|| panic!("{name} frame must carry a mask"));
        assert_eq!(nulls.count(QualityFlags::NO_DATA), 1, "{name}");
        for idx in 0..6 {
            assert_eq!(
                nulls.mask_of(QualityFlags::NO_DATA).get(idx),
                idx == 2,
                "{name} index {idx}"
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
    let strict = LoadCtx {
        nulls: NullPolicy::Reject,
        ..float_ctx.clone()
    };
    let refused = load_linear(&float_fits, &strict).unwrap_err();
    assert!(
        matches!(
            refused,
            FitsLoadError::NullSamples(NullSamples {
                count: 1,
                first_idx: 2,
            })
        ),
        "{refused:?}"
    );

    // Every entry that reads files takes its FITS policy from its config: the strict one refuses
    // the frame in a stack and in a drizzle, and the default one stacks it.
    let float_path = dir.join("float32_null.fits");
    fs::write(&float_path, &float_fits).unwrap();
    let paths = [float_path.clone(), float_path.clone()];
    let config = StackConfig {
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    let ingest = |ctx: &LoadCtx| IngestConfig {
        fits: ctx.fits.clone(),
        nulls: ctx.nulls,
        ..IngestConfig::with_cache_dir(dir.join("cache"))
    };
    let stacked = config.stack_paths(&paths, &ingest(&float_ctx), ProgressMonitor::unattended());
    assert!(stacked.is_ok(), "{stacked:?}");
    let refused = config.stack_paths(&paths, &ingest(&strict), ProgressMonitor::unattended());
    assert!(
        matches!(
            refused,
            Err(StackError::ImageLoad(ImageError::NullSamples {
                count: 1,
                first_idx: 2,
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
                    first_idx: 2,
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
    let image = Image::new(vec![2, 2], vec![f32::NAN; 4], Scaling::IDENTITY).unwrap();
    let fits = fits_bytes(image.view(), Some(&Header::new()));
    let loaded = load_linear(&fits, &LoadCtx::default()).unwrap();

    assert_eq!(loaded.channel(0).pixels(), &[0.0; 4]);
    let nulls = loaded.flags().unwrap();
    assert_eq!(nulls.count(QualityFlags::NO_DATA), 4);
    assert!((0..4).all(|idx| nulls.mask_of(QualityFlags::NO_DATA).get(idx)));
}

/// A mono sensor frame (`CFATYPE = 'MONO'`, which this crate writes) has no mosaic, so it loads as
/// linear data. A three-plane cube keeps the `BAYERPAT` of the sensor it was demosaiced from,
/// which describes none of its planes, so it loads too.
#[test]
fn mono_sensor_frames_and_stale_cube_patterns_load_as_linear() {
    let mut mono_header = Header::new();
    mono_header.set("CFATYPE", "MONO").unwrap();
    let mono = Image::new(
        vec![2, 2],
        vec![0.125f32, 0.25, 0.375, 0.5],
        Scaling::IDENTITY,
    )
    .unwrap();
    let fits = fits_bytes(mono.view(), Some(&mono_header));
    let loaded = load_linear(&fits, &LoadCtx::default()).unwrap();
    assert_eq!(loaded.channel(0).pixels(), &[0.125, 0.25, 0.375, 0.5]);
    assert_eq!(
        loaded.metadata.derived.provenance().unwrap().color,
        ColorProvenance::Monochrome
    );

    let mut stale = Header::new();
    stale.set("BAYERPAT", "RGGB").unwrap();
    let samples: Vec<f32> = (0..12).map(|i| i as f32 / 16.0).collect();
    let cube = Image::new(vec![2, 2, 3], samples.clone(), Scaling::IDENTITY).unwrap();
    let fits = fits_bytes(cube.view(), Some(&stale));
    let ctx = LoadCtx {
        fits: FitsLoadConfig {
            cube: FitsCubeInterpretation::Rgb,
            ..FitsLoadConfig::default()
        },
        ..LoadCtx::default()
    };
    let loaded = load_linear(&fits, &ctx).unwrap();
    for channel in 0..3 {
        assert_eq!(
            loaded.channel(channel).pixels(),
            &samples[channel * 4..(channel + 1) * 4]
        );
    }
}

#[test]
fn mosaic_fits_uses_the_cfa_calibration_route() {
    let size = USize2::new(32usize, 32usize);
    let pattern = CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb));
    let target = [0.8f32, 0.5, 0.2];
    let dark_value = 0.1f32;
    let pixels: Vec<f32> = (0..size.height)
        .flat_map(|y| {
            (0..size.width).map(move |x| target[pattern.slot_at(USizeVec2::new(x, y))] + dark_value)
        })
        .collect();
    let image = Image::new(
        vec![size.width, size.height],
        pixels.clone(),
        Scaling::IDENTITY,
    )
    .unwrap();
    let mut header = Header::new();
    header.set("BAYERPAT", "RGGB").unwrap();
    let dir = TempDir::new("lumos-fits-roundtrip");
    let path = dir.join("bayer_cfa.fits");
    write_fits(&path, image.view(), Some(&header));

    assert!(matches!(
        LoadCtx::default().linear(&path),
        Err(ImageError::ScientificInputRejected {
            reason: ScientificRefusal::MosaicFits,
            ..
        })
    ));
    let mut loaded = LoadCtx::default().cfa(&path).unwrap();
    assert_eq!(loaded.data().pixels(), pixels);
    assert_eq!(loaded.cfa_type(), pattern);
    let cache_loaded =
        CfaDecoder::load(&path, &LoadCtx::default(), &SharedGains::default()).unwrap();
    assert_eq!(cache_loaded.data(), loaded.data());
    assert_eq!(cache_loaded.cfa_type(), loaded.cfa_type());
    // The decode reads the whole frame as one chunk beside it: 1024 samples of 4 bytes swapped
    // into words, and the reader's section shape, 8 B for each of the two axes. The checksum's
    // finish grows the reader's staging buffer from the header scan's 2 880 B block to the
    // two-block data unit, 2 880 B more, and holds the old block while it moves.
    let Peek { info, read } = CfaDecoder::peek(&path, &LoadCtx::default()).unwrap();
    assert_eq!(
        Some(info),
        Some(FrameInfo {
            desc: PlanarDesc::new((size.width, size.height), ChannelLayout::Mono),
            cfa_type: pattern,
            decoder: DecoderBytes {
                source: 0,
                beside: 4096 + 16 + 2 * 2880,
            },
            gain_bytes: 0,
        })
    );
    // The peek keeps the open file, which holds no tile index for a plain image, and the load
    // reads on from it: the path it is given is only what an error reports, so one that names no
    // file still loads the frame.
    let CfaRead::Fits(file) = read else {
        panic!("a FITS peek keeps its file")
    };
    assert_eq!(file.held_bytes(), 0);
    let from_peek = CfaDecoder::load_read(
        CfaRead::Fits(file),
        &dir.join("absent.fits"),
        &LoadCtx::default(),
        &SharedGains::default(),
    )
    .unwrap();
    assert_eq!(from_peek.data(), loaded.data());
    assert_eq!(from_peek.cfa_type(), pattern);

    let preview = LoadCtx::default().preview(&path).unwrap();
    assert!(matches!(
        &preview.metadata.derived.provenance(),
        Some(crate::ImageProvenance {
            color: ColorProvenance::SensorRgb,
            demosaic: crate::DemosaicProvenance::LumosRcd,
            ..
        })
    ));
    assert!(preview.is_rgb());
    for y in 6..size.height - 6 {
        for x in 6..size.width - 6 {
            let channel = pattern.slot_at(USizeVec2::new(x, y));
            assert_eq!(
                preview.channel(channel).pixels()[size.idx_of(USizeVec2::new(x, y))],
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
            let channel = pattern.slot_at(USizeVec2::new(x, y));
            let expected = (target[channel] + dark_value) - dark_value;
            assert_eq!(
                demosaiced.channel(channel)[size.idx_of(USizeVec2::new(x, y))],
                expected
            );
        }
    }
}

#[test]
fn fits_nulls_of_every_non_finite_kind_are_summarized_together() {
    // NaN is the standard's flag, but ±inf reaches the same place: a sample the file declares that
    // no arithmetic downstream can use. All three count, and all three are masked.
    let size = USize2::new(4usize, 4usize);
    let mut pixels = vec![0.3f32; size.pixel_count()];
    pixels[0] = f32::NAN;
    pixels[5] = f32::INFINITY;
    pixels[10] = f32::NEG_INFINITY;
    let image = Image::new(vec![size.width, size.height], pixels, Scaling::IDENTITY).unwrap();
    let fits = fits_bytes(image.view(), Some(&Header::new()));

    let masked = load_linear(&fits, &LoadCtx::default()).unwrap();
    let nulls = masked.flags().unwrap();
    assert_eq!(nulls.count(QualityFlags::NO_DATA), 3);
    for idx in 0..size.pixel_count() {
        assert_eq!(
            nulls.mask_of(QualityFlags::NO_DATA).get(idx),
            matches!(idx, 0 | 5 | 10),
            "index {idx}"
        );
    }
    // Thirteen finite samples, all 0.3, so the median they fill with is 0.3 and the frame comes out
    // uniform — the fill is invisible in the samples, which is exactly why the mask has to exist.
    assert_eq!(masked.channel(0).pixels(), &[0.3f32; 16]);
}
