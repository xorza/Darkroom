//! Color calibration: neutralize the per-channel sky background and remove the residual green cast
//! from a one-shot-color stack. See `color_calibration/README.md` for the algorithm research.
//!
//! - [`NeutralizeBackground`] (linear, pre-stretch): estimate each channel's background and
//!   additively shift them to a common level, so the sky is neutral gray (R=G=B).
//! - [`Scnr`] (post-stretch): Subtractive Chromatic Noise Reduction — clamp green that exceeds the
//!   red/blue average, the residual green being noise on a color-balanced deep-sky image.

use crate::image_ops::rgb::Rgb;

use crate::error::InvalidConfigField;
use crate::image_ops::error::OpError;
use crate::io::image::linear::LinearImage;
use crate::math::statistics::ClippedStats;

#[cfg(test)]
mod tests;

/// Sigma-clip parameters for the robust per-channel background estimate (rejects stars/nebula).
const BACKGROUND_KAPPA: f32 = 2.5;
const BACKGROUND_ITERATIONS: usize = 5;
/// Cap on the per-channel sample size for the background estimate (uniform stride for larger
/// channels, matching `defect_map`'s `MAX_MEDIAN_SAMPLES`). A robust background median converges
/// well below this; small images stay exact (stride 1).
const MAX_BACKGROUND_SAMPLES: usize = 1_000_000;

/// Neutralize the per-channel sky background so the background is a neutral gray (R=G=B).
///
/// Estimates each channel's background as a sigma-clipped median, then additively shifts every
/// channel to the darkest channel's level: `IN_x = I_x − BI_x + min(BI_R, BI_G, BI_B)`. A
/// linear-domain operation — run after gradient/background extraction and before the stretch.
/// Additive, so it preserves signal *above* the background (and may push faint pixels slightly
/// negative, which the stretch's black point absorbs). No-op on grayscale.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeutralizeBackground;

impl NeutralizeBackground {
    /// Neutralize `image`'s background in place. A no-op on grayscale, which has no channels to
    /// bring to a common level.
    ///
    /// # Errors
    /// Never — the signature keeps the shape the other ops have, and `lens` drives them uniformly.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        if !image.is_rgb() {
            return Ok(());
        }
        let bg = channel_backgrounds(image);
        let target = bg.r.min(bg.g).min(bg.b);
        let (dr, dg, db) = (target - bg.r, target - bg.g, target - bg.b);
        image.map_rgb(move |px| Rgb {
            r: px.r + dr,
            g: px.g + dg,
            b: px.b + db,
        });
        Ok(())
    }
}

/// Per-channel sigma-clipped median background of an RGB image. Used by
/// [`NeutralizeBackground::apply`] and the colour-calibration tests/fixtures.
pub(crate) fn channel_backgrounds(image: &LinearImage) -> Rgb {
    let mut scratch = Vec::new();
    Rgb {
        r: channel_background(image.channel(0), &mut scratch),
        g: channel_background(image.channel(1), &mut scratch),
        b: channel_background(image.channel(2), &mut scratch),
    }
}

/// One channel's robust (sigma-clipped median) background: subsample the plane at a uniform stride
/// capped at `MAX_BACKGROUND_SAMPLES` (exact for small images) and take its sigma-clipped median.
fn channel_background(plane: &[f32], scratch: &mut Vec<f32>) -> f32 {
    let stride = (plane.len() / MAX_BACKGROUND_SAMPLES).max(1);
    let mut s: Vec<f32> = plane.iter().step_by(stride).copied().collect();
    ClippedStats::sigma_clipped(&mut s, scratch, BACKGROUND_KAPPA, BACKGROUND_ITERATIONS).median
}

/// Remove the residual green cast (Subtractive Chromatic Noise Reduction). Intended for the
/// stretched, already-color-balanced image. No-op on grayscale.
#[derive(Debug, Clone, Copy)]
pub struct Scnr {
    method: ScnrMethod,
}

/// Which green-removal protection [`Scnr`] applies.
#[derive(Debug, Clone, Copy)]
enum ScnrMethod {
    AverageNeutral,
    AdditiveMask { amount: f32 },
}

impl Default for Scnr {
    fn default() -> Self {
        Self::average_neutral()
    }
}

impl Scnr {
    /// Average Neutral: `G' = min(G, (R+B)/2)` — a full-strength clamp of green to the red/blue
    /// average. The default.
    pub fn average_neutral() -> Self {
        Self {
            method: ScnrMethod::AverageNeutral,
        }
    }

    /// Additive Mask with blend `amount` ∈ `[0,1]` (0 = no change, 1 = full strength): attenuates
    /// rather than clamps, so genuine teal (OIII planetary nebulae) survives. `m = min(1, R+B)`,
    /// `G' = G·(1−amount)·(1−m) + m·G`.
    pub fn additive_mask(amount: f32) -> Self {
        Self {
            method: ScnrMethod::AdditiveMask { amount },
        }
    }

    /// Remove the residual green cast from `image` in place.
    ///
    /// A no-op on grayscale, which has no green channel to subtract.
    ///
    /// # Errors
    /// [`OpError::InvalidConfig`] if the additive-mask amount is outside `[0, 1]`.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        match self.method {
            ScnrMethod::AverageNeutral => image.map_rgb(scnr_average_neutral),
            ScnrMethod::AdditiveMask { amount } => {
                image.map_rgb(move |px| scnr_additive_mask(px, amount));
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), InvalidConfigField> {
        if let ScnrMethod::AdditiveMask { amount } = self.method {
            InvalidConfigField::finite("SCNR amount", "finite and in [0, 1]", amount, |value| {
                (0.0..=1.0).contains(&value)
            })?;
        }
        Ok(())
    }
}

/// Average Neutral: clamp green to the red/blue average.
fn scnr_average_neutral(px: Rgb) -> Rgb {
    Rgb {
        r: px.r,
        g: px.g.min(0.5 * (px.r + px.b)),
        b: px.b,
    }
}

/// Additive Mask: attenuate green by `amount`, protected where R+B is large.
fn scnr_additive_mask(px: Rgb, amount: f32) -> Rgb {
    let m = (px.r + px.b).min(1.0);
    Rgb {
        r: px.r,
        g: px.g * (1.0 - amount) * (1.0 - m) + m * px.g,
        b: px.b,
    }
}
