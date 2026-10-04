//! Color calibration: neutralize the per-channel sky background and remove the residual green cast
//! from a one-shot-color stack.
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
use crate::math::statistics::subsample::Subsample;

/// Sigma-clip parameters for the robust per-channel background estimate (rejects stars/nebula).
const BACKGROUND_KAPPA: f32 = 2.5;
const BACKGROUND_ITERATIONS: usize = 5;

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

/// Per-channel sigma-clipped median background of an RGB image.
fn channel_backgrounds(image: &LinearImage) -> Rgb {
    let mut scratch = Vec::new();
    Rgb {
        r: channel_background(image.channel(0), &mut scratch),
        g: channel_background(image.channel(1), &mut scratch),
        b: channel_background(image.channel(2), &mut scratch),
    }
}

/// One channel's robust (sigma-clipped median) background, from a [`Subsample`] of the plane.
fn channel_background(plane: &[f32], scratch: &mut Vec<f32>) -> f32 {
    let mut s = Subsample::statistic_values(plane);
    ClippedStats::sigma_clipped(&mut s, scratch, BACKGROUND_KAPPA, BACKGROUND_ITERATIONS).median
}

/// Remove the residual green cast (Subtractive Chromatic Noise Reduction), as PixInsight's SCNR
/// does. Intended for the stretched, already-color-balanced image. No-op on grayscale.
///
/// Each protection method gives a full-strength green, and `amount` blends toward it from the
/// original: `G′ = (1 − amount)·G + amount·G_full`, exactly `G` at 0 and `G_full` at 1. For the
/// mask methods that is PixInsight's own `G·(1 − amount)·(1 − m) + m·G`, with `G_full = m·G`.
#[derive(Debug, Clone, Copy)]
pub struct Scnr {
    protection: ScnrProtection,
    amount: f32,
}

/// Which green-removal protection [`Scnr`] applies.
#[derive(Debug, Clone, Copy)]
enum ScnrProtection {
    AverageNeutral,
    MaximumNeutral,
    AdditiveMask,
    MaximumMask,
}

impl Default for Scnr {
    /// Average Neutral at full strength.
    fn default() -> Self {
        Self::average_neutral(1.0)
    }
}

impl Scnr {
    /// Average Neutral: green clamped to the red/blue mean, `G_full = min(G, (R + B)/2)`.
    pub const fn average_neutral(amount: f32) -> Self {
        Self {
            protection: ScnrProtection::AverageNeutral,
            amount,
        }
    }

    /// Maximum Neutral: green clamped to the larger of red and blue, `G_full = min(G, max(R, B))`
    /// — gentler than Average Neutral where one of the two is faint.
    pub const fn maximum_neutral(amount: f32) -> Self {
        Self {
            protection: ScnrProtection::MaximumNeutral,
            amount,
        }
    }

    /// Additive Mask: green attenuated where red and blue are faint, `G_full = m·G` with
    /// `m = min(1, R + B)`: attenuates rather than clamps, so genuine teal (OIII planetary nebulae)
    /// survives.
    pub const fn additive_mask(amount: f32) -> Self {
        Self {
            protection: ScnrProtection::AdditiveMask,
            amount,
        }
    }

    /// Maximum Mask: as Additive Mask with `m = max(R, B)`, which protects less where both are
    /// moderate.
    pub const fn maximum_mask(amount: f32) -> Self {
        Self {
            protection: ScnrProtection::MaximumMask,
            amount,
        }
    }

    /// Remove the residual green cast from `image` in place.
    ///
    /// A no-op on grayscale, which has no green channel to subtract.
    ///
    /// # Errors
    /// [`OpError::InvalidConfig`] if the amount is outside `[0, 1]`.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        let Self { protection, amount } = *self;
        image.map_rgb(move |px| Rgb {
            g: (1.0 - amount) * px.g + amount * protection.full_strength(px),
            ..px
        });
        Ok(())
    }

    fn validate(self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "SCNR amount",
            "finite and in [0, 1]",
            self.amount,
            |value| (0.0..=1.0).contains(&value),
        )
    }
}

impl ScnrProtection {
    /// The green of `px` at full strength.
    fn full_strength(self, px: Rgb) -> f32 {
        // A mask is a share of green to keep, so it lies in [0, 1] whatever the channels hold.
        let mask = |m: f32| m.clamp(0.0, 1.0) * px.g;
        match self {
            Self::AverageNeutral => px.g.min(f32::midpoint(px.r, px.b)),
            Self::MaximumNeutral => px.g.min(px.r.max(px.b)),
            Self::AdditiveMask => mask(px.r + px.b),
            Self::MaximumMask => mask(px.r.max(px.b)),
        }
    }
}

#[cfg(test)]
mod tests;
