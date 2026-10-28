use simd::IsaTier;

use crate::color::color_format::{ColorFormat, SampleType};
use crate::interleaved::conversion::scalar::{self, Luma};
use crate::interleaved::conversion::vector_conversion::VectorConversion;
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;
use crate::internals::create_test_image;
use common::USize2;

/// `(r·13933 + g·46871 + b·4732) / 65536`, rounded half to even:
/// red 255 → 3 552 915 / 65536 = 54.21 → 54; green 255 → 11 952 105 / 65536 = 182.38 → 182;
/// blue 255 → 1 206 660 / 65536 = 18.41 → 18; neutral 255 → 255 exactly. In `u16` the weights are
/// the same, so red 65535 → 913 099 155 / 65536 = 13 932.79 → 13 933. Float carries the weights
/// unrounded.
#[test]
fn luma_is_the_rounded_rec709_sum() {
    for (rgb, expected) in [
        ([255, 0, 0], 54),
        ([0, 255, 0], 182),
        ([0, 0, 255], 18),
        ([255, 255, 255], 255),
        ([0, 0, 0], 0),
    ] {
        assert_eq!(u8::luma(rgb[0], rgb[1], rgb[2]), expected, "{rgb:?}");
    }
    assert_eq!(u16::luma(65535, 0, 0), 13933);
    assert_eq!(u16::luma(65535, 65535, 65535), 65535);
    assert_eq!(f32::luma(1.0, 0.0, 0.0), 0.2126);
}

/// Exhaustive over `u8` neutrals, `R = G = B`: each is its own luma, which needs the weights to
/// sum to 65536 and the rounding to be exact.
#[test]
fn a_neutral_keeps_its_value() {
    for v in 0..=u8::MAX {
        assert_eq!(u8::luma(v, v, v), v);
    }
}

/// One pixel per channel-count pair, in `u8`: mono broadcasts and gains an opaque alpha, color
/// keeps its channels or reduces to luma, alpha is kept only into a format with alpha.
#[test]
fn channel_counts_expand_and_reduce() {
    let l = [100u8];
    let rgb = [255u8, 0, 0];
    let rgba = [255u8, 0, 0, 7];
    for (from, src, to, expected) in [
        (
            ColorFormat::MONO_U8,
            &l[..],
            ColorFormat::RGB_U8,
            &[100, 100, 100][..],
        ),
        (
            ColorFormat::MONO_U8,
            &l,
            ColorFormat::RGBA_U8,
            &[100, 100, 100, 255],
        ),
        (ColorFormat::RGB_U8, &rgb, ColorFormat::MONO_U8, &[54]),
        (
            ColorFormat::RGB_U8,
            &rgb,
            ColorFormat::RGBA_U8,
            &[255, 0, 0, 255],
        ),
        (ColorFormat::RGBA_U8, &rgba, ColorFormat::MONO_U8, &[54]),
        (
            ColorFormat::RGBA_U8,
            &rgba,
            ColorFormat::RGB_U8,
            &[255, 0, 0],
        ),
    ] {
        let mut dst = vec![0; to.byte_len()];
        scalar::row_converter(from, to)(src, &mut dst, 1);
        assert_eq!(dst, expected, "{from} -> {to}");
    }
}

/// Literals, not the code under test: `u8 → u16` is `v · 257`, so 64 → 16448, 128 → 32896,
/// 200 → 51400; `u16 → u8` rounds `v / 257`, so 32896 → 128 and 32767 → 127.5 − ε → 127;
/// `f32 → u8` rounds `x · 255`, so 0.5 → 127.5 → 128 (ties to even) and 0.2 → 51; the alpha
/// a format gains is opaque.
#[test]
fn whole_images_convert_by_the_rule() {
    let cases: [(ColorFormat, Vec<u8>, ColorFormat, Vec<u8>); 4] = [
        (
            ColorFormat::MONO_U8,
            vec![0, 64, 128, 200, 255],
            ColorFormat::MONO_U16,
            bytemuck::cast_slice(&[0u16, 16448, 32896, 51400, 65535]).to_vec(),
        ),
        (
            ColorFormat::MONO_U16,
            bytemuck::cast_slice(&[0u16, 32896, 32767, 65535, 128]).to_vec(),
            ColorFormat::MONO_U8,
            vec![0, 128, 127, 255, 0],
        ),
        (
            ColorFormat::MONO_F32,
            bytemuck::cast_slice(&[0.5f32, 0.2, 1.0, -1.0, 2.0]).to_vec(),
            ColorFormat::MONO_U8,
            vec![128, 51, 255, 0, 255],
        ),
        (
            ColorFormat::MONO_U8,
            vec![0, 51, 255, 1, 2],
            ColorFormat::RGBA_F32,
            bytemuck::cast_slice(&[0, 51, 255, 1, 2].map(|v: u8| {
                [
                    f32::from(v) / 255.0,
                    f32::from(v) / 255.0,
                    f32::from(v) / 255.0,
                    1.0,
                ]
            }))
            .to_vec(),
        ),
    ];
    for (from, bytes, to, expected) in cases {
        let image =
            InterleavedImage::new_with_data(InterleavedDesc::new(USize2::new(5, 1), from), bytes)
                .unwrap();
        let converted = image.convert_to(to);
        assert_eq!(
            converted.desc(),
            InterleavedDesc::new(USize2::new(5, 1), to)
        );
        assert_eq!(converted.bytes(), expected, "{from} -> {to}");
    }
}

/// A conversion to the image's own format is a full copy, not a black image.
#[test]
fn converting_to_the_same_format_copies() {
    for format in ColorFormat::ALL {
        let bytes: Vec<u8> = (0..3 * 2 * format.byte_len())
            .map(|i| (i * 37 % 251) as u8)
            .collect();
        let image = InterleavedImage::new_with_data(
            InterleavedDesc::new(USize2::new(3, 2), format),
            bytes.clone(),
        )
        .unwrap();
        assert_eq!(image.convert_to(format).bytes(), bytes, "{format}");
        assert_eq!(image.convert(format).bytes(), bytes, "{format}");
    }
}

/// Characterization snapshot: per source format, one digest of its conversion to every other
/// format, so a change to any conversion names the source it moved. It pins what the code does,
/// not what is right; a change that moves it states why. The source is the shared test pattern
/// (every byte value, or floats on `[0, 1)` including exact halves of a step) plus, for float
/// sources, values outside `[0, 1]` and both infinities, which the narrowing clamps. No NaN: its
/// payload after a float copy is unspecified. Every tier is bit-exact with the scalar reference,
/// so the digests hold on any host.
#[test]
fn conversion_snapshot() {
    /// FNV-1a, 64-bit: a stable digest that needs no dependency.
    fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
        for &byte in bytes {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
        hash
    }

    let digests = ColorFormat::ALL.map(|from| {
        let mut source = create_test_image(from, USize2::new(37, 5), 3);
        if from.sample_type == SampleType::F32 {
            let edges = [-1.0f32, -0.0, 1.5, 255.0, f32::INFINITY, f32::NEG_INFINITY];
            let floats: &mut [f32] = bytemuck::cast_slice_mut(source.bytes_mut());
            for (sample, edge) in floats.iter_mut().step_by(7).zip(edges.iter().cycle()) {
                *sample = *edge;
            }
        }
        ColorFormat::ALL
            .into_iter()
            .filter(|&to| to != from)
            .fold(0xcbf2_9ce4_8422_2325, |hash, to| {
                fnv1a(hash, source.convert_to(to).bytes())
            })
    });
    assert_eq!(
        digests,
        [
            0x9ea7_29de_559c_c2c0,
            0xd7e2_d9e6_0205_c2d6,
            0x31b8_5564_15cf_3954,
            0x9663_c6e6_3a09_8633,
            0xdf72_b5cb_a0c9_3f6e,
            0xfd40_a1fc_4c65_efab,
            0x3d90_45ee_858f_9220,
            0x57f6_0eb1_563a_d39c,
            0xd127_9c0a_f0fa_1ab8,
        ],
        "a conversion snapshot moved: state why, or find the regression"
    );
}

/// Every `u8` and every `u16`; for `f32`, the values on both sides of every rounding tie of both
/// integer scales — `(k + ½) / 255` and `(k + ½) / 65535`, two ulps either way — plus the
/// values that saturate, NaN and the infinities.
fn samples(sample_type: SampleType) -> Vec<u8> {
    match sample_type {
        SampleType::U8 => (0..=u8::MAX).collect(),
        SampleType::U16 => bytemuck::cast_slice(&(0..=u16::MAX).collect::<Vec<_>>()).to_vec(),
        SampleType::F32 => {
            let mut values = vec![
                0.0,
                -0.0,
                0.5,
                1.0,
                f32::MIN_POSITIVE,
                1e-40,
                -1e-30,
                -0.5,
                1.000_000_1,
                2.0,
                1e30,
                -1e30,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NAN,
            ];
            for full_scale in [255.0f64, 65535.0] {
                for k in 0..65535u32 {
                    if f64::from(k) >= full_scale {
                        break;
                    }
                    let tie = ((f64::from(k) + 0.5) / full_scale) as f32;
                    let below = tie.next_down();
                    let above = tie.next_up();
                    values.extend([below.next_down(), below, tie, above, above.next_up()]);
                }
            }
            bytemuck::cast_slice(&values).to_vec()
        }
    }
}

/// Widths around every vector width the kernels use (4, 8, 16 and 32 elements, 16 and 32
/// pixels), so every tail length is reached.
const WIDTHS: [usize; 17] = [1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65];

/// Every vector kernel on every tier this CPU has, against the scalar reference, bit for bit:
/// once over every source value in one long row, and once per width so every tail length runs.
#[test]
fn every_tier_matches_the_scalar_reference() {
    for tier in IsaTier::supported() {
        for (from, to) in pairs() {
            if let Some(kernel) = VectorConversion::of(from, to) {
                assert_matches_scalar(&tier.to_string(), from, to, |src, dst, width| {
                    kernel.convert_row(tier, src, dst, width);
                });
            }
        }
    }
}

/// The pairs with a vector kernel, on every arch: the six changes of sample type at each of the
/// three channel counts, and the six changes of channel count in `u8`.
#[test]
fn every_vectorized_pair_has_one_kernel() {
    let pairs = pairs().filter_map(|(from, to)| VectorConversion::of(from, to));
    let (elements, channels): (Vec<_>, Vec<_>) =
        pairs.partition(|kernel| matches!(kernel, VectorConversion::Elements(_)));
    assert_eq!([elements.len(), channels.len()], [18, 6]);
}

/// Every ordered pair of distinct formats.
fn pairs() -> impl Iterator<Item = (ColorFormat, ColorFormat)> {
    ColorFormat::ALL
        .into_iter()
        .flat_map(|from| ColorFormat::ALL.map(|to| (from, to)))
        .filter(|&(from, to)| from != to)
}

/// `convert`, a row kernel of `from` into `to`, against the scalar reference over [`samples`].
fn assert_matches_scalar(
    tier: &str,
    from: ColorFormat,
    to: ColorFormat,
    convert: impl Fn(&[u8], &mut [u8], usize),
) {
    // Cycled to fit the widest row; the whole of it is one more row.
    let mut source = samples(from.sample_type);
    let widest = WIDTHS[WIDTHS.len() - 1] * from.byte_len();
    while source.len() < widest {
        source.extend_from_within(..);
    }
    source.truncate(source.len() - source.len() % from.byte_len());
    let pixels = source.len() / from.byte_len();
    let widths = WIDTHS.into_iter().chain([pixels]);
    for (offset, width) in widths.enumerate() {
        let start = (offset * 7 % (pixels - width + 1)) * from.byte_len();
        let row = &source[start..start + width * from.byte_len()];
        // A source row runs on to the end of the image; the kernel must stop at its own.
        let padded = [row, &[0xA5; 64]].concat();
        let mut expected = vec![0; width * to.byte_len()];
        let mut actual = vec![0; width * to.byte_len()];
        scalar::row_converter(from, to)(&padded, &mut expected, width);
        convert(&padded, &mut actual, width);
        assert!(
            expected == actual,
            "{tier} {from} -> {to}, width {width}: first difference at byte {}",
            first_difference(&expected, &actual)
        );
    }
}

fn first_difference(expected: &[u8], actual: &[u8]) -> usize {
    expected
        .iter()
        .zip(actual)
        .position(|(expected, actual)| expected != actual)
        .unwrap_or(expected.len())
}
