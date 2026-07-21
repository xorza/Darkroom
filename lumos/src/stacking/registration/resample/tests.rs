use crate::io::image::ImageDimensions;
use crate::io::image::linear::LinearImage;
use crate::stacking::registration::config::{InterpolationMethod, WarpParams};
use crate::stacking::registration::resample;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use glam::DVec2;

const TOL: f32 = 1e-5;
const INTERPOLATION_METHODS: [InterpolationMethod; 6] = [
    InterpolationMethod::Nearest,
    InterpolationMethod::Bilinear,
    InterpolationMethod::Bicubic,
    InterpolationMethod::Lanczos2,
    InterpolationMethod::Lanczos3,
    InterpolationMethod::Lanczos4,
];

#[test]
fn translated_images_use_border_only_outside_source_footprint() {
    const WIDTH: usize = 8;
    const HEIGHT: usize = 6;
    const BORDER: f32 = -7.0;
    const CONSTANT: f32 = 3.25;
    let dimensions = ImageDimensions::new((WIDTH, HEIGHT), 1);
    let fixtures = [
        ("constant", vec![CONSTANT; WIDTH * HEIGHT]),
        (
            "ramp",
            (0..WIDTH * HEIGHT)
                .map(|index| 10.0 + (index % WIDTH) as f32 + (index / WIDTH) as f32 * 0.25)
                .collect(),
        ),
    ];

    for (fixture_name, pixels) in fixtures {
        let image = LinearImage::from_pixels(dimensions, pixels);
        for (translation, outside_x, inside_x) in [(-0.75, 0, 1), (0.75, WIDTH - 1, WIDTH - 2)] {
            let transform =
                WarpTransform::new(Transform::translation(DVec2::new(translation, 0.0)));
            for method in INTERPOLATION_METHODS {
                let result = resample::warp(
                    &image,
                    &transform,
                    &WarpParams {
                        method,
                        border_value: BORDER,
                    },
                );
                let y = HEIGHT / 2;
                assert_eq!(
                    result.image.channel(0)[(outside_x, y)],
                    BORDER,
                    "{fixture_name} {method:?} translation {translation}"
                );
                assert_eq!(
                    result.coverage[(outside_x, y)],
                    0.0,
                    "{fixture_name} {method:?} translation {translation}"
                );
                assert_eq!(
                    result.confidence[(outside_x, y)],
                    0.0,
                    "{fixture_name} {method:?} translation {translation}"
                );
                if fixture_name == "constant" {
                    let actual = result.image.channel(0)[(inside_x, y)];
                    assert!(
                        (actual - CONSTANT).abs() < TOL,
                        "{method:?} translation {translation}: expected {CONSTANT}, got {actual}"
                    );
                }
            }
        }
    }
}
