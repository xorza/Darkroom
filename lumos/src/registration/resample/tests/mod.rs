use std::sync::Arc;

use glam::DVec2;
use imaginarium::Buffer2;

use crate::image::background_split::BackgroundSplit;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::channel_layout::ChannelLayout;
use crate::image::flat_gain::FlatGain;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::linear_image::LinearImage;
use crate::image::mosaic_noise::MosaicNoise;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::internals::images::gray_image;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::registration::resample::source_image::SourceImage;
use crate::registration::resample::warp_config::{InterpolationMethod, WarpConfig};
use crate::registration::resample::{Warp, WarpResult, WarpScratch};
use crate::registration::transform::Transform;
use crate::registration::warp_transform::WarpTransform;
use crate::scratch_pool::Scratch;

/// A constant read back through normalized weights: each f32 sum of up to 64 terms rounds by
/// `64·ε` of its absolute sum, and a window the warp normalizes has `Σ|L| ≤ 8·Σ L` (its squares
/// sum to at most the square of its sum, over at most 64 taps), so the ratio is off by at most
/// `2·64·ε·8·3.25`.
const TOL: f32 = 2.0 * 64.0 * f32::EPSILON * 8.0 * 3.25;

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
    let dimensions = ImageDimensions::new((WIDTH, HEIGHT), ChannelLayout::Mono);
    let fixtures = [
        ("constant", vec![CONSTANT; WIDTH * HEIGHT]),
        (
            "ramp",
            (0..WIDTH * HEIGHT)
                .map(|idx| 10.0 + (idx % WIDTH) as f32 + (idx / WIDTH) as f32 * 0.25)
                .collect(),
        ),
    ];

    for (fixture_name, pixels) in fixtures {
        let image = LinearImage::from_interleaved(dimensions, pixels);
        for (translation, outside_x, inside_x) in [(-0.75, 0, 1), (0.75, WIDTH - 1, WIDTH - 2)] {
            let transform =
                WarpTransform::new(Transform::translation(DVec2::new(translation, 0.0)));
            for method in InterpolationMethod::ALL {
                let result = WarpConfig {
                    method,
                    border_value: BORDER,
                    ..Default::default()
                }
                .warp(&image, &transform);
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
    fn new(dimensions: ImageDimensions, null_idx: usize) -> Self {
        const FILL: f32 = 999.0;
        let mut pixels = vec![3.25f32; dimensions.pixel_count()];
        pixels[null_idx] = FILL;
        let mut nulls = vec![0.0f32; dimensions.pixel_count()];
        nulls[null_idx] = f32::NAN;

        let mut declared = LinearImage::from_interleaved(dimensions, pixels.clone());
        declared.set_flags(PixelFlags::of_non_finite(dimensions.size(), &[&nulls]));
        Self {
            declared,
            undeclared: LinearImage::from_interleaved(dimensions, pixels),
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
    let dimensions = ImageDimensions::new((16, 16), ChannelLayout::Mono);
    let fixture = NullFixture::new(dimensions, 8 * 16 + 8);
    let transform = WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5)));

    // Without the clamp, which would hide the control's fill under a negative lobe: the
    // positive lobes' mean it falls back to is the constant.
    let tolerance = TOL;
    for method in InterpolationMethod::ALL {
        let params = WarpConfig {
            method,
            border_value: BORDER,
            clamping_threshold: None,
        };
        let masked = params.warp(&fixture.declared, &transform);
        let plain = params.warp(&fixture.undeclared, &transform);

        // Every pixel the frame still supports reads the constant back to rounding: interpolating a
        // flat field over whichever taps survived is that field, so the fill never reaches the
        // result no matter how much of the window it took up.
        let mut reduced = 0;
        for idx in 0..dimensions.pixel_count() {
            let value = masked.image.channel(0).pixels()[idx];
            let coverage = masked.coverage.pixels()[idx];
            if coverage > 0.0 {
                assert!(
                    (value - CONSTANT).abs() <= tolerance,
                    "{method:?} pixel {idx}: coverage {coverage}, value {value}"
                );
            } else {
                assert_eq!(value, BORDER, "{method:?} pixel {idx}");
            }
            if coverage < plain.coverage.pixels()[idx] {
                reduced += 1;
            }
        }

        // The control frame smears instead: with nothing marking that sample as missing, the 999
        // lands in every output pixel whose window reached it.
        let smeared = (0..dimensions.pixel_count())
            .filter(|&idx| (plain.image.channel(0).pixels()[idx] - CONSTANT).abs() > tolerance)
            .count();
        assert!(smeared > 0, "{method:?}: the control must smear");

        // Coverage falls across exactly that footprint: it is the share of the kernel's magnitude
        // on data, so a lost tap of either sign costs it, and at this shift every tap weighs.
        assert_eq!(
            reduced, smeared,
            "{method:?}: coverage fell at {reduced} pixels, the fill reached {smeared}"
        );
    }
}

#[test]
fn the_footprint_a_null_reduces_is_the_kernels_own() {
    // Nearest takes one tap, so a null costs exactly the pixel it lands on. Lanczos4 takes 8 per
    // axis, so the same null costs a 64-pixel block. The parameter has to change the answer, or the
    // resampler is not composing the mask through its kernel at all.
    let dimensions = ImageDimensions::new((16, 16), ChannelLayout::Mono);
    let fixture = NullFixture::new(dimensions, 8 * 16 + 8);
    let transform = WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5)));

    // Against the same frame without the null, so the frame's own edge band — where a half-pixel
    // shift already costs coverage — is not counted as the null's doing.
    let reduced = |method| {
        let params = WarpConfig {
            method,
            border_value: 0.0,
            ..Default::default()
        };
        let masked = params.warp(&fixture.declared, &transform);
        let plain = params.warp(&fixture.undeclared, &transform);
        (0..dimensions.pixel_count())
            .filter(|&idx| masked.coverage.pixels()[idx] < plain.coverage.pixels()[idx])
            .count()
    };

    // A half-pixel shift puts the single nearest tap in exactly one output pixel...
    assert_eq!(reduced(InterpolationMethod::Nearest), 1);
    // ...and bilinear's 2x2 window in four: an output at (x, y) samples source (x - ½, y - ½), so
    // source column 8 is a tap for output columns 8 and 9, and likewise for rows.
    assert_eq!(reduced(InterpolationMethod::Bilinear), 4);
    // Lanczos4 reaches 8 taps per axis, every one weighing at this shift, so the same null costs
    // an 8×8 block.
    assert_eq!(reduced(InterpolationMethod::Lanczos4), 64);
}

#[test]
fn a_block_of_nulls_wider_than_the_kernel_leaves_no_support_at_all() {
    // Where every tap of the window is missing there is nothing to reconstruct from, and the pixel
    // has to read as uncovered rather than as a confident reading of the fill.
    const BORDER: f32 = -7.0;
    let dimensions = ImageDimensions::new((24, 24), ChannelLayout::Mono);
    let mut nulls = vec![0.0f32; dimensions.pixel_count()];
    for y in 6..18 {
        for x in 6..18 {
            nulls[y * 24 + x] = f32::NAN;
        }
    }
    let mut image = LinearImage::from_interleaved(dimensions, vec![3.25; dimensions.pixel_count()]);
    image.set_flags(PixelFlags::of_non_finite(dimensions.size(), &[&nulls]));

    let result = WarpConfig {
        method: InterpolationMethod::Bilinear,
        border_value: BORDER,
        ..Default::default()
    }
    .warp(
        &image,
        &WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.5))),
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
    // `WarpConfig::validate` is only reached through `RegistrationConfig::validate`, so a direct
    // caller of the public `warp` could fill every out-of-footprint pixel with NaN and have it
    // noticed only by a debug assert deep in the combine.
    let image = LinearImage::from_interleaved(
        ImageDimensions::new((4, 4), ChannelLayout::Mono),
        vec![0.5; 16],
    );
    let transform = WarpTransform::new(Transform::translation(DVec2::new(1.0, 1.0)));
    WarpConfig {
        border_value: f32::NAN,
        ..Default::default()
    }
    .warp(&image, &transform);
}
/// `warp_into` writes every plane in full, so buffers handed back dirty — NaN, or what the last
/// frame left — come out bit for bit what a fresh `warp` gives: mono and RGB, without nulls and
/// with them, for an affine model and a homography. The last frame flags other pixels — nulls and
/// a saturated core elsewhere — so the planes a set keeps for a flagged frame, its validity, its
/// grown flags and its flag plane, are each rewritten too, and the flags come out the same.
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
    let scratch = WarpScratch::default();
    for channels in [1, 3] {
        let dimensions = ImageDimensions::new(
            (size.width, size.height),
            ChannelLayout::try_from(channels).unwrap(),
        );
        let plain = LinearImage::from_interleaved(dimensions, signed_pixels(size, channels));
        let mut masked = plain.clone();
        masked.set_flags(PixelFlags::of_non_finite(size, &[&nulls]));
        let mut previous = LinearImage::from_interleaved(
            dimensions,
            signed_pixels(size, channels)
                .into_iter()
                .map(|value| value * -3.0)
                .collect(),
        );
        previous.set_flags(PixelFlags::from_fn(size, |idx| match idx {
            30 | 31 => QualityFlags::NO_DATA,
            100..=103 => QualityFlags::SATURATED,
            _ => QualityFlags::default(),
        }));
        for image in [&plain, &masked] {
            for transform in &transforms {
                for method in InterpolationMethod::ALL {
                    let params = WarpConfig {
                        method,
                        border_value: -7.0,
                        ..Default::default()
                    };
                    let fresh = params.warp(image, transform);

                    let mut sentinel = WarpResult::build(dimensions);
                    for plane in sentinel.image.planes_mut() {
                        plane.pixels_mut().fill(f32::NAN);
                    }
                    sentinel.coverage.pixels_mut().fill(f32::NAN);
                    sentinel.confidence.pixels_mut().fill(f32::NAN);
                    sentinel.warp_into(
                        &SourceImage::of(image),
                        Warp { transform, params },
                        &scratch,
                    );

                    let mut reused = WarpResult::build(dimensions);
                    reused.warp_into(
                        &SourceImage::of(&previous),
                        Warp {
                            transform: &transforms[0],
                            params,
                        },
                        &scratch,
                    );
                    reused.warp_into(
                        &SourceImage::of(image),
                        Warp { transform, params },
                        &scratch,
                    );

                    for buffers in [&sentinel, &reused] {
                        for channel in 0..channels {
                            assert_bitwise(
                                buffers.image.channel(channel).pixels(),
                                fresh.image.channel(channel).pixels(),
                                method,
                            );
                        }
                        assert_bitwise(buffers.coverage.pixels(), fresh.coverage.pixels(), method);
                        assert_eq!(
                            buffers.image.flags().map(PixelFlags::bytes),
                            fresh.image.flags().map(PixelFlags::bytes),
                            "{method:?}"
                        );
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
    let rgb = LinearImage::from_interleaved(
        ImageDimensions::new((size.width, size.height), ChannelLayout::Rgb),
        signed_pixels(size, 3),
    );
    let transform = WarpTransform::new(Transform::euclidean(DVec2::new(3.0, -2.0), 0.0175));
    for method in InterpolationMethod::ALL {
        let params = WarpConfig {
            method,
            border_value: 0.0,
            ..Default::default()
        };
        let warped = params.warp(&rgb, &transform);
        for channel in 0..3 {
            let mono = LinearImage::from_interleaved(
                ImageDimensions::new((size.width, size.height), ChannelLayout::Mono),
                rgb.channel(channel).pixels().to_vec(),
            );
            let alone = params.warp(&mono, &transform);
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
    for (idx, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(
            a.to_bits(),
            e.to_bits(),
            "{method:?} pixel {idx}: {a} against {e}"
        );
    }
}

mod plane;

/// A flag that says the value measures nothing leaves its pixel out of the interpolation as a null
/// does: the warp of a frame whose one flagged pixel holds a fill is, bit for bit, the warp with
/// that pixel `NO_DATA`, and carries no flags. A flag that says the value is a bound leaves the
/// value in and carries the flag to every output pixel whose sample gave the pixel weight. Such a
/// window sums its taps in another order than the unflagged warp's, so it matches that warp to the
/// rounding of 36 additions, each within half an ulp of a sum under `Σ|L|·999` =
/// `1.534678²·999` = 2353: `36·2353·ε` = 1.01e-2 for the values, and `36·Σ|L|²·ε` = 1.02e-5 for
/// the confidence, a ratio of sums of order `Σ|L|²`.
///
/// Under a half-pixel shift output x samples source x + 0.5, whose cell is x, and a Lanczos-3
/// window reads cells x − 2 ..= x + 3 with every tap weighing, so a pixel at (10, 10) reaches the
/// outputs x, y ∈ 7..=12 and no others. A null there costs them coverage, and the fill reaches none
/// of them. The coverage it leaves is graded: output (10, 10) has it at distance ½ on both axes,
/// where each axis's six taps weigh `L(½) = 6/π² = 0.607927`, `L(1.5) = −(2/3π)·(2/π) = −0.135095`
/// and `L(2.5) = (2/5π)·(3/5π) = 0.024317`, two of each, so `Σ|w| = 1.534678` and its coverage is
/// `1 − (0.607927 / 1.534678)² = 0.843083`. Each table weight is within 1.1e-7 of the kernel and
/// the f32 sums round by a few ulps, so 2e-6 holds it.
///
/// Under a whole-pixel shift by (1, 0) every tap but the centre sits at a whole distance, where the
/// kernel is exactly zero, so a carried flag reaches the one output (9, 10).
#[test]
fn a_flag_leaves_its_pixel_out_or_carries_it_by_what_it_says() {
    const CONSTANT: f32 = 0.25;
    let size = Size2us::new(24, 24);
    let flagged = 10 * 24 + 10;
    let warp = |flag: Option<QualityFlags>, shift: DVec2| {
        let mut pixels = vec![CONSTANT; size.pixel_count()];
        pixels[flagged] = 999.0;
        let mut image = gray_image(size, pixels);
        image.set_flags(flag.and_then(|flag| {
            PixelFlags::from_fn(size, |idx| {
                if idx == flagged {
                    flag
                } else {
                    QualityFlags::default()
                }
            })
        }));
        WarpConfig {
            method: InterpolationMethod::Lanczos3,
            border_value: 0.0,
            ..Default::default()
        }
        .warp(&image, &WarpTransform::new(Transform::translation(shift)))
    };
    let half = DVec2::splat(0.5);
    let within = |x: usize, y: usize| (7..=12).contains(&x) && (7..=12).contains(&y);

    let null = warp(Some(QualityFlags::NO_DATA), half);
    assert!(null.image.flags().is_none());
    let plain = warp(None, half);
    for y in 0..size.height {
        for x in 0..size.width {
            let idx = y * size.width + x;
            let coverage = null.coverage.pixels()[idx];
            assert_eq!(
                coverage < plain.coverage.pixels()[idx],
                within(x, y),
                "({x}, {y}): coverage {coverage}"
            );
            if coverage > 0.0 {
                let value = null.image.channel(0).pixels()[idx];
                assert!((value - CONSTANT).abs() <= TOL, "({x}, {y}): {value}");
            }
        }
    }
    let centre = null.coverage.pixels()[flagged];
    assert!((centre - 0.843_083).abs() <= 2e-6, "coverage {centre}");

    let close = |a: &WarpResult, b: &WarpResult| {
        let within = |a: &Buffer2<f32>, b: &Buffer2<f32>, tolerance: f32| {
            a.pixels()
                .iter()
                .zip(b.pixels())
                .all(|(a, b)| (a - b).abs() <= tolerance)
        };
        within(a.image.channel(0), b.image.channel(0), 1.01e-2)
            && within(&a.coverage, &b.coverage, 0.0)
            && within(&a.confidence, &b.confidence, 1.02e-5)
    };
    let same_bits = |a: &WarpResult, b: &WarpResult| {
        [
            (a.image.channel(0), b.image.channel(0)),
            (&a.coverage, &b.coverage),
            (&a.confidence, &b.confidence),
        ]
        .iter()
        .all(|(a, b)| {
            a.pixels()
                .iter()
                .zip(b.pixels())
                .all(|(a, b)| a.to_bits() == b.to_bits())
        })
    };
    let mut carried = 0;
    for (flag, name) in QualityFlags::NAMED {
        let warped = warp(Some(flag), half);
        if flag.intersects(QualityFlags::RESAMPLE_EXCLUDED) {
            assert!(warped.image.flags().is_none(), "{name}");
            assert!(same_bits(&warped, &null), "{name}");
            continue;
        }
        carried += 1;
        assert!(close(&warped, &plain), "{name}");
        let flags = warped.image.flags().unwrap();
        for y in 0..size.height {
            for x in 0..size.width {
                let expected = if within(x, y) {
                    flag
                } else {
                    QualityFlags::default()
                };
                assert_eq!(
                    flags.at_pos(Vec2us::new(x, y)),
                    expected,
                    "{name} ({x}, {y})"
                );
            }
        }

        let whole = warp(Some(flag), DVec2::new(1.0, 0.0));
        let flags = whole.image.flags().unwrap();
        assert_eq!(flags.count(flag), 1, "{name}");
        assert_eq!(flags.at_pos(Vec2us::new(9, 10)), flag, "{name}");
    }
    assert_eq!(carried, 2);
}

/// A warp moves the flat gain with the pixels: a frame whose gain rises as `1 + x/8` across 9
/// columns, shifted by 4, reads at output x the source's gain at x + 4, the node averages 1.5 and
/// 1.875 of `flat_gain`'s own test, each within 1e-6 of the line.
#[test]
fn a_warp_moves_the_flat_gain_with_the_pixels() {
    let size = Size2us::new(9, 9);
    let mut image = gray_image(size, vec![0.5; size.pixel_count()]);
    let divisor = Buffer2::from_fn(9, 9, |x, _| 1.0 / (1.0 + x as f32 / 8.0));
    image.set_flat_gain(Some(Arc::new(FlatGain::of_divisor(
        &divisor,
        &CfaType::Mono,
        |_| false,
    ))));
    let warped = WarpConfig::default().warp(
        &image,
        &WarpTransform::new(Transform::translation(DVec2::new(4.0, 0.0))),
    );
    let gain = warped.image.flat_gain().unwrap();
    for (x, expected) in [(0.0, 1.5), (4.0, 1.875)] {
        let at = gain.at(0, x, 4.0);
        assert!((at - expected).abs() <= 1e-6, "x = {x}: {at}");
    }
}

/// A warp keeps the quantization step only where a sample is still one source sample: the nearest
/// pixel keeps 0.25, and every interpolating kernel clears it. The mosaic's noise, measured before
/// any warp, stays either way.
#[test]
fn only_the_nearest_pixel_keeps_the_quantization_step() {
    let size = Size2us::new(9, 9);
    let mut image = gray_image(size, vec![0.5; size.pixel_count()]);
    image.metadata.derived.set_quantization_sigma(Some(0.25));
    let mosaic_noise = MosaicNoise {
        background: [0.1, 0.2, 0.3].map(BackgroundSplit::of_sigma),
        sky: [1.0; 3],
        quantization_sigma: Some(0.25),
    };
    image.metadata.derived.set_mosaic_noise(Some(mosaic_noise));
    let transform = WarpTransform::new(Transform::translation(DVec2::new(0.5, 0.0)));
    for method in InterpolationMethod::ALL {
        let warped = WarpConfig::with_method(method).warp(&image, &transform);
        let expected = (method == InterpolationMethod::Nearest).then_some(0.25);
        assert_eq!(
            warped.image.metadata.derived.quantization_sigma(),
            expected,
            "{method:?}"
        );
        assert_eq!(
            warped.image.metadata.derived.mosaic_noise(),
            Some(mosaic_noise),
            "{method:?}"
        );
    }
}
