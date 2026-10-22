//! [`Scnr`]: the residual green cast removed from a color-balanced image.

use common::{Introspect, IntrospectEnum};

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;
use crate::image::linear_image::LinearImage;
use crate::image::rgb::Rgb;
use crate::image_ops::error::OpError;

/// Remove the residual green cast (Subtractive Chromatic Noise Reduction), as PixInsight's SCNR
/// does. Intended for the stretched, already-color-balanced image. No-op on grayscale.
///
/// Each protection method gives a full-strength green, and `strength` blends toward it from the
/// original: `G′ = (1 − strength)·G + strength·G_full`, exactly `G` at 0 and `G_full` at 1. For the
/// mask methods that is PixInsight's own `G·(1 − s)·(1 − m) + m·G`, with `G_full = m·G`.
///
/// `type_id` is this config's identity to an introspecting consumer, which stores it, so it is
/// fixed for the life of the type.
#[derive(Debug, Clone, Copy, Introspect)]
#[config(type_id = "cb80e688-a5ed-42fd-9087-6a9639a8b056")]
pub struct Scnr {
    pub protection: ScnrProtection,
    /// The blend toward the protection's full-strength green, in `[0, 1]`.
    pub strength: f32,
}

/// Which green-removal protection [`Scnr`] applies; each one's full-strength green is on its
/// constructor.
///
/// `type_id` is this enum's identity to an introspecting consumer, which stores it, so it is
/// fixed for the life of the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntrospectEnum)]
#[config(type_id = "662e2432-b685-4b5b-bf05-0041814dc908")]
pub enum ScnrProtection {
    /// See [`Scnr::average_neutral`].
    AverageNeutral,
    /// See [`Scnr::additive_mask`].
    AdditiveMask,
    /// See [`Scnr::maximum_neutral`].
    MaximumNeutral,
    /// See [`Scnr::maximum_mask`].
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
    pub const fn average_neutral(strength: f32) -> Self {
        Self {
            protection: ScnrProtection::AverageNeutral,
            strength,
        }
    }

    /// Maximum Neutral: green clamped to the larger of red and blue, `G_full = min(G, max(R, B))`
    /// — gentler than Average Neutral where one of the two is faint.
    pub const fn maximum_neutral(strength: f32) -> Self {
        Self {
            protection: ScnrProtection::MaximumNeutral,
            strength,
        }
    }

    /// Additive Mask: green attenuated where red and blue are faint, `G_full = m·G` with
    /// `m = min(1, R + B)`: attenuates rather than clamps, so genuine teal (OIII planetary nebulae)
    /// survives.
    pub const fn additive_mask(strength: f32) -> Self {
        Self {
            protection: ScnrProtection::AdditiveMask,
            strength,
        }
    }

    /// Maximum Mask: as Additive Mask with `m = max(R, B)`, which protects less where both are
    /// moderate.
    pub const fn maximum_mask(strength: f32) -> Self {
        Self {
            protection: ScnrProtection::MaximumMask,
            strength,
        }
    }

    /// Remove the residual green cast from `image` in place.
    ///
    /// A no-op on grayscale, which has no green channel to subtract.
    ///
    /// # Errors
    /// [`OpError::Config`] if the strength is outside `[0, 1]`.
    pub fn apply(&self, image: &mut LinearImage) -> Result<(), OpError> {
        self.validate()?;
        let Self {
            protection,
            strength,
        } = *self;
        image.map_rgb(move |px| Rgb {
            g: (1.0 - strength) * px.g + strength * protection.full_strength(px),
            ..px
        });
        Ok(())
    }

    fn validate(self) -> Result<(), InvalidConfigField> {
        ConfigField::new("SCNR strength", self.strength)
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
