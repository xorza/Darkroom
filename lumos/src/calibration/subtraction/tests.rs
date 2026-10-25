use crate::calibration::calibration_pass::Saturation;
use crate::calibration::error::{DarkMismatch, SubtractionFault};
use crate::calibration::master_role::MasterRole;
use crate::calibration::subtraction::{self, Subtraction};
use crate::calibration::subtractor::Subtractor;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::QualityFlags;
use crate::image::metadata::calibration_state::CalibrationState;
use crate::image::metadata::sample_domain::{DomainMap, Pedestal, SampleDomain, ScaleOrigin};
use crate::internals::cfa::make_cfa;
use common::Size2us;

const SIZE: Size2us = Size2us::new(2, 2);

fn frame(samples: [f32; 4], exposure: f64) -> CfaImage {
    let mut frame = make_cfa(SIZE, samples.to_vec(), CfaType::Mono);
    frame.metadata.header.exposure_time = Some(exposure);
    frame
}

fn domain(scale: f64, pedestal: Pedestal) -> SampleDomain {
    SampleDomain {
        scale,
        origin: ScaleOrigin::Declared,
        pedestal,
        unit: None,
    }
}

/// Each master is mapped into the frame as it stands when that master applies. The light keeps a
/// pedestal of 2 at a scale of 4, as its bias does, so the bias maps as it is and leaves the
/// pedestal removed; the bias-removed dark, at a scale of 8, then maps at a gain of 8/4 = 2 and no
/// offset. Mapped against the light as it came, the dark would take its pedestal a second time,
/// (2 − 0)/4 = 0.5. The dark of 1 s scales by the light's 2 s, so a light sample of 0.75 loses the
/// bias's 0.125 and 2·2·0.0625 = 0.25 of dark signal: 0.375, in one rounding, with both parts
/// recorded and the pedestal removed.
#[test]
fn each_master_maps_into_the_frame_as_it_stands_when_it_applies() {
    let mut light = frame([0.75; 4], 2.0);
    light
        .metadata
        .derived
        .set_domain(Some(domain(4.0, Pedestal::Kept(2.0))));
    let mut bias = frame([0.125; 4], 0.0);
    bias.metadata
        .derived
        .set_domain(Some(domain(4.0, Pedestal::Kept(2.0))));
    let mut dark = frame([0.0625; 4], 1.0);
    dark.metadata
        .derived
        .set_domain(Some(domain(8.0, Pedestal::Removed)));
    dark.metadata
        .derived
        .set_calibration(CalibrationState::BIAS);
    let subtractors = [
        Subtractor {
            role: MasterRole::Bias,
            master: &bias,
        },
        Subtractor {
            role: MasterRole::Dark,
            master: &dark,
        },
    ];
    let subtraction = Subtraction::resolve(&light, subtractors).unwrap();
    let bias_term = subtraction.term(MasterRole::Bias).unwrap();
    assert_eq!(bias_term.map, DomainMap::IDENTITY);
    assert!(bias_term.matched.is_none());
    let dark_term = subtraction.term(MasterRole::Dark).unwrap();
    assert_eq!(
        dark_term.map,
        DomainMap {
            gain: 2.0,
            offset: 0.0
        }
    );
    assert_eq!(dark_term.matched_scale(), 2.0);
    subtraction.apply(&mut light, None, Saturation::Detect);
    assert_eq!(light.data().pixels(), &[0.375; 4]);
    assert_eq!(
        light.metadata.derived.calibration(),
        CalibrationState::ADDITIVE
    );
    assert_eq!(
        light.metadata.derived.domain(),
        Some(&domain(4.0, Pedestal::Removed))
    );
}

/// A frame as decoded is flagged where it saturated, at 0.95 of a span of 1, before its samples
/// move, and its `data_max` dropped; a master keeps both as they were. Either way the bias's pixel
/// of no measurement leaves the frame flagged `NO_DATA` there.
#[test]
fn a_frame_flags_its_saturation_and_a_master_keeps_its_own() {
    let mut bias = frame([0.125; 4], 0.0);
    bias.set_flags(PixelFlags::from_fn(SIZE, |idx| {
        if idx == 1 {
            QualityFlags::NO_DATA
        } else {
            QualityFlags::default()
        }
    }));
    let subtractors = [Subtractor {
        role: MasterRole::Bias,
        master: &bias,
    }];
    let imposed = subtraction::unmeasured(&subtractors);
    for (saturation, saturated, data_max) in [
        (Saturation::Detect, QualityFlags::SATURATED, None),
        (Saturation::Kept, QualityFlags::default(), Some(1.0)),
    ] {
        let mut taken = frame([0.5, 0.5, 0.96, 0.5], 1.0);
        taken.metadata.header.data_max = Some(1.0);
        Subtraction::resolve(&taken, subtractors).unwrap().apply(
            &mut taken,
            imposed.as_ref(),
            saturation,
        );
        let flags = taken.flags().unwrap();
        assert_eq!(flags.at(1), QualityFlags::NO_DATA, "{saturation:?}");
        assert_eq!(flags.at(2), saturated, "{saturation:?}");
        assert_eq!(flags.at(0), QualityFlags::default(), "{saturation:?}");
        assert_eq!(taken.metadata.header.data_max, data_max, "{saturation:?}");
        assert_eq!(taken.data().pixels(), &[0.375, 0.375, 0.835, 0.375]);
    }
}

/// What resolution refuses, each with the role that would not fit: a dark that holds the bias
/// taken at another exposure, and a master of another stated unit.
#[test]
fn a_master_that_does_not_fit_the_frame_is_refused_with_its_role() {
    let light = frame([0.5; 4], 2.0);
    let raw_dark = frame([0.0625; 4], 1.0);
    let error = Subtraction::resolve(
        &light,
        [Subtractor {
            role: MasterRole::Dark,
            master: &raw_dark,
        }],
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            SubtractionFault::Conditions {
                role: MasterRole::Dark,
                source: DarkMismatch::Exposure {
                    frame: 2.0,
                    dark: 1.0
                }
            }
        ),
        "{error:?}"
    );

    let mut light = frame([0.5; 4], 0.0);
    light.metadata.derived.set_domain(Some(SampleDomain {
        unit: Some("electron".to_owned()),
        ..domain(4.0, Pedestal::Removed)
    }));
    let mut bias = frame([0.125; 4], 0.0);
    bias.metadata.derived.set_domain(Some(SampleDomain {
        unit: Some("ADU".to_owned()),
        ..domain(4.0, Pedestal::Removed)
    }));
    let error = Subtraction::resolve(
        &light,
        [Subtractor {
            role: MasterRole::Bias,
            master: &bias,
        }],
    )
    .unwrap_err();
    assert!(
        matches!(
            error,
            SubtractionFault::Domain {
                role: MasterRole::Bias,
                ..
            }
        ),
        "{error:?}"
    );
}
