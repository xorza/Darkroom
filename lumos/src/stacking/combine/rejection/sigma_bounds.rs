//! The clip band the sigma-based rejectors share.

use crate::error::InvalidConfigField;

/// How far either side of the reference a value may sit before it is rejected, in units of the
/// spread estimate.
///
/// Sigma clipping, winsorized clipping and linear-fit clipping all reject on this same asymmetric
/// band and differ only in what they take as the reference and the spread — so the pair travels as
/// one value rather than as two `f32`s each of them declares, documents and validates separately.
/// Keeping them together is also what stops a low threshold reaching a rejector as its high one:
/// two bare `f32` parameters in a row can be swapped silently, a `SigmaBounds` cannot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SigmaBounds {
    /// Threshold for low outliers, below the reference.
    pub low: f32,
    /// Threshold for high outliers, above the reference.
    pub high: f32,
}

impl SigmaBounds {
    /// The same threshold either side.
    pub fn symmetric(sigma: f32) -> Self {
        Self {
            low: sigma,
            high: sigma,
        }
    }

    /// Separate thresholds — e.g. clipping bright satellite trails harder than dark pixels.
    pub fn asymmetric(low: f32, high: f32) -> Self {
        Self { low, high }
    }

    /// Both thresholds must be finite and positive: they scale a spread estimate, so a non-positive
    /// one inverts the keep-band and a non-finite one empties it.
    ///
    /// Reports under the field names a caller sets, `sigma_low` and `sigma_high`, rather than this
    /// type's own — the error names what the user wrote, not how it is stored.
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite("sigma_low", "finite and positive", self.low, |value| {
            value > 0.0
        })?;
        InvalidConfigField::finite("sigma_high", "finite and positive", self.high, |value| {
            value > 0.0
        })
    }
}
