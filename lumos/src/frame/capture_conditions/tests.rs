use crate::frame::capture_conditions::*;

fn conditions(exposure_time: Option<f64>, ccd_temp: Option<f64>) -> CaptureConditions {
    CaptureConditions {
        exposure_time,
        ccd_temp,
        camera_temp: None,
    }
}

#[test]
fn a_set_shares_the_mean_of_values_that_agree() {
    // 300, 301 and 302.5 s span 2.5 s, inside 1% of 302.5 = 3.025 s: the mean is 903.5 / 3.
    // −10.0 and −9.5 °C span 0.5 °C: the mean is −9.75 °C.
    let set = [
        conditions(Some(300.0), Some(-10.0)),
        conditions(Some(301.0), Some(-9.5)),
        conditions(Some(302.5), Some(-9.75)),
    ];
    assert_eq!(CaptureConditions::check_exposures(set), Ok(()));
    assert_eq!(
        CaptureConditions::shared(set),
        conditions(Some(903.5 / 3.0), Some(-29.25 / 3.0))
    );
}

#[test]
fn a_silent_or_disagreeing_frame_leaves_the_condition_unshared() {
    // One frame states no temperature: the exposures still agree and are shared.
    let silent = [
        conditions(Some(120.0), Some(0.0)),
        conditions(Some(120.0), None),
    ];
    assert_eq!(CaptureConditions::check_exposures(silent), Ok(()));
    assert_eq!(
        CaptureConditions::shared(silent),
        conditions(Some(120.0), None)
    );
    // Nothing stated at all: nothing to compare, nothing shared.
    let none = [conditions(None, None), conditions(None, None)];
    assert_eq!(CaptureConditions::check_exposures(none), Ok(()));
    assert_eq!(CaptureConditions::shared(none), conditions(None, None));
    // Camera bodies at 20.5 and 21 °C share 20.75; at 20.5 and 24 °C — a body warming through a
    // session of darks — they share none, and the set is not refused for it.
    let body = |camera_temp| CaptureConditions {
        camera_temp: Some(camera_temp),
        ..conditions(Some(120.0), None)
    };
    assert_eq!(
        CaptureConditions::shared([body(20.5), body(21.0)]).camera_temp,
        Some(20.75)
    );
    let warming = [body(20.5), body(24.0)];
    assert_eq!(CaptureConditions::temperature_disagreement(warming), None);
    assert_eq!(CaptureConditions::shared(warming).camera_temp, None);
}

#[test]
fn the_first_frame_out_of_range_is_named_against_the_far_end() {
    // 120 s then 300 s: 180 s apart, far outside 1% of 300 s. Frame 1 is the first out, and
    // the low end it is too far from is frame 0.
    let mixed = [
        conditions(Some(120.0), None),
        conditions(Some(300.0), None),
        conditions(Some(300.0), None),
    ];
    assert_eq!(
        CaptureConditions::check_exposures(mixed),
        Err(ConditionMismatch {
            condition: CaptureCondition::Exposure,
            idx: 1,
            value: 300.0,
            reference_idx: 0,
            reference: 120.0,
        })
    );
    assert_eq!(CaptureConditions::shared(mixed), conditions(None, None));
    // Temperatures −10.0, −9.4 and −10.5: the third widens the range to 1.1 °C, past 1 °C,
    // below the low end, so it is named against the high end, frame 1.
    let drifting = [
        conditions(None, Some(-10.0)),
        conditions(None, Some(-9.4)),
        conditions(None, Some(-10.5)),
    ];
    assert_eq!(
        CaptureConditions::temperature_disagreement(drifting),
        Some(ConditionMismatch {
            condition: CaptureCondition::Temperature,
            idx: 2,
            value: -10.5,
            reference_idx: 1,
            reference: -9.4,
        })
    );
    assert_eq!(CaptureConditions::check_exposures(drifting), Ok(()));
    assert_eq!(CaptureConditions::shared(drifting).ccd_temp, None);
}
