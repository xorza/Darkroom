use crate::io::image::null_mask::NullMask;
use crate::stacking::registration::config::{InterpolationMethod, WarpParams};
use crate::stacking::registration::resample;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::testing::prelude::*;

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

/// A constant field with one null in it, and the same pixels with the null undeclared.
///
/// A kernel reconstructing a constant from any subset of its taps returns the constant, so the
/// declared frame must warp to `CONSTANT` everywhere it has support — whatever the fill under the
/// null was. The undeclared frame is the control: nothing tells the resampler that sample is not
/// data, so it interpolates it like any other and the fill spreads.
fn null_fixture(dimensions: ImageDimensions, null_index: usize) -> (LinearImage, LinearImage) {
    const FILL: f32 = 999.0;
    let mut pixels = vec![3.25f32; dimensions.pixel_count()];
    pixels[null_index] = FILL;
    let mut nulls = vec![0.0f32; dimensions.pixel_count()];
    nulls[null_index] = f32::NAN;

    let mut declared = LinearImage::from_pixels(dimensions, pixels.clone());
    declared.nulls = NullMask::of_non_finite(dimensions.size(), &[&nulls]);
    (declared, LinearImage::from_pixels(dimensions, pixels))
}

#[test]
fn a_null_is_reconstructed_from_its_surviving_taps_rather_than_smeared() {
    // Half a pixel, so every tap of every kernel carries weight and a null actually reaches its
    // neighbours. On an exact integer shift the separable kernels are zero away from the centre and
    // nothing would spread at all.
    const CONSTANT: f32 = 3.25;
    const BORDER: f32 = -7.0;
    let dimensions = ImageDimensions::new((16, 16), 1);
    let (declared, undeclared) = null_fixture(dimensions, 8 * 16 + 8);
    let transform = WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5)));

    for method in INTERPOLATION_METHODS {
        let params = WarpParams {
            method,
            border_value: BORDER,
        };
        let masked = resample::warp(&declared, &transform, &params);
        let plain = resample::warp(&undeclared, &transform, &params);

        // Every pixel the frame still supports reads the constant back exactly: interpolating a
        // flat field over whichever taps survived is that field, so the fill never reaches the
        // result no matter how much of the window it took up.
        let mut reduced = 0;
        for index in 0..dimensions.pixel_count() {
            let value = masked.image.channel(0).pixels()[index];
            let coverage = masked.coverage.pixels()[index];
            if coverage > 0.0 {
                assert!(
                    (value - CONSTANT).abs() < 1e-3,
                    "{method:?} pixel {index}: coverage {coverage}, value {value}"
                );
            } else {
                assert_eq!(value, BORDER, "{method:?} pixel {index}");
            }
            if coverage < plain.coverage.pixels()[index] {
                reduced += 1;
            }
        }

        // The control frame smears instead: with nothing marking that sample as missing, the 999
        // lands in every output pixel whose window reached it.
        let smeared = (0..dimensions.pixel_count())
            .filter(|&index| (plain.image.channel(0).pixels()[index] - CONSTANT).abs() > 1e-3)
            .count();
        assert!(smeared > 0, "{method:?}: the control must smear");

        // Coverage falls across that same footprint — exactly across it for a kernel whose taps are
        // all positive, and across part of it for one with negative lobes: a window that lost only
        // a negative tap sums its survivors past one and clamps back to full. See
        // `MaskedWarp::fold_into_quality`; the reconstructed value above is exact either way.
        assert!(reduced > 0, "{method:?}: coverage must fall somewhere");
        assert!(
            reduced <= smeared,
            "{method:?}: coverage fell at {reduced} pixels, more than the {smeared} the fill reached"
        );
        if matches!(
            method,
            InterpolationMethod::Nearest | InterpolationMethod::Bilinear
        ) {
            assert_eq!(
                reduced, smeared,
                "{method:?} has no negative lobes, so the two sets must coincide"
            );
        }
    }
}

#[test]
fn the_footprint_a_null_reduces_is_the_kernels_own() {
    // Nearest takes one tap, so a null costs exactly the pixel it lands on. Lanczos4 takes 8 per
    // axis, so the same null costs a 64-pixel block. The parameter has to change the answer, or the
    // resampler is not composing the mask through its kernel at all.
    let dimensions = ImageDimensions::new((16, 16), 1);
    let (declared, undeclared) = null_fixture(dimensions, 8 * 16 + 8);
    let transform = WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5)));

    // Against the same frame without the null, so the frame's own edge band — where a half-pixel
    // shift already costs coverage — is not counted as the null's doing.
    let reduced = |method| {
        let params = WarpParams {
            method,
            border_value: 0.0,
        };
        let masked = resample::warp(&declared, &transform, &params);
        let plain = resample::warp(&undeclared, &transform, &params);
        (0..dimensions.pixel_count())
            .filter(|&index| masked.coverage.pixels()[index] < plain.coverage.pixels()[index])
            .count()
    };

    // A half-pixel shift puts the single nearest tap in exactly one output pixel...
    assert_eq!(reduced(InterpolationMethod::Nearest), 1);
    // ...and bilinear's 2x2 window in four: an output at (x, y) samples source (x - ½, y - ½), so
    // source column 8 is a tap for output columns 8 and 9, and likewise for rows.
    assert_eq!(reduced(InterpolationMethod::Bilinear), 4);
    // Lanczos4 reaches 8 taps per axis, so the same null costs a far wider block. Only the taps it
    // weights positively show up here, which is why this is an ordering rather than 64.
    assert!(
        reduced(InterpolationMethod::Lanczos4) > reduced(InterpolationMethod::Bilinear),
        "Lanczos4 reduced {}, bilinear {}",
        reduced(InterpolationMethod::Lanczos4),
        reduced(InterpolationMethod::Bilinear)
    );
}

#[test]
fn a_block_of_nulls_wider_than_the_kernel_leaves_no_support_at_all() {
    // Where every tap of the window is missing there is nothing to reconstruct from, and the pixel
    // has to read as uncovered rather than as a confident reading of the fill.
    const BORDER: f32 = -7.0;
    let dimensions = ImageDimensions::new((24, 24), 1);
    let mut nulls = vec![0.0f32; dimensions.pixel_count()];
    for y in 6..18 {
        for x in 6..18 {
            nulls[y * 24 + x] = f32::NAN;
        }
    }
    let mut image = LinearImage::from_pixels(dimensions, vec![3.25; dimensions.pixel_count()]);
    image.nulls = NullMask::of_non_finite(dimensions.size(), &[&nulls]);

    let result = resample::warp(
        &image,
        &WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5))),
        &WarpParams {
            method: InterpolationMethod::Bilinear,
            border_value: BORDER,
        },
    );

    // Well inside the block, past the kernel's reach from any valid pixel.
    let centre = 12 * 24 + 12;
    assert_eq!(result.coverage.pixels()[centre], 0.0);
    assert_eq!(result.confidence.pixels()[centre], 0.0);
    assert_eq!(result.image.channel(0).pixels()[centre], BORDER);

    // And well outside it the frame is untouched, so the block cost only its own neighbourhood.
    let corner = 2 * 24 + 2;
    assert_eq!(result.coverage.pixels()[corner], 1.0);
    assert!((result.image.channel(0).pixels()[corner] - 3.25).abs() < 1e-5);
}

#[test]
#[should_panic(expected = "warp border_value must be finite")]
fn warp_refuses_a_non_finite_border() {
    // `WarpParams::validate` is only reached through `RegistrationConfig::validate`, so a direct
    // caller of the public `warp` could fill every out-of-footprint pixel with NaN and have it
    // noticed only by a debug assert deep in the combine.
    let image = LinearImage::from_pixels(ImageDimensions::new((4, 4), 1), vec![0.5; 16]);
    let transform = WarpTransform::new(Transform::translation(DVec2::new(1.0, 1.0)));
    resample::warp(
        &image,
        &transform,
        &WarpParams {
            border_value: f32::NAN,
            ..Default::default()
        },
    );
}
