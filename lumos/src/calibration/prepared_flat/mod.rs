//! [`PreparedFlat`]: a stacked flat turned into the divisor calibration applies.
//!
//! Cold-pixel detection runs on the flat with its additive part removed, before the normalization
//! here clamps near-zero photosites away.

use std::sync::Arc;

use rayon::prelude::*;

use crate::calibration::error::CalibrationError;
use crate::image::calibration_state::CalibrationState;
use crate::image::cfa::cfa_color::CfaColor;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::FlatGain;
use crate::image::quality_flags::QualityFlags;
use crate::math::vec2us::Vec2us;

/// Bounds amplification at dead and near-zero photosites while keeping every pixel calibrated.
pub(super) const MIN_NORMALIZED_FLAT: f32 = 0.1;

/// A flat ready to divide a light by: its additive part removed, normalized to a mean of one per
/// CFA color, and floored at [`MIN_NORMALIZED_FLAT`].
///
/// A type rather than a `CfaImage` in the flat's slot, so a raw flat cannot be divided by, and a
/// prepared one cannot be prepared twice.
#[derive(Debug)]
pub(crate) struct PreparedFlat {
    divisor: CfaImage,
    /// Photosites the floor raised: they are corrected by less than their vignetting asks.
    floored: usize,
    /// The gain the divisor applies, which every light it divides carries for its noise model.
    gain: Arc<FlatGain>,
}

impl PreparedFlat {
    /// Normalize a flat whose additive part is already removed, per CFA color, and floor it.
    ///
    /// # Errors
    /// [`CalibrationError::NonPositiveFlat`] when a color (or the whole mono frame) has no
    /// positive mean: a property of the user's flats, not of this code.
    pub(crate) fn new(mut flat: CfaImage) -> Result<Self, CalibrationError> {
        normalize(&mut flat)?;
        Ok(Self::from_divisor(flat))
    }

    /// A divisor prepared before, as a saved bundle holds it.
    pub(crate) fn from_divisor(divisor: CfaImage) -> Self {
        let floored = divisor
            .data()
            .par_iter()
            .filter(|&&value| value <= MIN_NORMALIZED_FLAT)
            .count();
        let flags = divisor.flags();
        let values = divisor.data().pixels();
        let gain = Arc::new(FlatGain::of_divisor(
            divisor.data(),
            &divisor.cfa_type(),
            |idx| {
                values[idx] <= MIN_NORMALIZED_FLAT
                    || flags.is_some_and(|flags| flags.is_flagged(idx))
            },
        ));
        Self {
            divisor,
            floored,
            gain,
        }
    }

    pub(crate) const fn divisor(&self) -> &CfaImage {
        &self.divisor
    }

    /// Resident RAM held by the flat: its divisor and the gain it applies.
    pub(crate) fn ram_bytes(&self) -> usize {
        self.divisor.ram_bytes() + self.gain.ram_bytes()
    }

    pub(crate) const fn floored(&self) -> usize {
        self.floored
    }

    /// Whether the divisor sits at its floor at `idx`.
    pub(super) fn is_floored(&self, idx: usize) -> bool {
        self.divisor.data().pixels()[idx] <= MIN_NORMALIZED_FLAT
    }

    /// Record in `image`, divided by this flat, that it lost the flat, and the gain that scaled its
    /// noise.
    pub(crate) fn record(&self, image: &mut CfaImage) {
        debug_assert!(
            !image.metadata.derived.calibration().flat,
            "the flat is divided out twice"
        );
        image
            .metadata
            .derived
            .record_removal(CalibrationState::FLAT);
        image.set_flat_gain(Some(Arc::clone(&self.gain)));
    }
}

/// One row's sum and count of each color.
#[derive(Debug, Default)]
struct ColorSums {
    sums: [f64; 3],
    counts: [u64; 3],
}

/// Normalize `flat` to a mean of one per color of its pattern, over the photosites that hold a
/// measurement, and floor it at [`MIN_NORMALIZED_FLAT`]. A fill or a saturated bound would move the
/// mean every other photosite is divided by.
fn normalize(flat: &mut CfaImage) -> Result<(), CalibrationError> {
    let width = flat.data().width();
    let cfa_type = flat.cfa_type();
    let colors = cfa_type.color_count();
    // Each row's sums, added in row order: a rayon `reduce` would add them in whatever tree its
    // work stealing built, and the means would move with the thread count.
    let rows: Vec<ColorSums> = {
        let flags = flat.flags();
        flat.data()
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
                    let color = cfa_type.slot_at(Vec2us::new(x, y));
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
        for color in 0..colors {
            sums[color] += row.sums[color];
            counts[color] += row.counts[color];
        }
    }

    // In f64, so each normalized sample is one correctly rounded quotient: a mean narrowed to f32
    // and inverted in f32 rounds three times.
    let mut means = [0.0f64; 3];
    for color in 0..colors {
        // A color with no measured photosite divides 0 by 0.
        let mean = sums[color] / counts[color] as f64;
        if mean.is_nan() || mean <= 0.0 {
            return Err(CalibrationError::NonPositiveFlat {
                color: if cfa_type == CfaType::Mono {
                    None
                } else {
                    Some(CfaColor::from_idx(color).expect("a mosaic has three colors"))
                },
            });
        }
        means[color] = mean;
    }

    flat.data_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, value) in row.iter_mut().enumerate() {
                let color = cfa_type.slot_at(Vec2us::new(x, y));
                *value = ((f64::from(*value) / means[color]) as f32).max(MIN_NORMALIZED_FLAT);
            }
        });
    Ok(())
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
