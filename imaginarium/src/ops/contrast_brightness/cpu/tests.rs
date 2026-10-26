use crate::color::color_format::{ColorFormat, SampleType};
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::create_test_image;
use crate::internals::image_diff::{max_pixel_diff, pixels_equal};
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;
use crate::ops::contrast_brightness::{ContrastBrightness, cpu};
use common::Size2us;
use simd::IsaTier;

/// Builds a single-row image from raw channel bytes.
fn image_from_channels(format: ColorFormat, width: usize, bytes: Vec<u8>) -> InterleavedImage {
    let desc = InterleavedDesc::new(Size2us::new(width, 1), format);
    assert_eq!(
        bytes.len(),
        desc.row_byte_len(),
        "row size mismatch for {format}"
    );
    InterleavedImage::new_with_data(desc, bytes).unwrap()
}

/// Long enough that every kernel runs whole steps and then a partial one: a step is 32 `u8`, 16
/// `u16` or 8 `f32` samples, so 37 mono samples leave 5, 5 and 5 over, and 37 RGBA pixels (148
/// samples) leave 20, 4 and 4.
const SWEEP_LEN: usize = 37;

/// `contrast = 2.0, brightness = 0.0` folds to `value * 2 - mid`, whose ties
/// land exactly on `.5` — so these also pin the round-half-to-even behaviour.
const CONTRAST_TWO: ContrastBrightness = ContrastBrightness {
    contrast: 2.0,
    brightness: 0.0,
};

#[test]
fn contrast_two_maps_hand_computed_values_u8() {
    // offset = 127.5 * (1 - 2) = -127.5, clamped to [0, 255], ties to even:
    //    0 -> -127.5 -> clamp    ->   0
    //   64 ->  128 - 127.5 = 0.5 ->   0  (tie, down to even)
    //   65 ->  130 - 127.5 = 2.5 ->   2  (tie, down to even)
    //  128 ->  256 - 127.5 = 128.5 -> 128  (tie, down to even)
    //  255 ->  510 - 127.5 = 382.5 -> clamp -> 255
    let inputs: [u8; 5] = [0, 64, 65, 128, 255];
    let expected: [u8; 5] = [0, 0, 2, 128, 255];

    let bytes: Vec<u8> = (0..SWEEP_LEN).map(|i| inputs[i % 5]).collect();
    let want: Vec<u8> = (0..SWEEP_LEN).map(|i| expected[i % 5]).collect();

    let mut image = image_from_channels(ColorFormat::MONO_U8, SWEEP_LEN, bytes);
    CONTRAST_TWO.apply_cpu(&mut image);

    assert_eq!(image.bytes(), want.as_slice());
}

#[test]
fn contrast_two_maps_hand_computed_values_u16() {
    // offset = 32767.5 * (1 - 2) = -32767.5, clamped to [0, 65535]:
    //      0 -> -32767.5 -> clamp             ->     0
    //  16384 ->  32768 - 32767.5 = 0.5        ->     0  (tie, down to even)
    //  16385 ->  32770 - 32767.5 = 2.5        ->     2  (tie, down to even)
    //  32768 ->  65536 - 32767.5 = 32768.5    -> 32768  (tie, down to even)
    //  65535 -> 131070 - 32767.5 = 98302.5    -> clamp -> 65535
    let inputs: [u16; 5] = [0, 16384, 16385, 32768, 65535];
    let expected: [u16; 5] = [0, 0, 2, 32768, 65535];

    let values: Vec<u16> = (0..SWEEP_LEN).map(|i| inputs[i % 5]).collect();
    let want: Vec<u16> = (0..SWEEP_LEN).map(|i| expected[i % 5]).collect();

    let mut image = image_from_channels(
        ColorFormat::MONO_U16,
        SWEEP_LEN,
        bytemuck::cast_slice(&values).to_vec(),
    );
    CONTRAST_TWO.apply_cpu(&mut image);

    assert_eq!(image.bytes(), bytemuck::cast_slice::<u16, u8>(&want));
}

#[test]
fn contrast_two_maps_hand_computed_values_f32() {
    // offset = 0.5 * (1 - 2) = -0.5, clamped to [0, 1] — every step exact in f32:
    //  0.00 -> -0.5 -> clamp -> 0.0
    //  0.25 ->  0.5 - 0.5    -> 0.0
    //  0.50 ->  1.0 - 0.5    -> 0.5
    //  0.75 ->  1.5 - 0.5    -> 1.0
    //  1.00 ->  2.0 - 0.5    -> clamp -> 1.0
    let inputs: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
    let expected: [f32; 5] = [0.0, 0.0, 0.5, 1.0, 1.0];

    let values: Vec<f32> = (0..SWEEP_LEN).map(|i| inputs[i % 5]).collect();
    let want: Vec<f32> = (0..SWEEP_LEN).map(|i| expected[i % 5]).collect();

    let mut image = image_from_channels(
        ColorFormat::MONO_F32,
        SWEEP_LEN,
        bytemuck::cast_slice(&values).to_vec(),
    );
    CONTRAST_TWO.apply_cpu(&mut image);

    assert_eq!(image.bytes(), bytemuck::cast_slice::<f32, u8>(&want));
}

/// Brightness alone adds `brightness · max`, exactly where nothing saturates: at 0.2 that is
/// `0.2 · 255 = 51` for `u8` (100 → 151), `0.2 · 65535 = 13107` for `u16` (1000 → 14107),
/// and `0.2` itself for `f32` (0.25 → 0.25 + 0.2, rounded once in f32).
#[test]
fn brightness_adds_its_share_of_the_full_scale() {
    let params = ContrastBrightness::new(1.0, 0.2);
    let mut image = image_from_channels(ColorFormat::MONO_U8, 1, vec![100]);
    params.apply_cpu(&mut image);
    assert_eq!(image.bytes(), &[151]);

    let mut image = image_from_channels(
        ColorFormat::MONO_U16,
        1,
        bytemuck::cast_slice(&[1000u16]).to_vec(),
    );
    params.apply_cpu(&mut image);
    assert_eq!(image.bytes(), bytemuck::cast_slice::<u16, u8>(&[14107]));

    let mut image = image_from_channels(
        ColorFormat::MONO_F32,
        1,
        bytemuck::cast_slice(&[0.25f32]).to_vec(),
    );
    params.apply_cpu(&mut image);
    assert_eq!(
        image.bytes(),
        bytemuck::cast_slice::<f32, u8>(&[0.25f32 + 0.2])
    );
}

#[test]
fn contrast_two_transforms_rgb_and_leaves_alpha_alone() {
    // Same mapping as the contiguous u8 case, but laid out as RGBA pixels: the three
    // color channels move, the fourth is copied through even where it would
    // otherwise have clamped.
    let rgb: [u8; 3] = [64, 65, 128];
    let rgb_want: [u8; 3] = [0, 2, 128];
    let alphas: [u8; 4] = [0, 64, 200, 255];

    let mut bytes = Vec::with_capacity(SWEEP_LEN * 4);
    let mut want = Vec::with_capacity(SWEEP_LEN * 4);
    for i in 0..SWEEP_LEN {
        bytes.extend_from_slice(&rgb);
        bytes.push(alphas[i % 4]);
        want.extend_from_slice(&rgb_want);
        want.push(alphas[i % 4]);
    }

    let mut image = image_from_channels(ColorFormat::RGBA_U8, SWEEP_LEN, bytes);
    CONTRAST_TWO.apply_cpu(&mut image);

    assert_eq!(image.bytes(), want.as_slice());
}

#[test]
fn identity_params_leave_every_format_bit_identical() {
    // contrast 1.0 / brightness 0.0 folds to `value * 1.0 + 0.0`, which is
    // exact for floats too — so this is an equality check, not an epsilon one.
    for format in ColorFormat::ALL {
        let input = create_test_image(format, Size2us::new(17, 5), 0);
        let mut output = input.clone();

        ContrastBrightness::new(1.0, 0.0).apply_cpu(&mut output);

        assert!(
            pixels_equal(&input, &output),
            "no-change failed for format {format}"
        );
    }
}

#[test]
fn alpha_survives_every_alpha_format() {
    for format in ColorFormat::ALL
        .into_iter()
        .filter(|format| format.has_alpha())
    {
        let input = create_test_image(format, Size2us::new(16, 4), 0);
        let mut output = input.clone();

        ContrastBrightness::new(2.0, 0.3).apply_cpu(&mut output);

        let channel_count = format.channel_count.count();
        let sample_byte_len = format.sample_type.byte_len();
        let alpha_offset = (channel_count - 1) * sample_byte_len;
        let pixel_byte_len = channel_count * sample_byte_len;

        for row in 0..4 {
            let row_start = row * input.desc().row_byte_len();
            for x in 0..16 {
                let pixel_start = row_start + x * pixel_byte_len;
                let alpha_start = pixel_start + alpha_offset;
                let in_alpha = &input.bytes()[alpha_start..alpha_start + sample_byte_len];
                let out_alpha = &output.bytes()[alpha_start..alpha_start + sample_byte_len];
                assert_eq!(
                    in_alpha, out_alpha,
                    "alpha mismatch for format {format} at pixel ({x}, {row})"
                );
            }
        }
    }
}

#[test]
fn every_knob_direction_changes_every_format() {
    // Contrast and brightness, each above and below its identity value, plus a
    // combined move and a width that lands mid-vector.
    let cases = [
        ("brightness up", ContrastBrightness::new(1.0, 0.2), 8, 2),
        ("brightness down", ContrastBrightness::new(1.0, -0.2), 8, 2),
        ("contrast up", ContrastBrightness::new(2.0, 0.0), 8, 2),
        ("contrast down", ContrastBrightness::new(0.5, 0.0), 8, 2),
        ("combined", ContrastBrightness::new(1.5, 0.1), 17, 5),
        ("odd dimensions", ContrastBrightness::new(1.3, -0.05), 17, 7),
    ];

    for (label, params, width, height) in cases {
        for format in ColorFormat::ALL {
            let input = create_test_image(format, Size2us::new(width, height), 0);
            let mut output = input.clone();

            params.apply_cpu(&mut output);

            assert!(
                !pixels_equal(&input, &output),
                "{label} should change output for format {format}"
            );
        }
    }
}

#[test]
fn extreme_brightness_clamps_to_the_range_ends() {
    for format in ColorFormat::ALL {
        let input = create_test_image(format, Size2us::new(4, 2), 0);
        let affine_max = ChannelAffine::new(ContrastBrightness::default(), format).max;

        // Saturating up pins every color channel at the format's maximum, and
        // saturating down pins it at zero. Alpha is exempt.
        for (brightness, want) in [(1.0, affine_max), (-1.0, 0.0)] {
            let mut output = input.clone();
            ContrastBrightness::new(1.0, brightness).apply_cpu(&mut output);

            let channel_count = format.channel_count.count();
            let color_count = channel_count - usize::from(format.has_alpha());
            for (index, value) in channel_values(&output).iter().enumerate() {
                if index % channel_count >= color_count {
                    continue;
                }
                assert_eq!(
                    *value, want,
                    "brightness {brightness} did not saturate channel {index} of {format}"
                );
            }
        }
    }
}

/// Every color channel of `image` as an `f32`, in its own units.
fn channel_values(image: &InterleavedImage) -> Vec<f32> {
    let format = image.desc().color_format();
    match format.sample_type {
        SampleType::U8 => image.bytes().iter().map(|v| f32::from(*v)).collect(),
        SampleType::U16 => bytemuck::cast_slice::<u8, u16>(image.bytes())
            .iter()
            .map(|v| f32::from(*v))
            .collect(),
        SampleType::F32 => bytemuck::cast_slice::<u8, f32>(image.bytes()).to_vec(),
    }
}

/// Contrast-only, brightness-only, combined, and clamp-heavy.
const PARAM_SWEEP: [(f32, f32); 4] = [(2.0, 0.0), (1.0, 0.2), (1.5, 0.1), (0.5, -0.8)];

/// Every tier's kernel agrees with the scalar reference bit for bit, floats included: both
/// evaluate the same affine in the same order. Width 3 is shorter than any step, 17 whole steps
/// and a partial one. The float images carry NaN, the infinities, out-of-range values and a
/// negative zero, in the alpha lanes too, which a kernel keeps as they are.
#[test]
fn every_tier_matches_the_scalar_reference() {
    for tier in IsaTier::supported() {
        for format in ColorFormat::ALL {
            for width in [3, 17] {
                let mut input = create_test_image(format, Size2us::new(width, 5), 0);
                if format.sample_type == SampleType::F32 {
                    let specials = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5, 1.5, -0.0];
                    let samples: &mut [f32] = bytemuck::cast_slice_mut(input.bytes_mut());
                    for (sample, special) in
                        samples.iter_mut().step_by(3).zip(specials.iter().cycle())
                    {
                        *sample = *special;
                    }
                }
                for (contrast, brightness) in PARAM_SWEEP {
                    let op = ContrastBrightness::new(contrast, brightness);

                    let mut actual = input.clone();
                    cpu::apply_on(tier, op, &mut actual);
                    let expected = scalar(op, &input);

                    assert!(
                        pixels_equal(&expected, &actual),
                        "{tier} diverges from the scalar reference for {format} width {width} \
                         (contrast={contrast}, brightness={brightness}): diff={}",
                        max_pixel_diff(&expected, &actual)
                    );
                    assert!(
                        !pixels_equal(&input, &actual),
                        "params ({contrast}, {brightness}) left {format} unchanged"
                    );
                }
            }
        }
    }
}

/// The scalar reference over every row of `input`, past the kernels.
fn scalar(op: ContrastBrightness, input: &InterleavedImage) -> InterleavedImage {
    let mut output = input.clone();
    let format = input.desc().color_format();
    let affine = ChannelAffine::new(op, format);
    for row in output.bytes_mut().chunks_mut(input.desc().row_byte_len()) {
        cpu::internals::scalar_row(affine, format, row);
    }
    output
}
