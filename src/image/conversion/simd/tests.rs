use crate::common::color_format::{ALL_FORMATS, SampleType};
use crate::image::conversion::scalar;
use crate::image::conversion::simd::{Tier, tier_converter};

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

/// Every kernel of every tier this CPU has, against the scalar reference, bit for bit: once over
/// every source value in one long row, and once per width so every tail length runs.
#[test]
fn every_tier_matches_the_scalar_reference() {
    let mut tiers_run = 0;
    for tier in Tier::ALL {
        if !tier.is_supported() {
            eprintln!("conversion tier {tier:?} is not supported on this CPU: not tested");
            continue;
        }
        tiers_run += 1;
        for from in ALL_FORMATS {
            for to in ALL_FORMATS {
                if from == to {
                    continue;
                }
                let Some(kernel) = tier_converter(tier, from, to) else {
                    continue;
                };
                // Cycled to fit the widest row; the whole of it is one more row.
                let mut source = samples(from.sample_type);
                let widest = WIDTHS[WIDTHS.len() - 1] * from.byte_count();
                while source.len() < widest {
                    source.extend_from_within(..);
                }
                source.truncate(source.len() - source.len() % from.byte_count());
                let pixels = source.len() / from.byte_count();
                let widths = WIDTHS.into_iter().chain([pixels]);
                for (offset, width) in widths.enumerate() {
                    let start = (offset * 7 % (pixels - width + 1)) * from.byte_count();
                    let row = &source[start..start + width * from.byte_count()];
                    // A source row runs on to the end of the image; the kernel must stop at its own.
                    let padded = [row, &[0xA5; 64]].concat();
                    let mut expected = vec![0; width * to.byte_count()];
                    let mut actual = vec![0; width * to.byte_count()];
                    scalar::row_converter(from, to)(&padded, &mut expected, width);
                    // SAFETY: `tier` is supported on this CPU, checked above.
                    unsafe { kernel(&padded, &mut actual, width) };
                    assert!(
                        expected == actual,
                        "{tier:?} {from} -> {to}, width {width}: first difference at byte {}",
                        first_difference(&expected, &actual)
                    );
                }
            }
        }
    }
    assert!(tiers_run > 0, "no conversion tier ran");
}

fn first_difference(expected: &[u8], actual: &[u8]) -> usize {
    expected
        .iter()
        .zip(actual)
        .position(|(expected, actual)| expected != actual)
        .unwrap_or(expected.len())
}

/// Dispatch picks a kernel for the pairs each tier vectorizes, and none for the rest: the sweep
/// above covers exactly these. Counted per arch from the tables.
#[test]
fn every_vectorized_pair_has_a_kernel_at_its_tiers() {
    let count = |tier: Tier| {
        ALL_FORMATS
            .into_iter()
            .flat_map(|from| ALL_FORMATS.map(|to| (from, to)))
            .filter(|&(from, to)| from != to && tier_converter(tier, from, to).is_some())
            .count()
    };
    // Five element pairs × three channel counts, plus f32 → u16 from SSE4.1 on; the six u8
    // channel shuffles from SSSE3 on.
    #[cfg(target_arch = "x86_64")]
    assert_eq!(
        Tier::ALL.map(count),
        [15, 15 + 6, 18 + 6, 18 + 6],
        "SSE2, SSSE3, SSE4.1, AVX2"
    );
    #[cfg(target_arch = "aarch64")]
    assert_eq!(Tier::ALL.map(count), [18 + 6]);
}
