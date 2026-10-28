use crate::color::color_format::{ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::create_test_image;
use crate::internals::image_diff::{max_pixel_diff, pixels_equal};
use crate::ops::clamp::Clamp;
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;
use crate::ops::contrast_brightness::{ContrastBrightness, cpu};
use common::Size2us;
use simd::IsaTier;
use strum::IntoEnumIterator;

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
const CONTRAST_TWO: ContrastBrightness = ContrastBrightness::new(2.0, 0.0);

/// `samples` in `format`'s sample type, one channel's values in its own units, as bytes.
fn sample_bytes(format: ColorFormat, samples: &[f32]) -> Vec<u8> {
    match format.sample_type {
        SampleType::U8 => samples.iter().map(|&value| u8::from_f32(value)).collect(),
        SampleType::U16 => samples
            .iter()
            .flat_map(|&value| u16::from_f32(value).to_ne_bytes())
            .collect(),
        SampleType::F32 => bytemuck::cast_slice(samples).to_vec(),
    }
}

/// Each row adjusts a mono row of `SWEEP_LEN` samples cycling through its inputs, in its format's
/// own units, every step of the arithmetic exact or a tie:
///
/// - Contrast 2 folds to `2v − mid`, `mid` 127.5, 32767.5 or 0.5. `u8`: 0 → −127.5 → 0,
///   64 → 0.5 → 0 and 65 → 2.5 → 2 (ties to even), 128 → 128.5 → 128, 255 → 382.5 → 255; `u16`
///   the same at 0, 16384, 16385, 32768 and 65535. An integer store saturates, under any clamp;
///   `f32` keeps −0.5 and 1.5 under `None`, holds −0.5 at 0 under `Black` and 1.5 at 1 too under
///   `BlackAndWhite`.
/// - Brightness alone adds `b · max`: 0.2 · 255 = 51 (100 → 151), 0.2 · 65535 = 13107
///   (1000 → 14107), and 0.2 to an `f32`, rounded once, through −0.2 and past 1.
#[test]
fn contrast_and_brightness_map_hand_computed_values() {
    let brightness = ContrastBrightness::new(1.0, 0.2);
    let rows: [(ContrastBrightness, ColorFormat, &[f32], &[f32]); 9] = [
        (
            CONTRAST_TWO,
            ColorFormat::MONO_U8,
            &[0.0, 64.0, 65.0, 128.0, 255.0],
            &[0.0, 0.0, 2.0, 128.0, 255.0],
        ),
        (
            CONTRAST_TWO,
            ColorFormat::MONO_U16,
            &[0.0, 16384.0, 16385.0, 32768.0, 65535.0],
            &[0.0, 0.0, 2.0, 32768.0, 65535.0],
        ),
        (
            CONTRAST_TWO.clamp(Clamp::BlackAndWhite),
            ColorFormat::MONO_U8,
            &[0.0, 64.0, 65.0, 128.0, 255.0],
            &[0.0, 0.0, 2.0, 128.0, 255.0],
        ),
        (
            CONTRAST_TWO.clamp(Clamp::None),
            ColorFormat::MONO_F32,
            &[0.0, 0.25, 0.5, 0.75, 1.0],
            &[-0.5, 0.0, 0.5, 1.0, 1.5],
        ),
        (
            CONTRAST_TWO,
            ColorFormat::MONO_F32,
            &[0.0, 0.25, 0.5, 0.75, 1.0],
            &[0.0, 0.0, 0.5, 1.0, 1.5],
        ),
        (
            CONTRAST_TWO.clamp(Clamp::BlackAndWhite),
            ColorFormat::MONO_F32,
            &[0.0, 0.25, 0.5, 0.75, 1.0],
            &[0.0, 0.0, 0.5, 1.0, 1.0],
        ),
        (brightness, ColorFormat::MONO_U8, &[100.0], &[151.0]),
        (brightness, ColorFormat::MONO_U16, &[1000.0], &[14107.0]),
        (
            brightness,
            ColorFormat::MONO_F32,
            &[0.25, -0.2, 1.5],
            &[0.25 + 0.2, -0.2 + 0.2, 1.5 + 0.2],
        ),
    ];
    for (params, format, inputs, expected) in rows {
        let cycle = |values: &[f32]| -> Vec<f32> {
            (0..SWEEP_LEN).map(|i| values[i % values.len()]).collect()
        };
        let mut image =
            image_from_channels(format, SWEEP_LEN, sample_bytes(format, &cycle(inputs)));
        params.apply_cpu(&mut image);
        assert_eq!(
            channel_values(&image),
            cycle(expected),
            "{params:?} {format}"
        );
    }
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

/// Brightness ±1 saturates every color channel of an integer format at its maximum or at zero
/// under every clamp, and moves an `f32` channel by exactly ±1 to where its clamp holds it: past
/// 1 and below 0 under `None`, at 0 from below under `Black`, within `[0, 1]` under
/// `BlackAndWhite`. Alpha is exempt.
#[test]
fn extreme_brightness_saturates_integers_and_moves_floats() {
    for (format, clamp) in ColorFormat::ALL
        .into_iter()
        .flat_map(|format| Clamp::iter().map(move |clamp| (format, clamp)))
    {
        let input = create_test_image(format, Size2us::new(4, 2), 0);
        let affine_max = ChannelAffine::new(ContrastBrightness::default(), format).max;
        for brightness in [1.0f32, -1.0] {
            let mut output = input.clone();
            ContrastBrightness::new(1.0, brightness)
                .clamp(clamp)
                .apply_cpu(&mut output);

            let channel_count = format.channel_count.count();
            let color_count = channel_count - usize::from(format.has_alpha());
            let values = channel_values(&input)
                .into_iter()
                .zip(channel_values(&output));
            for (index, (before, after)) in values.enumerate() {
                if index % channel_count >= color_count {
                    continue;
                }
                let want = match format.sample_type {
                    SampleType::F32 => match clamp {
                        Clamp::None => before + brightness,
                        Clamp::Black => (before + brightness).max(0.0),
                        Clamp::BlackAndWhite => (before + brightness).clamp(0.0, 1.0),
                    },
                    SampleType::U8 | SampleType::U16 if brightness > 0.0 => affine_max,
                    SampleType::U8 | SampleType::U16 => 0.0,
                };
                assert_eq!(
                    after, want,
                    "brightness {brightness}, {clamp:?}, channel {index} of {format}"
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
/// negative zero, in the alpha lanes too, which a kernel keeps as they are, under every clamp.
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
                for ((contrast, brightness), clamp) in PARAM_SWEEP
                    .into_iter()
                    .flat_map(|params| Clamp::iter().map(move |clamp| (params, clamp)))
                {
                    let op = ContrastBrightness::new(contrast, brightness).clamp(clamp);

                    let mut actual = input.clone();
                    cpu::apply_on(tier, op, &mut actual);
                    let expected = scalar(op, &input);

                    assert!(
                        pixels_equal(&expected, &actual),
                        "{tier} diverges from the scalar reference for {format} width {width} \
                         ({op:?}): diff={}",
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
