//! Color calibration: neutralize the per-channel sky background and remove the residual green cast
//! from a one-shot-color stack. See `color_calibration/README.md` for the algorithm research.
//!
//! - [`NeutralizeBackground`] (linear, pre-stretch): estimate each channel's background and
//!   additively shift them to a common level, so the sky is neutral gray (R=G=B).
//! - [`Scnr`] (post-stretch): Subtractive Chromatic Noise Reduction — clamp green that exceeds the
//!   red/blue average, the residual green being noise on a color-balanced deep-sky image.

use common::Rgb;
use imaginarium::{ChannelCount, Image};

use crate::image_ops::op::{OpError, ensure, require_f32_master};
use crate::image_ops::par_map_pixels;
use crate::math::statistics::sigma_clipped_median_mad;

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
    /// Neutralize `image`'s background in place.
    ///
    /// # Errors
    /// [`OpError::UnsupportedFormat`] unless `image` is `L_F32`/`RGB_F32` (a no-op on grayscale).
    pub fn apply(&self, image: &mut Image) -> Result<(), OpError> {
        require_f32_master(image)?;
        if image.desc.color_format.channel_count != ChannelCount::Rgb {
            return Ok(()); // no-op on grayscale
        }
        let bg = channel_backgrounds(image);
        let target = bg.r.min(bg.g).min(bg.b);
        let (dr, dg, db) = (target - bg.r, target - bg.g, target - bg.b);
        par_map_pixels(
            image,
            |l| l,
            move |px| Rgb {
                r: px.r + dr,
                g: px.g + dg,
                b: px.b + db,
            },
        );
        Ok(())
    }
}

/// Per-channel sigma-clipped median background of an RGB f32 image, read straight from the
/// interleaved samples. Used by [`NeutralizeBackground::apply`] and the colour-calibration tests/fixtures.
pub(crate) fn channel_backgrounds(image: &Image) -> Rgb {
    let samples: &[f32] = bytemuck::cast_slice(image.bytes());
    let mut scratch = Vec::new();
    Rgb {
        r: channel_background(samples, 0, &mut scratch),
        g: channel_background(samples, 1, &mut scratch),
        b: channel_background(samples, 2, &mut scratch),
    }
}

/// One channel's robust (sigma-clipped median) background: subsample channel `channel` from the
/// interleaved RGB `samples` (every third value, uniform stride capped at `MAX_BACKGROUND_SAMPLES`;
/// exact for small images) and take its sigma-clipped median.
fn channel_background(samples: &[f32], channel: usize, scratch: &mut Vec<f32>) -> f32 {
    let stride = (samples.len() / 3 / MAX_BACKGROUND_SAMPLES).max(1);
    let mut s: Vec<f32> = samples[channel..]
        .iter()
        .step_by(3 * stride)
        .copied()
        .collect();
    sigma_clipped_median_mad(&mut s, scratch, BACKGROUND_KAPPA, BACKGROUND_ITERATIONS).median
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

    /// Additive Mask with blend `amount` ∈ [0,1] (0 = no change, 1 = full strength): attenuates
    /// rather than clamps, so genuine teal (OIII planetary nebulae) survives. `m = min(1, R+B)`,
    /// `G' = G·(1−amount)·(1−m) + m·G`.
    pub fn additive_mask(amount: f32) -> Self {
        Self {
            method: ScnrMethod::AdditiveMask { amount },
        }
    }

    /// Remove the residual green cast from `image` in place.
    ///
    /// # Errors
    /// [`OpError::UnsupportedFormat`] unless `image` is `L_F32`/`RGB_F32` (a no-op on grayscale);
    /// [`OpError::InvalidConfig`] if the additive-mask amount is outside `[0, 1]`.
    pub fn apply(&self, image: &mut Image) -> Result<(), OpError> {
        self.validate()?;
        require_f32_master(image)?;
        if image.desc.color_format.channel_count != ChannelCount::Rgb {
            return Ok(()); // no-op on grayscale
        }
        match self.method {
            ScnrMethod::AverageNeutral => par_map_pixels(image, |l| l, scnr_average_neutral),
            ScnrMethod::AdditiveMask { amount } => {
                par_map_pixels(image, |l| l, move |px| scnr_additive_mask(px, amount));
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), OpError> {
        if let ScnrMethod::AdditiveMask { amount } = self.method {
            ensure((0.0..=1.0).contains(&amount), || {
                format!("SCNR amount must be in [0, 1], got {amount}")
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
