use super::*;
use imaginarium::ChannelLayout;

use crate::image::error::FrameDescError;

#[test]
fn drizzle_config_default() {
    let config = DrizzleConfig::default();
    assert_eq!(config.scale, 2.0);
    assert_eq!(config.pixfrac, 0.8);
    assert_eq!(config.kernel, DrizzleKernel::Square);
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
        (
            DrizzleConfig {
                pixfrac: 1.5,
                ..Default::default()
            },
            "pixfrac",
            1.5,
        ),
        (
            DrizzleConfig {
                fill_value: f32::INFINITY,
                ..Default::default()
            },
            "fill_value",
            f64::INFINITY,
        ),
        (
            DrizzleConfig {
                min_weight_fraction: -0.1,
                ..Default::default()
            },
            "min_weight_fraction",
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
        DrizzleConfig {
            kernel: DrizzleKernel::Lanczos,
            ..Default::default()
        }
        .validate(),
        Err(DrizzleConfigError::InvalidLanczosSampling {
            scale: 2.0,
            pixfrac: 0.8,
        })
    );

    for kernel in DrizzleKernel::ALL {
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
            DrizzleAccumulator::new(&constant_image(Size2us::new(2, 2), 0.0), config, 0),
            Err(DrizzleError::Config(DrizzleConfigError::Field(invalid)))
                if invalid.field == "pixfrac"
        ));
    }

    let error = DrizzleAccumulator::new(
        &constant_image(Size2us::new(2, 2), 0.0),
        DrizzleConfig {
            pixfrac: 1.5,
            ..Default::default()
        },
        0,
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "pixfrac must be finite and in (0, 1], got 1.5"
    );
}

#[test]
fn drizzle_output_grid_is_the_input_scaled() {
    let config = DrizzleConfig::default();
    let acc = accumulator(PlanarDesc::new((100, 80), ChannelLayout::Rgb), config);
    let desc = acc.finalize().image.desc();
    assert_eq!(desc.width(), 200);
    assert_eq!(desc.height(), 160);
    assert_eq!(desc.channel_count(), 3);
}

/// A scale is refused before any output plane is allocated: one whose grid passes
/// `MAX_SIDE` (2 × 10⁹ pixels a side from 2 × 2 at 10⁹) as no image's size.
#[test]
fn a_scale_past_any_image_is_refused() {
    let frames = || {
        vec![DrizzleFrame::new(
            constant_image(Size2us::new(2, 2), 1.0),
            warp_of(Transform::identity()),
        )]
    };
    let at = |scale| drizzle_plain(frames(), &kernel_config(DrizzleKernel::Square, scale, 1.0));
    assert!(matches!(
        at(1e9),
        Err(DrizzleError::OutputSize(FrameDescError::SidePastMax { size }))
            if size == Size2us::new(2_000_000_000, 2_000_000_000)
    ));
}
