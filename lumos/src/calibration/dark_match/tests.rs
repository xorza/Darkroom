use crate::calibration::dark_match::*;

fn conditions(exposure_time: Option<f64>, ccd_temp: Option<f64>) -> CaptureConditions {
    CaptureConditions {
        exposure_time,
        ccd_temp,
        camera_temp: None,
    }
}

#[test]
fn a_bias_free_dark_of_another_exposure_scales_by_the_ratio() {
    // 300 s light, 120 s dark: 300 / 120 = 2.5.
    let matched = DarkMatch::new(
        conditions(Some(300.0), Some(-10.0)),
        conditions(Some(120.0), Some(-10.5)),
        DarkBias::Removed,
    )
    .unwrap();
    assert_eq!(
        matched,
        DarkMatch {
            scale: Some(2.5),
            comparison: DarkComparison {
                exposure_compared: true,
                temperature: Some(TemperatureSource::Sensor),
                temperature_difference: None,
            },
        }
    );
    assert_eq!(matched.factor(), 2.5);
    // The same pair with the bias still in the dark cannot be scaled, nor can a dark of 0 s,
    // whose ratio 300 / 0 has no finite value.
    for (dark, bias) in [(120.0, DarkBias::Held), (0.0, DarkBias::Removed)] {
        assert_eq!(
            DarkMatch::new(
                conditions(Some(300.0), None),
                conditions(Some(dark), None),
                bias,
            ),
            Err(DarkMismatch::Exposure { frame: 300.0, dark })
        );
    }
}

#[test]
fn agreeing_or_unstated_conditions_leave_the_dark_unscaled() {
    // 300 and 301 s are inside 1% of 301 s: one exposure, whatever the bias.
    let agreeing = DarkMatch::new(
        conditions(Some(301.0), None),
        conditions(Some(300.0), None),
        DarkBias::Held,
    )
    .unwrap();
    assert_eq!(
        agreeing,
        DarkMatch {
            scale: None,
            comparison: DarkComparison {
                exposure_compared: true,
                temperature: None,
                temperature_difference: None,
            },
        }
    );
    assert_eq!(agreeing.factor(), 1.0);
    let silent = DarkMatch::new(
        conditions(None, Some(0.0)),
        conditions(Some(60.0), Some(0.5)),
        DarkBias::Held,
    )
    .unwrap();
    assert_eq!(
        silent,
        DarkMatch {
            scale: None,
            comparison: DarkComparison {
                exposure_compared: false,
                temperature: Some(TemperatureSource::Sensor),
                temperature_difference: None,
            },
        }
    );
}

/// A dark of another temperature is taken, with the difference recorded: −7 against −10 °C,
/// 3 °C apart, past the 1 °C tolerance. At −9 °C, 1 °C apart, the bound itself, nothing is.
#[test]
fn a_dark_of_another_temperature_is_recorded_not_refused() {
    let matched = DarkMatch::new(
        conditions(Some(300.0), Some(-7.0)),
        conditions(Some(120.0), Some(-10.0)),
        DarkBias::Removed,
    )
    .unwrap();
    assert_eq!(
        matched,
        DarkMatch {
            scale: Some(2.5),
            comparison: DarkComparison {
                exposure_compared: true,
                temperature: Some(TemperatureSource::Sensor),
                temperature_difference: Some(TemperatureDifference {
                    reading: TemperatureSource::Sensor,
                    frame: -7.0,
                    dark: -10.0,
                }),
            },
        }
    );
    let within = DarkMatch::new(
        conditions(Some(300.0), Some(-9.0)),
        conditions(Some(300.0), Some(-10.0)),
        DarkBias::Held,
    )
    .unwrap();
    assert_eq!(within.comparison.temperature_difference, None);
}

/// The camera body's temperature stands in only where the two do not both state the sensor's:
/// two bodies at 21 and 21.5 °C match on it, at 21 and 23 °C differ on it, and two sensors
/// that agree decide alone though their bodies are 5 °C apart. A sensor reading on one side
/// only is no pair, so the bodies are compared.
#[test]
fn the_camera_temperature_matches_only_where_the_sensors_do_not() {
    let with_body = |ccd_temp, camera_temp| CaptureConditions {
        camera_temp,
        ..conditions(Some(300.0), ccd_temp)
    };
    let matched = |frame, dark| DarkMatch::new(frame, dark, DarkBias::Held);
    assert_eq!(
        matched(with_body(None, Some(21.0)), with_body(None, Some(21.5)))
            .unwrap()
            .comparison
            .temperature,
        Some(TemperatureSource::Camera)
    );
    assert_eq!(
        matched(with_body(None, Some(21.0)), with_body(None, Some(23.0)))
            .unwrap()
            .comparison
            .temperature_difference,
        Some(TemperatureDifference {
            reading: TemperatureSource::Camera,
            frame: 21.0,
            dark: 23.0,
        })
    );
    let sensors = matched(
        with_body(Some(-10.0), Some(20.0)),
        with_body(Some(-10.5), Some(25.0)),
    )
    .unwrap();
    assert_eq!(
        sensors.comparison.temperature,
        Some(TemperatureSource::Sensor)
    );
    assert!(!sensors.comparison.unverified().temperature);
    let one_sensor = matched(
        with_body(Some(-10.0), Some(21.0)),
        with_body(None, Some(21.5)),
    )
    .unwrap();
    assert_eq!(
        one_sensor.comparison.temperature,
        Some(TemperatureSource::Camera)
    );
}
