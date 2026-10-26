use common::Size2us;
use simd::IsaTier;
use strum::IntoEnumIterator;

use crate::color::color_format::{ColorFormat, SampleType};
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::create_test_image;
use crate::internals::image_diff::{max_pixel_diff, pixels_equal};
use crate::ops::blend::cpu;
use crate::ops::blend::{Blend, BlendMode};

/// Blends `src` over `dst` into a fresh image through the public CPU entry
/// point, i.e. through whatever vector kernel this CPU dispatches to.
fn blend(params: Blend, src: &InterleavedImage, dst: &InterleavedImage) -> InterleavedImage {
    let mut output = InterleavedImage::new_zeroed(dst.desc());
    params.apply_cpu(src, dst, &mut output);
    output
}

/// Hand-computed from the formulas on `BlendMode`, through the formula and through an `f32`
/// image the CPU blends, whose store keeps every value, out of range included:
///
/// - At alpha 1 the mix is the blended value alone: Add 0.6 + 0.2 = 0.8, and 0.7 + 0.6 = 1.3 held
///   at one; Subtract 0.2 − 0.6 = −0.4 held at zero, 0.6 − 0.2 = 0.4; Multiply 0.12, by white
///   the destination, by black zero; Screen 1 − 0.4 · 0.8 = 0.68; Overlay below 0.5 doubles the
///   product, 2 · 0.6 · 0.2 = 0.24, and above it the screen, 1 − 2 · 0.4 · 0.2 = 0.84. Normal
///   passes −0.2 and 1.5 through.
/// - The alpha mix rides on top, `blended · alpha + dst · (1 − alpha)`: Multiply at 0.5 is
///   0.12 · 0.5 + 0.2 · 0.5 = 0.16, and alpha 0 is the destination.
///
/// The operands and the expected values are decimals, which f32 holds to within 6e-8 each; two
/// products and a sum of them stay well inside 1e-6.
#[test]
fn blend_mode_formulas() {
    let rows = [
        (BlendMode::Normal, 1.0, 0.6, 0.2, 0.6),
        (BlendMode::Normal, 1.0, -0.2, 0.5, -0.2),
        (BlendMode::Normal, 1.0, 1.5, 0.5, 1.5),
        (BlendMode::Add, 1.0, 0.6, 0.2, 0.8),
        (BlendMode::Add, 1.0, 0.7, 0.6, 1.0),
        (BlendMode::Subtract, 1.0, 0.6, 0.2, 0.0),
        (BlendMode::Subtract, 1.0, 0.2, 0.6, 0.4),
        (BlendMode::Multiply, 1.0, 0.6, 0.2, 0.12),
        (BlendMode::Multiply, 1.0, 1.0, 0.4, 0.4),
        (BlendMode::Multiply, 1.0, 0.0, 0.4, 0.0),
        (BlendMode::Screen, 1.0, 0.6, 0.2, 0.68),
        (BlendMode::Overlay, 1.0, 0.6, 0.2, 0.24),
        (BlendMode::Overlay, 1.0, 0.6, 0.8, 0.84),
        (BlendMode::Multiply, 0.5, 0.6, 0.2, 0.16),
        (BlendMode::Multiply, 0.0, 0.6, 0.2, 0.2),
    ];
    let pixel = |value: f32| {
        InterleavedImage::new_with_data(
            InterleavedDesc::new(Size2us::new(1, 1), ColorFormat::MONO_F32),
            value.to_ne_bytes().to_vec(),
        )
        .unwrap()
    };
    for (mode, alpha, src, dst, want) in rows {
        let formula = mode.blend(src, dst, alpha);
        let output = blend(Blend::new(mode, alpha), &pixel(src), &pixel(dst));
        let image = f32::from_ne_bytes(output.bytes().try_into().unwrap());
        for got in [formula, image] {
            assert!(
                (got - want).abs() < 1e-6,
                "{mode:?}(src={src}, dst={dst}, alpha={alpha}) = {got}, want {want}"
            );
        }
    }
}

/// Every `u8` and `u16` value blended over itself by `Normal` at alpha 0.3 is itself: the mix
/// `v · 0.3 + v · 0.7` lands within a few f32 ulps of `v`, on either side, and rounding brings
/// it back. Through whatever kernel dispatch picks.
#[test]
fn blending_a_value_over_itself_keeps_it() {
    let bytes: Vec<u8> = (0..=u8::MAX).collect();
    let words: Vec<u16> = (0..=u16::MAX).collect();
    for (format, width, data) in [
        (ColorFormat::RGBA_U8, 64, bytes.clone()),
        (ColorFormat::MONO_U8, 256, bytes),
        (
            ColorFormat::RGBA_U16,
            16384,
            bytemuck::cast_slice(&words).to_vec(),
        ),
    ] {
        let image = InterleavedImage::new_with_data(
            InterleavedDesc::new(Size2us::new(width, 1), format),
            data,
        )
        .unwrap();
        let output = blend(Blend::new(BlendMode::Normal, 0.3), &image, &image);
        assert!(
            pixels_equal(&image, &output),
            "{format}: off by {}",
            max_pixel_diff(&image, &output)
        );
    }
}

/// In every format, `Normal` at alpha 0 is the destination and at alpha 1 the source, exactly:
/// `blended · 0 + dst · 1` is `dst`, and the integer formats round-trip their `/ max` and `· max`
/// without loss.
#[test]
fn alpha_zero_returns_dst_and_alpha_one_returns_src() {
    for format in ColorFormat::ALL {
        let src = create_test_image(format, Size2us::new(8, 4), 0);
        let dst = create_test_image(format, Size2us::new(8, 4), 100);
        for (alpha, expected) in [(0.0, &dst), (1.0, &src)] {
            let output = blend(Blend::new(BlendMode::Normal, alpha), &src, &dst);
            assert!(
                pixels_equal(expected, &output),
                "alpha {alpha}, {format}: off by {}",
                max_pixel_diff(expected, &output)
            );
        }
    }
}

/// Every tier's kernel agrees with the scalar reference exactly: the kernels evaluate the same
/// expression in the same order and the same units, down to dividing where the reference
/// divides.
///
/// A step is eight `u8` pixels or two `f32` ones. Width 1 is shorter than either step, 3 is
/// shorter than a `u8` step and one past an `f32` one, 17 is whole steps and one pixel, 64 whole
/// steps alone. The float images carry NaN, the infinities, out-of-range values and a negative
/// zero, which every backend must pass through as the reference does, and a NaN alpha reaches
/// the integer paths.
#[test]
fn every_tier_matches_the_scalar_reference() {
    for tier in IsaTier::supported() {
        for format in ColorFormat::ALL {
            for width in [1, 3, 17, 64] {
                let src = create_test_image(format, Size2us::new(width, 3), 0);
                let mut dst = create_test_image(format, Size2us::new(width, 3), 100);
                if format.sample_type == SampleType::F32 {
                    let specials = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5, 1.5, -0.0];
                    let samples: &mut [f32] = bytemuck::cast_slice_mut(dst.bytes_mut());
                    for (sample, special) in
                        samples.iter_mut().step_by(5).zip(specials.iter().cycle())
                    {
                        *sample = *special;
                    }
                }

                for mode in BlendMode::iter() {
                    for alpha in [0.0, 0.35, 1.0, f32::NAN] {
                        let params = Blend::new(mode, alpha);
                        let mut output = InterleavedImage::new_zeroed(dst.desc());
                        cpu::apply_on(tier, params, &src, &dst, &mut output);
                        let reference = scalar(params, &src, &dst);

                        assert!(
                            pixels_equal(&reference, &output),
                            "{tier} {format} {mode:?} alpha={alpha} width={width}: \
                             kernel and scalar reference differ by {}",
                            max_pixel_diff(&reference, &output)
                        );
                    }
                }
            }
        }
    }
}

/// `Add` and `Subtract` clamp by compare-select: a NaN goes to the bound, and a `−0` result of
/// `Subtract` to `+0`, so `−0 − +0` blended at alpha one is `+0`, not `−0`.
#[test]
fn clamps_take_nan_to_the_bound_and_negative_zero_to_positive() {
    assert_eq!(BlendMode::Add.blend(f32::NAN, 0.25, 1.0), 1.0);
    assert_eq!(BlendMode::Subtract.blend(f32::NAN, 0.25, 1.0), 0.0);
    // `blended · 1 + dst · 0`: `+0 · 1 + (−0) · 0 = +0 + −0 = +0`.
    let zero = BlendMode::Subtract.blend(0.0, -0.0, 1.0);
    assert_eq!(zero.to_bits(), 0.0f32.to_bits());
}

/// The scalar reference over every row of `src` and `dst`, past the kernels.
fn scalar(params: Blend, src: &InterleavedImage, dst: &InterleavedImage) -> InterleavedImage {
    let mut output = InterleavedImage::new_zeroed(dst.desc());
    let format = src.desc().color_format();
    let stride = src.desc().row_byte_len();
    let rows = src.bytes().chunks(stride).zip(dst.bytes().chunks(stride));
    for ((src_row, dst_row), out_row) in rows.zip(output.bytes_mut().chunks_mut(stride)) {
        cpu::internals::scalar_row(params, format, src_row, dst_row, out_row);
    }
    output
}
