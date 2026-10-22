use crate::calibration::error::DarkMismatch;
use crate::calibration::master_subtraction::*;
use crate::image::cfa::cfa_pattern::CfaPattern;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::dark_comparison::DarkComparison;
use crate::internals::cfa::constant_cfa;
use crate::math::size2us::Size2us;

fn frame(value: f32, exposure: f64) -> CfaImage {
    let mut frame = constant_cfa(Size2us::new(2, 2), value, CfaType::Mono);
    frame.metadata.header.exposure_time = Some(exposure);
    frame
}

fn lost(mut frame: CfaImage, calibration: CalibrationState) -> CfaImage {
    frame.metadata.derived.set_calibration(calibration);
    frame
}

/// A flat of 2 s at 0.5 loses what each subtractor still holds. A bias of 1/32 leaves 0.46875
/// and the bias recorded. A flat-dark of 1 s that lost its bias holds 1/64 of dark signal,
/// scaled by 2/1 to 1/32: the same 0.46875, and the dark signal recorded, with the temperature
/// neither states as unverified; the bias is matched to nothing. A flat-dark of 1 s that still
/// holds its bias cannot be scaled, and a flat that lost its bias already cannot lose it
/// again.
#[test]
fn each_frame_loses_what_the_subtractor_holds() {
    let bias = frame(0.031_25, 0.0);
    let thermal = lost(frame(0.015_625, 1.0), CalibrationState::BIAS);
    let raw = frame(0.031_25 + 0.015_625, 1.0);
    for (subtractor, removed, compared) in [
        (
            Subtractor {
                role: MasterRole::Bias,
                master: &bias,
            },
            CalibrationState::BIAS,
            None,
        ),
        (
            Subtractor {
                role: MasterRole::FlatDark,
                master: &thermal,
            },
            CalibrationState::THERMAL,
            Some(DarkComparison {
                exposure_compared: true,
                temperature: None,
                temperature_difference: None,
            }),
        ),
    ] {
        let mut flat = frame(0.5, 2.0);
        MasterSubtraction::new(MasterRole::Flat, &[subtractor])
            .unwrap()
            .unwrap()
            .apply(3, &mut flat)
            .unwrap();
        assert_eq!(
            flat.data().pixels(),
            &[0.468_75; 4],
            "{:?}",
            subtractor.role
        );
        assert_eq!(flat.metadata.derived.calibration(), removed);
        assert_eq!(flat.metadata.derived.dark_comparison(), compared);
    }

    let unscalable = MasterSubtraction::new(
        MasterRole::Flat,
        &[Subtractor {
            role: MasterRole::FlatDark,
            master: &raw,
        }],
    )
    .unwrap()
    .unwrap();
    assert!(matches!(
        unscalable.apply(3, &mut frame(0.5, 2.0)),
        Err(SubtractionError::SubtractorConditions {
            idx: 3,
            source: DarkMismatch::Exposure {
                frame: 2.0,
                dark: 1.0
            }
        })
    ));

    let mut bias_free = lost(frame(0.5, 2.0), CalibrationState::BIAS);
    let error = MasterSubtraction::new(
        MasterRole::Flat,
        &[Subtractor {
            role: MasterRole::Bias,
            master: &bias,
        }],
    )
    .unwrap()
    .unwrap()
    .apply(3, &mut bias_free)
    .unwrap_err();
    assert!(matches!(
        error,
        SubtractionError::SubtractedTwice {
            idx: 3,
            subtractor: MasterRole::Bias
        }
    ));
    assert_eq!(bias_free.data().pixels(), &[0.5; 4]);

    // A frame of another pattern, then of another size, than the subtractor's, each named apart.
    let subtraction = MasterSubtraction::new(
        MasterRole::Flat,
        &[Subtractor {
            role: MasterRole::Bias,
            master: &bias,
        }],
    )
    .unwrap()
    .unwrap();
    let rggb = CfaType::Bayer(CfaPattern::Rggb);
    let mut mosaic = constant_cfa(Size2us::new(2, 2), 0.5, rggb);
    assert!(matches!(
        subtraction.apply(4, &mut mosaic),
        Err(SubtractionError::SubtractorPattern {
            idx: 4,
            frame,
            subtractor: CfaType::Mono,
        }) if frame == rggb
    ));
    let mut wide = constant_cfa(Size2us::new(4, 2), 0.5, CfaType::Mono);
    assert!(matches!(
        subtraction.apply(5, &mut wide),
        Err(SubtractionError::SubtractorShape {
            idx: 5,
            frame,
            subtractor,
        }) if frame == Size2us::new(4, 2) && subtractor == Size2us::new(2, 2)
    ));
}

/// A subtractor that lost what it holds, a flat, and one that holds more than the stacked
/// frames may lose are refused before any frame is read.
#[test]
fn a_subtractor_that_does_not_fit_the_role_is_refused() {
    let spent_bias = lost(frame(0.031_25, 0.0), CalibrationState::BIAS);
    let spent_flat_dark = lost(frame(0.0, 1.0), CalibrationState::ADDITIVE);
    let raw_flat_dark = frame(0.046_875, 1.0);
    let flat = frame(0.5, 1.0);
    let bias = frame(0.031_25, 0.0);
    for (target, subtractor, over_calibrated) in [
        (MasterRole::Flat, (MasterRole::Bias, &spent_bias), true),
        (
            MasterRole::Flat,
            (MasterRole::FlatDark, &spent_flat_dark),
            true,
        ),
        (MasterRole::Flat, (MasterRole::Flat, &flat), false),
        (
            MasterRole::Dark,
            (MasterRole::FlatDark, &raw_flat_dark),
            false,
        ),
        (MasterRole::Bias, (MasterRole::Bias, &bias), false),
    ] {
        let (role, master) = subtractor;
        let error = MasterSubtraction::new(target, &[Subtractor { role, master }]).unwrap_err();
        if over_calibrated {
            assert!(
                matches!(error, SubtractionError::OverCalibratedSubtractor { subtractor } if subtractor == role),
                "{target:?} from {role:?}: {error:?}"
            );
        } else {
            assert!(
                matches!(
                    error,
                    SubtractionError::SubtractorForRole { target: t, subtractor: s }
                        if t == target && s == role
                ),
                "{target:?} from {role:?}: {error:?}"
            );
        }
    }
    // A dark may lose its bias.
    assert!(
        MasterSubtraction::new(
            MasterRole::Dark,
            &[Subtractor {
                role: MasterRole::Bias,
                master: &bias,
            }],
        )
        .is_ok()
    );
}
