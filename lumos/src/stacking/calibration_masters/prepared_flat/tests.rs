use rayon::prelude::*;

use crate::CfaType;
use crate::io::astro_image::cfa::CfaImage;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::stacking::calibration_masters::prepared_flat::{
    MIN_NORMALIZED_FLAT, apply, normalize, subtract,
};
use crate::testing::make_cfa;

fn prepare(flat: CfaImage, subtractor: Option<&CfaImage>) -> CfaImage {
    normalize(subtract(flat, subtractor))
}

fn standard_xtrans() -> CfaType {
    CfaType::XTrans([
        [1, 0, 1, 1, 2, 1],
        [2, 1, 2, 0, 1, 0],
        [1, 2, 1, 1, 0, 1],
        [1, 2, 1, 1, 0, 1],
        [0, 1, 0, 2, 1, 2],
        [1, 0, 1, 1, 2, 1],
    ])
}

fn reference_apply(light: &mut CfaImage, flat: &CfaImage, subtractor: Option<&CfaImage>) {
    let Some(cfa_type) = flat.metadata.cfa_type.as_ref() else {
        reference_apply_mono(light, flat, subtractor);
        return;
    };
    if cfa_type.num_colors() == 1 {
        reference_apply_mono(light, flat, subtractor);
        return;
    }

    let width = flat.data.width();
    let (sums, counts) = (0..flat.data.height())
        .into_par_iter()
        .map(|y| {
            let flat_row = flat.data.row(y);
            let subtractor_row = subtractor.map(|image| image.data.row(y));
            let mut sums = [0.0f64; 3];
            let mut counts = [0u64; 3];
            for x in 0..width {
                let color = cfa_type.color_at(x, y) as usize;
                let value = subtractor_row.map_or(flat_row[x], |row| flat_row[x] - row[x]);
                sums[color] += value as f64;
                counts[color] += 1;
            }
            (sums, counts)
        })
        .reduce(
            || ([0.0f64; 3], [0u64; 3]),
            |(mut sums_a, mut counts_a), (sums_b, counts_b)| {
                for color in 0..3 {
                    sums_a[color] += sums_b[color];
                    counts_a[color] += counts_b[color];
                }
                (sums_a, counts_a)
            },
        );
    let inv_means: [f32; 3] =
        std::array::from_fn(|color| 1.0 / (sums[color] / counts[color] as f64) as f32);

    light
        .data
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            let flat_row = flat.data.row(y);
            let subtractor_row = subtractor.map(|image| image.data.row(y));
            for (x, light) in row.iter_mut().enumerate() {
                let color = cfa_type.color_at(x, y) as usize;
                let value = subtractor_row.map_or(flat_row[x], |row| flat_row[x] - row[x]);
                *light /= (value * inv_means[color]).max(MIN_NORMALIZED_FLAT);
            }
        });
}

fn reference_apply_mono(light: &mut CfaImage, flat: &CfaImage, subtractor: Option<&CfaImage>) {
    let sum = match subtractor {
        Some(subtractor) => flat
            .data
            .par_iter()
            .zip(subtractor.data.par_iter())
            .map(|(flat, subtractor)| (flat - subtractor) as f64)
            .sum::<f64>(),
        None => flat.data.par_iter().map(|&value| value as f64).sum(),
    };
    let inv_mean = 1.0 / (sum / flat.data.len() as f64) as f32;
    light
        .data
        .par_iter_mut()
        .zip(flat.data.par_iter())
        .enumerate()
        .for_each(|(index, (light, flat))| {
            let value = subtractor.map_or(*flat, |image| *flat - image.data[index]);
            *light /= (value * inv_mean).max(MIN_NORMALIZED_FLAT);
        });
}

#[test]
fn prepared_flat_matches_hand_computed_mono_calibration() {
    let flat = make_cfa(2, 2, vec![0.0, 1.0, 1.0, 2.0], CfaType::Mono);
    let prepared = prepare(flat, None);
    assert_eq!(
        prepared.data.pixels(),
        &[MIN_NORMALIZED_FLAT, 1.0, 1.0, 2.0]
    );

    let mut light = make_cfa(2, 2, vec![1.0; 4], CfaType::Mono);
    apply(&prepared, &mut light);
    assert_eq!(light.data.pixels(), &[10.0, 1.0, 1.0, 0.5]);
}

#[test]
fn prepared_flat_is_bit_exact_for_bayer_and_xtrans_with_subtraction() {
    for cfa_type in [CfaType::Bayer(CfaPattern::Rggb), standard_xtrans()] {
        let (width, height) = match cfa_type {
            CfaType::Bayer(_) => (4, 4),
            CfaType::XTrans(_) => (6, 6),
            CfaType::Mono => unreachable!(),
        };
        let means = [0.5f32, 1.0, 0.25];
        let mut counts = [0usize; 3];
        let mut flat_pixels = Vec::with_capacity(width * height);
        let mut expected_divisors = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let color = cfa_type.color_at(x, y) as usize;
                let divisor = if counts[color].is_multiple_of(2) {
                    0.5
                } else {
                    1.5
                };
                counts[color] += 1;
                expected_divisors.push(divisor);
                flat_pixels.push(0.125 + means[color] * divisor);
            }
        }

        let flat = make_cfa(width, height, flat_pixels, cfa_type.clone());
        let subtractor = make_cfa(width, height, vec![0.125; width * height], cfa_type.clone());
        let prepared = prepare(flat, Some(&subtractor));
        assert_eq!(
            prepared
                .data
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            expected_divisors
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );

        let mut light = make_cfa(width, height, vec![0.75; width * height], cfa_type);
        apply(&prepared, &mut light);
        let expected = expected_divisors
            .iter()
            .map(|divisor| (0.75f32 / divisor).to_bits())
            .collect::<Vec<_>>();
        assert_eq!(
            light
                .data
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn prepared_flat_matches_previous_per_light_equation_bit_exactly() {
    for (cfa_type, width, height) in [
        (CfaType::Mono, 13usize, 7usize),
        (CfaType::Bayer(CfaPattern::Rggb), 14, 8),
        (standard_xtrans(), 12, 12),
    ] {
        let pixels = (0..width * height)
            .map(|index| 0.4 + (index.wrapping_mul(37) % 101) as f32 * 0.003)
            .collect::<Vec<_>>();
        let subtractor_pixels = (0..width * height)
            .map(|index| 0.02 + (index.wrapping_mul(11) % 17) as f32 * 0.0005)
            .collect::<Vec<_>>();
        let flat = make_cfa(width, height, pixels, cfa_type.clone());
        let subtractor = make_cfa(width, height, subtractor_pixels, cfa_type.clone());
        let light = make_cfa(
            width,
            height,
            (0..width * height)
                .map(|index| 0.1 + (index.wrapping_mul(19) % 89) as f32 * 0.004)
                .collect(),
            cfa_type,
        );

        let mut expected = light.clone();
        reference_apply(&mut expected, &flat, Some(&subtractor));
        let prepared = prepare(flat, Some(&subtractor));
        let mut actual = light;
        apply(&prepared, &mut actual);
        assert_eq!(
            actual
                .data
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            expected
                .data
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
    }
}

#[test]
#[should_panic(expected = "Flat subtractor dimensions mismatch")]
fn preparation_rejects_mismatched_subtractor_dimensions() {
    let flat = make_cfa(2, 2, vec![1.0; 4], CfaType::Mono);
    let subtractor = make_cfa(3, 2, vec![0.1; 6], CfaType::Mono);
    prepare(flat, Some(&subtractor));
}
