//! [`Scnr`]: the residual green cast removed from a colour-balanced image.

use crate::image_ops::rgb::Rgb;

use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;
use crate::image::linear_image::LinearImage;
use crate::image_ops::error::OpError;

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
        ConfigValue::new("SCNR amount", self.amount)
            .finite_and("finite and in [0, 1]", |value| (0.0..=1.0).contains(&value))
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
