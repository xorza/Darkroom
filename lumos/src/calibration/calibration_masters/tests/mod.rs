mod cosmic_ray_scratch;
mod dark_scale;
#[cfg(feature = "real-data")]
mod real_data;
mod synthetic;

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Mutex;

use common::{CancelToken, TempDir, fs};
use fits_well::FitsReader;
use fits_well::header::Header;
use fits_well::image::Bitpix;
use fits_well::io::ChecksumStatus;
use imaginarium::Buffer2;

use crate::calibration::calibration_masters::MasterBuild;
use crate::calibration::calibration_outcome::CalibrationOutcome;
use crate::calibration::dark_scale::{DarkScale, DarkScaleSource};
use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;
use crate::calibration::defect_map::{DefectMap, HotPixels};
use crate::calibration::error::MasterStackError;
use crate::calibration::error::{CalibrationError, DarkMismatch, MasterBuildError};
use crate::calibration::master_cache::{CachedMaster, MasterCache, MasterInputs};
use crate::calibration::master_stack::MasterStack;
use crate::calibration::prepared_flat::PreparedFlat;
use crate::calibration::subtraction::internals::subtract_bias;
use crate::calibration::subtractor::Subtractor;
use crate::combine::error::{StackConfigError, StackError};
use crate::combine::rejection::Rejection;
use crate::combine::stack_config::StackConfig;
use crate::combine::stack_config::combine::Combine;
use crate::combine::stack_config::combine_method::CombineMethod;
use crate::combine::stack_config::small_n::SmallN;
use crate::combine::stack_config::weighting::Weighting;
use crate::frame::capture_condition::CaptureCondition;
use crate::frame::error::ConditionMismatch;
use crate::image::calibration_state::CalibrationState;
use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::cfa::mosaic::Mosaic;
use crate::image::cfa::xtrans_pattern::XTransPattern;
use crate::image::dark_comparison::DarkComparison;
use crate::image::derived_facts::DerivedFacts;
use crate::image::observation_header::ObservationHeader;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::sample_domain::{Pedestal, SampleDomain, ScaleOrigin};
use crate::image::temperature_difference::TemperatureDifference;
use crate::image::temperature_source::TemperatureSource;
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::assertions::{assert_close, bits};
use crate::internals::cfa::{XTRANS_PATTERN, cfa_from_plane, constant_cfa, make_cfa};
use crate::internals::fits::rewrite_fits;
use crate::io::fits::metadata::{FORMAT_KEYWORD, VERSION_KEYWORD};
use crate::io::load_ctx::LoadCtx;
use crate::io::writer;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::noise::QUANTIZATION_SIGMA_PER_STEP;
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::{
    BundleError, CalibrationComponent, CalibrationMasters, CalibrationSet, ColorDefects,
    ColorShares, DefectSummary, DefectTableError, FitsRefusal, ImageError, ImageMetadata,
    MasterRole,
};

/// The bundle `images` builds, at the default defect threshold.
fn bundle(images: CalibrationSet<Option<CfaImage>>) -> CalibrationMasters {
    CalibrationMasters::from_images(images, SigmaThreshold::DEFAULT, &CancelToken::never()).unwrap()
}

#[test]
fn calibrating_a_calibrated_light_is_refused() {
    // A second calibrate() would subtract the dark / divide the flat twice. The record can come
    // from the file (`LUMCALB`, `LUMCALD`, `LUMCALF`), so it is refused as input, and the light is
    // left as it was.
    let mut images = CalibrationSet::default();
    *images.get_mut(MasterRole::Dark) =
        Some(constant_cfa(Size2us::new(4, 4), 0.125, CfaType::Mono));
    let masters = CalibrationMasters::from_images(
        images,
        SigmaThreshold::new(5.0).unwrap(),
        &CancelToken::never(),
    )
    .unwrap();
    let mut light = constant_cfa(Size2us::new(4, 4), 0.5, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();
    // The dark still holds the bias, so the light lost both additive parts.
    assert_eq!(
        light.metadata.derived.calibration(),
        CalibrationState::ADDITIVE
    );
    assert_eq!(light.data().pixels(), &[0.375; 16]);
    assert_eq!(
        masters.calibrate(&mut light),
        Err(CalibrationError::AlreadyCalibrated)
    );
    assert_eq!(light.data().pixels(), &[0.375; 16]);
}

/// A flat whose subtracted mean is not positive — a dark given as the flat, or a subtractor above
/// the flat's level — is refused when the set is built, per color for a CFA flat.
#[test]
fn a_flat_with_no_positive_mean_is_refused() {
    for (cfa_type, flat, subtractor, color) in [
        (CfaType::Mono, 0.0f32, None, None),
        (CfaType::Mono, 0.25, Some(0.5f32), None),
        (
            CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            0.25,
            Some(0.5),
            Some(CfaColor::Red),
        ),
    ] {
        let mut images = CalibrationSet::default();
        *images.get_mut(MasterRole::Flat) = Some(constant_cfa(Size2us::new(4, 4), flat, cfa_type));
        *images.get_mut(MasterRole::FlatDark) =
            subtractor.map(|level| constant_cfa(Size2us::new(4, 4), level, cfa_type));
        assert!(
            matches!(
                CalibrationMasters::from_images(images, SigmaThreshold::new(5.0).unwrap(), &CancelToken::never()),
                Err(CalibrationError::NonPositiveFlat { color: c }) if c == color
            ),
            "{cfa_type:?} flat {flat} minus {subtractor:?}"
        );
    }
}

/// A bundle whose masters span two sensor patterns is refused when it is assembled: the flat-dark
/// is subtracted from the flat pixel for pixel before any light is there to compare with.
#[test]
fn a_bundle_spanning_two_patterns_is_refused() {
    let images = CalibrationSet {
        flat: Some(constant_cfa(
            Size2us::new(2, 2),
            0.5,
            CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
        )),
        flat_dark: Some(constant_cfa(
            Size2us::new(2, 2),
            0.1,
            CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr)),
        )),
        ..CalibrationSet::default()
    };
    assert!(matches!(
        CalibrationMasters::from_images(images, SigmaThreshold::DEFAULT, &CancelToken::never()),
        Err(CalibrationError::CfaPatternMismatch {
            component: MasterRole::FlatDark,
            expected: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            master: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr)),
        })
    ));
}

fn masters_with_component(role: MasterRole, cfa_type: CfaType) -> CalibrationMasters {
    masters_with_sized_component(role, cfa_type, Size2us::new(2, 2))
}

fn masters_with_sized_component(
    role: MasterRole,
    cfa_type: CfaType,
    size: Size2us,
) -> CalibrationMasters {
    let master = constant_cfa(size, 1.0, cfa_type);
    let mut images = CalibrationSet::default();
    *images.get_mut(role) = Some(master);
    bundle(images)
}

#[test]
fn calibrate_rejects_mismatched_cfa_before_mutation() {
    #[derive(Debug)]
    struct Case {
        role: MasterRole,
        light: CfaType,
        master: CfaType,
        expected: CalibrationError,
    }

    // The same layout one row down is a different X-Trans phase.
    let xtrans_a = CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN));
    let mut shifted = XTRANS_PATTERN.rows();
    shifted.rotate_left(1);
    let xtrans_b = CfaType::Mosaic(Mosaic::XTrans(XTransPattern::new(shifted).unwrap()));
    let cases = [
        Case {
            role: MasterRole::Dark,
            light: CfaType::Mono,
            master: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            expected: CalibrationError::CfaPatternMismatch {
                component: MasterRole::Dark,
                expected: CfaType::Mono,
                master: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            },
        },
        Case {
            role: MasterRole::Flat,
            light: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            master: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr)),
            expected: CalibrationError::CfaPatternMismatch {
                component: MasterRole::Flat,
                expected: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
                master: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Bggr)),
            },
        },
        Case {
            role: MasterRole::Bias,
            light: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
            master: xtrans_a,
            expected: CalibrationError::CfaPatternMismatch {
                component: MasterRole::Bias,
                expected: CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb)),
                master: xtrans_a,
            },
        },
        Case {
            role: MasterRole::Dark,
            light: xtrans_a,
            master: xtrans_b,
            expected: CalibrationError::CfaPatternMismatch {
                component: MasterRole::Dark,
                expected: xtrans_a,
                master: xtrans_b,
            },
        },
    ];

    for case in cases {
        let masters = masters_with_component(case.role, case.master);
        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, case.light);
        let original_data = light.data().to_vec();

        assert_eq!(masters.calibrate(&mut light), Err(case.expected));
        assert_eq!(light.data().pixels(), original_data);
        assert_eq!(light.metadata.derived.calibration(), CalibrationState::NONE);
    }

    // A master whose pattern matches but whose extent does not is bad input too — unchecked, the
    // pass would index past it, which is not a report a caller can act on. Every
    // role a bundle keeps is covered because each is applied by a different operation; the
    // flat-dark is spent on the flat, so a bundle never holds one to check.
    for component in [MasterRole::Dark, MasterRole::Flat, MasterRole::Bias] {
        let masters = masters_with_sized_component(component, CfaType::Mono, Size2us::new(4, 4));
        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        let original_data = light.data().to_vec();

        assert_eq!(
            masters.calibrate(&mut light),
            Err(CalibrationError::DimensionMismatch {
                component: component.into(),
                expected: Size2us::new(2, 2),
                master: Size2us::new(4, 4),
            })
        );
        assert_eq!(light.data().pixels(), original_data);
        assert_eq!(light.metadata.derived.calibration(), CalibrationState::NONE);
    }

    // A master of another camera is refused when both name theirs, and calibrates when they name
    // the same one or either names none.
    let named = |instrument: Option<&str>| {
        let mut master = constant_cfa(Size2us::new(2, 2), 0.1, CfaType::Mono);
        master.metadata.header.instrument = instrument.map(str::to_owned);
        bundle(CalibrationSet {
            bias: Some(master),
            ..CalibrationSet::default()
        })
    };
    let light = |instrument: Option<&str>| {
        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light.metadata.header.instrument = instrument.map(str::to_owned);
        light
    };
    let mut other = light(Some("Canon EOS 600D"));
    assert_eq!(
        named(Some("Canon EOS 6D")).calibrate(&mut other),
        Err(CalibrationError::InstrumentMismatch {
            component: MasterRole::Bias,
            light: "Canon EOS 600D".to_owned(),
            master: "Canon EOS 6D".to_owned(),
        })
    );
    assert_eq!(other.metadata.derived.calibration(), CalibrationState::NONE);
    for (master, frame) in [
        (Some("Canon EOS 6D"), Some("Canon EOS 6D")),
        (None, Some("Canon EOS 6D")),
        (Some("Canon EOS 6D"), None),
    ] {
        assert!(
            named(master).calibrate(&mut light(frame)).is_ok(),
            "master {master:?}, light {frame:?}"
        );
    }

    // A master on another declared span converts by the exact ratio of the spans. The light
    // reads 0.5; the dark or bias reads 0.25 on a span four times the light's, so it is worth
    // 0.25 × 4 = 1.0 of the light's units and leaves −0.5. The flat divides a normalized copy of
    // itself, so its span cancels; the flat-dark only ever calibrates the flat.
    for (component, expected) in [
        (MasterRole::Dark, -0.5f32),
        (MasterRole::Bias, -0.5),
        (MasterRole::Flat, 0.5),
        (MasterRole::FlatDark, 0.5),
    ] {
        let mut master = constant_cfa(Size2us::new(2, 2), 0.25, CfaType::Mono);
        master
            .metadata
            .derived
            .set_domain(Some(raw_domain(65_532.0)));
        let mut images = CalibrationSet::default();
        *images.get_mut(component) = Some(master);
        let masters = bundle(images);

        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light
            .metadata
            .derived
            .set_domain(Some(raw_domain(16_383.0)));
        assert_eq!(
            masters.calibrate(&mut light).map(drop),
            Ok(()),
            "{component:?}"
        );
        assert_eq!(light.data().pixels(), &[expected; 4], "{component:?}");
    }

    // A span the decoder had to assume cannot be converted: subtracting a `[0, 1]` master from a
    // light on another span would remove ~0.01 from ~3000 and then mark the light calibrated.
    // Refused for the master the light subtracts, before the light is touched.
    for component in [MasterRole::Dark, MasterRole::Bias] {
        let mut master = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        master
            .metadata
            .derived
            .set_domain(Some(fits_domain(1.0, None, ScaleOrigin::Assumed)));
        let mut images = CalibrationSet::default();
        *images.get_mut(component) = Some(master);
        let masters = bundle(images);

        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light
            .metadata
            .derived
            .set_domain(Some(raw_domain(16_383.0)));
        let original_data = light.data().to_vec();

        assert_eq!(
            masters.calibrate(&mut light),
            Err(CalibrationError::SampleDomainMismatch {
                component,
                frame: raw_domain(16_383.0),
                master: fits_domain(1.0, None, ScaleOrigin::Assumed),
            })
        );
        assert_eq!(light.data().pixels(), original_data);
        assert_eq!(light.metadata.derived.calibration(), CalibrationState::NONE);
    }

    // The same failure with no span to give it away: a master and a light on one span whose BUNIT
    // names different quantities. The subtraction would run and report success.
    {
        let mut master = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        master.metadata.derived.set_domain(Some(fits_domain(
            1.0,
            Some("count/s"),
            ScaleOrigin::Declared,
        )));
        let mut images = CalibrationSet::default();
        *images.get_mut(MasterRole::Dark) = Some(master);
        let masters = bundle(images);

        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light.metadata.derived.set_domain(Some(fits_domain(
            1.0,
            Some("Jy/beam"),
            ScaleOrigin::Declared,
        )));
        let original_data = light.data().to_vec();

        assert_eq!(
            masters.calibrate(&mut light),
            Err(CalibrationError::SampleDomainMismatch {
                component: MasterRole::Dark,
                frame: fits_domain(1.0, Some("Jy/beam"), ScaleOrigin::Declared),
                master: fits_domain(1.0, Some("count/s"), ScaleOrigin::Declared),
            })
        );
        assert_eq!(light.data().pixels(), original_data);
        assert_eq!(light.metadata.derived.calibration(), CalibrationState::NONE);

        // A master that states no unit is "cannot tell", not a disagreement, so the same pair
        // calibrates once the master stops naming a quantity — the unit is the axis, not its
        // presence.
        let mut master = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        master
            .metadata
            .derived
            .set_domain(Some(fits_domain(1.0, None, ScaleOrigin::Declared)));
        let mut images = CalibrationSet::default();
        *images.get_mut(MasterRole::Dark) = Some(master);
        let masters = bundle(images);
        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light.metadata.derived.set_domain(Some(fits_domain(
            1.0,
            Some("Jy/beam"),
            ScaleOrigin::Declared,
        )));
        assert_eq!(masters.calibrate(&mut light).map(drop), Ok(()));
    }

    // ...and a set that does match still calibrates, so the extent check is not rejecting on
    // sheer presence. The dark also carries a defect map detected at its own size, which the
    // same pass checks against the light.
    let masters = masters_with_sized_component(MasterRole::Dark, CfaType::Mono, Size2us::new(4, 4));
    let mut light = constant_cfa(Size2us::new(4, 4), 0.5, CfaType::Mono);
    assert_eq!(masters.calibrate(&mut light).map(drop), Ok(()));

    // Matching spans pass, and so does a pair where only one side declares one: an undeclared
    // span is "cannot tell", which must not reject the in-memory fixtures the rest of this file
    // is built from.
    for (light_span, master_span) in [
        (Some(65_535.0), Some(65_535.0)),
        (Some(65_535.0), None),
        (None, Some(65_535.0)),
        (None, None),
    ] {
        let mut master = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        master
            .metadata
            .derived
            .set_domain(master_span.map(raw_domain));
        let mut images = CalibrationSet::default();
        *images.get_mut(MasterRole::Dark) = Some(master);
        let masters = bundle(images);

        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light
            .metadata
            .derived
            .set_domain(light_span.map(raw_domain));
        assert_eq!(
            masters.calibrate(&mut light).map(drop),
            Ok(()),
            "light {light_span:?} against master {master_span:?}"
        );
    }
}

/// A dark that kept its pedestal of 2048 ADU on a 65536 span, applied to a RAW
/// light with the black removed on a 16384 span: the map is gain 4 and offset −2048/16384 = −0.125,
/// so a dark holding only the pedestal, 2048/65536 = 0.03125, maps to 4 × 0.03125 − 0.125 = 0 and
/// leaves the light as it was. Before, the scale alone was applied, and the light lost 0.125.
/// Dyadic values, so the result is exact. The light's pedestal is then removed, and a dark whose
/// pedestal nobody recorded is refused rather than guessed.
///
/// With a bias beside it, every frame keeping 2048 on the 65536 span — a bias of 0.03125, the dark
/// 0.0625 above it, the light 0.25 above that — the dark loses the bias and its pedestal with it.
/// The light loses the bias first, and its pedestal with it, so the dark's map has to be taken
/// against the light without one: the light keeps 0.25 exactly. A map taken against the light as
/// it was would carry the pedestal over and remove it twice, leaving 0.25 − 2048/65536 = 0.21875.
#[test]
fn a_dark_that_kept_its_pedestal_subtracts_only_its_signal() {
    let kept = SampleDomain {
        scale: 65_536.0,
        origin: ScaleOrigin::Declared,
        pedestal: Pedestal::Kept(2048.0),
        unit: None,
    };
    let calibrate = |pedestal| {
        let mut dark = constant_cfa(Size2us::new(2, 2), 0.031_25, CfaType::Mono);
        dark.metadata.derived.set_domain(Some(SampleDomain {
            pedestal,
            ..kept.clone()
        }));
        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light
            .metadata
            .derived
            .set_domain(Some(raw_domain(16_384.0)));
        bundle(CalibrationSet {
            dark: Some(dark),
            ..Default::default()
        })
        .calibrate(&mut light)
        .map(|_| light)
    };

    let light = calibrate(Pedestal::Kept(2048.0)).unwrap();
    assert_eq!(light.data().pixels(), &[0.5; 4]);
    assert_eq!(
        light.metadata.derived.domain().unwrap().pedestal,
        Pedestal::Removed
    );
    assert!(matches!(
        calibrate(Pedestal::Unknown),
        Err(CalibrationError::SampleDomainMismatch { .. })
    ));

    let kept_frame = |value| {
        let mut frame = constant_cfa(Size2us::new(2, 2), value, CfaType::Mono);
        frame.metadata.derived.set_domain(Some(kept.clone()));
        frame
    };
    let masters = bundle(CalibrationSet {
        dark: Some(kept_frame(0.093_75)),
        bias: Some(kept_frame(0.031_25)),
        ..Default::default()
    });
    let mut light = kept_frame(0.343_75);
    masters.calibrate(&mut light).unwrap();
    assert_eq!(light.data().pixels(), &[0.25; 4]);
    assert_eq!(
        light.metadata.derived.domain().unwrap().pedestal,
        Pedestal::Removed
    );
    assert_eq!(
        light.metadata.derived.calibration(),
        CalibrationState::ADDITIVE
    );
}

/// A RAW decode's domain over a span of `scale`, for pairing frames that differ only in domain.
fn raw_domain(scale: f64) -> SampleDomain {
    SampleDomain {
        scale,
        origin: ScaleOrigin::Declared,
        pedestal: Pedestal::Removed,
        unit: None,
    }
}

/// The same, for the one decoder that can also declare what its samples measure. A third-party
/// FITS file records no pedestal.
fn fits_domain(scale: f64, unit: Option<&str>, origin: ScaleOrigin) -> SampleDomain {
    SampleDomain {
        scale,
        origin,
        pedestal: Pedestal::Unknown,
        unit: unit.map(str::to_owned),
    }
}

#[test]
fn a_set_that_cannot_be_loaded_cannot_be_built() {
    // Masters that describe different sensors: with no flat present nothing else compares them,
    // so `from_images` has to refuse a set that `save` would write and only `load` reject.
    let mismatched = CalibrationSet {
        dark: Some(constant_cfa(Size2us::new(4, 4), 0.01, CfaType::Mono)),
        flat: None,
        bias: Some(constant_cfa(Size2us::new(8, 8), 0.005, CfaType::Mono)),
        flat_dark: None,
    };
    assert_eq!(
        CalibrationMasters::from_images(mismatched, SigmaThreshold::DEFAULT, &CancelToken::never())
            .unwrap_err()
            .to_string(),
        // Dark is seen first, so it sets the expectation the bias then fails.
        "bias master is 8x8, expected 4x4",
    );

    // The flat is the one role whose subtractor was already checked, so it must still be caught
    // when it is the odd one out rather than the reference.
    let mismatched_flat = CalibrationSet {
        dark: Some(constant_cfa(Size2us::new(4, 4), 0.01, CfaType::Mono)),
        flat: Some(constant_cfa(Size2us::new(6, 6), 1.0, CfaType::Mono)),
        bias: None,
        flat_dark: None,
    };
    assert_eq!(
        CalibrationMasters::from_images(
            mismatched_flat,
            SigmaThreshold::DEFAULT,
            &CancelToken::never()
        )
        .unwrap_err()
        .to_string(),
        "flat master is 6x6, expected 4x4",
    );

    // A coherent set of the same shape is still accepted.
    let coherent = CalibrationSet {
        dark: Some(constant_cfa(Size2us::new(4, 4), 0.01, CfaType::Mono)),
        flat: None,
        bias: Some(constant_cfa(Size2us::new(4, 4), 0.005, CfaType::Mono)),
        flat_dark: None,
    };
    let masters =
        CalibrationMasters::from_images(coherent, SigmaThreshold::DEFAULT, &CancelToken::never())
            .expect("a coherent set builds");
    assert_eq!(
        masters.components().collect::<Vec<_>>(),
        [
            CalibrationComponent::Master(MasterRole::Dark),
            CalibrationComponent::Master(MasterRole::Bias),
            CalibrationComponent::Defects
        ]
    );
}

#[test]
fn every_component_round_trips_through_its_extname() {
    // One name table: the writer stamps `extname()` and `bundle_idxs` recognizes the HDU by
    // feeding it back through `from_extname`, so a role that fails to round-trip is a role that
    // silently vanishes from a saved bundle.
    for component in MasterRole::ALL
        .into_iter()
        .map(CalibrationComponent::Master)
        .chain([CalibrationComponent::Defects])
    {
        assert_eq!(
            CalibrationComponent::from_extname(component.extname()),
            Some(component)
        );
    }
    assert_eq!(CalibrationComponent::from_extname("MASTER_LIGHT"), None);
    // Only the flat is stored prepared; `read_master` rejects a bundle that says otherwise.
    assert!(MasterRole::Flat.prepared());
    for role in [MasterRole::Dark, MasterRole::Bias, MasterRole::FlatDark] {
        assert!(!role.prepared(), "{role} must not be prepared");
    }
}

/// An empty role stacks to no master without touching a file, and a set of no masters is an
/// empty bundle. Each role stacks under its own preset; a flat-dark is a dark taken at the flat's
/// exposure, so it shares the dark's.
#[test]
fn empty_roles_yield_no_masters() {
    let empty: Vec<PathBuf> = Vec::new();
    for role in MasterRole::ALL {
        let master = MasterStack {
            paths: &empty,
            role,
            config: role.stack_config(),
            ingest: &IngestConfig::default(),
            subtract: &[],
        }
        .run(ProgressMonitor::unattended())
        .unwrap();
        assert!(master.is_none(), "{role}");
    }
    let masters = bundle(CalibrationSet::default());
    assert_eq!(masters.components().collect::<Vec<_>>(), Vec::new());

    for (role, preset) in [
        (MasterRole::Dark, StackConfig::bias_or_dark()),
        (MasterRole::FlatDark, StackConfig::bias_or_dark()),
        (MasterRole::Flat, StackConfig::flat()),
        (MasterRole::Bias, StackConfig::bias_or_dark()),
    ] {
        assert_eq!(
            format!("{:?}", role.stack_config()),
            format!("{preset:?}"),
            "{role}"
        );
    }
}

#[test]
fn new_constructor() {
    let dark = constant_cfa(Size2us::new(4, 4), 0.1, CfaType::Mono);
    let flat = constant_cfa(Size2us::new(4, 4), 0.8, CfaType::Mono);
    let bias = constant_cfa(Size2us::new(4, 4), 0.02, CfaType::Mono);
    let flat_dark = constant_cfa(Size2us::new(4, 4), 0.03, CfaType::Mono);

    let masters = bundle(CalibrationSet {
        dark: Some(dark),
        flat: Some(flat),
        bias: Some(bias),
        flat_dark: Some(flat_dark),
    });
    assert_eq!(
        masters.components().collect::<Vec<_>>(),
        vec![
            CalibrationComponent::Master(MasterRole::Dark),
            CalibrationComponent::Master(MasterRole::Flat),
            CalibrationComponent::Master(MasterRole::Bias),
            CalibrationComponent::Defects,
        ]
    );
    assert_eq!(
        masters
            .components()
            .map(|component| component.to_string())
            .collect::<Vec<_>>(),
        // The flat-dark is spent on the flat and not kept.
        vec!["dark", "flat", "bias", "defects"]
    );
    let summary = masters.defect_summary().unwrap();
    assert_eq!(
        (
            summary.hot_pixels,
            summary.cold_pixels,
            summary.defective_pixels
        ),
        (0, 0, 0)
    );
}

#[test]
fn cold_detection_uses_subtracted_unfloored_flat_response() {
    #[derive(Debug, Clone, Copy)]
    enum SubtractorKind {
        Bias,
        FlatDark,
    }

    let width = 7;
    let height = 7;
    let dead = 3 * width + 3;

    for kind in [SubtractorKind::Bias, SubtractorKind::FlatDark] {
        let mut flat_pixels = vec![11.0; width * height];
        flat_pixels[dead] = 10.0;
        let flat = make_cfa(Size2us::new(width, height), flat_pixels, CfaType::Mono);
        let subtractor = constant_cfa(Size2us::new(width, height), 10.0, CfaType::Mono);
        let mut images = CalibrationSet {
            flat: Some(flat),
            ..Default::default()
        };
        match kind {
            SubtractorKind::Bias => images.bias = Some(subtractor),
            SubtractorKind::FlatDark => images.flat_dark = Some(subtractor),
        }

        let masters = bundle(images);
        let defects = masters.defect_map.as_ref().unwrap();
        assert_eq!(defects.cold_idxs(), [dead], "{kind:?}");

        let prepared = masters.flat.as_ref().unwrap().divisor();
        assert_eq!(prepared.data()[dead], 0.1, "{kind:?}");
        let expected_normal = 49.0 / 48.0;
        assert!(
            (prepared.data()[0] - expected_normal).abs() < 1e-6,
            "{kind:?}: {} != {expected_normal}",
            prepared.data()[0]
        );
    }
}

#[test]
fn from_images_rejects_cancelled_operation() {
    let cancel = CancelToken::new();
    cancel.cancel();

    let result = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(constant_cfa(Size2us::new(4, 4), 0.1, CfaType::Mono)),
            ..Default::default()
        },
        SigmaThreshold::DEFAULT,
        &cancel,
    );

    assert!(matches!(result, Err(CalibrationError::Cancelled)));
}

#[test]
fn new_no_dark_no_hot_pixels() {
    let flat = constant_cfa(Size2us::new(4, 4), 0.8, CfaType::Mono);

    let masters = bundle(CalibrationSet {
        flat: Some(flat),
        ..Default::default()
    });

    assert_eq!(
        masters.components().collect::<Vec<_>>(),
        vec![
            CalibrationComponent::Master(MasterRole::Flat),
            CalibrationComponent::Defects
        ]
    );
    let summary = masters.defect_summary().unwrap();
    assert_eq!(
        (
            summary.hot_pixels,
            summary.cold_pixels,
            summary.defective_pixels
        ),
        (0, 0, 0)
    );
}

/// A light loses the dark, or the bias when there is no dark — never both. Dyadic levels keep
/// every subtraction exact: 0.5 − 0.125 = 0.375 and 0.5 − 0.0625 = 0.4375. Where a master holds no
/// measurement, the light holds none either: a dark saturated at pixel 5 and a bias with no data at
/// pixel 9 leave the light `NO_DATA` at both, through the bias the dark lost, and nowhere else. A
/// bias whose pixel 9 holds a meaningless 0.5 would leave the light 0 there; the light holds the
/// median of its neighbours instead, 0.4375.
#[test]
fn calibrate_subtracts_the_dark_or_else_the_bias() {
    let size = Size2us::new(4, 4);
    let level = |value| Some(constant_cfa(size, value, CfaType::Mono));
    for (dark, bias, expected) in [
        (Some(0.125), None, 0.375f32),
        (None, Some(0.0625), 0.4375),
        (Some(0.125), Some(0.0625), 0.375),
    ] {
        let masters = bundle(CalibrationSet {
            dark: dark.and_then(level),
            bias: bias.and_then(level),
            ..Default::default()
        });
        let mut light = constant_cfa(size, 0.5, CfaType::Mono);
        masters.calibrate(&mut light).unwrap();
        assert!(
            light.data().iter().all(|&v| v == expected),
            "dark {dark:?}, bias {bias:?}: {:?}",
            light.data().pixels()
        );
        assert!(light.flags().is_none());
    }

    let flagged = |value, idx, flag| {
        let mut master = constant_cfa(size, value, CfaType::Mono);
        master.set_flags(PixelFlags::from_fn(size, |at| {
            if at == idx {
                flag
            } else {
                QualityFlags::default()
            }
        }));
        master
    };
    let masters = bundle(CalibrationSet {
        dark: Some(flagged(0.125, 5, QualityFlags::SATURATED)),
        bias: Some(flagged(0.0625, 9, QualityFlags::NO_DATA)),
        ..Default::default()
    });
    let mut light = constant_cfa(size, 0.5, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();
    // Where a master held no measurement the light holds none either, and calibration repairs it.
    let flags = light.flags().unwrap();
    assert_eq!(flags.count(QualityFlags::NO_DATA), 2);
    assert_eq!(flags.count(QualityFlags::REPAIRED), 2);
    let null = QualityFlags::NO_DATA.union(QualityFlags::REPAIRED);
    assert_eq!(flags.at(5), null);
    assert_eq!(flags.at(9), null);

    let mut bias = flagged(0.0625, 9, QualityFlags::NO_DATA);
    bias.data_mut()[9] = 0.5;
    let masters = bundle(CalibrationSet {
        bias: Some(bias),
        ..Default::default()
    });
    let mut light = constant_cfa(size, 0.5, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();
    assert_eq!(light.data().pixels(), &[0.4375; 16]);
}

/// The flat divides out the vignetting `v` and leaves `signal · mean(v)`, whichever masters are
/// present — provided the light loses the dark (or else the bias) and the flat loses the
/// flat-dark (or else the bias). A light `signal·v + s_light` against a flat `k·v + s_flat`:
/// subtracting puts `signal·v` over `k·v`, normalized by `k·mean(v)`. Every level is dyadic, so
/// both subtractions and the mean are exact; the divisor `v/mean(v)` and the division round once
/// each, so every pixel lands within ε of `signal · mean(v)`. A flat that lost the wrong
/// subtractor would be off by the difference over `k·v`. A flat-dark that lost its bias holds its
/// thermal 1/64 alone, so the flat holds the bias under it, 1/32 + 1/64 = 3/64, and loses both.
///
/// A flat stacked with a subtractor taken from each frame already lost that part, and holds only
/// the rest: one that lost its thermal 1/64 to a thermal flat-dark still loses its bias here, and
/// one that lost a part is never given it again. A flat that lost its bias beside a flat-dark that
/// still holds one, with no bias to separate the two, is refused.
#[test]
fn calibrate_divides_by_the_flat_less_its_own_subtractor() {
    let size = Size2us::new(2, 1);
    let (signal, vignetting, k) = ([0.25f32, 0.5], [0.75f32, 1.0], 0.5f32);
    let mean_v = f32::midpoint(vignetting[0], vignetting[1]);
    let (dark, bias, flat_dark) = (0.0625f32, 0.03125f32, 0.015_625f32);
    let level = |value| constant_cfa(size, value, CfaType::Mono);
    let none = CalibrationState::NONE;
    for (name, has_dark, has_bias, has_flat_dark, thermal_flat_dark, flat_lost) in [
        ("flat alone", false, false, false, false, none),
        ("dark and bias", true, true, false, false, none),
        ("dark and flat-dark", true, false, true, false, none),
        ("bias and flat-dark", false, true, true, false, none),
        ("all three", true, true, true, false, none),
        ("bias and thermal flat-dark", false, true, true, true, none),
        ("all three, thermal flat-dark", true, true, true, true, none),
        (
            "flat lost its dark signal",
            false,
            true,
            true,
            true,
            CalibrationState::THERMAL,
        ),
        (
            "flat lost both",
            true,
            true,
            true,
            true,
            CalibrationState::ADDITIVE,
        ),
        (
            "flat lost its bias",
            false,
            true,
            false,
            false,
            CalibrationState::BIAS,
        ),
    ] {
        let light_sub = if has_dark {
            dark
        } else if has_bias {
            bias
        } else {
            0.0
        };
        let flat_holds = if thermal_flat_dark {
            bias + flat_dark
        } else if has_flat_dark {
            flat_dark
        } else if has_bias {
            bias
        } else {
            0.0
        };
        let flat_sub = flat_holds
            - if flat_lost.bias { bias } else { 0.0 }
            - if flat_lost.thermal { flat_dark } else { 0.0 };
        let light: Vec<f32> = (0..2)
            .map(|i| signal[i] * vignetting[i] + light_sub)
            .collect();
        let mut flat = make_cfa(
            size,
            (0..2).map(|i| k * vignetting[i] + flat_sub).collect(),
            CfaType::Mono,
        );
        flat.metadata.derived.set_calibration(flat_lost);
        let masters = bundle(CalibrationSet {
            dark: has_dark.then(|| level(dark)),
            flat: Some(flat),
            bias: has_bias.then(|| level(bias)),
            flat_dark: has_flat_dark.then(|| {
                let mut flat_dark = level(flat_dark);
                if thermal_flat_dark {
                    flat_dark
                        .metadata
                        .derived
                        .set_calibration(CalibrationState::BIAS);
                }
                flat_dark
            }),
        });
        let mut light = make_cfa(size, light, CfaType::Mono);
        masters.calibrate(&mut light).unwrap();
        for (i, &signal) in signal.iter().enumerate() {
            let expected = signal * mean_v;
            assert_close!(
                light.data()[i],
                expected,
                f32::EPSILON * expected,
                "{name}, pixel {i}"
            );
        }
    }

    let mut flat = level(0.5);
    flat.metadata
        .derived
        .set_calibration(CalibrationState::BIAS);
    assert_eq!(
        CalibrationMasters::from_images(
            CalibrationSet {
                flat: Some(flat),
                flat_dark: Some(level(flat_dark)),
                ..Default::default()
            },
            SigmaThreshold::DEFAULT,
            &CancelToken::never(),
        )
        .unwrap_err(),
        CalibrationError::UnusableFlatDark
    );
}

#[test]
fn sigma_threshold_affects_detection() {
    // 6×6 mono dark with *real* noise so σ genuinely scales the threshold: 18 px at 90, 18 at 110
    // (one of the 110s replaced by a warm 400). Per-color stats (mono): median ≈ 100, MAD ≈ 10 →
    // sigma ≈ 10·1.4826 ≈ 14.8.
    //   sigma=3:  threshold ≈ 100 + 3·14.8  ≈ 145 → 400 > 145 (only the warm px) → 1 detected
    //   sigma=40: threshold ≈ 100 + 40·14.8 ≈ 693 → 400 < 693                    → 0 detected
    // (No relative σ floor: detectability scales with the real noise, not a fraction of the median.)
    let mut pixels: Vec<f32> = (0..36)
        .map(|i| if i % 2 == 0 { 90.0 } else { 110.0 })
        .collect();
    pixels[15] = 400.0; // index 15 is odd → was 110

    let dark_strict = make_cfa(Size2us::new(6, 6), pixels.clone(), CfaType::Mono);
    let dark_loose = make_cfa(Size2us::new(6, 6), pixels, CfaType::Mono);

    let masters_strict = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark_strict),
            ..Default::default()
        },
        SigmaThreshold::new(3.0).unwrap(),
        &CancelToken::never(),
    )
    .unwrap();
    let masters_loose = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark_loose),
            ..Default::default()
        },
        SigmaThreshold::new(40.0).unwrap(),
        &CancelToken::never(),
    )
    .unwrap();

    let strict_count = masters_strict.defect_summary().unwrap().hot_pixels;
    let loose_count = masters_loose.defect_summary().unwrap().hot_pixels;

    assert_eq!(strict_count, 1, "σ 3: 400 is past 145, and only it");
    assert_eq!(loose_count, 0, "σ 40: 400 is short of 693");
}

#[test]
fn defect_detection_zero_median_no_false_positives() {
    // A quantized bias can have MAD=0 even though a few samples occupy adjacent ADC levels.
    let mut data = vec![0.0f32; 100];
    // Add a few pixels with tiny values (normal bias noise)
    data[10] = 0.0001;
    data[20] = 0.0002;
    data[30] = 0.0001;
    // Add one genuine hot pixel
    data[50] = 0.5;

    let dark = CfaImage::from_plane(
        Buffer2::new(10, 10, data),
        CfaType::Mono,
        ImageMetadata {
            header: ObservationHeader::default(),
            derived: DerivedFacts::builder()
                .quantization_sigma(Some(QUANTIZATION_SIGMA_PER_STEP / 4095.0))
                .build(),
        },
    );

    let defect_map = DefectMap::new(dark.size())
        .detect_hot(
            &dark,
            SigmaThreshold::new(5.0).unwrap(),
            &CancelToken::never(),
        )
        .unwrap();

    // The tiny values one ADC step up are not flagged; the genuine outlier at 0.5 is, alone.
    assert_eq!(defect_map.hot().idxs, [50]);
}

/// A hot pixel is repaired from its own color. The light's red, green and blue sit at 0.5, 0.3 and
/// 0.2 over a dark of 0.0625, so after subtraction the hot red at (2, 2) can only come back as
/// 0.5 − 0.0625 = 0.4375 if every neighbour it took the median of was red — a green or blue one
/// would pull it toward 0.2375 or 0.1375. The hot light pixel reads 0.9, under the saturation level,
/// so the repair is the one flag it gets.
///
/// Neither repair reads a pixel with no measurement. With the four nearest of the hot pixel's eight
/// red neighbours null in the light, each filled with 0.9, a median over all eight would be
/// (0.4375 + 0.8375)/2 = 0.6375; over the four measured ones it is 0.4375. Each null is then
/// repaired from its own measured reds, the repaired hot pixel left out: 0.4375 too.
#[test]
fn calibrate_hot_pixel_correction() {
    let (w, h) = (6, 6);
    let pattern = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let mut dark_pixels = Buffer2::new_filled(w, h, 0.0625_f32);
    dark_pixels[(2, 2)] = 0.9;
    let masters = bundle(CalibrationSet {
        dark: Some(cfa_from_plane(dark_pixels, pattern)),
        ..Default::default()
    });
    // The hot pixel at (2, 2) is red, one of the nine reds of 6 × 6 RGGB.
    let summary = masters.defect_summary().unwrap();
    assert_eq!(
        (
            summary.hot_pixels,
            summary.cold_pixels,
            summary.defective_pixels
        ),
        (1, 0, 1)
    );
    assert_eq!(summary.percentage(), 100.0 / (w * h) as f32);
    assert_eq!(
        summary.colors.of(CfaColor::Red).unwrap().percentage(),
        100.0 / 9.0
    );

    let baseline = [0.5f32, 0.3, 0.2];
    let mut light_pixels = Buffer2::new_default(w, h);
    for y in 0..h {
        for x in 0..w {
            light_pixels[(x, y)] = baseline[pattern.slot_at(Vec2us::new(x, y))];
        }
    }
    light_pixels[(2, 2)] = 0.9;
    let mut light = cfa_from_plane(light_pixels.clone(), pattern);
    masters.calibrate(&mut light).unwrap();
    assert_eq!(light.data()[2 * w + 2], 0.5 - 0.0625);
    // The repaired pixel says so, and no other does.
    let flags = light.flags().unwrap();
    assert_eq!(flags.count(QualityFlags::REPAIRED), 1);
    assert_eq!(
        flags.at_pos(Vec2us::new(2, 2)),
        QualityFlags::DEFECT.union(QualityFlags::REPAIRED)
    );

    let nulls = [(0, 2), (4, 2), (2, 0), (2, 4)].map(|(x, y)| y * w + x);
    for &idx in &nulls {
        light_pixels.pixels_mut()[idx] = 0.9;
    }
    let mut light = cfa_from_plane(light_pixels, pattern);
    light.set_flags(PixelFlags::from_fn(Size2us::new(w, h), |idx| {
        if nulls.contains(&idx) {
            QualityFlags::NO_DATA
        } else {
            QualityFlags::default()
        }
    }));
    masters.calibrate(&mut light).unwrap();
    assert_eq!(light.data()[2 * w + 2], 0.5 - 0.0625);
    for idx in nulls {
        assert_eq!(light.data()[idx], 0.5 - 0.0625, "null at {idx}");
    }
}

#[test]
fn prepared_master_fits_bundle_round_trips_flat_and_calibration_bit_exactly() {
    let cfa_type = CfaType::Mosaic(Mosaic::Bayer(CfaPattern::Rggb));
    let flat = CfaImage::from_plane(
        Buffer2::new(
            4,
            4,
            vec![
                0.5, 0.7, 0.9, 0.7, 0.7, 0.4, 0.7, 0.4, 0.9, 0.7, 0.5, 0.7, 0.7, 0.4, 0.7, 0.4,
            ],
        ),
        cfa_type,
        ImageMetadata {
            header: ObservationHeader {
                camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
                ..Default::default()
            },
            derived: DerivedFacts::builder()
                .dark_comparison(Some(UNSHARED_TEMPERATURE))
                .build(),
        },
    );
    let mut masters = bundle(CalibrationSet {
        dark: Some(constant_cfa(Size2us::new(4, 4), 0.05, cfa_type)),
        flat: Some(flat),
        bias: Some(constant_cfa(Size2us::new(4, 4), 0.1, cfa_type)),
        flat_dark: None,
    });
    let dark = masters.dark.as_mut().unwrap();
    dark.metadata.derived.set_quantization_sigma(Some(0.000_02));
    // Pixel 5 is saturated and a defect: 2 + 4 = 6. The dark's flags go in its own `LUMFLAGS`
    // extension, right after it.
    let dark_flags = PixelFlags::from_fn(Size2us::new(4, 4), |idx| {
        QualityFlags::from_byte(if idx == 5 { 6 } else { 0 })
    });
    dark.set_flags(dark_flags.clone());
    // Two hot pixels with excesses f32 rounds, and pixel 10 cold as well.
    masters.defect_map = Some(
        DefectMap::from_lists(
            Size2us::new(4, 4),
            HotPixels {
                idxs: vec![5, 10],
                excess: vec![0.1, 3.0e-5],
            },
            vec![10],
        )
        .unwrap(),
    );
    let prepared_bits = bits(masters.flat.as_ref().unwrap().divisor().data().pixels());

    let mut expected = constant_cfa(Size2us::new(4, 4), 0.75, cfa_type);
    masters.calibrate(&mut expected).unwrap();

    let dir = TempDir::new("lumos-calibration-roundtrip");
    let path = dir.join("masters.fits");
    writer::calibration_bundle(&masters, &path).unwrap();
    // A bundle is no single image, whichever loader is pointed at it.
    let refused = |result: Result<(), ImageError>| {
        matches!(
            result,
            Err(ImageError::FitsUnsupported {
                reason: FitsRefusal::NotStandaloneImage { format },
                ..
            }) if format == "CALMASTR"
        )
    };
    let ctx = LoadCtx::default();
    assert!(refused(ctx.cfa(&path).map(drop)));
    assert!(refused(ctx.linear(&path).map(drop)));
    assert!(refused(ctx.preview(&path).map(drop)));
    let cache_bytes = fs::read(&path).unwrap();
    let mut reader = FitsReader::from_bytes(&cache_bytes).unwrap();
    assert_eq!(reader.hdus().len(), 6);
    assert_eq!(reader.hdus()[0].header.naxis().unwrap(), 0);
    assert_eq!(
        reader.hdus()[0].header.get_text(FORMAT_KEYWORD).unwrap(),
        Some("CALMASTR")
    );
    let extension_names = reader
        .hdus()
        .iter()
        .skip(1)
        .map(|hdu| hdu.header.get_text("EXTNAME").unwrap().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        extension_names,
        [
            "MASTER_DARK",
            "LUMFLAGS",
            "MASTER_FLAT",
            "MASTER_BIAS",
            "DEFECT_MAP"
        ]
    );
    assert_eq!(
        reader.hdus()[2].header.get_text("LUMFOR").unwrap(),
        Some("MASTER_DARK")
    );
    for image_idx in [1, 3, 4] {
        assert_eq!(
            reader.hdus()[image_idx].header.bitpix().unwrap(),
            Bitpix::F32
        );
    }
    for idx in 0..reader.hdus().len() {
        let report = reader.verify_checksum(idx).unwrap();
        assert_eq!(report.datasum, ChecksumStatus::Valid);
        assert_eq!(report.checksum, ChecksumStatus::Valid);
    }
    let loaded = LoadCtx::default().calibration_masters(&path).unwrap();
    // The flat's stack left its flat-dark's temperature uncompared, and the bundle keeps saying so.
    assert_eq!(loaded.flat_dark(), Some(UNSHARED_TEMPERATURE));
    assert_eq!(
        loaded.dark.as_ref().unwrap().flags().unwrap().bytes(),
        dark_flags.as_ref().unwrap().bytes()
    );
    assert!(loaded.bias.as_ref().unwrap().flags().is_none());
    // Flags whose master is gone would be dropped unseen, so the bundle is refused.
    let orphan = dir.join("orphan.fits");
    fs::write(&orphan, &cache_bytes).unwrap();
    rewrite_fits(&orphan, |idx, _, _| idx != 1);
    let error = LoadCtx::default().calibration_masters(&orphan).unwrap_err();
    assert!(matches!(error, BundleError::Flags(_)), "{error:?}");
    assert!(
        error
            .to_string()
            .contains("is for \"MASTER_DARK\", which is no image of the file"),
        "{error}"
    );
    // The caller's context reaches every master's decode: a cancelled one stops the load.
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(matches!(
        LoadCtx::new(cancel, u64::MAX).calibration_masters(&path),
        Err(BundleError::Cancelled)
    ));

    let mut invalid_version = cache_bytes.clone();
    let version_card = invalid_version
        .windows(8)
        .position(|window| window == VERSION_KEYWORD.as_bytes())
        .unwrap();
    let version_digit = invalid_version[version_card..version_card + 80]
        .iter()
        .rposition(u8::is_ascii_digit)
        .unwrap();
    invalid_version[version_card + version_digit] = b'4';
    fs::write(&path, invalid_version).unwrap();
    assert!(matches!(
        LoadCtx::default().calibration_masters(&path),
        Err(BundleError::Version {
            found: 4,
            expected: 6
        })
    ));

    // The bias is stored as it is, 0.1 at every pixel; the dark beside it holds its thermal signal
    // alone, 0.05 − 0.1, and records that it lost the bias.
    assert_eq!(
        reader.hdus()[1].header.get_logical("LUMCALB").unwrap(),
        Some(true)
    );
    assert_eq!(
        reader.hdus()[1].header.get_logical("LUMCALD").unwrap(),
        None
    );
    let mut invalid_data = cache_bytes.clone();
    let sample = 0.1f32.to_be_bytes();
    let repeated_sample = sample.repeat(4);
    let pixel_offset = invalid_data
        .windows(repeated_sample.len())
        .position(|window| window == repeated_sample)
        .unwrap();
    invalid_data[pixel_offset] ^= 0x01;
    fs::write(&path, invalid_data).unwrap();
    let error = LoadCtx::default().calibration_masters(&path).unwrap_err();
    assert!(
        matches!(error, BundleError::Checksum { hdu: 4 }),
        "{error:?}"
    );

    assert_eq!(
        bits(loaded.flat.as_ref().unwrap().divisor().data().pixels()),
        prepared_bits
    );
    assert_eq!(
        loaded
            .flat
            .as_ref()
            .unwrap()
            .divisor()
            .metadata
            .header
            .camera_white_balance,
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        loaded
            .dark
            .as_ref()
            .unwrap()
            .metadata
            .derived
            .quantization_sigma(),
        Some(0.000_02)
    );
    assert_eq!(
        loaded.dark.as_ref().unwrap().metadata.derived.calibration(),
        CalibrationState::BIAS
    );
    assert_eq!(
        loaded.bias.as_ref().unwrap().metadata.derived.calibration(),
        CalibrationState::NONE
    );
    assert_eq!(
        loaded.components().collect::<Vec<_>>(),
        masters.components().collect::<Vec<_>>()
    );
    assert_eq!(loaded.defect_summary(), masters.defect_summary());
    let (loaded_defects, defects) = (
        loaded.defect_map.as_ref().unwrap(),
        masters.defect_map.as_ref().unwrap(),
    );
    assert_eq!(loaded_defects.hot(), defects.hot());
    assert_eq!(loaded_defects.cold_idxs(), [10]);

    // Each way a bundle can be malformed, rewritten with fresh checksums so the check behind them
    // is what refuses it. The HDUs are the primary, the dark, its flags, the flat, the bias and the
    // defect table, whose rows are 13 bytes — KIND, INDEX (8, big-endian), EXCESS (4) — hot 5 and
    // 10, then cold 10.
    let malformed = dir.join("malformed.fits");
    let refused = |edit: &mut dyn FnMut(usize, &mut Header, &mut Vec<u8>) -> bool| {
        fs::write(&malformed, &cache_bytes).unwrap();
        rewrite_fits(&malformed, edit);
        LoadCtx::default()
            .calibration_masters(&malformed)
            .unwrap_err()
    };
    let set = |hdu: usize, key: &'static str, value: &'static str| {
        refused(&mut |idx, header, _| {
            if idx == hdu {
                header.set(key, value).unwrap();
            }
            true
        })
    };
    assert!(matches!(
        set(0, FORMAT_KEYWORD, "OTHER"),
        BundleError::NotABundle
    ));
    assert!(matches!(
        set(4, "EXTNAME", "MASTER_FOO"),
        BundleError::UnknownExtension { extname } if extname == "MASTER_FOO"
    ));
    assert!(matches!(
        set(3, "EXTNAME", "MASTER_BIAS"),
        BundleError::DuplicateExtension { extname } if extname == "MASTER_BIAS"
    ));
    assert!(matches!(
        refused(&mut |idx, _, _| !(1..=3).contains(&idx)),
        BundleError::DefectsWithoutMaster
    ));
    let table = |at: usize, bytes: &[u8]| {
        refused(&mut |idx, _, data| {
            if idx == 5 {
                data[at..at + bytes.len()].copy_from_slice(bytes);
            }
            true
        })
    };
    for (error, expected) in [
        (table(0, &[2]), DefectTableError::Kind { row: 0, kind: 2 }),
        (
            table(2 * 13 + 1, &16i64.to_be_bytes()),
            DefectTableError::Idx { row: 2 },
        ),
        (
            table(13 + 9, &(-1.0f32).to_be_bytes()),
            DefectTableError::Excess { row: 1 },
        ),
        (
            table(2 * 13 + 9, &0.5f32.to_be_bytes()),
            DefectTableError::Excess { row: 2 },
        ),
    ] {
        assert!(
            matches!(error, BundleError::DefectTable(found) if found == expected),
            "{error:?}"
        );
    }

    let mut actual = constant_cfa(Size2us::new(4, 4), 0.75, cfa_type);
    loaded.calibrate(&mut actual).unwrap();
    assert_eq!(bits(actual.data().pixels()), bits(expected.data().pixels()));
}

#[test]
fn empty_master_fits_bundle_round_trips_as_a_checksummed_primary_hdu() {
    let dir = TempDir::new("lumos-empty-masters");
    let path = dir.join("masters.fits");
    writer::calibration_bundle(&CalibrationMasters::default(), &path).unwrap();

    let bytes = fs::read(&path).unwrap();
    let mut reader = FitsReader::from_bytes(&bytes).unwrap();
    assert_eq!(reader.hdus().len(), 1);
    let report = reader.verify_checksum(0).unwrap();
    assert_eq!(report.datasum, ChecksumStatus::Valid);
    assert_eq!(report.checksum, ChecksumStatus::Valid);

    let loaded = LoadCtx::default().calibration_masters(&path).unwrap();
    assert_eq!(loaded.components().collect::<Vec<_>>(), []);
}

#[test]
fn ram_bytes_sums_present_frames_and_defects() {
    // A 10×8 mono CFA frame holds 80 f32 pixels = 320 bytes.
    let dark = constant_cfa(Size2us::new(10, 8), 0.1, CfaType::Mono);
    assert_eq!(dark.ram_bytes(), 10 * 8 * 4);

    // A defect map holds its index lists and its mask. The mask's rows pad to 128 bits, so a
    // 10-wide row is two words: 8 rows × 2 × 8 B = 128 B, all of an empty map. Three indices add
    // 3 × 8 B.
    let mask_bytes = 8 * 2 * size_of::<u64>();
    assert_eq!(DefectMap::new(Size2us::new(10, 8)).ram_bytes(), mask_bytes);
    let defects = DefectMap::from_lists(
        Size2us::new(10, 8),
        HotPixels {
            idxs: vec![1, 2],
            excess: vec![0.5, 0.25],
        },
        vec![7],
    )
    .unwrap();
    let list_bytes = 3 * size_of::<usize>() + 2 * size_of::<f32>();
    assert_eq!(defects.ram_bytes(), list_bytes + mask_bytes);

    // The bundle sums present roles + the defect map; absent roles add nothing.
    let masters = CalibrationMasters {
        bias: None,
        dark: Some(dark),
        flat: Some(PreparedFlat::from_divisor(constant_cfa(
            Size2us::new(4, 4),
            1.0,
            CfaType::Mono,
        ))),
        defect_map: Some(defects),
        imposed: None,
    };
    // 320 (dark: 80·4) + 64 (flat: 16·4) + the flat's gain, a node at pixels 0 and 4 of each
    // 4-pixel axis, 2 × 2 nodes of 4 B, 16 + the defects' 32 + 128.
    assert_eq!(
        masters.ram_bytes(),
        10 * 8 * 4 + 4 * 4 * 4 + 2 * 2 * 4 + list_bytes + mask_bytes
    );
    // Three of the 80 pixels of a mono sensor: 3.75%.
    let summary = masters.defect_summary().unwrap();
    assert_eq!(
        summary,
        DefectSummary {
            hot_pixels: 2,
            cold_pixels: 1,
            defective_pixels: 3,
            pixels: 80,
            colors: ColorShares::Mono(ColorDefects {
                hot: 2,
                cold: 1,
                defective: 3,
                photosites: 80,
            }),
        }
    );
    assert_eq!(summary.percentage(), 3.75);
}

#[test]
fn a_master_stack_rejects_an_invalid_config_before_reading_anything() {
    // `MasterStack` builds its own CFA cache, so it is the entry point most likely to drift
    // out of the shared `combine_cached` gate and reach the reducer unvalidated. A NaN sigma
    // rejects every sample at every pixel and yields a silently black master; a negative one
    // inverts the clip band and faults on the survivor range. Both are configuration errors and
    // must be reported as such.
    // A path inside a fresh empty directory names no file, wherever the test runs.
    let dir = TempDir::new("lumos-invalid-config");
    let missing = [dir.join("missing.fits")];

    for rejection in [Rejection::sigma_clip(f32::NAN), Rejection::sigma_clip(-1.0)] {
        let config = StackConfig {
            combine: Combine {
                method: CombineMethod::Mean(rejection),
                small_n: SmallN::median_below(5),
            },
            ..StackConfig::bias_or_dark()
        };
        let error = MasterStack {
            paths: &missing,
            role: MasterRole::Dark,
            config,
            ingest: &IngestConfig::default(),
            subtract: &[],
        }
        .run(ProgressMonitor::unattended())
        .unwrap_err();
        assert!(
            matches!(
                error,
                MasterStackError::Stack(StackError::Config(StackConfigError::Field(invalid))) if invalid.field == "sigma.low"
            ),
            "expected the config to be rejected, got {error:?}"
        );
    }

    // The frame-count half of the gate, which only reaches this entry point because it routes
    // through `combine_cached`: a manual weight per frame, against one path.
    let error = MasterStack {
        paths: &missing,
        role: MasterRole::Dark,
        config: StackConfig {
            weighting: Weighting::Manual(vec![1.0, 1.0]),
            ..StackConfig::bias_or_dark()
        },
        ingest: &IngestConfig::default(),
        subtract: &[],
    }
    .run(ProgressMonitor::unattended())
    .unwrap_err();
    assert!(
        matches!(
            error,
            MasterStackError::Stack(StackError::Config(
                StackConfigError::ManualWeightCountMismatch {
                    expected: 1,
                    actual: 2
                }
            ))
        ),
        "expected the weight count to be checked against the frame count, got {error:?}"
    );

    // Reported from the config alone: the paths are never opened, so a valid config on the same
    // missing files fails differently.
    let error = MasterStack {
        paths: &missing,
        role: MasterRole::Dark,
        config: StackConfig::bias_or_dark(),
        ingest: &IngestConfig::default(),
        subtract: &[],
    }
    .run(ProgressMonitor::unattended())
    .unwrap_err();
    assert!(
        !matches!(error, MasterStackError::Stack(StackError::Config(_))),
        "a valid config must get past validation and fail on the missing file, got {error:?}"
    );
}

/// A frame on a 4096 span whose pedestal is `pedestal`.
fn with_pedestal(mut frame: CfaImage, pedestal: Pedestal) -> CfaImage {
    frame.metadata.derived.set_domain(Some(SampleDomain {
        pedestal,
        ..raw_domain(4096.0)
    }));
    frame
}

/// A flat or a light that may still hold an offset is never divided without a subtractor:
/// `(S + b)/flat` puts the offset under the vignetting. A flat whose pedestal the
/// decoder removed, as a RAW decode does, needs none; one that kept it or does not say is refused.
/// The light's rule is the same, once the bundle holds a flat and no bias or dark that holds the
/// offset: a dark that lost its bias when it was stacked removes none. A flat-dark that lost its
/// bias removes the dark signal alone, so with no bias the flat keeps its offset and is refused. A
/// master that lost more than its role can — a bias anything, a flat-dark its dark signal too — is
/// refused as such.
#[test]
fn an_offset_is_never_divided_by_the_flat() {
    let size = Size2us::new(4, 4);
    let flat = |pedestal| with_pedestal(constant_cfa(size, 0.5, CfaType::Mono), pedestal);
    for pedestal in [Pedestal::Kept(256.0), Pedestal::Unknown] {
        assert_eq!(
            CalibrationMasters::from_images(
                CalibrationSet {
                    flat: Some(flat(pedestal)),
                    ..Default::default()
                },
                SigmaThreshold::DEFAULT,
                &CancelToken::never(),
            )
            .unwrap_err(),
            CalibrationError::FlatWithoutSubtractor,
            "{pedestal:?}"
        );
    }
    let masters = bundle(CalibrationSet {
        flat: Some(flat(Pedestal::Removed)),
        ..Default::default()
    });
    let mut kept = with_pedestal(
        constant_cfa(size, 0.5, CfaType::Mono),
        Pedestal::Kept(256.0),
    );
    assert_eq!(
        masters.calibrate(&mut kept),
        Err(CalibrationError::LightWithoutSubtractor)
    );
    assert_eq!(kept.metadata.derived.calibration(), CalibrationState::NONE);
    let mut thermal = constant_cfa(size, 0.0625, CfaType::Mono);
    thermal
        .metadata
        .derived
        .set_calibration(CalibrationState::BIAS);
    let with_thermal_dark = bundle(CalibrationSet {
        flat: Some(flat(Pedestal::Removed)),
        dark: Some(thermal),
        ..Default::default()
    });
    assert_eq!(
        with_thermal_dark.calibrate(&mut kept),
        Err(CalibrationError::LightWithoutSubtractor)
    );
    for (role, lost, expected) in [
        (
            MasterRole::Bias,
            CalibrationState::BIAS,
            CalibrationError::OverCalibratedMaster {
                component: MasterRole::Bias,
            },
        ),
        (
            MasterRole::FlatDark,
            CalibrationState::ADDITIVE,
            CalibrationError::OverCalibratedMaster {
                component: MasterRole::FlatDark,
            },
        ),
        (
            MasterRole::FlatDark,
            CalibrationState::BIAS,
            CalibrationError::FlatWithoutSubtractor,
        ),
    ] {
        let mut subtractor = constant_cfa(size, 0.125, CfaType::Mono);
        subtractor.metadata.derived.set_calibration(lost);
        let mut set = CalibrationSet {
            flat: Some(flat(Pedestal::Kept(256.0))),
            ..Default::default()
        };
        match role {
            MasterRole::Bias => set.bias = Some(subtractor),
            _ => set.flat_dark = Some(subtractor),
        }
        assert_eq!(
            CalibrationMasters::from_images(set, SigmaThreshold::DEFAULT, &CancelToken::never())
                .unwrap_err(),
            expected,
            "{role:?} that lost {lost:?}"
        );
    }
    let mut removed = with_pedestal(constant_cfa(size, 0.5, CfaType::Mono), Pedestal::Removed);
    assert_eq!(
        masters.calibrate(&mut removed),
        Ok(CalibrationOutcome::default())
    );
    assert_eq!(removed.data().pixels(), &[0.5; 16]);
}

/// Calibration flags saturation before it moves the samples, so no level is applied after it. A
/// light with no decoder flags saturates at 0.95:
/// - its saturated 1.0, less a dark of 1/8, reads 0.875 and stays flagged;
/// - its 0.92 under a flat of 0.9 reads 1.022 and is not flagged.
///
/// A light whose decoder flagged saturation keeps the decoder's flags: its unflagged 0.97 stays
/// unflagged. Either way the light records that its flags are complete, drops the `DATAMAX` the
/// samples moved away from, and keeps both through a FITS save and reload.
#[test]
fn calibration_records_saturation_before_it_moves_the_samples() {
    let size = Size2us::new(2, 2);
    let saturated = |light: &CfaImage| {
        (0..4)
            .filter(|&idx| {
                light
                    .flags()
                    .is_some_and(|flags| flags.at(idx).intersects(QualityFlags::SATURATED))
            })
            .collect::<Vec<_>>()
    };

    let dark = bundle(CalibrationSet {
        dark: Some(constant_cfa(size, 0.125, CfaType::Mono)),
        ..Default::default()
    });
    let mut light = make_cfa(size, vec![1.0, 0.5, 0.5, 0.5], CfaType::Mono);
    light.metadata.header.data_max = Some(1.0);
    dark.calibrate(&mut light).unwrap();
    assert_eq!(light.data().pixels()[0], 0.875);
    assert_eq!(saturated(&light), [0]);
    assert!(light.metadata.derived.saturation_flagged());
    assert_eq!(light.metadata.header.data_max, None);

    let dir = TempDir::new("lumos-calibrated-saturation");
    let path = dir.path().join("light.fits");
    writer::cfa_fits(&light, &path).unwrap();
    let loaded = LoadCtx::default().cfa(&path).unwrap();
    assert!(loaded.metadata.derived.saturation_flagged());
    assert_eq!(saturated(&loaded), [0]);

    let flat = bundle(CalibrationSet {
        flat: Some(make_cfa(size, vec![1.0, 0.9, 1.05, 1.05], CfaType::Mono)),
        ..Default::default()
    });
    let mut light = make_cfa(size, vec![0.5, 0.92, 0.5, 0.5], CfaType::Mono);
    flat.calibrate(&mut light).unwrap();
    assert_eq!(light.data().pixels()[1], 0.92f32 / 0.9);
    assert_eq!(saturated(&light), Vec::<usize>::new());
    assert!(light.metadata.derived.saturation_flagged());

    let mut decoded = make_cfa(size, vec![0.5, 0.5, 0.97, 0.5], CfaType::Mono);
    decoded.metadata.derived.set_saturation_flagged(true);
    dark.calibrate(&mut decoded).unwrap();
    assert_eq!(saturated(&decoded), Vec::<usize>::new());
}
/// A dark compared on the exposure, and on no temperature: the two share none.
const UNSHARED_TEMPERATURE: DarkComparison = DarkComparison {
    exposure_compared: true,
    temperature: None,
    temperature_difference: None,
};

/// A dark compared on the sensor's temperature and the exposure, found apart by `difference`.
fn sensor_compared(difference: Option<TemperatureDifference>) -> DarkComparison {
    DarkComparison {
        exposure_compared: true,
        temperature: Some(TemperatureSource::Sensor),
        temperature_difference: difference,
    }
}

/// A dark is matched to the light. Lights of 300 s at 0.5, a bias of 0.125 and a dark
/// of 120 s holding the bias and a thermal signal of 0.0625.
/// - Without the bias the dark cannot be separated, and is refused.
/// - With it, the dark keeps 0.0625 of thermal signal, scaled by 300/120 = 2.5 to 0.15625: the
///   light is 0.5 − 0.125 − 0.15625 = 0.21875, exact in binary.
/// - At the light's own exposure, or within 1% of it, no scale; with an exposure undeclared on
///   either side, no scale and the outcome says it was not compared.
/// - Temperatures 3 °C apart calibrate as any others, the difference in the outcome and the
///   light's temperature unverified; with one undeclared, as a DSLR's, the outcome says so.
/// - A dark that lost its bias per frame when it was stacked, holds the thermal
///   0.0625 alone and is not given the bias a second time: with the bias the light is 0.21875
///   again, and without it the light keeps its bias, 0.5 − 0.15625 = 0.34375.
#[test]
fn a_dark_is_matched_to_the_light() {
    let size = Size2us::new(4, 4);
    let dark = |exposure: Option<f64>, temperature: Option<f64>| {
        let mut dark = constant_cfa(size, 0.1875, CfaType::Mono);
        dark.metadata.header.exposure_time = exposure;
        dark.metadata.header.ccd_temp = temperature;
        dark
    };
    let light = |exposure: Option<f64>, temperature: Option<f64>| {
        let mut light = constant_cfa(size, 0.5, CfaType::Mono);
        light.metadata.header.exposure_time = exposure;
        light.metadata.header.ccd_temp = temperature;
        light
    };
    let bias = || constant_cfa(size, 0.125, CfaType::Mono);
    // A flat dark has no hot pixel to fit on, so a bias-removed one is scaled by the exposures.
    let ratio = |factor| {
        Some(DarkScale {
            factor,
            source: DarkScaleSource::Exposure { fit_pixels: 0 },
        })
    };

    let unseparated = bundle(CalibrationSet {
        dark: Some(dark(Some(120.0), Some(-10.0))),
        ..Default::default()
    });
    assert_eq!(
        unseparated.calibrate(&mut light(Some(300.0), Some(-10.0))),
        Err(CalibrationError::DarkMismatch {
            component: MasterRole::Dark,
            source: DarkMismatch::Exposure {
                frame: 300.0,
                dark: 120.0
            }
        })
    );

    let separated = bundle(CalibrationSet {
        dark: Some(dark(Some(120.0), Some(-10.0))),
        bias: Some(bias()),
        ..Default::default()
    });
    let mut scaled = light(Some(300.0), Some(-10.0));
    assert_eq!(
        separated.calibrate(&mut scaled),
        Ok(CalibrationOutcome {
            dark_scale: ratio(2.5),
            dark: Some(sensor_compared(None)),
        })
    );
    assert_eq!(scaled.data().pixels(), &[0.218_75; 16]);

    for exposure in [120.0, 121.0] {
        let mut matched = light(Some(exposure), Some(-10.0));
        assert_eq!(
            separated.calibrate(&mut matched),
            Ok(CalibrationOutcome {
                dark_scale: ratio(1.0),
                dark: Some(sensor_compared(None)),
            })
        );
        assert_eq!(matched.data().pixels(), &[0.3125; 16], "{exposure} s");
    }
    let mut warmer = light(Some(120.0), Some(-7.0));
    assert_eq!(
        separated.calibrate(&mut warmer),
        Ok(CalibrationOutcome {
            dark_scale: ratio(1.0),
            dark: Some(sensor_compared(Some(TemperatureDifference {
                reading: TemperatureSource::Sensor,
                frame: -7.0,
                dark: -10.0,
            }))),
        })
    );
    assert_eq!(warmer.data().pixels(), &[0.3125; 16]);
    // The light keeps the record its outcome reports.
    assert_eq!(
        warmer.metadata.derived.dark_comparison(),
        Some(sensor_compared(Some(TemperatureDifference {
            reading: TemperatureSource::Sensor,
            frame: -7.0,
            dark: -10.0,
        })))
    );
    // A condition one side does not state is reported, and recorded on the light.
    for (exposure, temperature, compared) in [
        (
            None,
            Some(-10.0),
            DarkComparison {
                exposure_compared: false,
                temperature: Some(TemperatureSource::Sensor),
                temperature_difference: None,
            },
        ),
        (
            Some(120.0),
            None,
            DarkComparison {
                exposure_compared: true,
                temperature: None,
                temperature_difference: None,
            },
        ),
    ] {
        let mut silent = light(exposure, temperature);
        assert_eq!(
            separated.calibrate(&mut silent),
            Ok(CalibrationOutcome {
                dark: Some(compared),
                dark_scale: ratio(1.0),
            })
        );
        assert_eq!(silent.metadata.derived.dark_comparison(), Some(compared));
    }

    let thermal = || {
        let mut dark = constant_cfa(size, 0.0625, CfaType::Mono);
        dark.metadata.header.exposure_time = Some(120.0);
        dark.metadata.header.ccd_temp = Some(-10.0);
        dark.metadata
            .derived
            .set_calibration(CalibrationState::BIAS);
        dark
    };
    for (bias, expected) in [(Some(bias()), 0.218_75), (None, 0.343_75)] {
        let stacked_calibrated = bundle(CalibrationSet {
            dark: Some(thermal()),
            bias,
            ..Default::default()
        });
        let mut scaled = light(Some(300.0), Some(-10.0));
        assert_eq!(
            stacked_calibrated.calibrate(&mut scaled),
            Ok(CalibrationOutcome {
                dark_scale: ratio(2.5),
                dark: Some(sensor_compared(None)),
            })
        );
        assert_eq!(scaled.data().pixels(), &[expected; 16]);
    }
}

/// A flat-dark is matched to the flat as a dark is to a light. A flat of 2 s at −10 °C over the
/// vignetting `v` = (0.75, 1.0), `0.5·v` above a bias of 1/32 and a thermal signal of 2/64; a
/// flat-dark of 1 s holding 1/64 of it.
/// - Without its bias the flat-dark is scaled by 2/1 to 2/64, and the flat loses that and the
///   bias, so a light `signal·v` above the bias is left `signal · mean(v)` within ε.
/// - A flat-dark of the flat's own 2 s is not scaled, so the flat keeps 1/64 of the thermal
///   signal: its first pixel is 0.375 + 1/64 = 0.390625 against a mean of 0.453125, and the light
///   there reads 0.1875 · 0.453125 / 0.390625 = 0.2175 rather than 0.21875.
/// - A condition the flat does not state, or one its frames' flat-dark match left uncompared, is
///   reported by the bundle.
/// - With the bias still in it and no bias to separate it, the flat-dark cannot be scaled, and is
///   refused. One 1.5 °C warmer than the flat is taken, and its temperature reported unverified.
#[test]
fn a_flat_dark_is_matched_to_the_flat() {
    let size = Size2us::new(2, 1);
    let (signal, vignetting) = ([0.25f32, 0.5], [0.75f32, 1.0]);
    let (bias, thermal) = (0.031_25f32, 0.015_625f32);
    let conditions = |mut frame: CfaImage, exposure: f64, temperature: f64| {
        frame.metadata.header.exposure_time = Some(exposure);
        frame.metadata.header.ccd_temp = Some(temperature);
        frame
    };
    let flat = || {
        conditions(
            make_cfa(
                size,
                (0..2)
                    .map(|i| 0.5 * vignetting[i] + bias + 2.0 * thermal)
                    .collect(),
                CfaType::Mono,
            ),
            2.0,
            -10.0,
        )
    };
    let flat_dark = |temperature| {
        conditions(
            constant_cfa(size, bias + thermal, CfaType::Mono),
            1.0,
            temperature,
        )
    };
    let masters = bundle(CalibrationSet {
        flat: Some(flat()),
        bias: Some(constant_cfa(size, bias, CfaType::Mono)),
        flat_dark: Some(flat_dark(-10.0)),
        ..Default::default()
    });
    let mut light = make_cfa(
        size,
        (0..2).map(|i| signal[i] * vignetting[i] + bias).collect(),
        CfaType::Mono,
    );
    masters.calibrate(&mut light).unwrap();
    let mean_v = f32::midpoint(vignetting[0], vignetting[1]);
    for (i, &signal) in signal.iter().enumerate() {
        let expected = signal * mean_v;
        assert_close!(
            light.data()[i],
            expected,
            f32::EPSILON * expected,
            "pixel {i}"
        );
    }
    let unscaled = bundle(CalibrationSet {
        flat: Some(flat()),
        bias: Some(constant_cfa(size, bias, CfaType::Mono)),
        flat_dark: Some(conditions(flat_dark(-10.0), 2.0, -10.0)),
        ..Default::default()
    });
    let mut light = make_cfa(
        size,
        (0..2).map(|i| signal[i] * vignetting[i] + bias).collect(),
        CfaType::Mono,
    );
    unscaled.calibrate(&mut light).unwrap();
    assert_close!(light.data()[0], 0.2175, 2.0 * f32::EPSILON * 0.2175);

    // What the match compared is kept on the prepared flat: a flat that states no temperature
    // leaves it uncompared here, and one whose frames each took a flat-dark of no stated exposure
    // keeps that record.
    assert_eq!(masters.flat_dark(), Some(sensor_compared(None)));
    let mut silent = flat();
    silent.metadata.header.ccd_temp = None;
    let mut stacked = flat();
    stacked
        .metadata
        .derived
        .set_calibration(CalibrationState::THERMAL);
    stacked
        .metadata
        .derived
        .set_dark_comparison(Some(DarkComparison {
            exposure_compared: false,
            temperature: Some(TemperatureSource::Sensor),
            temperature_difference: None,
        }));
    for (flat, compared) in [
        (silent, UNSHARED_TEMPERATURE),
        (
            stacked,
            DarkComparison {
                exposure_compared: false,
                temperature: Some(TemperatureSource::Sensor),
                temperature_difference: None,
            },
        ),
    ] {
        let masters = bundle(CalibrationSet {
            flat: Some(flat),
            bias: Some(constant_cfa(size, bias, CfaType::Mono)),
            flat_dark: Some(flat_dark(-10.0)),
            ..Default::default()
        });
        assert_eq!(masters.flat_dark(), Some(compared));
    }

    assert_eq!(
        CalibrationMasters::from_images(
            CalibrationSet {
                flat: Some(flat()),
                flat_dark: Some(flat_dark(-10.0)),
                ..Default::default()
            },
            SigmaThreshold::DEFAULT,
            &CancelToken::never(),
        )
        .unwrap_err(),
        CalibrationError::DarkMismatch {
            component: MasterRole::FlatDark,
            source: DarkMismatch::Exposure {
                frame: 2.0,
                dark: 1.0,
            },
        }
    );
    let warmer = bundle(CalibrationSet {
        flat: Some(flat()),
        bias: Some(constant_cfa(size, bias, CfaType::Mono)),
        flat_dark: Some(flat_dark(-8.5)),
        ..Default::default()
    });
    // A flat-dark 1.5 °C warmer than the flat, past the tolerance, is reported as a dark is.
    assert_eq!(
        warmer.flat_dark(),
        Some(sensor_compared(Some(TemperatureDifference {
            reading: TemperatureSource::Sensor,
            frame: -10.0,
            dark: -8.5,
        })))
    );
}

/// Each flat takes its bias before the flats are normalized and combined. Two 8 × 8
/// flats of a field `f` that is 1 everywhere but 0.5 at the four corners, at 0.25·f and 0.5·f, both
/// on an offset of 1/32, with that offset as the bias. The dimmer flat is the reference whether
/// the two noiseless frames' noise estimates tie, the first winning, or follow their structure.
/// - Subtracted per frame they are 0.25·f and 0.5·f; the second's median 0.5 against the first's
///   0.25 gives it gain 1/2, and the mean is 0.25·f: a corner reads half the centre, exactly.
/// - Normalized first, the medians are 0.28125 and 0.53125, and the second's gain 9/17 scales its
///   offset too. The centre averages to 0.28125 and a corner to (0.15625 + 0.28125·9/17)/2 =
///   0.152574; less the offset, a corner reads 0.121324/0.25 = 0.4853 of the centre: a vignetting
///   residual of −2.9%.
///
/// Both on the memory tier and on the disk tier, which spills the subtracted frames for the run.
#[test]
fn flats_are_calibrated_before_they_are_combined() {
    let size = Size2us::new(8, 8);
    let offset = 1.0 / 32.0;
    let field = |x: usize, y: usize| {
        if (x == 0 || x == 7) && (y == 0 || y == 7) {
            0.5
        } else {
            1.0
        }
    };
    let dir = TempDir::new("lumos-flat-calibration");
    let paths: Vec<PathBuf> = [0.25f32, 0.5]
        .into_iter()
        .enumerate()
        .map(|(idx, level)| {
            let flat = make_cfa(
                size,
                (0..size.pixel_count())
                    .map(|i| offset + level * field(i % 8, i / 8))
                    .collect(),
                CfaType::Mono,
            );
            let path = dir.join(format!("flat_{idx}.fits"));
            writer::cfa_fits(&flat, &path).unwrap();
            path
        })
        .collect();
    let bias = constant_cfa(size, offset, CfaType::Mono);
    let corner_over_centre = |master: &CfaImage| master.data()[(0, 0)] / master.data()[(3, 3)];
    for memory_override in [None, Some(1)] {
        let config = StackConfig {
            combine: Combine {
                small_n: SmallN::none(),
                ..Combine::sigma_clip(2.5)
            },
            ..StackConfig::flat()
        };
        let ingest = IngestConfig {
            memory_override,
            cache_dir: dir.join("cache"),
            ..IngestConfig::default()
        };
        let calibrated = MasterStack {
            paths: &paths,
            role: MasterRole::Flat,
            config: config.clone(),
            ingest: &ingest,
            subtract: &[Subtractor {
                role: MasterRole::Bias,
                master: &bias,
            }],
        }
        .run(ProgressMonitor::unattended())
        .unwrap()
        .unwrap();
        assert_eq!(
            calibrated.metadata.derived.calibration(),
            CalibrationState::BIAS
        );
        assert_eq!(corner_over_centre(&calibrated), 0.5, "{memory_override:?}");

        let mut late = MasterStack {
            paths: &paths,
            role: MasterRole::Flat,
            config,
            ingest: &ingest,
            subtract: &[],
        }
        .run(ProgressMonitor::unattended())
        .unwrap()
        .unwrap();
        subtract_bias(&mut late, &bias);
        let ratio = corner_over_centre(&late);
        assert!(
            (ratio - 0.4853).abs() < 1e-4,
            "{memory_override:?}: {ratio}"
        );
    }
}

/// The builder stacks each role under its preset and takes the additive parts from every flat
/// before the combine, as `flats_are_calibrated_before_they_are_combined` does by hand: the bias,
/// then the flat-dark's dark signal scaled to the flat. The flat-darks are 1 s of the bias 1/32
/// plus 1/64 of dark signal; the flats are 2 s of the bias, twice that dark signal, and their
/// light. Taken from each flat, the bias leaves the dark signal, which scales by 2, so the flat
/// master's corner reads exactly 0.5 of its centre. Before, the flat-dark went to the flats with
/// its bias in, and a dark holding its bias cannot scale, so the 2 s flats were refused.
///
/// Its cache is read before each role is stacked and written once it is, under the key its read
/// gave: it stores the bias and the flat-dark with nothing subtracted and the flats with both
/// masters' frames, and a dark it already holds is not stacked — its frames name no file, and the
/// build still succeeds with the held dark.
#[test]
fn the_builder_calibrates_the_flats_and_keeps_each_master() {
    /// One master the cache was asked to keep: its role, its frame count, and the role and frame
    /// count of each master taken from its frames.
    #[derive(Debug, PartialEq)]
    struct Kept {
        role: MasterRole,
        frames: usize,
        subtracted: Vec<(MasterRole, usize)>,
    }
    #[derive(Debug, Default)]
    struct Recorder {
        held_dark: Option<CfaImage>,
        stored: Mutex<Vec<Kept>>,
    }
    impl MasterCache<PathBuf> for Recorder {
        type Error = Infallible;
        type Key = Kept;

        fn load(
            &self,
            inputs: &MasterInputs<'_, PathBuf>,
        ) -> Result<CachedMaster<Kept>, Infallible> {
            Ok(match (inputs.role, &self.held_dark) {
                (MasterRole::Dark, Some(dark)) => CachedMaster::Kept(Box::new(dark.clone())),
                _ => CachedMaster::Missing(Kept {
                    role: inputs.role,
                    frames: inputs.frames.len(),
                    subtracted: inputs
                        .subtracted
                        .iter()
                        .map(|subtracted| (subtracted.role, subtracted.frames.len()))
                        .collect(),
                }),
            })
        }

        fn store(&self, key: Kept, _: &CfaImage) -> Result<(), Infallible> {
            self.stored.lock().unwrap().push(key);
            Ok(())
        }
    }

    let size = Size2us::new(8, 8);
    let offset = 1.0 / 32.0;
    let dir = TempDir::new("lumos-master-builder");
    let write = |name: &str, exposure: f64, value: &dyn Fn(usize) -> f32| -> PathBuf {
        let mut frame = make_cfa(
            size,
            (0..size.pixel_count()).map(value).collect(),
            CfaType::Mono,
        );
        frame.metadata.header.exposure_time = Some(exposure);
        let path = dir.join(name);
        writer::cfa_fits(&frame, &path).unwrap();
        path
    };
    let vignette = |i: usize| {
        let (x, y) = (i % 8, i / 8);
        if (x == 0 || x == 7) && (y == 0 || y == 7) {
            0.5
        } else {
            1.0
        }
    };
    let dark_per_second = 1.0 / 64.0;
    let bias: Vec<PathBuf> = (0..2)
        .map(|idx| write(&format!("bias_{idx}.fits"), 0.0, &|_| offset))
        .collect();
    let flat_darks: Vec<PathBuf> = (0..2)
        .map(|idx| {
            write(&format!("flat_dark_{idx}.fits"), 1.0, &|_| {
                offset + dark_per_second
            })
        })
        .collect();
    let flats: Vec<PathBuf> = [0.5f32, 0.25]
        .into_iter()
        .enumerate()
        .map(|(idx, level)| {
            write(&format!("flat_{idx}.fits"), 2.0, &|i| {
                offset + 2.0 * dark_per_second + level * vignette(i)
            })
        })
        .collect();
    let darks = vec![dir.join("absent_dark.fits")];
    let mut held_dark = constant_cfa(size, 0.01, CfaType::Mono);
    held_dark
        .metadata
        .derived
        .set_calibration(CalibrationState::BIAS);
    let cache = Recorder {
        held_dark: Some(held_dark),
        ..Recorder::default()
    };
    let masters = CalibrationMasters::build(
        MasterBuild {
            frames: CalibrationSet {
                dark: darks.as_slice(),
                flat: flats.as_slice(),
                bias: bias.as_slice(),
                flat_dark: flat_darks.as_slice(),
            },
            sigma_threshold: SigmaThreshold::DEFAULT,
            cache: &cache,
        },
        &IngestConfig::with_cache_dir(dir.join("cache")),
        ProgressMonitor::unattended(),
    )
    .unwrap();
    let flat = masters.flat.as_ref().unwrap().divisor();
    assert_eq!(flat.data()[(0, 0)] / flat.data()[(3, 3)], 0.5);
    assert!(masters.dark.is_some(), "the held dark is in the set");
    assert_eq!(
        *cache.stored.lock().unwrap(),
        [
            Kept {
                role: MasterRole::Bias,
                frames: 2,
                subtracted: vec![],
            },
            Kept {
                role: MasterRole::FlatDark,
                frames: 2,
                subtracted: vec![],
            },
            Kept {
                role: MasterRole::Flat,
                frames: 2,
                subtracted: vec![(MasterRole::Bias, 2), (MasterRole::FlatDark, 2)],
            },
        ]
    );
}

/// A cancel inside a role's stack reaches the caller as the build's own cancel, as one between
/// roles does, not as a failed stack: a cache that cancels the build as it is read for the bias,
/// whose two frames are then stacked under the cancel.
#[test]
fn a_cancel_inside_a_stack_is_the_builds_cancel() {
    #[derive(Debug)]
    struct Cancelling(CancelToken);
    impl MasterCache<PathBuf> for Cancelling {
        type Error = Infallible;
        type Key = ();

        fn load(&self, _: &MasterInputs<'_, PathBuf>) -> Result<CachedMaster<()>, Infallible> {
            self.0.cancel();
            Ok(CachedMaster::Missing(()))
        }

        fn store(&self, (): (), _: &CfaImage) -> Result<(), Infallible> {
            Ok(())
        }
    }

    let dir = TempDir::new("lumos-master-build-cancel");
    let bias: Vec<PathBuf> = (0..2)
        .map(|idx| {
            let path = dir.join(format!("bias_{idx}.fits"));
            writer::cfa_fits(
                &constant_cfa(Size2us::new(8, 8), 1.0 / 32.0, CfaType::Mono),
                &path,
            )
            .unwrap();
            path
        })
        .collect();
    let cancel = CancelToken::new();
    let error = CalibrationMasters::build(
        MasterBuild {
            frames: CalibrationSet {
                dark: &[],
                flat: &[],
                bias: bias.as_slice(),
                flat_dark: &[],
            },
            sigma_threshold: SigmaThreshold::DEFAULT,
            cache: &Cancelling(cancel.clone()),
        },
        &IngestConfig::with_cache_dir(dir.join("cache")),
        ProgressMonitor::new(ProgressCallback::default(), cancel),
    )
    .unwrap_err();
    assert!(matches!(error, MasterBuildError::Cancelled), "{error:?}");
}

/// A master states the exposure and temperature its frames share; the frames of a dark have to
/// share the exposure, and a temperature they disagree on is warned of. Mono 4 × 4 frames on disk:
/// - darks of 120, 300 and 300 s are refused at frame 1, the first outside 1% of frame 0's 120 s;
/// - darks of 300 and 301 s at −10 and −10.5 °C agree, and the master states their means, 300.5 s
///   and −10.25 °C;
/// - darks of 300 s at −10 and −7 °C, 3 °C apart, stack, and the master states no temperature;
/// - flats of 1 and 2 s are a flat's business: they stack, and the master states no exposure;
/// - the same flats, the second with no stated temperature, each take a bias-free flat-dark of 1 s
///   at −10 °C: the master records the temperature as unverified, though frame 0, whose metadata
///   it keeps, compared it.
#[test]
fn dark_frames_share_one_exposure() {
    let dir = TempDir::new("lumos-master-conditions");
    let write = |name: &str, frames: &[(f64, Option<f64>)]| -> Vec<PathBuf> {
        frames
            .iter()
            .enumerate()
            .map(|(idx, &(exposure, temperature))| {
                let mut frame = constant_cfa(Size2us::new(4, 4), 0.25, CfaType::Mono);
                frame.metadata.header.exposure_time = Some(exposure);
                frame.metadata.header.ccd_temp = temperature;
                let path = dir.join(format!("{name}_{idx}.fits"));
                writer::cfa_fits(&frame, &path).unwrap();
                path
            })
            .collect()
    };
    let stack = |paths: &[PathBuf], role: MasterRole, subtract: &[Subtractor<'_>]| {
        MasterStack {
            paths,
            role,
            config: role.stack_config(),
            ingest: &IngestConfig::default(),
            subtract,
        }
        .run(ProgressMonitor::unattended())
    };

    let mixed = write(
        "mixed",
        &[
            (120.0, Some(-10.0)),
            (300.0, Some(-10.0)),
            (300.0, Some(-10.0)),
        ],
    );
    let error = stack(&mixed, MasterRole::Dark, &[]).unwrap_err();
    assert!(
        matches!(
            error,
            MasterStackError::Conditions {
                role: MasterRole::Dark,
                source: ConditionMismatch {
                    condition: CaptureCondition::Exposure,
                    idx: 1,
                    value: 300.0,
                    reference_idx: 0,
                    reference: 120.0,
                },
            }
        ),
        "{error:?}"
    );

    let agreeing = write("agreeing", &[(300.0, Some(-10.0)), (301.0, Some(-10.5))]);
    let master = stack(&agreeing, MasterRole::Dark, &[]).unwrap().unwrap();
    assert_eq!(master.metadata.header.exposure_time, Some(300.5));
    assert_eq!(master.metadata.header.ccd_temp, Some(-10.25));

    let drifting = write("drifting", &[(300.0, Some(-10.0)), (300.0, Some(-7.0))]);
    let master = stack(&drifting, MasterRole::Dark, &[]).unwrap().unwrap();
    assert_eq!(master.metadata.header.exposure_time, Some(300.0));
    assert_eq!(master.metadata.header.ccd_temp, None);

    let flats = write("flats", &[(1.0, Some(-10.0)), (2.0, Some(-10.0))]);
    let master = stack(&flats, MasterRole::Flat, &[]).unwrap().unwrap();
    assert_eq!(master.metadata.header.exposure_time, None);
    assert_eq!(master.metadata.header.ccd_temp, Some(-10.0));
    assert_eq!(master.metadata.derived.dark_comparison(), None);

    let mut flat_dark = constant_cfa(Size2us::new(4, 4), 0.031_25, CfaType::Mono);
    flat_dark.metadata.header.exposure_time = Some(1.0);
    flat_dark.metadata.header.ccd_temp = Some(-10.0);
    flat_dark
        .metadata
        .derived
        .set_calibration(CalibrationState::BIAS);
    let silent = write("silent", &[(1.0, Some(-10.0)), (2.0, None)]);
    let master = stack(
        &silent,
        MasterRole::Flat,
        &[Subtractor {
            role: MasterRole::FlatDark,
            master: &flat_dark,
        }],
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        master.metadata.derived.calibration(),
        CalibrationState::THERMAL
    );
    // The second frame shares no temperature with the flat-dark: the stack's record is the
    // union of the frames', its temperature uncompared.
    assert_eq!(
        master.metadata.derived.dark_comparison(),
        Some(UNSHARED_TEMPERATURE)
    );
}

/// Every flag a calibration sets is counted as it is set, and the counts are what a recount of the
/// plane finds: a light saturated at one pixel and holding a null at another, against a bias with
/// no measurement at a third, a dark with a hot pixel, and a flat at its floor at a fourth, is
/// left with `SATURATED`, `NO_DATA`, `REPAIRED`, `DEFECT` and `FLAT_FLOOR`, each where its cause
/// is, and the counts agree with the bytes.
#[test]
fn calibration_counts_its_flags_as_it_sets_them() {
    let size = Size2us::new(16, 16);
    let at = |x: usize, y: usize| y * size.width + x;
    let mut bias = constant_cfa(size, 0.0625, CfaType::Mono);
    bias.set_flags(PixelFlags::from_fn(size, |idx| {
        if idx == at(4, 4) {
            QualityFlags::NO_DATA
        } else {
            QualityFlags::default()
        }
    }));
    let mut dark = constant_cfa(size, 0.125, CfaType::Mono);
    dark.data_mut()[at(8, 8)] = 0.9;
    let mut flat = constant_cfa(size, 0.5, CfaType::Mono);
    flat.data_mut()[at(12, 3)] = 0.01;
    let masters = bundle(CalibrationSet {
        dark: Some(dark),
        flat: Some(flat),
        bias: Some(bias),
        flat_dark: None,
    });
    let mut light = constant_cfa(size, 0.5, CfaType::Mono);
    light.data_mut()[at(2, 10)] = 0.99;
    light.set_flags(PixelFlags::from_fn(size, |idx| {
        if idx == at(10, 12) {
            QualityFlags::NO_DATA
        } else {
            QualityFlags::default()
        }
    }));
    masters.calibrate(&mut light).unwrap();

    let flags = light.flags().unwrap();
    let bytes = flags.bytes();
    for flag in [
        QualityFlags::NO_DATA,
        QualityFlags::SATURATED,
        QualityFlags::DEFECT,
        QualityFlags::REPAIRED,
        QualityFlags::FLAT_FLOOR,
    ] {
        let recount = bytes
            .iter()
            .filter(|&&byte| QualityFlags::from_byte(byte).intersects(flag))
            .count();
        assert!(recount > 0, "{flag:?} was set somewhere");
        assert_eq!(
            flags.count(flag),
            u64::try_from(recount).unwrap(),
            "{flag:?}"
        );
    }
    assert!(flags.at(at(2, 10)).intersects(QualityFlags::SATURATED));
    assert!(flags.at(at(4, 4)).intersects(QualityFlags::NO_DATA));
    assert!(flags.at(at(10, 12)).intersects(QualityFlags::REPAIRED));
    assert!(flags.at(at(8, 8)).intersects(QualityFlags::DEFECT));
    assert!(flags.at(at(12, 3)).intersects(QualityFlags::FLAT_FLOOR));
}
