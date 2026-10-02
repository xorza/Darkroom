//! Turning a stacked flat into the divisor calibration applies.
//!
//! Three steps in order, each its own function because the defect detector has to see the middle
//! one: subtract the flat's own bias or flat-dark, normalize per CFA colour to a mean of one, and
//! divide a light by the result. Cold-pixel detection runs on the *subtracted* flat, before
//! normalization clamps near-zero photosites away.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::cfa::{CfaImage, CfaType};
use crate::math::vec2us::Vec2us;
use crate::stacking::calibration_masters::error::CalibrationError;

// Bounds amplification at dead/near-zero photosites while keeping every pixel calibrated.
const MIN_NORMALIZED_FLAT: f32 = 0.1;

/// Subtract the flat's own bias or flat-dark, given with the factor that expresses its samples in
/// the flat's domain.
pub(super) fn subtract(mut flat: CfaImage, subtractor: Option<(&CfaImage, f32)>) -> CfaImage {
    if let Some((subtractor, scale)) = subtractor {
        flat.subtract(subtractor, scale);
    }

    flat
}

/// Normalize the subtracted flat to a mean of one, per CFA colour.
///
/// # Errors
/// [`CalibrationError::NonPositiveFlat`] when a colour (or the whole mono frame) has no positive
/// mean: a property of the user's flats, not of this code.
pub(super) fn normalize(mut flat: CfaImage) -> Result<CfaImage, CalibrationError> {
    match flat.cfa_type {
        CfaType::Mono => normalize_mono(&mut flat.data)?,
        cfa_type @ (CfaType::Bayer(_) | CfaType::XTrans(_)) => {
            normalize_cfa(&mut flat.data, &cfa_type)?;
        }
    }
    Ok(flat)
}

pub(super) fn apply(flat: &CfaImage, image: &mut CfaImage) {
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

fn normalize_mono(flat: &mut Buffer2<f32>) -> Result<(), CalibrationError> {
    let sum: f64 = flat.par_iter().map(|&value| f64::from(value)).sum();
    let mean = (sum / flat.len() as f64) as f32;
    if mean.is_nan() || mean <= f32::EPSILON {
        return Err(CalibrationError::NonPositiveFlat { channel: None });
    }
    let inv_mean = 1.0 / mean;

    flat.par_iter_mut()
        .for_each(|value| *value = (*value * inv_mean).max(MIN_NORMALIZED_FLAT));
    Ok(())
}

fn normalize_cfa(flat: &mut Buffer2<f32>, cfa_type: &CfaType) -> Result<(), CalibrationError> {
    let width = flat.width();
    let (sums, counts) = flat
        .par_chunks_mut(width)
        .enumerate()
        .map(|(y, row)| {
            let mut sums = [0.0f64; 3];
            let mut counts = [0u64; 3];
            for (x, value) in row.iter_mut().enumerate() {
                let color = cfa_type.color_at(Vec2us::new(x, y)) as usize;
                sums[color] += f64::from(*value);
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
        // A channel with no pixels divides 0 by 0.
        let mean = (sums[color] / counts[color] as f64) as f32;
        if mean.is_nan() || mean <= f32::EPSILON {
            return Err(CalibrationError::NonPositiveFlat {
                channel: Some(color),
            });
        }
        inv_means[color] = 1.0 / mean;
    }

    flat.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, value) in row.iter_mut().enumerate() {
            let color = cfa_type.color_at(Vec2us::new(x, y)) as usize;
            *value = (*value * inv_means[color]).max(MIN_NORMALIZED_FLAT);
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests;
