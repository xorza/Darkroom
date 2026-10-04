//! [`PreparedFlat`]: a stacked flat turned into the divisor calibration applies.
//!
//! Cold-pixel detection runs on the flat with its additive part removed, before the normalization
//! here clamps near-zero photosites away.

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
        normalize(&mut flat)?;
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

    /// Divide `image` by the flat; flag [`QualityFlags::NO_DATA`] where the flat holds no
    /// measurement, and [`QualityFlags::FLAT_FLOOR`] where the divisor sits at its floor.
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
        image.take_master_flags(flat);
        if self.floored > 0 {
            let divisors = flat.data.pixels();
            let size = image.size();
            PixelFlags::add_where(&mut image.flags, size, QualityFlags::FLAT_FLOOR, |index| {
                divisors[index] <= MIN_NORMALIZED_FLAT
            });
        }
    }
}

/// One row's sum and count of each colour.
#[derive(Debug, Default)]
struct ColorSums {
    sums: [f64; 3],
    counts: [u64; 3],
}

/// Normalize `flat` to a mean of one per colour of its pattern, over the photosites that hold a
/// measurement, and floor it at [`MIN_NORMALIZED_FLAT`]. A fill or a saturated bound would move the
/// mean every other photosite is divided by.
fn normalize(flat: &mut CfaImage) -> Result<(), CalibrationError> {
    let width = flat.data.width();
    let cfa_type = flat.cfa_type;
    let colours = cfa_type.num_colors();
    // Each row's sums, added in row order: a rayon `reduce` would add them in whatever tree its
    // work stealing built, and the means would move with the thread count.
    let rows: Vec<ColorSums> = {
        let flags = flat.flags.as_ref();
        flat.data
            .par_chunks(width)
            .enumerate()
            .map(|(y, row)| {
                let mut sums = ColorSums::default();
                for (x, &value) in row.iter().enumerate() {
                    if flags.is_some_and(|flags| {
                        flags.at(y * width + x).intersects(QualityFlags::UNMEASURED)
                    }) {
                        continue;
                    }
                    let color = usize::from(cfa_type.color_at(Vec2us::new(x, y)));
                    sums.sums[color] += f64::from(value);
                    sums.counts[color] += 1;
                }
                sums
            })
            .collect()
    };
    let mut sums = [0.0f64; 3];
    let mut counts = [0u64; 3];
    for row in &rows {
        for color in 0..colours {
            sums[color] += row.sums[color];
            counts[color] += row.counts[color];
        }
    }

    let mut inv_means = [0.0f32; 3];
    for color in 0..colours {
        // A colour with no measured photosite divides 0 by 0.
        let mean = (sums[color] / counts[color] as f64) as f32;
        if mean.is_nan() || mean <= 0.0 {
            return Err(CalibrationError::NonPositiveFlat {
                channel: (cfa_type != CfaType::Mono).then_some(color),
            });
        }
        inv_means[color] = 1.0 / mean;
    }

    flat.data
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, value) in row.iter_mut().enumerate() {
                let color = usize::from(cfa_type.color_at(Vec2us::new(x, y)));
                *value = (*value * inv_means[color]).max(MIN_NORMALIZED_FLAT);
            }
        });
    Ok(())
}

#[cfg(test)]
mod tests;
