use crate::internals::prelude::*;
use crate::io::image::pixel_flags::{Flags, PixelFlags};
use crate::registration::config::{InterpolationMethod, WarpParams};
use crate::registration::resample;
use crate::registration::resample::WarpBuffers;
use crate::registration::transform::{Transform, WarpTransform};

/// A constant read back through normalized weights: the f32 sum of up to 64 terms of 3.25 rounds
/// to at most 64·ε·3.25 = 2.5e-5.
const TOL: f32 = 2.5e-5;

/// A `size` image of `channels` signed, unstructured planes, interleaved.
fn signed_pixels(size: Size2us, channels: usize) -> Vec<f32> {
    (0..size.pixel_count() * channels)
        .map(|i| ((i * 13 + i / size.width * 7) % 31) as f32 / 9.0 - 1.7)
        .collect()
}

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
            for method in InterpolationMethod::ALL {
                let result = resample::warp(
                    &image,
                    &transform,
                    WarpParams {
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

/// A constant field with one null in it, paired with the same pixels and no mask.
///
/// A kernel reconstructing a constant from any subset of its taps returns the constant, so
/// [`Self::declared`](NullFixture::declared) must warp to `CONSTANT` everywhere it has support —
/// whatever the fill under the null was. [`Self::undeclared`](NullFixture::undeclared) is the
/// control: nothing tells the resampler that sample is not data, so it interpolates it like any
/// other and the fill spreads.
#[derive(Debug)]
struct NullFixture {
    declared: LinearImage,
    undeclared: LinearImage,
}

impl NullFixture {
    fn new(dimensions: ImageDimensions, null_index: usize) -> Self {
        const FILL: f32 = 999.0;
        let mut pixels = vec![3.25f32; dimensions.pixel_count()];
        pixels[null_index] = FILL;
        let mut nulls = vec![0.0f32; dimensions.pixel_count()];
        nulls[null_index] = f32::NAN;

        let mut declared = LinearImage::from_pixels(dimensions, pixels.clone());
        declared.flags = PixelFlags::of_non_finite(dimensions.size(), &[&nulls]);
        Self {
            declared,
            undeclared: LinearImage::from_pixels(dimensions, pixels),
        }
    }
}

#[test]
fn a_null_is_reconstructed_from_its_surviving_taps_rather_than_smeared() {
    // Half a pixel, so every tap of every kernel carries weight and a null actually reaches its
    // neighbours. On an exact integer shift the separable kernels are zero away from the centre and
    // nothing would spread at all.
    const CONSTANT: f32 = 3.25;
    const BORDER: f32 = -7.0;
    let dimensions = ImageDimensions::new((16, 16), 1);
    let fixture = NullFixture::new(dimensions, 8 * 16 + 8);
    let transform = WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5)));

    // The masked value is the ratio of two warps, each rounding to `(SIZE² + 3)·ε` of its absolute
    // weight sum — under 2 for any kernel here — times its scale, `CONSTANT` and 1. One null takes
    // one tap, at most `L(½)² ≈ 0.37` at this shift, so the denominator stays above 0.6.
    let tolerance = 2.0 * 67.0 * f32::EPSILON * 2.0 * CONSTANT / 0.6;
    for method in InterpolationMethod::ALL {
        let params = WarpParams {
            method,
            border_value: BORDER,
        };
        let masked = resample::warp(&fixture.declared, &transform, params);
        let plain = resample::warp(&fixture.undeclared, &transform, params);

        // Every pixel the frame still supports reads the constant back to rounding: interpolating a
        // flat field over whichever taps survived is that field, so the fill never reaches the
        // result no matter how much of the window it took up.
        let mut reduced = 0;
        for index in 0..dimensions.pixel_count() {
            let value = masked.image.channel(0).pixels()[index];
            let coverage = masked.coverage.pixels()[index];
            if coverage > 0.0 {
                assert!(
                    (value - CONSTANT).abs() <= tolerance,
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
            .filter(|&index| (plain.image.channel(0).pixels()[index] - CONSTANT).abs() > tolerance)
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
    let fixture = NullFixture::new(dimensions, 8 * 16 + 8);
    let transform = WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5)));

    // Against the same frame without the null, so the frame's own edge band — where a half-pixel
    // shift already costs coverage — is not counted as the null's doing.
    let reduced = |method| {
        let params = WarpParams {
            method,
            border_value: 0.0,
        };
        let masked = resample::warp(&fixture.declared, &transform, params);
        let plain = resample::warp(&fixture.undeclared, &transform, params);
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
    image.flags = PixelFlags::of_non_finite(dimensions.size(), &[&nulls]);

    let result = resample::warp(
        &image,
        &WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5))),
        WarpParams {
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
    assert_eq!(result.image.channel(0).pixels()[corner], 3.25);
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
        WarpParams {
            border_value: f32::NAN,
            ..Default::default()
        },
    );
}
/// `warp_into` writes every plane in full, so buffers handed back dirty — NaN, or what the last
/// frame left — come out bit for bit what a fresh `warp` gives: mono and RGB, without nulls and
/// with them, for an affine model and a homography.
#[test]
fn warp_into_overwrites_dirty_buffers_completely() {
    let size = Size2us::new(20, 14);
    let transforms = [
        WarpTransform::new(Transform::similarity(DVec2::new(1.5, -2.25), 0.05, 1.02)),
        WarpTransform::new(Transform::homography([
            1.01, 0.02, -1.5, -0.01, 0.99, 2.0, 1e-3, -2e-3,
        ])),
    ];
    let mut nulls = vec![0.0f32; size.pixel_count()];
    nulls[5 * size.width + 7] = f32::NAN;
    nulls[9 * size.width + 13] = f32::NAN;
    for channels in [1, 3] {
        let dimensions = ImageDimensions::new((size.width, size.height), channels);
        let plain = LinearImage::from_pixels(dimensions, signed_pixels(size, channels));
        let mut masked = plain.clone();
        masked.flags = PixelFlags::of_non_finite(size, &[&nulls]);
        let previous = LinearImage::from_pixels(
            dimensions,
            signed_pixels(size, channels)
                .into_iter()
                .map(|value| value * -3.0)
                .collect(),
        );
        for image in [&plain, &masked] {
            for transform in &transforms {
                for method in InterpolationMethod::ALL {
                    let params = WarpParams {
                        method,
                        border_value: -7.0,
                    };
                    let fresh = resample::warp(image, transform, params);

                    let mut sentinel = WarpBuffers::new(dimensions);
                    for plane in sentinel.pixels.planes_mut() {
                        plane.pixels_mut().fill(f32::NAN);
                    }
                    sentinel.coverage.pixels_mut().fill(f32::NAN);
                    sentinel.confidence.pixels_mut().fill(f32::NAN);
                    sentinel.warp_into(image, transform, params);

                    let mut reused = WarpBuffers::new(dimensions);
                    reused.warp_into(&previous, &transforms[0], params);
                    reused.warp_into(image, transform, params);

                    for buffers in [&sentinel, &reused] {
                        for channel in 0..channels {
                            assert_bitwise(
                                buffers.pixels.channel(channel).pixels(),
                                fresh.image.channel(channel).pixels(),
                                method,
                            );
                        }
                        assert_bitwise(buffers.coverage.pixels(), fresh.coverage.pixels(), method);
                        assert_bitwise(
                            buffers.confidence.pixels(),
                            fresh.confidence.pixels(),
                            method,
                        );
                    }
                }
            }
        }
    }
}

/// Every channel of an RGB warp is the warp of that channel alone, bit for bit: the channels share
/// the row's positions and maps but nothing of each other's values.
#[test]
fn an_rgb_warp_is_three_mono_warps() {
    let size = Size2us::new(20, 14);
    let rgb = LinearImage::from_pixels(
        ImageDimensions::new((size.width, size.height), 3),
        signed_pixels(size, 3),
    );
    let transform = WarpTransform::new(Transform::euclidean(DVec2::new(3.0, -2.0), 0.0175));
    for method in InterpolationMethod::ALL {
        let params = WarpParams {
            method,
            border_value: 0.0,
        };
        let warped = resample::warp(&rgb, &transform, params);
        for channel in 0..3 {
            let mono = LinearImage::from_pixels(
                ImageDimensions::new((size.width, size.height), 1),
                rgb.channel(channel).pixels().to_vec(),
            );
            let alone = resample::warp(&mono, &transform, params);
            assert_bitwise(
                warped.image.channel(channel).pixels(),
                alone.image.channel(0).pixels(),
                method,
            );
            assert_bitwise(warped.coverage.pixels(), alone.coverage.pixels(), method);
            assert_bitwise(
                warped.confidence.pixels(),
                alone.confidence.pixels(),
                method,
            );
        }
    }
}

fn assert_bitwise(actual: &[f32], expected: &[f32], method: InterpolationMethod) {
    assert_eq!(actual.len(), expected.len());
    for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            a.to_bits(),
            e.to_bits(),
            "{method:?} pixel {index}: {a} against {e}"
        );
    }
}

mod plane;

/// A flag reaches every output pixel whose kernel window reads the flagged source pixel. Under a
/// half-pixel shift output x samples source x + 0.5, whose cell is x, and a Lanczos-3 window reads
/// cells x − 2 ..= x + 3. A saturated source pixel at (10, 10) is therefore read by the outputs
/// x, y ∈ 7..=12: 6 × 6 = 36 of them, and no others. `NO_DATA` becomes coverage, not a flag.
#[test]
fn a_flag_reaches_every_output_its_kernel_window_reads() {
    let size = Size2us::new(24, 24);
    let mut image = gray_image(size, vec![0.25; size.pixel_count()]);
    image.flags = PixelFlags::from_fn(size, |index| match index {
        index if index == 10 * 24 + 10 => Flags::SATURATED,
        index if index == 20 * 24 + 20 => Flags::NO_DATA,
        _ => Flags::default(),
    });
    let transform = WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5)));
    let warped = resample::warp(
        &image,
        &transform,
        WarpParams {
            method: InterpolationMethod::Lanczos3,
            border_value: 0.0,
        },
    );
    let flags = warped.image.flags.unwrap();
    assert_eq!(flags.count(Flags::SATURATED), 36);
    assert_eq!(flags.count(Flags::NO_DATA), 0);
    for y in 7..=12 {
        for x in 7..=12 {
            assert_eq!(
                flags.at_pos(Vec2us::new(x, y)),
                Flags::SATURATED,
                "({x}, {y})"
            );
        }
    }
}
