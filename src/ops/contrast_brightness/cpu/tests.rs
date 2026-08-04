#[cfg(target_arch = "aarch64")]
use super::neon_kernel;
use super::{ChannelAffine, RowKernel, apply_kernel, apply_typed};
#[cfg(target_arch = "x86_64")]
use super::{avx2_kernel, sse41_kernel};
use crate::common::color_format::{
    ALL_FORMATS, ALPHA_FORMATS, ChannelSize, ChannelType, ColorFormat,
};
use crate::common::image_diff::{max_pixel_diff, pixels_equal};
use crate::common::internals::{create_test_image, load_lena_rgba_u8_61x38};
use crate::image::{Image, ImageDesc};
use crate::ops::contrast_brightness::ContrastBrightness;

fn pixels_changed(img1: &Image, img2: &Image) -> bool {
    !pixels_equal(img1, img2)
}

/// Builds a single-row image from raw channel bytes.
fn image_from_channels(format: ColorFormat, width: usize, bytes: Vec<u8>) -> Image {
    let desc = ImageDesc::new(width, 1, format);
    assert_eq!(
        bytes.len(),
        desc.row_bytes(),
        "row size mismatch for {format}"
    );
    Image::new_with_data(desc, bytes).unwrap()
}

/// Long enough that every kernel runs at least one full vector body and then a
/// tail: the widest flat body is 16 `u8`, so 21 leaves 5 over; the widest
/// alpha-preserving body is 16 RGBA pixels on NEON, so 21 pixels leave 5 over.
const SWEEP_LEN: usize = 21;

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

    let mut image = image_from_channels(ColorFormat::L_U8, SWEEP_LEN, bytes);
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
        ColorFormat::L_U16,
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
        ColorFormat::L_F32,
        SWEEP_LEN,
        bytemuck::cast_slice(&values).to_vec(),
    );
    CONTRAST_TWO.apply_cpu(&mut image);

    assert_eq!(image.bytes(), bytemuck::cast_slice::<f32, u8>(&want));
}

#[test]
fn contrast_two_transforms_rgb_and_leaves_alpha_alone() {
    // Same mapping as the flat u8 case, but laid out as RGBA pixels: the three
    // colour channels move, the fourth is copied through even where it would
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
    for format in ALL_FORMATS {
        let input = create_test_image(*format, 17, 5, 0);
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
    for format in ALPHA_FORMATS {
        let input = create_test_image(*format, 16, 4, 0);
        let mut output = input.clone();

        ContrastBrightness::new(2.0, 0.3).apply_cpu(&mut output);

        let channels = format.channel_count.channel_count() as usize;
        let channel_size = format.channel_size.byte_count() as usize;
        let alpha_offset = (channels - 1) * channel_size;
        let pixel_size = channels * channel_size;

        for row in 0..4 {
            let row_start = row * input.desc().row_bytes();
            for x in 0..16 {
                let pixel_start = row_start + x * pixel_size;
                let alpha_start = pixel_start + alpha_offset;
                let in_alpha = &input.bytes()[alpha_start..alpha_start + channel_size];
                let out_alpha = &output.bytes()[alpha_start..alpha_start + channel_size];
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
        for format in ALL_FORMATS {
            let input = create_test_image(*format, width, height, 0);
            let mut output = input.clone();

            params.apply_cpu(&mut output);

            assert!(
                pixels_changed(&input, &output),
                "{label} should change output for format {format}"
            );
        }
    }
}

#[test]
fn extreme_brightness_clamps_to_the_range_ends() {
    for format in ALL_FORMATS {
        let input = create_test_image(*format, 4, 2, 0);
        let affine_max = ChannelAffine::new(&ContrastBrightness::default(), *format).max;

        // Saturating up pins every colour channel at the format's maximum, and
        // saturating down pins it at zero. Alpha is exempt.
        for (brightness, want) in [(1.0, affine_max), (-1.0, 0.0)] {
            let mut output = input.clone();
            ContrastBrightness::new(1.0, brightness).apply_cpu(&mut output);

            let channels = format.channel_count.channel_count() as usize;
            let colour_channels = if format.channel_count.channel_count() == 4 {
                channels - 1
            } else {
                channels
            };
            for (index, value) in channel_values(&output).iter().enumerate() {
                if index % channels >= colour_channels {
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

/// Every colour channel of `image` as an `f32`, in its own units.
fn channel_values(image: &Image) -> Vec<f32> {
    let format = image.desc().color_format;
    match (format.channel_size, format.channel_type) {
        (ChannelSize::_8bit, ChannelType::UInt) => {
            image.bytes().iter().map(|v| f32::from(*v)).collect()
        }
        (ChannelSize::_16bit, ChannelType::UInt) => bytemuck::cast_slice::<u8, u16>(image.bytes())
            .iter()
            .map(|v| f32::from(*v))
            .collect(),
        (ChannelSize::_32bit, ChannelType::Float) => {
            bytemuck::cast_slice::<u8, f32>(image.bytes()).to_vec()
        }
        _ => unreachable!("unsupported format in ALL_FORMATS"),
    }
}

/// Runs the scalar reference over `image`, dispatched on its storage type.
fn apply_reference(image: &mut Image, op: ContrastBrightness) {
    let format = image.desc().color_format;
    match (format.channel_size, format.channel_type) {
        (ChannelSize::_8bit, ChannelType::UInt) => apply_typed::<u8>(image, op),
        (ChannelSize::_16bit, ChannelType::UInt) => apply_typed::<u16>(image, op),
        (ChannelSize::_32bit, ChannelType::Float) => apply_typed::<f32>(image, op),
        _ => unreachable!("unsupported format in ALL_FORMATS"),
    }
}

/// Contrast-only, brightness-only, combined, and clamp-heavy.
const PARAM_SWEEP: [(f32, f32); 4] = [(2.0, 0.0), (1.0, 0.2), (1.5, 0.1), (0.5, -0.8)];

#[test]
fn simd_matches_the_scalar_reference_bit_for_bit() {
    // Every ISA tier this CPU can run, not just the one dispatch would pick —
    // otherwise the SSE4.1 kernels go untested on any AVX2 machine. 17x5
    // exercises each kernel's tail (a width that is no vector's multiple).
    // Both paths evaluate the same fused affine in the same order, so floats
    // are held to equality here too, not an epsilon.
    for (tier, select) in kernel_tiers() {
        for format in ALL_FORMATS {
            let kernel =
                select(*format).unwrap_or_else(|| panic!("{tier} has no kernel for {format}"));

            for (contrast, brightness) in PARAM_SWEEP {
                let input = create_test_image(*format, 17, 5, 0);
                let op = ContrastBrightness::new(contrast, brightness);

                let mut actual = input.clone();
                // SAFETY: `kernel_tiers` only yields tiers this CPU supports.
                unsafe { apply_kernel(kernel, &op, &mut actual) };

                let mut expected = input.clone();
                apply_reference(&mut expected, op);

                let diff = max_pixel_diff(&expected, &actual);
                assert!(
                    pixels_equal(&expected, &actual),
                    "{tier} diverges from the scalar reference for {format} \
                     (contrast={contrast}, brightness={brightness}): diff={diff}"
                );
                // Sanity: the sweep actually transformed the pixels.
                assert!(
                    pixels_changed(&input, &actual),
                    "params ({contrast}, {brightness}) left {format} unchanged"
                );
            }
        }
    }
}

/// A SIMD tier: its name, and the selector mapping a format to its kernel.
type KernelTier = (&'static str, fn(ColorFormat) -> Option<RowKernel>);

/// The SIMD tiers this CPU can actually execute.
#[cfg(target_arch = "x86_64")]
fn kernel_tiers() -> Vec<KernelTier> {
    let mut tiers: Vec<KernelTier> = Vec::new();
    if crate::cpu_features::has_sse4_1() {
        tiers.push(("sse4.1", sse41_kernel));
    }
    if crate::cpu_features::has_avx2() {
        tiers.push(("avx2", avx2_kernel));
    }
    assert!(!tiers.is_empty(), "x86_64 without SSE4.1 is not supported");
    tiers
}

#[cfg(target_arch = "aarch64")]
fn kernel_tiers() -> Vec<KernelTier> {
    vec![("neon", neon_kernel)]
}

#[test]
fn test_large_image() {
    let input = load_lena_rgba_u8_61x38();
    let mut output = input.clone();

    ContrastBrightness::new(1.2, 0.05).apply_cpu(&mut output);

    assert!(
        pixels_changed(&input, &output),
        "large image test should change output"
    );
}
