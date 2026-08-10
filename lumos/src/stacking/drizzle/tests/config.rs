use super::*;

#[test]
fn drizzle_config_default() {
    let config = DrizzleConfig::default();
    assert_eq!(config.scale, 2.0);
    assert_eq!(config.pixfrac, 0.8);
    assert_eq!(config.kernel, DrizzleKernel::Turbo);
}

#[test]
fn drizzle_config_presets() {
    let x1_5 = DrizzleConfig::x1_5();
    assert_eq!(x1_5.scale, 1.5);

    let x2 = DrizzleConfig::x2();
    assert_eq!(x2.scale, 2.0);

    let x3 = DrizzleConfig::x3();
    assert_eq!(x3.scale, 3.0);
}

#[test]
fn drizzle_config_builder() {
    let config = DrizzleConfig::default()
        .with_pixfrac(0.5)
        .with_kernel(DrizzleKernel::Gaussian)
        .with_min_coverage(0.2);

    assert_eq!(config.pixfrac, 0.5);
    assert_eq!(config.kernel, DrizzleKernel::Gaussian);
    assert_eq!(config.min_coverage, 0.2);
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn drizzle_config_invalid_parameters_return_exact_errors() {
    // Each case: the config, and the field its rejection must name with the value it carries.
    let range_checks = [
        (
            DrizzleConfig {
                scale: 0.0,
                ..Default::default()
            },
            "scale",
            0.0,
        ),
        (DrizzleConfig::default().with_pixfrac(1.5), "pixfrac", 1.5),
        (
            DrizzleConfig {
                fill_value: f32::INFINITY,
                ..Default::default()
            },
            "fill_value",
            f64::INFINITY,
        ),
        (
            DrizzleConfig::default().with_min_coverage(-0.1),
            "min_coverage",
            // -0.1 has no exact f64 twin: compare against the f32 the field actually holds.
            f64::from(-0.1f32),
        ),
    ];
    for (config, field, value) in range_checks {
        let DrizzleConfigError::Field(invalid) = config.validate().unwrap_err() else {
            panic!("{field} should be reported as an out-of-range field")
        };
        assert_eq!((invalid.field, invalid.value), (field, value));
    }

    // The kernel constrains scale and pixfrac together, so it keeps its own variant.
    assert_eq!(
        DrizzleConfig::default()
            .with_kernel(DrizzleKernel::Lanczos)
            .validate(),
        Err(DrizzleConfigError::InvalidLanczosSampling {
            scale: 2.0,
            pixfrac: 0.8,
        })
    );

    for kernel in [
        DrizzleKernel::Square,
        DrizzleKernel::Turbo,
        DrizzleKernel::Point,
        DrizzleKernel::Gaussian,
        DrizzleKernel::Lanczos,
    ] {
        let config = DrizzleConfig {
            scale: 1.0,
            pixfrac: 0.0,
            kernel,
            ..Default::default()
        };
        let DrizzleConfigError::Field(invalid) = config.validate().unwrap_err() else {
            panic!("{kernel:?} must reject zero pixfrac before kernel arithmetic")
        };
        assert_eq!((invalid.field, invalid.value), ("pixfrac", 0.0));
        assert!(matches!(
            DrizzleAccumulator::new(ImageDimensions::new((2, 2), 1), config),
            Err(DrizzleError::Config(DrizzleConfigError::Field(invalid)))
                if invalid.field == "pixfrac"
        ));
    }

    let error = DrizzleAccumulator::new(
        ImageDimensions::new((2, 2), 1),
        DrizzleConfig::default().with_pixfrac(1.5),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "pixfrac must be finite and in (0, 1], got 1.5"
    );
}

#[test]
fn drizzle_accumulator_dimensions() {
    let config = DrizzleConfig::x2();
    let acc = accumulator(ImageDimensions::new((100, 80), 3), config);
    let dims = acc.dimensions();
    assert_eq!(dims.width(), 200);
    assert_eq!(dims.height(), 160);
    assert_eq!(dims.channels(), 3);
}

#[test]
fn lanczos_kernel_is_symmetric_and_vanishes_outside_its_support() {
    // Center value
    assert!((lanczos_kernel(0.0, 3.0) - 1.0).abs() < f32::EPSILON);

    // Outside support
    assert!((lanczos_kernel(3.5, 3.0) - 0.0).abs() < f32::EPSILON);

    // Symmetry
    let pos = lanczos_kernel(1.5, 3.0);
    let neg = lanczos_kernel(-1.5, 3.0);
    assert!((pos - neg).abs() < 1e-6);
}
