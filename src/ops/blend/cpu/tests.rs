use strum::IntoEnumIterator;

use crate::common::color_format::{ALL_FORMATS, ColorFormat, SampleType};
use crate::common::image_diff::{max_pixel_diff, pixels_equal};
use crate::common::internals::create_test_image;
use crate::image::Image;
use crate::image::image_desc::ImageDesc;
use crate::ops::blend::cpu;
use crate::ops::blend::{Blend, BlendMode};

/// Blends `src` over `dst` into a fresh image through the public CPU entry
/// point, i.e. through whatever SIMD kernel this CPU dispatches to.
fn blend(params: Blend, src: &Image, dst: &Image) -> Image {
    let mut output = Image::new_black(dst.desc()).unwrap();
    params.apply_cpu(src, dst, &mut output);
    output
}

#[test]
fn blend_mode_formulas() {
    // Hand-computed from the formulas on `BlendMode`, at alpha = 1 so the mix
    // term is the blended value alone.
    let cases = [
        (BlendMode::Normal, 0.6, 0.2, 0.6),
        (BlendMode::Add, 0.6, 0.2, 0.8),
        // 0.7 + 0.6 = 1.3, clamped at one.
        (BlendMode::Add, 0.7, 0.6, 1.0),
        // 0.2 - 0.6 = -0.4, clamped at zero.
        (BlendMode::Subtract, 0.6, 0.2, 0.0),
        (BlendMode::Subtract, 0.2, 0.6, 0.4),
        (BlendMode::Multiply, 0.6, 0.2, 0.12),
        // 1 - (1 - 0.6) * (1 - 0.2) = 1 - 0.4 * 0.8 = 0.68
        (BlendMode::Screen, 0.6, 0.2, 0.68),
        // dst < 0.5, so Multiply doubled: 2 * 0.6 * 0.2 = 0.24
        (BlendMode::Overlay, 0.6, 0.2, 0.24),
        // dst >= 0.5, so Screen doubled: 1 - 2 * 0.4 * 0.2 = 0.84
        (BlendMode::Overlay, 0.6, 0.8, 0.84),
    ];
    // The operands and the expected values are decimals, which f32 holds to within 6e-8 each;
    // two products and a sum of them stay well inside 1e-6.
    for (mode, src, dst, want) in cases {
        let got = mode.blend(src, dst, 1.0);
        assert!(
            (got - want).abs() < 1e-6,
            "{mode:?}(src={src}, dst={dst}) = {got}, want {want}"
        );
    }

    // The alpha mix rides on top: blended * alpha + dst * (1 - alpha).
    // Multiply(0.6, 0.2) = 0.12, so 0.12 * 0.5 + 0.2 * 0.5 = 0.16.
    let mixed = BlendMode::Multiply.blend(0.6, 0.2, 0.5);
    assert!(
        (mixed - 0.16).abs() < 1e-6,
        "alpha mix = {mixed}, want 0.16"
    );
    // Alpha zero drops the blend entirely.
    let none = BlendMode::Multiply.blend(0.6, 0.2, 0.0);
    assert!((none - 0.2).abs() < 1e-6, "alpha=0 = {none}, want dst 0.2");
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
        (ColorFormat::L_U8, 256, bytes),
        (
            ColorFormat::RGBA_U16,
            16384,
            bytemuck::cast_slice(&words).to_vec(),
        ),
    ] {
        let image = Image::new_with_data(ImageDesc::new(width, 1, format), data).unwrap();
        let output = blend(Blend::new(BlendMode::Normal, 0.3), &image, &image);
        assert!(
            pixels_equal(&image, &output),
            "{format}: off by {}",
            max_pixel_diff(&image, &output)
        );
    }
}

#[test]
fn alpha_zero_returns_dst() {
    for format in ALL_FORMATS {
        let src = create_test_image(format, 8, 4, 0);
        let dst = create_test_image(format, 8, 4, 100);

        let output = blend(Blend::new(BlendMode::Normal, 0.0), &src, &dst);

        // `blended * 0 + dst * 1` is dst exactly, and the integer formats
        // round-trip their `/ max` and `* max` without loss.
        assert!(
            pixels_equal(&dst, &output),
            "alpha=0 must return dst for {format}, off by {}",
            max_pixel_diff(&dst, &output)
        );
    }
}

#[test]
fn alpha_one_normal_returns_src() {
    for format in ALL_FORMATS {
        let src = create_test_image(format, 8, 4, 0);
        let dst = create_test_image(format, 8, 4, 100);

        let output = blend(Blend::new(BlendMode::Normal, 1.0), &src, &dst);

        assert!(
            pixels_equal(&src, &output),
            "alpha=1 Normal must return src for {format}, off by {}",
            max_pixel_diff(&src, &output)
        );
    }
}

#[test]
fn multiply_by_white_keeps_dst_color() {
    let format = ColorFormat::RGBA_U8;
    let mut src = create_test_image(format, 8, 4, 50);
    let dst = create_test_image(format, 8, 4, 100);
    // White color channels, dst's alpha, so only the color result is under test.
    for (i, byte) in src.bytes_mut().iter_mut().enumerate() {
        *byte = if i % 4 == 3 { dst.bytes()[i] } else { u8::MAX };
    }

    let output = blend(Blend::new(BlendMode::Multiply, 1.0), &src, &dst);

    for (i, (&want, &got)) in dst.bytes().iter().zip(output.bytes()).enumerate() {
        if i % 4 != 3 {
            assert_eq!(got, want, "channel {i}: multiply by white must return dst");
        }
    }
}

#[test]
fn multiply_by_black_zeroes_color() {
    let format = ColorFormat::RGBA_U8;
    let mut src = create_test_image(format, 8, 4, 50);
    for (i, byte) in src.bytes_mut().iter_mut().enumerate() {
        if i % 4 != 3 {
            *byte = 0;
        }
    }
    let dst = create_test_image(format, 8, 4, 100);

    let output = blend(Blend::new(BlendMode::Multiply, 1.0), &src, &dst);

    for (i, &byte) in output.bytes().iter().enumerate() {
        if i % 4 != 3 {
            assert_eq!(byte, 0, "channel {i}: multiply by black must be black");
        }
    }
}

/// Every SIMD kernel must agree with the scalar reference it specializes,
/// exactly: the vector path evaluates the same expression in the same order and
/// the same units, down to dividing where the reference divides.
///
/// The widths straddle the four-pixel vector body: 3 is tail only, 17 is four
/// vectors plus a tail, 64 is vectors only. The float images carry NaN, the
/// infinities and out-of-range values, which every backend must pass through as
/// the reference does, and a NaN alpha reaches the integer paths.
#[test]
fn simd_matches_scalar_reference() {
    for format in ALL_FORMATS {
        for width in [3, 17, 64] {
            let src = create_test_image(format, width, 3, 0);
            let mut dst = create_test_image(format, width, 3, 100);
            if format.sample_type == SampleType::F32 {
                let specials = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5, 1.5, -0.0];
                let samples: &mut [f32] = bytemuck::cast_slice_mut(dst.bytes_mut());
                for (sample, special) in samples.iter_mut().step_by(5).zip(specials.iter().cycle())
                {
                    *sample = *special;
                }
            }

            for mode in BlendMode::iter() {
                for alpha in [0.0, 0.35, 1.0, f32::NAN] {
                    let params = Blend::new(mode, alpha);
                    let dispatched = blend(params, &src, &dst);
                    let mut reference = Image::new_black(dst.desc()).unwrap();
                    cpu::apply_scalar(params, &src, &dst, &mut reference);

                    assert!(
                        pixels_equal(&reference, &dispatched),
                        "{format} {mode:?} alpha={alpha} width={width}: \
                         SIMD and scalar differ by {}",
                        max_pixel_diff(&reference, &dispatched)
                    );
                }
            }
        }
    }
}
