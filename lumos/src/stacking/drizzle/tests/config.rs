use crate::stacking::drizzle::tests::*;

#[test]
fn test_drizzle_config_default() {
    let config = DrizzleConfig::default();
    assert!((config.scale - 2.0).abs() < f32::EPSILON);
    assert!((config.pixfrac - 0.8).abs() < f32::EPSILON);
    assert_eq!(config.kernel, DrizzleKernel::Turbo);
}

#[test]
fn test_drizzle_config_presets() {
    let x1_5 = DrizzleConfig::x1_5();
    assert!((x1_5.scale - 1.5).abs() < f32::EPSILON);

    let x2 = DrizzleConfig::x2();
    assert!((x2.scale - 2.0).abs() < f32::EPSILON);

    let x3 = DrizzleConfig::x3();
    assert!((x3.scale - 3.0).abs() < f32::EPSILON);
}

#[test]
fn test_drizzle_config_builder() {
    let config = DrizzleConfig::default()
        .with_pixfrac(0.5)
        .with_kernel(DrizzleKernel::Gaussian)
        .with_min_coverage(0.2);

    assert!((config.pixfrac - 0.5).abs() < f32::EPSILON);
    assert_eq!(config.kernel, DrizzleKernel::Gaussian);
    assert!((config.min_coverage - 0.2).abs() < f32::EPSILON);
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn test_drizzle_config_invalid_parameters_return_exact_errors() {
    let cases = [
        (
            DrizzleConfig {
                scale: 0.0,
                ..Default::default()
            },
            DrizzleConfigError::InvalidScale { value: 0.0 },
        ),
        (
            DrizzleConfig::default().with_pixfrac(1.5),
            DrizzleConfigError::InvalidPixfrac { value: 1.5 },
        ),
        (
            DrizzleConfig {
                fill_value: f32::INFINITY,
                ..Default::default()
            },
            DrizzleConfigError::InvalidFillValue {
                value: f32::INFINITY,
            },
        ),
        (
            DrizzleConfig::default().with_min_coverage(-0.1),
            DrizzleConfigError::InvalidMinCoverage { value: -0.1 },
        ),
        (
            DrizzleConfig::default().with_kernel(DrizzleKernel::Lanczos),
            DrizzleConfigError::InvalidLanczosSampling {
                scale: 2.0,
                pixfrac: 0.8,
            },
        ),
    ];

    for (config, expected) in cases {
        assert_eq!(config.validate(), Err(expected));
    }

    let error = DrizzleAccumulator::new(
        ImageDimensions::new((2, 2), 1),
        DrizzleConfig::default().with_pixfrac(1.5),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "pixfrac must be finite and between 0 and 1, got 1.5"
    );
    assert!(matches!(
        error,
        DrizzleError::Config(DrizzleConfigError::InvalidPixfrac { value: 1.5 })
    ));
}

#[test]
fn test_drizzle_accumulator_rejects_invalid_dimensions() {
    let dimensions = ImageDimensions {
        size: (0, 2).into(),
        channels: 2,
    };
    let error = DrizzleAccumulator::new(dimensions, DrizzleConfig::default()).unwrap_err();
    assert!(matches!(
        error,
        DrizzleError::InvalidInputDimensions {
            width: 0,
            height: 2,
            channels: 2,
        }
    ));
}

#[test]
fn test_drizzle_accumulator_dimensions() {
    let config = DrizzleConfig::x2();
    let acc = accumulator(ImageDimensions::new((100, 80), 3), config);
    let dims = acc.dimensions();
    assert_eq!(dims.size.x, 200);
    assert_eq!(dims.size.y, 160);
    assert_eq!(dims.channels, 3);
}

#[test]
fn test_lanczos_kernel() {
    // Center value
    assert!((lanczos_kernel(0.0, 3.0) - 1.0).abs() < f32::EPSILON);

    // Outside support
    assert!((lanczos_kernel(3.5, 3.0) - 0.0).abs() < f32::EPSILON);

    // Symmetry
    let pos = lanczos_kernel(1.5, 3.0);
    let neg = lanczos_kernel(-1.5, 3.0);
    assert!((pos - neg).abs() < 1e-6);
}
