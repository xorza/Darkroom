use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::astro_image::cfa::{CfaImage, CfaType};

// Bounds amplification at dead/near-zero photosites while keeping every pixel calibrated.
const MIN_NORMALIZED_FLAT: f32 = 0.1;

pub(crate) fn prepare(mut flat: CfaImage, subtractor: Option<&CfaImage>) -> CfaImage {
    if let Some(subtractor) = subtractor {
        assert!(
            subtractor.data.width() == flat.data.width()
                && subtractor.data.height() == flat.data.height(),
            "Flat subtractor dimensions mismatch: {}x{} vs {}x{}",
            subtractor.data.width(),
            subtractor.data.height(),
            flat.data.width(),
            flat.data.height()
        );
    }

    match flat.metadata.cfa_type.as_ref() {
        Some(cfa_type) if cfa_type.num_colors() == 3 => {
            prepare_cfa(
                &mut flat.data,
                subtractor.map(|image| &image.data),
                cfa_type,
            );
        }
        _ => prepare_mono(&mut flat.data, subtractor.map(|image| &image.data)),
    }

    flat
}

pub(crate) fn apply(flat: &CfaImage, image: &mut CfaImage) {
    assert!(
        image.data.width() == flat.data.width() && image.data.height() == flat.data.height(),
        "Flat dimensions mismatch: {}x{} vs {}x{}",
        image.data.width(),
        image.data.height(),
        flat.data.width(),
        flat.data.height()
    );

    image
        .data
        .par_iter_mut()
        .zip(flat.data.par_iter())
        .for_each(|(pixel, divisor)| *pixel /= divisor);
}

fn prepare_mono(flat: &mut Buffer2<f32>, subtractor: Option<&Buffer2<f32>>) {
    let sum: f64 = match subtractor {
        Some(subtractor) => flat
            .par_iter_mut()
            .zip(subtractor.par_iter())
            .map(|(flat, subtractor)| {
                *flat -= subtractor;
                *flat as f64
            })
            .sum(),
        None => flat.par_iter().map(|&value| value as f64).sum(),
    };
    let mean = (sum / flat.len() as f64) as f32;
    assert!(
        mean > f32::EPSILON,
        "Flat frame mean is zero or negative after subtraction"
    );
    let inv_mean = 1.0 / mean;

    flat.par_iter_mut()
        .for_each(|value| *value = (*value * inv_mean).max(MIN_NORMALIZED_FLAT));
}

fn prepare_cfa(flat: &mut Buffer2<f32>, subtractor: Option<&Buffer2<f32>>, cfa_type: &CfaType) {
    let width = flat.width();
    let (sums, counts) = flat
        .par_chunks_mut(width)
        .enumerate()
        .map(|(y, row)| {
            let subtractor_row = subtractor.map(|image| image.row(y));
            let mut sums = [0.0f64; 3];
            let mut counts = [0u64; 3];
            for (x, value) in row.iter_mut().enumerate() {
                if let Some(subtractor_row) = subtractor_row {
                    *value -= subtractor_row[x];
                }
                let color = cfa_type.color_at(x, y) as usize;
                sums[color] += *value as f64;
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

    let mut inv_means = [0.0f32; 3];
    for color in 0..3 {
        assert!(
            counts[color] > 0,
            "Flat has no pixels for color channel {color}"
        );
        let mean = (sums[color] / counts[color] as f64) as f32;
        assert!(
            mean > f32::EPSILON,
            "Flat channel {color} mean is zero or negative"
        );
        inv_means[color] = 1.0 / mean;
    }

    flat.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, value) in row.iter_mut().enumerate() {
            let color = cfa_type.color_at(x, y) as usize;
            *value = (*value * inv_means[color]).max(MIN_NORMALIZED_FLAT);
        }
    });
}

#[cfg(test)]
mod tests;
