//! [`PreparedFlat`]: a stacked flat turned into the divisor calibration applies.
//!
//! Cold-pixel detection runs on the flat with its additive part removed, before the normalization
//! here clamps near-zero photosites away.

use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::calibration_masters::error::CalibrationError;
use crate::io::image::cfa::{CfaImage, CfaType};
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::math::vec2us::Vec2us;

/// Bounds amplification at dead and near-zero photosites while keeping every pixel calibrated.
pub(crate) const MIN_NORMALIZED_FLAT: f32 = 0.1;

/// A flat ready to divide a light by: its additive part removed, normalized to a mean of one per CFA
/// colour, and floored at [`MIN_NORMALIZED_FLAT`].
///
/// A type rather than a `CfaImage` in the flat's slot, so a raw flat cannot be divided by, and a
/// prepared one cannot be prepared twice.
#[derive(Debug)]
pub(crate) struct PreparedFlat {
    divisor: CfaImage,
    /// Photosites the floor raised: they are corrected by less than their vignetting asks.
    floored: usize,
}

impl PreparedFlat {
    /// Normalize a flat whose additive part is already removed, per CFA colour, and floor it.
    ///
    /// # Errors
    /// [`CalibrationError::NonPositiveFlat`] when a colour (or the whole mono frame) has no positive
    /// mean: a property of the user's flats, not of this code.
    pub(crate) fn new(mut flat: CfaImage) -> Result<Self, CalibrationError> {
        match flat.cfa_type {
            CfaType::Mono => normalize_mono(&mut flat.data)?,
            cfa_type @ (CfaType::Bayer(_) | CfaType::XTrans(_)) => {
                normalize_cfa(&mut flat.data, &cfa_type)?;
            }
        }
        Ok(Self::from_divisor(flat))
    }

    /// A divisor prepared before, as a saved bundle holds it.
    pub(crate) fn from_divisor(divisor: CfaImage) -> Self {
        let floored = divisor
            .data
            .par_iter()
            .filter(|&&value| value <= MIN_NORMALIZED_FLAT)
            .count();
        Self { divisor, floored }
    }

    pub(crate) const fn divisor(&self) -> &CfaImage {
        &self.divisor
    }

    pub(crate) const fn floored(&self) -> usize {
        self.floored
    }

    /// Divide `image` by the flat, and flag [`QualityFlags::FLAT_FLOOR`] where the divisor sits at its
    /// floor.
    pub(crate) fn apply(&self, image: &mut CfaImage) {
        let flat = &self.divisor;
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
        if self.floored > 0 {
            let divisors = flat.data.pixels();
            let size = image.size();
            PixelFlags::add_where(&mut image.flags, size, QualityFlags::FLAT_FLOOR, |index| {
                divisors[index] <= MIN_NORMALIZED_FLAT
            });
        }
    }
}

fn normalize_mono(flat: &mut Buffer2<f32>) -> Result<(), CalibrationError> {
    let sum: f64 = flat.par_iter().map(|&value| f64::from(value)).sum();
    let mean = (sum / flat.len() as f64) as f32;
    if mean.is_nan() || mean <= 0.0 {
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
        if mean.is_nan() || mean <= 0.0 {
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
