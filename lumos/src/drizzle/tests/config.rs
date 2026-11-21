use super::*;
use imaginarium::ChannelLayout;

use crate::image::error::FrameDescError;

#[test]
fn drizzle_config_default() {
    let config = DrizzleConfig::default();
    assert_eq!(
        config.sampling,
        DrizzleSampling::Drops {
            shape: DropShape::Square,
            scale: DrizzleScale::new(2.0),
            pixfrac: Pixfrac::new(0.8),
        }
    );
}

/// The fill value and the fill gate are the fields a config checks, each refused by its name with
/// the value it carries, before any frame is read or plane allocated; the sampling is valid by
/// construction.
#[test]
fn drizzle_config_invalid_parameters_return_exact_errors() {
    let range_checks = [
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
        let invalid = config.validate().unwrap_err();
        assert_eq!((invalid.field, invalid.value), (field, value));
        assert!(matches!(
            DrizzleAccumulator::new(&constant_image(USize2::new(2, 2), 0.0), config, 0),
            Err(DrizzleError::Config(refused)) if refused.field == field
        ));
    }
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
            constant_image(USize2::new(2, 2), 1.0),
            warp_of(Transform::identity()),
        )]
    };
    let at = |scale| drizzle_plain(frames(), &kernel_config(Kernel::Square, scale, 1.0));
    assert!(matches!(
        at(1e9),
        Err(DrizzleError::OutputSize(FrameDescError::SidePastMax { size }))
            if size == USize2::new(2_000_000_000, 2_000_000_000)
    ));
}
