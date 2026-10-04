mod mem_budget;
#[cfg(feature = "real-data")]
mod real_data;
mod synthetic;

use crate::calibration_masters::DEFAULT_SIGMA_THRESHOLD;
use crate::calibration_masters::calibration_outcome::CalibrationOutcome;
use crate::calibration_masters::defect_map::DefectMap;
use crate::calibration_masters::error::CalibrationError;
use crate::calibration_masters::master_dark::{DarkBias, MasterDark};
use crate::calibration_masters::prepared_flat::PreparedFlat;
use crate::calibration_masters::stack_cfa_master;
use crate::combine::config::{CombineMethod, SmallN, StackConfig, Weighting};
use crate::combine::error::{StackConfigError, StackError};
use crate::combine::rejection::Rejection;
use crate::ingest::ingest_config::IngestConfig;
use crate::internals::assertions::bits;
use crate::internals::cfa::XTRANS_PATTERN;
use crate::internals::cfa::cfa_from_plane;
use crate::internals::cfa::{constant_cfa, make_cfa};
use crate::internals::fits::rewrite_fits;
use crate::internals::prelude::*;
use crate::io::image::cfa::{CfaImage, CfaType, QUANTIZATION_SIGMA_PER_STEP};
use crate::io::image::load_context::LoadContext;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::io::image::preview_image::PreviewImage;
use crate::io::image::sample_domain::{DomainMap, Pedestal, SampleDomain, ScaleOrigin};
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::io::raw::demosaic::xtrans::xtrans_pattern::XTransPattern;
use crate::progress::progress_callback::ProgressCallback;
use crate::{
    CalibrationComponent, CalibrationMasters, CalibrationSet, DefectSummary, ImageError,
    ImageMetadata, MasterRole,
};
use common::TempDir;
use fits_well::FitsReader;
use fits_well::image::Bitpix;
use fits_well::io::ChecksumStatus;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

/// The bundle `images` builds, at the default defect threshold.
fn bundle(images: CalibrationSet<Option<CfaImage>>) -> CalibrationMasters {
    CalibrationMasters::from_images(images, DEFAULT_SIGMA_THRESHOLD, &CancelToken::never()).unwrap()
}

#[test]
fn calibrating_a_calibrated_light_is_refused() {
    // A second calibrate() would subtract the dark / divide the flat twice. The flag can come from
    // the file (`LUMCAL`), so it is refused as input, and the light is left as it was.
    let mut images = CalibrationSet::default();
    *images.get_mut(MasterRole::Dark) =
        Some(constant_cfa(Size2us::new(4, 4), 0.125, CfaType::Mono));
    let masters = CalibrationMasters::from_images(images, 5.0, &CancelToken::never()).unwrap();
    let mut light = constant_cfa(Size2us::new(4, 4), 0.5, CfaType::Mono);
    masters.calibrate(&mut light).unwrap();
    assert!(light.metadata.calibrated);
    assert_eq!(light.data.pixels(), &[0.375; 16]);
    assert_eq!(
        masters.calibrate(&mut light),
        Err(CalibrationError::AlreadyCalibrated)
    );
    assert_eq!(light.data.pixels(), &[0.375; 16]);
}

/// A flat whose subtracted mean is not positive — a dark given as the flat, or a subtractor above
/// the flat's level — is refused when the set is built, per colour for a CFA flat.
#[test]
fn a_flat_with_no_positive_mean_is_refused() {
    for (cfa_type, flat, subtractor, channel) in [
        (CfaType::Mono, 0.0f32, None, None),
        (CfaType::Mono, 0.25, Some(0.5f32), None),
        (CfaType::Bayer(CfaPattern::Rggb), 0.25, Some(0.5), Some(0)),
    ] {
        let mut images = CalibrationSet::default();
        *images.get_mut(MasterRole::Flat) = Some(constant_cfa(Size2us::new(4, 4), flat, cfa_type));
        *images.get_mut(MasterRole::FlatDark) =
            subtractor.map(|level| constant_cfa(Size2us::new(4, 4), level, cfa_type));
        assert!(
            matches!(
                CalibrationMasters::from_images(images, 5.0, &CancelToken::never()),
                Err(CalibrationError::NonPositiveFlat { channel: c }) if c == channel
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
            CfaType::Bayer(CfaPattern::Rggb),
        )),
        flat_dark: Some(constant_cfa(
            Size2us::new(2, 2),
            0.1,
            CfaType::Bayer(CfaPattern::Bggr),
        )),
        ..CalibrationSet::default()
    };
    assert!(matches!(
        CalibrationMasters::from_images(images, DEFAULT_SIGMA_THRESHOLD, &CancelToken::never()),
        Err(CalibrationError::CfaPatternMismatch {
            component: MasterRole::FlatDark,
            expected: CfaType::Bayer(CfaPattern::Rggb),
            master: CfaType::Bayer(CfaPattern::Bggr),
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
    let xtrans_a = CfaType::XTrans(XTRANS_PATTERN);
    let mut shifted = *XTRANS_PATTERN.rows();
    shifted.rotate_left(1);
    let xtrans_b = CfaType::XTrans(XTransPattern::new(shifted).unwrap());
    let cases = [
        Case {
            role: MasterRole::Dark,
            light: CfaType::Mono,
            master: CfaType::Bayer(CfaPattern::Rggb),
            expected: CalibrationError::CfaPatternMismatch {
                component: MasterRole::Dark,
                expected: CfaType::Mono,
                master: CfaType::Bayer(CfaPattern::Rggb),
            },
        },
        Case {
            role: MasterRole::Flat,
            light: CfaType::Bayer(CfaPattern::Rggb),
            master: CfaType::Bayer(CfaPattern::Bggr),
            expected: CalibrationError::CfaPatternMismatch {
                component: MasterRole::Flat,
                expected: CfaType::Bayer(CfaPattern::Rggb),
                master: CfaType::Bayer(CfaPattern::Bggr),
            },
        },
        Case {
            role: MasterRole::Bias,
            light: CfaType::Bayer(CfaPattern::Rggb),
            master: xtrans_a,
            expected: CalibrationError::CfaPatternMismatch {
                component: MasterRole::Bias,
                expected: CfaType::Bayer(CfaPattern::Rggb),
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
        let original_data = light.data.to_vec();

        assert_eq!(masters.calibrate(&mut light), Err(case.expected));
        assert_eq!(light.data.pixels(), original_data);
        assert!(!light.metadata.calibrated);
    }

    // A master whose pattern matches but whose extent does not is bad input too — unchecked, it
    // would reach `CfaImage::subtract`'s assert, which is not a report a caller can act on. Every
    // role a bundle keeps is covered because each is applied by a different operation; the
    // flat-dark is spent on the flat, so a bundle never holds one to check.
    for component in [MasterRole::Dark, MasterRole::Flat, MasterRole::Bias] {
        let masters = masters_with_sized_component(component, CfaType::Mono, Size2us::new(4, 4));
        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        let original_data = light.data.to_vec();

        assert_eq!(
            masters.calibrate(&mut light),
            Err(CalibrationError::DimensionMismatch {
                component: component.into(),
                expected: Size2us::new(2, 2),
                master: Size2us::new(4, 4),
            })
        );
        assert_eq!(light.data.pixels(), original_data);
        assert!(!light.metadata.calibrated);
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
        master.metadata.domain = Some(raw_domain(65_532.0));
        let mut images = CalibrationSet::default();
        *images.get_mut(component) = Some(master);
        let masters = bundle(images);

        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light.metadata.domain = Some(raw_domain(16_383.0));
        assert_eq!(
            masters.calibrate(&mut light).map(drop),
            Ok(()),
            "{component:?}"
        );
        assert_eq!(light.data.pixels(), &[expected; 4], "{component:?}");
    }

    // A span the decoder had to assume cannot be converted: subtracting a `[0, 1]` master from a
    // light on another span would remove ~0.01 from ~3000 and then mark the light calibrated.
    // Refused for the master the light subtracts, before the light is touched.
    for component in [MasterRole::Dark, MasterRole::Bias] {
        let mut master = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        master.metadata.domain = Some(fits_domain(1.0, None, ScaleOrigin::Assumed));
        let mut images = CalibrationSet::default();
        *images.get_mut(component) = Some(master);
        let masters = bundle(images);

        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light.metadata.domain = Some(raw_domain(16_383.0));
        let original_data = light.data.to_vec();

        assert_eq!(
            masters.calibrate(&mut light),
            Err(CalibrationError::SampleDomainMismatch {
                component,
                frame: raw_domain(16_383.0),
                master: fits_domain(1.0, None, ScaleOrigin::Assumed),
            })
        );
        assert_eq!(light.data.pixels(), original_data);
        assert!(!light.metadata.calibrated);
    }

    // The same failure with no span to give it away: a master and a light on one span whose BUNIT
    // names different quantities. The subtraction would run and report success.
    {
        let mut master = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        master.metadata.domain = Some(fits_domain(1.0, Some("count/s"), ScaleOrigin::Declared));
        let mut images = CalibrationSet::default();
        *images.get_mut(MasterRole::Dark) = Some(master);
        let masters = bundle(images);

        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light.metadata.domain = Some(fits_domain(1.0, Some("Jy/beam"), ScaleOrigin::Declared));
        let original_data = light.data.to_vec();

        assert_eq!(
            masters.calibrate(&mut light),
            Err(CalibrationError::SampleDomainMismatch {
                component: MasterRole::Dark,
                frame: fits_domain(1.0, Some("Jy/beam"), ScaleOrigin::Declared),
                master: fits_domain(1.0, Some("count/s"), ScaleOrigin::Declared),
            })
        );
        assert_eq!(light.data.pixels(), original_data);
        assert!(!light.metadata.calibrated);

        // A master that states no unit is "cannot tell", not a disagreement, so the same pair
        // calibrates once the master stops naming a quantity — the unit is the axis, not its
        // presence.
        let mut master = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        master.metadata.domain = Some(fits_domain(1.0, None, ScaleOrigin::Declared));
        let mut images = CalibrationSet::default();
        *images.get_mut(MasterRole::Dark) = Some(master);
        let masters = bundle(images);
        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light.metadata.domain = Some(fits_domain(1.0, Some("Jy/beam"), ScaleOrigin::Declared));
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
        master.metadata.domain = master_span.map(raw_domain);
        let mut images = CalibrationSet::default();
        *images.get_mut(MasterRole::Dark) = Some(master);
        let masters = bundle(images);

        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light.metadata.domain = light_span.map(raw_domain);
        assert_eq!(
            masters.calibrate(&mut light).map(drop),
            Ok(()),
            "light {light_span:?} against master {master_span:?}"
        );
    }
}

/// Review item 4.4. A dark that kept its pedestal of 2048 ADU on a 65536 span, applied to a RAW
/// light with the black removed on a 16384 span: the map is gain 4 and offset −2048/16384 = −0.125,
/// so a dark holding only the pedestal, 2048/65536 = 0.03125, maps to 4 × 0.03125 − 0.125 = 0 and
/// leaves the light as it was. Before, the scale alone was applied, and the light lost 0.125.
/// Dyadic values, so the result is exact. The light's pedestal is then removed, and a dark whose
/// pedestal nobody recorded is refused rather than guessed.
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
        dark.metadata.domain = Some(SampleDomain {
            pedestal,
            ..kept.clone()
        });
        let mut light = constant_cfa(Size2us::new(2, 2), 0.5, CfaType::Mono);
        light.metadata.domain = Some(raw_domain(16_384.0));
        bundle(CalibrationSet {
            dark: Some(dark),
            ..Default::default()
        })
        .calibrate(&mut light)
        .map(|_| light)
    };

    let light = calibrate(Pedestal::Kept(2048.0)).unwrap();
    assert_eq!(light.data.pixels(), &[0.5; 4]);
    assert_eq!(light.metadata.domain.unwrap().pedestal, Pedestal::Removed);
    assert!(matches!(
        calibrate(Pedestal::Unknown),
        Err(CalibrationError::SampleDomainMismatch { .. })
    ));
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
        CalibrationMasters::from_images(mismatched, DEFAULT_SIGMA_THRESHOLD, &CancelToken::never())
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
            DEFAULT_SIGMA_THRESHOLD,
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
        CalibrationMasters::from_images(coherent, DEFAULT_SIGMA_THRESHOLD, &CancelToken::never())
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
    // One name table: the writer stamps `extname()` and `bundle_indices` recognizes the HDU by
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
        let master = stack_cfa_master(
            &empty,
            role.stack_config(),
            None,
            ProgressCallback::default(),
            CancelToken::never(),
        )
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
        // The flat-dark is spent on the flat and not kept (review 26.1).
        vec!["dark", "flat", "bias", "defects"]
    );
    assert_eq!(
        masters.defect_summary(),
        Some(DefectSummary {
            hot_pixels: 0,
            cold_pixels: 0,
            percentage: 0.0,
        })
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
        assert_eq!(defects.cold_indices(), [dead], "{kind:?}");

        let prepared = masters.flat.as_ref().unwrap().divisor();
        assert_eq!(prepared.data[dead], 0.1, "{kind:?}");
        let expected_normal = 49.0 / 48.0;
        assert!(
            (prepared.data[0] - expected_normal).abs() < 1e-6,
            "{kind:?}: {} != {expected_normal}",
            prepared.data[0]
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
        DEFAULT_SIGMA_THRESHOLD,
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
    assert_eq!(
        masters.defect_summary(),
        Some(DefectSummary {
            hot_pixels: 0,
            cold_pixels: 0,
            percentage: 0.0,
        })
    );
}

/// A light loses the dark, or the bias when there is no dark — never both. Dyadic levels keep
/// every subtraction exact: 0.5 − 0.125 = 0.375 and 0.5 − 0.0625 = 0.4375.
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
            light.data.iter().all(|&v| v == expected),
            "dark {dark:?}, bias {bias:?}: {:?}",
            light.data.pixels()
        );
    }
}

/// The flat divides out the vignetting `v` and leaves `signal · mean(v)`, whichever masters are
/// present — provided the light loses the dark (or else the bias) and the flat loses the
/// flat-dark (or else the bias). A light `signal·v + s_light` against a flat `k·v + s_flat`:
/// subtracting puts `signal·v` over `k·v`, normalized by `k·mean(v)`. Every level is dyadic, so
/// both subtractions and the mean are exact; the divisor `v/mean(v)` and the division round once
/// each, so every pixel lands within ε of `signal · mean(v)`. A flat that lost the wrong
/// subtractor would be off by the difference over `k·v`. A flat-dark marked calibrated holds its
/// thermal 1/64 alone, so the flat holds the bias under it, 1/32 + 1/64 = 3/64, and loses both.
#[test]
fn calibrate_divides_by_the_flat_less_its_own_subtractor() {
    let size = Size2us::new(2, 1);
    let (signal, vignetting, k) = ([0.25f32, 0.5], [0.75f32, 1.0], 0.5f32);
    let mean_v = f32::midpoint(vignetting[0], vignetting[1]);
    let (dark, bias, flat_dark) = (0.0625f32, 0.03125f32, 0.015_625f32);
    let level = |value| constant_cfa(size, value, CfaType::Mono);
    for (name, has_dark, has_bias, has_flat_dark, thermal_flat_dark) in [
        ("flat alone", false, false, false, false),
        ("dark and bias", true, true, false, false),
        ("dark and flat-dark", true, false, true, false),
        ("bias and flat-dark", false, true, true, false),
        ("all three", true, true, true, false),
        ("bias and thermal flat-dark", false, true, true, true),
        ("all three, thermal flat-dark", true, true, true, true),
    ] {
        let light_sub = if has_dark {
            dark
        } else if has_bias {
            bias
        } else {
            0.0
        };
        let flat_sub = if thermal_flat_dark {
            bias + flat_dark
        } else if has_flat_dark {
            flat_dark
        } else if has_bias {
            bias
        } else {
            0.0
        };
        let light: Vec<f32> = (0..2)
            .map(|i| signal[i] * vignetting[i] + light_sub)
            .collect();
        let flat: Vec<f32> = (0..2).map(|i| k * vignetting[i] + flat_sub).collect();
        let masters = bundle(CalibrationSet {
            dark: has_dark.then(|| level(dark)),
            flat: Some(make_cfa(size, flat, CfaType::Mono)),
            bias: has_bias.then(|| level(bias)),
            flat_dark: has_flat_dark.then(|| {
                let mut flat_dark = level(flat_dark);
                flat_dark.metadata.calibrated = thermal_flat_dark;
                flat_dark
            }),
        });
        let mut light = make_cfa(size, light, CfaType::Mono);
        masters.calibrate(&mut light).unwrap();
        for (i, &signal) in signal.iter().enumerate() {
            let expected = signal * mean_v;
            assert_close!(
                light.data[i],
                expected,
                f32::EPSILON * expected,
                "{name}, pixel {i}"
            );
        }
    }
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
        3.0,
        &CancelToken::never(),
    )
    .unwrap();
    let masters_loose = CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(dark_loose),
            ..Default::default()
        },
        40.0,
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

    let dark = CfaImage {
        data: Buffer2::new(10, 10, data),
        cfa_type: CfaType::Mono,
        metadata: ImageMetadata {
            quantization_sigma: Some(QUANTIZATION_SIGMA_PER_STEP / 4095.0),
            ..ImageMetadata::default()
        },
        flags: None,
    };

    let defect_map = DefectMap::new(dark.size())
        .detect_hot(&dark, 5.0, &CancelToken::never())
        .unwrap();

    // The tiny values one ADC step up are not flagged; the genuine outlier at 0.5 is, alone.
    assert_eq!(defect_map.hot_indices(), [50]);
}

/// A hot pixel is repaired from its own colour. The light's red, green and blue sit at 0.5, 0.3 and
/// 0.2 over a dark of 0.0625, so after subtraction the hot red at (2, 2) can only come back as
/// 0.5 − 0.0625 = 0.4375 if every neighbour it took the median of was red — a green or blue one
/// would pull it toward 0.2375 or 0.1375.
#[test]
fn calibrate_hot_pixel_correction() {
    let (w, h) = (6, 6);
    let pattern = CfaType::Bayer(CfaPattern::Rggb);
    let mut dark_pixels = Buffer2::new_filled(w, h, 0.0625_f32);
    dark_pixels[(2, 2)] = 0.9;
    let masters = bundle(CalibrationSet {
        dark: Some(cfa_from_plane(dark_pixels, pattern)),
        ..Default::default()
    });
    assert_eq!(
        masters.defect_summary(),
        Some(DefectSummary {
            hot_pixels: 1,
            cold_pixels: 0,
            percentage: 100.0 / (w * h) as f32,
        })
    );

    let baseline = [0.5f32, 0.3, 0.2];
    let mut light_pixels = Buffer2::new_default(w, h);
    for y in 0..h {
        for x in 0..w {
            light_pixels[(x, y)] = baseline[pattern.color_at(Vec2us::new(x, y)) as usize];
        }
    }
    light_pixels[(2, 2)] = 0.99;
    let mut light = cfa_from_plane(light_pixels, pattern);
    masters.calibrate(&mut light).unwrap();
    assert_eq!(light.data[2 * w + 2], 0.5 - 0.0625);
    // The repaired pixel says so, and no other does.
    let flags = light.flags.as_ref().unwrap();
    assert_eq!(flags.count(QualityFlags::REPAIRED), 1);
    assert_eq!(
        flags.at_pos(Vec2us::new(2, 2)),
        QualityFlags::DEFECT.union(QualityFlags::REPAIRED)
    );
}

#[test]
fn prepared_master_fits_bundle_round_trips_flat_and_calibration_bit_exactly() {
    let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
    let flat = CfaImage {
        data: Buffer2::new(
            4,
            4,
            vec![
                0.5, 0.7, 0.9, 0.7, 0.7, 0.4, 0.7, 0.4, 0.9, 0.7, 0.5, 0.7, 0.7, 0.4, 0.7, 0.4,
            ],
        ),
        cfa_type,
        metadata: ImageMetadata {
            camera_white_balance: Some([2.0, 1.0, 1.5, 1.0]),
            ..Default::default()
        },
        flags: None,
    };
    let mut masters = bundle(CalibrationSet {
        dark: Some(constant_cfa(Size2us::new(4, 4), 0.05, cfa_type)),
        flat: Some(flat),
        bias: Some(constant_cfa(Size2us::new(4, 4), 0.1, cfa_type)),
        flat_dark: None,
    });
    let dark = &mut masters.dark.as_mut().unwrap().image;
    dark.metadata.quantization_sigma = Some(0.000_02);
    // Pixel 5 is saturated and a defect: 2 + 4 = 6. The dark's flags go in its own `LUMFLAGS`
    // extension, right after it.
    let dark_flags = PixelFlags::from_fn(Size2us::new(4, 4), |index| {
        QualityFlags::from_byte(if index == 5 { 6 } else { 0 })
    });
    dark.flags = dark_flags.clone();
    let prepared_bits = bits(masters.flat.as_ref().unwrap().divisor().data.pixels());

    let mut expected = constant_cfa(Size2us::new(4, 4), 0.75, cfa_type);
    masters.calibrate(&mut expected).unwrap();

    let directory = TempDir::new("lumos-calibration-roundtrip");
    let path = directory.join("masters.fits");
    masters.save(&path).unwrap();
    // A bundle is no single image, whichever loader is pointed at it.
    let refused = |result: Result<(), ImageError>| {
        matches!(
            result,
            Err(ImageError::FitsUnsupported { reason, .. })
                if reason.contains("CALMASTR") && reason.contains("standalone CFAIMAGE")
        )
    };
    let context = LoadContext::default();
    assert!(refused(CfaImage::from_file(&path, &context).map(drop)));
    assert!(refused(LinearImage::from_file(&path, &context).map(drop)));
    assert!(refused(PreviewImage::from_file(&path, &context).map(drop)));
    let cache_bytes = fs::read(&path).unwrap();
    let mut reader = FitsReader::from_bytes(&cache_bytes).unwrap();
    assert_eq!(reader.hdus().len(), 6);
    assert_eq!(reader.hdus()[0].header.naxis().unwrap(), 0);
    assert_eq!(
        reader.hdus()[0].header.get_text("LUMOSFMT").unwrap(),
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
    for image_index in [1, 3, 4] {
        assert_eq!(
            reader.hdus()[image_index].header.bitpix().unwrap(),
            Bitpix::F32
        );
    }
    for index in 0..reader.hdus().len() {
        let report = reader.verify_checksum(index).unwrap();
        assert_eq!(report.datasum, ChecksumStatus::Valid);
        assert_eq!(report.checksum, ChecksumStatus::Valid);
    }
    let loaded = CalibrationMasters::load(&path, &LoadContext::default()).unwrap();
    assert_eq!(
        loaded.dark.as_ref().unwrap().image.flags().unwrap().bytes(),
        dark_flags.as_ref().unwrap().bytes()
    );
    assert!(loaded.bias.as_ref().unwrap().flags().is_none());
    // Flags whose master is gone would be dropped unseen, so the bundle is refused.
    let orphan = directory.join("orphan.fits");
    fs::write(&orphan, &cache_bytes).unwrap();
    rewrite_fits(&orphan, |index, _, _| index != 1);
    let error = CalibrationMasters::load(&orphan, &LoadContext::default()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("is for \"MASTER_DARK\", which is no image of the file"),
        "{error}"
    );
    // The caller's context reaches every master's decode: a cancelled one stops the load.
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        CalibrationMasters::load(&path, &LoadContext::new(cancel, u64::MAX))
            .unwrap_err()
            .kind(),
        ErrorKind::Interrupted
    );

    let mut invalid_version = cache_bytes.clone();
    let version_card = invalid_version
        .windows(8)
        .position(|window| window == b"LUMOSVER")
        .unwrap();
    let version_digit = invalid_version[version_card..version_card + 80]
        .iter()
        .rposition(u8::is_ascii_digit)
        .unwrap();
    invalid_version[version_card + version_digit] = b'0';
    fs::write(&path, invalid_version).unwrap();
    assert_eq!(
        CalibrationMasters::load(&path, &LoadContext::default())
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidData
    );

    // The bias is stored as it is, 0.1 at every pixel; the dark beside it holds its thermal signal
    // alone, 0.05 − 0.1, and says so.
    assert_eq!(
        reader.hdus()[1].header.get_text("LUMDBIAS").unwrap(),
        Some("REMOVED")
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
    assert_eq!(
        CalibrationMasters::load(&path, &LoadContext::default())
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidData
    );

    assert_eq!(
        bits(loaded.flat.as_ref().unwrap().divisor().data.pixels()),
        prepared_bits
    );
    assert_eq!(
        loaded
            .flat
            .as_ref()
            .unwrap()
            .divisor()
            .metadata
            .camera_white_balance,
        Some([2.0, 1.0, 1.5, 1.0])
    );
    assert_eq!(
        loaded
            .dark
            .as_ref()
            .unwrap()
            .image
            .metadata
            .quantization_sigma,
        Some(0.000_02)
    );
    assert_eq!(
        loaded.components().collect::<Vec<_>>(),
        masters.components().collect::<Vec<_>>()
    );
    assert_eq!(loaded.defect_summary(), masters.defect_summary());

    let mut actual = constant_cfa(Size2us::new(4, 4), 0.75, cfa_type);
    loaded.calibrate(&mut actual).unwrap();
    assert_eq!(bits(actual.data.pixels()), bits(expected.data.pixels()));
}

#[test]
fn empty_master_fits_bundle_round_trips_as_a_checksummed_primary_hdu() {
    let dir = TempDir::new("lumos-empty-masters");
    let path = dir.join("masters.fits");
    CalibrationMasters::default().save(&path).unwrap();

    let bytes = fs::read(&path).unwrap();
    let mut reader = FitsReader::from_bytes(&bytes).unwrap();
    assert_eq!(reader.hdus().len(), 1);
    let report = reader.verify_checksum(0).unwrap();
    assert_eq!(report.datasum, ChecksumStatus::Valid);
    assert_eq!(report.checksum, ChecksumStatus::Valid);

    let loaded = CalibrationMasters::load(&path, &LoadContext::default()).unwrap();
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
    let defects = DefectMap::from_indices(Size2us::new(10, 8), vec![1, 2], vec![7]).unwrap();
    assert_eq!(defects.ram_bytes(), 3 * size_of::<usize>() + mask_bytes);

    // The bundle sums present roles + the defect map; absent roles add nothing.
    let masters = CalibrationMasters {
        bias: None,
        dark: Some(MasterDark {
            image: dark,
            bias: DarkBias::Included,
        }),
        flat: Some(PreparedFlat::from_divisor(constant_cfa(
            Size2us::new(4, 4),
            1.0,
            CfaType::Mono,
        ))),
        defect_map: Some(defects),
    };
    // 320 (dark: 80·4) + 64 (flat: 16·4) + the defects' 24 + 128.
    assert_eq!(
        masters.ram_bytes(),
        10 * 8 * 4 + 4 * 4 * 4 + 3 * size_of::<usize>() + mask_bytes
    );
    // Three of the 80 pixels: 3.75%.
    assert_eq!(
        masters.defect_summary(),
        Some(DefectSummary {
            hot_pixels: 2,
            cold_pixels: 1,
            percentage: 3.75,
        })
    );
}

#[test]
fn stack_cfa_master_rejects_an_invalid_config_before_reading_anything() {
    // `stack_cfa_master` builds its own CFA cache, so it is the entry point most likely to drift
    // out of the shared `combine_cached` gate and reach the reducer unvalidated. A NaN sigma
    // rejects every sample at every pixel and yields a silently black master; a negative one
    // inverts the clip band and faults on the survivor range. Both are configuration errors and
    // must be reported as such.
    // A path inside a fresh empty directory names no file, wherever the test runs.
    let dir = TempDir::new("lumos-invalid-config");
    let missing = [dir.join("missing.fits")];

    for rejection in [Rejection::sigma_clip(f32::NAN), Rejection::sigma_clip(-1.0)] {
        let config = StackConfig {
            method: CombineMethod::Mean(rejection),
            ..StackConfig::bias_or_dark()
        };
        let error = stack_cfa_master(
            &missing,
            config,
            None,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                StackError::Config(StackConfigError::Field(invalid)) if invalid.field == "sigma_low"
            ),
            "expected the config to be rejected, got {error:?}"
        );
    }

    // The frame-count half of the gate, which only reaches this entry point because it routes
    // through `combine_cached`: a manual weight per frame, against one path.
    let error = stack_cfa_master(
        &missing,
        StackConfig {
            weighting: Weighting::Manual(vec![1.0, 1.0]),
            ..StackConfig::bias_or_dark()
        },
        None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            StackError::Config(StackConfigError::ManualWeightCountMismatch {
                expected: 1,
                actual: 2
            })
        ),
        "expected the weight count to be checked against the frame count, got {error:?}"
    );

    // Reported from the config alone: the paths are never opened, so a valid config on the same
    // missing files fails differently.
    let error = stack_cfa_master(
        &missing,
        StackConfig::bias_or_dark(),
        None,
        ProgressCallback::default(),
        CancelToken::never(),
    )
    .unwrap_err();
    assert!(
        !matches!(error, StackError::Config(_)),
        "a valid config must get past validation and fail on the missing file, got {error:?}"
    );
}

/// A frame on a 4096 span whose pedestal is `pedestal`.
fn with_pedestal(mut frame: CfaImage, pedestal: Pedestal) -> CfaImage {
    frame.metadata.domain = Some(SampleDomain {
        pedestal,
        ..raw_domain(4096.0)
    });
    frame
}

/// A flat or a light that may still hold an offset is never divided without a subtractor
/// (review 4.2): `(S + b)/flat` puts the offset under the vignetting. A flat whose pedestal the
/// decoder removed, as a RAW decode does, needs none; one that kept it or does not say is refused.
/// The light's rule is the same, once the bundle holds a flat and no bias or dark that holds the
/// offset: a dark marked calibrated lost its bias when it was stacked, and removes none. A bias or
/// flat-dark marked calibrated has no offset left to remove, and is refused.
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
                DEFAULT_SIGMA_THRESHOLD,
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
    assert!(!kept.metadata.calibrated);
    let mut thermal = constant_cfa(size, 0.0625, CfaType::Mono);
    thermal.metadata.calibrated = true;
    let with_thermal_dark = bundle(CalibrationSet {
        flat: Some(flat(Pedestal::Removed)),
        dark: Some(thermal),
        ..Default::default()
    });
    assert_eq!(
        with_thermal_dark.calibrate(&mut kept),
        Err(CalibrationError::LightWithoutSubtractor)
    );
    for role in [MasterRole::Bias, MasterRole::FlatDark] {
        let mut subtractor = constant_cfa(size, 0.125, CfaType::Mono);
        subtractor.metadata.calibrated = true;
        let mut set = CalibrationSet {
            flat: Some(flat(Pedestal::Kept(256.0))),
            ..Default::default()
        };
        match role {
            MasterRole::Bias => set.bias = Some(subtractor),
            _ => set.flat_dark = Some(subtractor),
        }
        assert_eq!(
            CalibrationMasters::from_images(set, DEFAULT_SIGMA_THRESHOLD, &CancelToken::never())
                .unwrap_err(),
            CalibrationError::CalibratedSubtractor { component: role },
        );
    }
    let mut removed = with_pedestal(constant_cfa(size, 0.5, CfaType::Mono), Pedestal::Removed);
    assert_eq!(
        masters.calibrate(&mut removed),
        Ok(CalibrationOutcome::default())
    );
    assert_eq!(removed.data.pixels(), &[0.5; 16]);
}

/// A dark is matched to the light (review 4.3). Lights of 300 s at 0.5, a bias of 0.125 and a dark
/// of 120 s holding the bias and a thermal signal of 0.0625.
/// - Without the bias the dark cannot be separated, and is refused.
/// - With it, the dark keeps 0.0625 of thermal signal, scaled by 300/120 = 2.5 to 0.15625: the
///   light is 0.5 − 0.125 − 0.15625 = 0.21875, exact in binary.
/// - At the light's own exposure, or within 1% of it, no scale; with an exposure undeclared on
///   either side, no scale and the outcome says it was not compared.
/// - Temperatures 1.5 °C apart are refused; with one undeclared, as a DSLR's, the outcome says so.
/// - A dark marked calibrated, its bias taken per frame when it was stacked, holds the thermal
///   0.0625 alone and is not given the bias a second time: with the bias the light is 0.21875
///   again, and without it the light keeps its bias, 0.5 − 0.15625 = 0.34375.
#[test]
fn a_dark_is_matched_to_the_light() {
    let size = Size2us::new(4, 4);
    let dark = |exposure: Option<f64>, temperature: Option<f64>| {
        let mut dark = constant_cfa(size, 0.1875, CfaType::Mono);
        dark.metadata.exposure_time = exposure;
        dark.metadata.ccd_temp = temperature;
        dark
    };
    let light = |exposure: Option<f64>, temperature: Option<f64>| {
        let mut light = constant_cfa(size, 0.5, CfaType::Mono);
        light.metadata.exposure_time = exposure;
        light.metadata.ccd_temp = temperature;
        light
    };
    let bias = || constant_cfa(size, 0.125, CfaType::Mono);

    let unseparated = bundle(CalibrationSet {
        dark: Some(dark(Some(120.0), Some(-10.0))),
        ..Default::default()
    });
    assert_eq!(
        unseparated.calibrate(&mut light(Some(300.0), Some(-10.0))),
        Err(CalibrationError::DarkExposureMismatch {
            light: 300.0,
            dark: 120.0
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
            dark_scale: Some(2.5),
            ..CalibrationOutcome::default()
        })
    );
    assert_eq!(scaled.data.pixels(), &[0.218_75; 16]);

    for exposure in [120.0, 121.0] {
        let mut matched = light(Some(exposure), Some(-10.0));
        assert_eq!(
            separated.calibrate(&mut matched),
            Ok(CalibrationOutcome::default())
        );
        assert_eq!(matched.data.pixels(), &[0.3125; 16], "{exposure} s");
    }
    assert_eq!(
        separated.calibrate(&mut light(None, Some(-10.0))),
        Ok(CalibrationOutcome {
            unverified_exposure: true,
            ..CalibrationOutcome::default()
        })
    );
    assert_eq!(
        separated.calibrate(&mut light(Some(120.0), Some(-8.5))),
        Err(CalibrationError::DarkTemperatureMismatch {
            light: -8.5,
            dark: -10.0
        })
    );
    assert_eq!(
        separated.calibrate(&mut light(Some(120.0), None)),
        Ok(CalibrationOutcome {
            unverified_temperature: true,
            ..CalibrationOutcome::default()
        })
    );

    let thermal = || {
        let mut dark = constant_cfa(size, 0.0625, CfaType::Mono);
        dark.metadata.exposure_time = Some(120.0);
        dark.metadata.ccd_temp = Some(-10.0);
        dark.metadata.calibrated = true;
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
                dark_scale: Some(2.5),
                ..CalibrationOutcome::default()
            })
        );
        assert_eq!(scaled.data.pixels(), &[expected; 16]);
    }
}

/// Each flat takes its bias before the flats are normalized and combined (review 4.1). Two 8 × 8
/// flats of a field `f` that is 1 everywhere but 0.5 at the four corners, at 0.5·f and 0.25·f, both
/// on an offset of 1/32, with that offset as the bias.
/// - Subtracted per frame they are 0.5·f and 0.25·f; the second's median 0.25 against the first's
///   0.5 gives it gain 2, and the mean is 0.5·f: a corner reads half the centre, exactly.
/// - Normalized first, the medians are 0.53125 and 0.28125, and the second's gain 17/9 scales its
///   offset too. The centre averages to 0.53125 and a corner to (0.28125 + 0.15625·17/9)/2; less
///   the offset, a corner reads 0.5139 of the centre: a vignetting residual of 2.8%.
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
    let directory = TempDir::new("lumos-flat-calibration");
    let paths: Vec<PathBuf> = [0.5f32, 0.25]
        .into_iter()
        .enumerate()
        .map(|(index, level)| {
            let flat = make_cfa(
                size,
                (0..size.pixel_count())
                    .map(|i| offset + level * field(i % 8, i / 8))
                    .collect(),
                CfaType::Mono,
            );
            let path = directory.join(format!("flat_{index}.fits"));
            flat.save_fits(&path).unwrap();
            path
        })
        .collect();
    let bias = constant_cfa(size, offset, CfaType::Mono);
    let corner_over_centre = |master: &CfaImage| master.data[(0, 0)] / master.data[(3, 3)];
    for memory_override in [None, Some(1)] {
        let config = StackConfig {
            small_n: SmallN::none(),
            ingest: IngestConfig {
                memory_override,
                cache_dir: directory.join("cache"),
                ..IngestConfig::default()
            },
            ..StackConfig::flat()
        };
        let calibrated = stack_cfa_master(
            &paths,
            config.clone(),
            Some(&bias),
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .unwrap();
        assert!(calibrated.metadata.calibrated);
        assert_eq!(corner_over_centre(&calibrated), 0.5, "{memory_override:?}");

        let mut late = stack_cfa_master(
            &paths,
            config,
            None,
            ProgressCallback::default(),
            CancelToken::never(),
        )
        .unwrap()
        .unwrap();
        late.subtract(&bias, DomainMap::IDENTITY);
        let ratio = corner_over_centre(&late);
        assert!(
            (ratio - 0.5139).abs() < 1e-4,
            "{memory_override:?}: {ratio}"
        );
    }
}
